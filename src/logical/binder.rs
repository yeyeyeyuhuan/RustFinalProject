//! 绑定 / 语义分析:把"语法正确"的 AST 变成"语义正确且带类型"的逻辑计划。
//!
//! 职责:名称解析(列名 → 位置索引)、类型推导与检查、聚合合法性校验。

use std::sync::Arc;

use super::expr::Expr;
use super::plan::{LogicalPlan, SortExpr};
use crate::datasource::DataSource;
use crate::error::{QueryError, Result};
use crate::sql::ast::*;
use crate::types::{DataType, Field, ScalarValue, Schema};

/// 表目录:绑定器据此把表名解析为数据源。由执行层的 SessionContext 实现。
pub trait Catalog {
    fn get_table(&self, name: &str) -> Option<Arc<dyn DataSource>>;
}

/// 绑定一条语句为逻辑计划。
pub fn bind(stmt: &Statement, catalog: &dyn Catalog) -> Result<LogicalPlan> {
    match stmt {
        Statement::Select(s) => bind_select(s, catalog),
    }
}

fn bind_select(s: &SelectStmt, catalog: &dyn Catalog) -> Result<LogicalPlan> {
    // ---- FROM + JOIN:构造扫描 / 连接子树,得到当前(合并)Schema ----
    let (mut plan, scan_schema) = bind_from(s, catalog)?;

    // ---- WHERE ----
    if let Some(f) = &s.filter {
        let pred = bind_expr(f, &scan_schema)?;
        ensure_boolean(&pred, &scan_schema, "WHERE")?;
        plan = LogicalPlan::Filter {
            predicate: pred,
            input: Box::new(plan),
        };
    }

    // ---- 判定是否为聚合查询 ----
    let mut aggr_asts: Vec<AstExpr> = Vec::new();
    for item in &s.projections {
        if let SelectItem::Expr { expr, .. } = item {
            collect_aggregates(expr, &mut aggr_asts);
        }
    }
    if let Some(h) = &s.having {
        collect_aggregates(h, &mut aggr_asts);
    }
    for o in &s.order_by {
        collect_aggregates(&o.expr, &mut aggr_asts);
    }
    let is_aggregate = !s.group_by.is_empty() || !aggr_asts.is_empty();

    // bind_schema:投影 / HAVING / ORDER BY 解析所依据的 Schema。
    let bind_schema: Arc<Schema>;

    if is_aggregate {
        // 分组表达式与聚合表达式均先对扫描 Schema 绑定。
        let group_expr = s
            .group_by
            .iter()
            .map(|g| bind_expr(g, &scan_schema))
            .collect::<Result<Vec<_>>>()?;
        let aggr_expr = aggr_asts
            .iter()
            .map(|a| bind_aggregate(a, &scan_schema))
            .collect::<Result<Vec<_>>>()?;

        // 聚合输出 Schema = 分组列 ++ 聚合列。
        let mut fields = Vec::new();
        for (i, g) in group_expr.iter().enumerate() {
            fields.push(Field::new(
                ast_expr_name(&s.group_by[i]),
                g.data_type(&scan_schema)?,
                g.nullable(&scan_schema),
            ));
        }
        for (j, a) in aggr_expr.iter().enumerate() {
            fields.push(Field::new(
                ast_expr_name(&aggr_asts[j]),
                a.data_type(&scan_schema)?,
                a.nullable(&scan_schema),
            ));
        }
        let agg_schema = Arc::new(Schema::new(fields));

        plan = LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema: Arc::clone(&agg_schema),
            input: Box::new(plan),
        };
        bind_schema = Arc::clone(&agg_schema);

        // HAVING:对聚合输出解析,作为 Aggregate 之上的 Filter。
        if let Some(h) = &s.having {
            let pred = resolve_aggregated(h, &s.group_by, &aggr_asts)?;
            ensure_boolean(&pred, &agg_schema, "HAVING")?;
            plan = LogicalPlan::Filter {
                predicate: pred,
                input: Box::new(plan),
            };
        }
    } else {
        if s.having.is_some() {
            return Err(QueryError::bind("HAVING 必须配合 GROUP BY 或聚合函数使用"));
        }
        bind_schema = Arc::clone(&scan_schema);
    }

    // ---- ORDER BY(置于投影之下,绑定到 bind_schema)----
    if !s.order_by.is_empty() {
        let mut sort_exprs = Vec::new();
        for o in &s.order_by {
            let e = if is_aggregate {
                resolve_aggregated(&o.expr, &s.group_by, &aggr_asts)?
            } else {
                bind_expr(&o.expr, &bind_schema)?
            };
            sort_exprs.push(SortExpr {
                expr: e,
                asc: o.asc,
            });
        }
        plan = LogicalPlan::Sort {
            exprs: sort_exprs,
            input: Box::new(plan),
        };
    }

    // ---- 投影 ----
    let (proj_exprs, proj_schema) = bind_projection(
        &s.projections,
        &bind_schema,
        is_aggregate,
        &s.group_by,
        &aggr_asts,
    )?;
    plan = LogicalPlan::Projection {
        exprs: proj_exprs,
        schema: proj_schema,
        input: Box::new(plan),
    };

    // ---- LIMIT ----
    if let Some(n) = s.limit {
        plan = LogicalPlan::Limit {
            skip: 0,
            fetch: Some(n as usize),
            input: Box::new(plan),
        };
    }

    Ok(plan)
}

/// 绑定 FROM + 一系列 JOIN,返回(计划子树, 合并后的 Schema)。
fn bind_from(s: &SelectStmt, catalog: &dyn Catalog) -> Result<(LogicalPlan, Arc<Schema>)> {
    let (mut plan, mut schema) = bind_table(&s.from, catalog)?;
    for join in &s.joins {
        let (right_plan, right_schema) = bind_table(&join.table, catalog)?;
        let merged = Arc::new(merge_for_join(&schema, &right_schema, join.join_type));
        let on = bind_expr(&join.on, &merged)?;
        ensure_boolean(&on, &merged, "JOIN ON")?;
        plan = LogicalPlan::Join {
            left: Box::new(plan),
            right: Box::new(right_plan),
            on,
            join_type: join.join_type,
            schema: Arc::clone(&merged),
        };
        schema = merged;
    }
    Ok((plan, schema))
}

/// 把一个表引用绑定为带限定 Schema 的 Scan 节点。
fn bind_table(tref: &TableRef, catalog: &dyn Catalog) -> Result<(LogicalPlan, Arc<Schema>)> {
    let source = catalog
        .get_table(&tref.name)
        .ok_or_else(|| QueryError::bind(format!("表 `{}` 未注册", tref.name)))?;
    let qualifier = tref.alias.clone().unwrap_or_else(|| tref.name.clone());
    let schema = Arc::new(qualify_schema(&source.schema(), &qualifier));
    let plan = LogicalPlan::Scan {
        table_name: tref.name.clone(),
        source,
        projection: None,
        projected_schema: Arc::clone(&schema),
    };
    Ok((plan, schema))
}

fn bind_projection(
    items: &[SelectItem],
    bind_schema: &Schema,
    is_aggregate: bool,
    group_asts: &[AstExpr],
    aggr_asts: &[AstExpr],
) -> Result<(Vec<Expr>, Arc<Schema>)> {
    let mut exprs = Vec::new();
    let mut fields = Vec::new();

    for item in items {
        match item {
            SelectItem::Wildcard => {
                if is_aggregate {
                    return Err(QueryError::bind("聚合查询中不支持 SELECT *"));
                }
                for (i, f) in bind_schema.fields.iter().enumerate() {
                    exprs.push(Expr::Column(i));
                    fields.push(f.clone());
                }
            }
            SelectItem::Expr { expr, alias } => {
                let bound = if is_aggregate {
                    resolve_aggregated(expr, group_asts, aggr_asts)?
                } else {
                    bind_expr(expr, bind_schema)?
                };
                let dt = bound.data_type(bind_schema)?;
                let nullable = bound.nullable(bind_schema);
                let name = alias.clone().unwrap_or_else(|| ast_expr_name(expr));
                fields.push(Field::new(name, dt, nullable));
                let bound = match alias {
                    Some(a) => Expr::Alias(Box::new(bound), a.clone()),
                    None => bound,
                };
                exprs.push(bound);
            }
        }
    }

    Ok((exprs, Arc::new(Schema::new(fields))))
}

/// 普通(非聚合上下文)表达式绑定:列名 → 位置索引;不允许聚合函数出现。
fn bind_expr(ast: &AstExpr, schema: &Schema) -> Result<Expr> {
    match ast {
        AstExpr::Literal(lit) => Ok(Expr::Literal(literal_to_scalar(lit))),
        AstExpr::Column { table, name } => {
            let idx = schema.index_of(table.as_deref(), name)?;
            Ok(Expr::Column(idx))
        }
        AstExpr::Binary { left, op, right } => {
            let l = bind_expr(left, schema)?;
            let r = bind_expr(right, schema)?;
            // 日期列与日期字符串字面量比较时,把字面量隐式转为 Date。
            let (l, r) = coerce_date_compare(l, r, schema, *op)?;
            check_binary(*op, &l, &r, schema)?;
            Ok(Expr::Binary {
                left: Box::new(l),
                op: *op,
                right: Box::new(r),
            })
        }
        AstExpr::Unary { op, expr } => {
            let e = bind_expr(expr, schema)?;
            check_unary(*op, &e, schema)?;
            Ok(Expr::Unary {
                op: *op,
                expr: Box::new(e),
            })
        }
        AstExpr::IsNull { expr, negated } => Ok(Expr::IsNull {
            expr: Box::new(bind_expr(expr, schema)?),
            negated: *negated,
        }),
        AstExpr::Aggregate { .. } => Err(QueryError::bind("此处不允许使用聚合函数")),
        AstExpr::Wildcard => Err(QueryError::bind("非法的 `*` 用法")),
    }
}

/// 绑定一个聚合 AST 为 `Expr::Aggregate`(参数对扫描 Schema 绑定)。
fn bind_aggregate(ast: &AstExpr, schema: &Schema) -> Result<Expr> {
    let AstExpr::Aggregate { func, arg } = ast else {
        return Err(QueryError::bind("内部错误:bind_aggregate 收到非聚合表达式"));
    };
    match (func, arg.as_ref()) {
        (AggregateFunc::Count, AstExpr::Wildcard) => Ok(Expr::Aggregate {
            func: *func,
            arg: None,
        }),
        (_, AstExpr::Wildcard) => Err(QueryError::bind(format!("{}(*) 不合法", func.name()))),
        (f, a) => {
            let arg_expr = bind_expr(a, schema)?;
            let dt = arg_expr.data_type(schema)?;
            if matches!(f, AggregateFunc::Sum | AggregateFunc::Avg) && !dt.is_numeric() {
                return Err(QueryError::bind(format!(
                    "{} 需要数值参数,实际为 {dt}",
                    f.name()
                )));
            }
            Ok(Expr::Aggregate {
                func: *f,
                arg: Some(Box::new(arg_expr)),
            })
        }
    }
}

/// 在聚合上下文中解析表达式:整体匹配某分组表达式 → 对应分组列;聚合 → 对应聚合列;
/// 裸列(未出现在 GROUP BY 中)→ 报错。
fn resolve_aggregated(
    ast: &AstExpr,
    group_asts: &[AstExpr],
    aggr_asts: &[AstExpr],
) -> Result<Expr> {
    if let Some(pos) = group_asts.iter().position(|g| g == ast) {
        return Ok(Expr::Column(pos));
    }
    match ast {
        AstExpr::Aggregate { .. } => {
            let pos = aggr_asts
                .iter()
                .position(|a| a == ast)
                .ok_or_else(|| QueryError::bind("内部错误:聚合表达式未登记"))?;
            Ok(Expr::Column(group_asts.len() + pos))
        }
        AstExpr::Literal(lit) => Ok(Expr::Literal(literal_to_scalar(lit))),
        AstExpr::Binary { left, op, right } => Ok(Expr::Binary {
            left: Box::new(resolve_aggregated(left, group_asts, aggr_asts)?),
            op: *op,
            right: Box::new(resolve_aggregated(right, group_asts, aggr_asts)?),
        }),
        AstExpr::Unary { op, expr } => Ok(Expr::Unary {
            op: *op,
            expr: Box::new(resolve_aggregated(expr, group_asts, aggr_asts)?),
        }),
        AstExpr::IsNull { expr, negated } => Ok(Expr::IsNull {
            expr: Box::new(resolve_aggregated(expr, group_asts, aggr_asts)?),
            negated: *negated,
        }),
        AstExpr::Column { .. } => Err(QueryError::bind(format!(
            "列 `{}` 必须出现在 GROUP BY 中或聚合函数内",
            ast_expr_name(ast)
        ))),
        AstExpr::Wildcard => Err(QueryError::bind("非法的 `*` 用法")),
    }
}

// ---- 类型检查 ----

/// 日期列 vs 日期字符串字面量的隐式转换:把可解析的 Utf8 字面量替换为 Date 字面量。
fn coerce_date_compare(l: Expr, r: Expr, schema: &Schema, op: BinaryOp) -> Result<(Expr, Expr)> {
    if !op.is_comparison() {
        return Ok((l, r));
    }
    let lt = l.data_type(schema)?;
    let rt = r.data_type(schema)?;
    let l = if rt == DataType::Date {
        try_to_date(l)
    } else {
        l
    };
    let r = if lt == DataType::Date {
        try_to_date(r)
    } else {
        r
    };
    Ok((l, r))
}

/// 若表达式是可解析为日期的 Utf8 字面量,则转为 Date 字面量。
fn try_to_date(e: Expr) -> Expr {
    if let Expr::Literal(ScalarValue::Utf8(s)) = &e
        && let Some(days) = crate::types::date::parse_date(s)
    {
        return Expr::Literal(ScalarValue::Date(days));
    }
    e
}

fn check_binary(op: BinaryOp, l: &Expr, r: &Expr, schema: &Schema) -> Result<()> {
    // 任一侧为 NULL 字面量时放宽检查(结果按 NULL 传播)。
    if l.is_null_literal() || r.is_null_literal() {
        return Ok(());
    }
    let lt = l.data_type(schema)?;
    let rt = r.data_type(schema)?;
    if op.is_arithmetic() {
        if !lt.is_numeric() || !rt.is_numeric() {
            return Err(QueryError::type_err(format!(
                "算术运算需要数值类型: {lt} {} {rt}",
                op.symbol()
            )));
        }
    } else if op.is_comparison() {
        let ok = (lt.is_numeric() && rt.is_numeric()) || lt == rt;
        if !ok {
            return Err(QueryError::type_err(format!(
                "比较运算类型不匹配: {lt} {} {rt}",
                op.symbol()
            )));
        }
    } else {
        // 逻辑运算
        if lt != DataType::Boolean || rt != DataType::Boolean {
            return Err(QueryError::type_err(format!(
                "逻辑运算需要布尔类型: {lt} {} {rt}",
                op.symbol()
            )));
        }
    }
    Ok(())
}

fn check_unary(op: UnaryOp, e: &Expr, schema: &Schema) -> Result<()> {
    if e.is_null_literal() {
        return Ok(());
    }
    let dt = e.data_type(schema)?;
    match op {
        UnaryOp::Neg if !dt.is_numeric() => Err(QueryError::type_err(format!(
            "一元负号需要数值类型,实际 {dt}"
        ))),
        UnaryOp::Not if dt != DataType::Boolean => {
            Err(QueryError::type_err(format!("NOT 需要布尔类型,实际 {dt}")))
        }
        _ => Ok(()),
    }
}

fn ensure_boolean(expr: &Expr, schema: &Schema, ctx: &str) -> Result<()> {
    let dt = expr.data_type(schema)?;
    if dt != DataType::Boolean {
        Err(QueryError::type_err(format!(
            "{ctx} 条件必须是布尔类型,实际 {dt}"
        )))
    } else {
        Ok(())
    }
}

// ---- 辅助 ----

/// 收集表达式中出现的聚合函数(去重,不递归进聚合参数)。
fn collect_aggregates(e: &AstExpr, out: &mut Vec<AstExpr>) {
    match e {
        AstExpr::Aggregate { .. } => {
            if !out.contains(e) {
                out.push(e.clone());
            }
        }
        AstExpr::Binary { left, right, .. } => {
            collect_aggregates(left, out);
            collect_aggregates(right, out);
        }
        AstExpr::Unary { expr, .. } | AstExpr::IsNull { expr, .. } => collect_aggregates(expr, out),
        _ => {}
    }
}

/// 合并左右 Schema。LEFT JOIN 时右侧各列变为可空(无匹配行时补 NULL)。
fn merge_for_join(left: &Schema, right: &Schema, join_type: JoinType) -> Schema {
    let mut fields = left.fields.clone();
    for f in &right.fields {
        let mut f = f.clone();
        if join_type == JoinType::Left {
            f.nullable = true;
        }
        fields.push(f);
    }
    Schema::new(fields)
}

fn qualify_schema(schema: &Schema, qualifier: &str) -> Schema {
    Schema::new(
        schema
            .fields
            .iter()
            .map(|f| f.clone().with_qualifier(qualifier))
            .collect(),
    )
}

fn literal_to_scalar(lit: &Literal) -> ScalarValue {
    match lit {
        Literal::Int(v) => ScalarValue::Int64(*v),
        Literal::Float(v) => ScalarValue::Float64(*v),
        Literal::Str(s) => ScalarValue::Utf8(s.clone()),
        Literal::Bool(b) => ScalarValue::Boolean(*b),
        // NULL 字面量类型未知,占位为 Int64;类型检查对 NULL 字面量放宽。
        Literal::Null => ScalarValue::Null(DataType::Int64),
    }
}

/// 由 AST 表达式生成输出列名(用于 Schema 字段名)。
pub fn ast_expr_name(e: &AstExpr) -> String {
    match e {
        AstExpr::Column { name, .. } => name.clone(),
        AstExpr::Literal(l) => literal_name(l),
        AstExpr::Wildcard => "*".to_string(),
        AstExpr::Aggregate { func, arg } => format!("{}({})", func.name(), ast_expr_name(arg)),
        AstExpr::Binary { left, op, right } => {
            format!(
                "{} {} {}",
                ast_expr_name(left),
                op.symbol(),
                ast_expr_name(right)
            )
        }
        AstExpr::Unary { op, expr } => format!("{}{}", op.symbol(), ast_expr_name(expr)),
        AstExpr::IsNull { expr, negated } => format!(
            "{} IS{} NULL",
            ast_expr_name(expr),
            if *negated { " NOT" } else { "" }
        ),
    }
}

fn literal_name(l: &Literal) -> String {
    match l {
        Literal::Int(v) => v.to_string(),
        Literal::Float(v) => v.to_string(),
        Literal::Str(s) => format!("'{s}'"),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "NULL".to_string(),
    }
}
