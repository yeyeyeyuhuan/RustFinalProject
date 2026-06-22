//! 逻辑计划的树形缩进打印(供 `.explain` 与报告使用)。

use std::fmt::Write;

use super::expr::Expr;
use super::plan::LogicalPlan;
use crate::sql::ast::{AggregateFunc, UnaryOp};
use crate::types::Schema;

/// 把逻辑计划渲染成多行缩进文本。
pub fn explain(plan: &LogicalPlan) -> String {
    let mut out = String::new();
    fmt_plan(plan, 0, &mut out);
    out
}

fn fmt_plan(plan: &LogicalPlan, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match plan {
        LogicalPlan::Scan {
            table_name,
            projected_schema,
            projection,
            ..
        } => {
            let cols = if projection.is_some() {
                format!(" 投影列={}", schema_cols(projected_schema))
            } else {
                String::new()
            };
            let _ = writeln!(out, "{pad}Scan: {table_name}{cols}");
        }
        LogicalPlan::Filter { predicate, input } => {
            let _ = writeln!(out, "{pad}Filter: {}", expr_str(predicate));
            fmt_plan(input, indent + 1, out);
        }
        LogicalPlan::Projection { exprs, input, .. } => {
            let _ = writeln!(out, "{pad}Projection: {}", exprs_str(exprs));
            fmt_plan(input, indent + 1, out);
        }
        LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            input,
            ..
        } => {
            let _ = writeln!(
                out,
                "{pad}Aggregate: groupBy=[{}], aggr=[{}]",
                exprs_str(group_expr),
                exprs_str(aggr_expr)
            );
            fmt_plan(input, indent + 1, out);
        }
        LogicalPlan::Sort { exprs, input } => {
            let keys: Vec<String> = exprs
                .iter()
                .map(|s| {
                    format!(
                        "{} {}",
                        expr_str(&s.expr),
                        if s.asc { "ASC" } else { "DESC" }
                    )
                })
                .collect();
            let _ = writeln!(out, "{pad}Sort: {}", keys.join(", "));
            fmt_plan(input, indent + 1, out);
        }
        LogicalPlan::Limit { skip, fetch, input } => {
            let f = fetch
                .map(|n| n.to_string())
                .unwrap_or_else(|| "ALL".to_string());
            let _ = writeln!(out, "{pad}Limit: skip={skip}, fetch={f}");
            fmt_plan(input, indent + 1, out);
        }
        LogicalPlan::Join {
            left,
            right,
            on,
            join_type,
            ..
        } => {
            let _ = writeln!(out, "{pad}Join({}): on {}", join_type.name(), expr_str(on));
            fmt_plan(left, indent + 1, out);
            fmt_plan(right, indent + 1, out);
        }
    }
}

fn schema_cols(schema: &Schema) -> String {
    schema
        .fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn exprs_str(exprs: &[Expr]) -> String {
    exprs.iter().map(expr_str).collect::<Vec<_>>().join(", ")
}

/// 把表达式渲染为可读文本(列以 `#i` 表示位置索引)。
pub fn expr_str(e: &Expr) -> String {
    match e {
        Expr::Column(i) => format!("#{i}"),
        Expr::Literal(v) => v.to_string(),
        Expr::Alias(inner, name) => format!("{} AS {name}", expr_str(inner)),
        Expr::Cast { expr, to } => format!("CAST({} AS {to})", expr_str(expr)),
        Expr::IsNull { expr, negated } => format!(
            "{} IS{} NULL",
            expr_str(expr),
            if *negated { " NOT" } else { "" }
        ),
        Expr::Unary { op, expr } => match op {
            UnaryOp::Neg => format!("-{}", expr_str(expr)),
            UnaryOp::Not => format!("NOT {}", expr_str(expr)),
        },
        Expr::Binary { left, op, right } => {
            format!("({} {} {})", expr_str(left), op.symbol(), expr_str(right))
        }
        Expr::Aggregate { func, arg } => {
            let inner = match arg {
                Some(a) => expr_str(a),
                None => "*".to_string(),
            };
            let name = match func {
                AggregateFunc::Count => "COUNT",
                AggregateFunc::Sum => "SUM",
                AggregateFunc::Avg => "AVG",
                AggregateFunc::Min => "MIN",
                AggregateFunc::Max => "MAX",
            };
            format!("{name}({inner})")
        }
    }
}
