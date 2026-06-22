//! 物理计划生成:把 `LogicalPlan` 翻译成物理算子树,并做物理层决策
//! (逻辑 Aggregate → HashAggregate)。

use std::collections::BTreeSet;
use std::sync::Arc;

use super::PhysicalOperator;
use super::expr::compile;
use super::operators::{
    AggrSpec, FilterExec, HashAggregateExec, HashJoinExec, LimitExec, ProjectionExec, ScanExec,
    SortExec,
};
use crate::error::{QueryError, Result};
use crate::logical::{Expr, LogicalPlan};
use crate::sql::ast::{BinaryOp, JoinType};
use crate::types::DataType;

/// 物理计划生成配置。
#[derive(Clone, Copy, Default)]
pub struct PlannerConfig {
    /// 是否启用 partitioned 并行(聚合 / 连接)。
    pub parallel: bool,
}

/// 把逻辑计划编译为可执行的物理算子树(串行)。
pub fn create_physical_plan(plan: &LogicalPlan) -> Result<Box<dyn PhysicalOperator>> {
    create_physical_plan_with(plan, &PlannerConfig::default())
}

/// 按配置(可启用并行)把逻辑计划编译为物理算子树。
pub fn create_physical_plan_with(
    plan: &LogicalPlan,
    cfg: &PlannerConfig,
) -> Result<Box<dyn PhysicalOperator>> {
    match plan {
        LogicalPlan::Scan {
            source,
            projection,
            projected_schema,
            ..
        } => {
            let iter = source.scan(projection.clone())?;
            Ok(Box::new(ScanExec::new(iter, Arc::clone(projected_schema))))
        }
        LogicalPlan::Filter { predicate, input } => {
            let child = create_physical_plan_with(input, cfg)?;
            let schema = child.schema();
            let pred = compile(predicate)?;
            Ok(Box::new(FilterExec::new(child, pred, schema)))
        }
        LogicalPlan::Projection {
            exprs,
            schema,
            input,
        } => {
            let child = create_physical_plan_with(input, cfg)?;
            let pes = exprs.iter().map(compile).collect::<Result<Vec<_>>>()?;
            Ok(Box::new(ProjectionExec::new(
                child,
                pes,
                Arc::clone(schema),
            )))
        }
        LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema,
            input,
        } => {
            let child = create_physical_plan_with(input, cfg)?;
            let group_pes = group_expr.iter().map(compile).collect::<Result<Vec<_>>>()?;
            let group_n = group_expr.len();
            let mut specs = Vec::with_capacity(aggr_expr.len());
            for (j, a) in aggr_expr.iter().enumerate() {
                let Expr::Aggregate { func, arg } = a else {
                    return Err(QueryError::exec("聚合节点包含非聚合表达式"));
                };
                let arg_pe = match arg {
                    Some(e) => Some(compile(e)?),
                    None => None,
                };
                let out_type = schema.field(group_n + j).data_type;
                specs.push(AggrSpec {
                    func: *func,
                    arg: arg_pe,
                    out_type,
                });
            }
            Ok(Box::new(HashAggregateExec::new(
                child,
                group_pes,
                specs,
                Arc::clone(schema),
                cfg.parallel,
            )))
        }
        LogicalPlan::Sort { exprs, input } => {
            let child = create_physical_plan_with(input, cfg)?;
            let schema = child.schema();
            let keys = exprs
                .iter()
                .map(|s| compile(&s.expr).map(|pe| (pe, s.asc)))
                .collect::<Result<Vec<_>>>()?;
            Ok(Box::new(SortExec::new(child, keys, schema)))
        }
        LogicalPlan::Limit { skip, fetch, input } => {
            let child = create_physical_plan_with(input, cfg)?;
            Ok(Box::new(LimitExec::new(child, *skip, *fetch)))
        }
        LogicalPlan::Join {
            left,
            right,
            on,
            join_type,
            schema,
        } => {
            let left_schema = left.schema();
            let right_schema = right.schema();
            let left_ncols = left_schema.len();

            // 代价优化:INNER 选较小一侧 build(未知按较大处理);LEFT 固定 build 右侧、
            // probe 左侧,以便对无匹配的左行内联补 NULL。
            let build_left = match join_type {
                JoinType::Left => false,
                JoinType::Inner => {
                    let l = left.estimated_rows().unwrap_or(usize::MAX);
                    let r = right.estimated_rows().unwrap_or(usize::MAX);
                    l <= r
                }
            };

            let left_op = create_physical_plan_with(left, cfg)?;
            let right_op = create_physical_plan_with(right, cfg)?;

            let (mut left_keys, mut right_keys, residual) = extract_equijoin(on, left_ncols);
            if left_keys.is_empty() {
                return Err(QueryError::exec(
                    "JOIN 需要至少一个等值条件(暂不支持纯 theta join)",
                ));
            }
            // 隐式数值提升:键两侧类型不一致且均为数值 → 统一转 Float64,保证哈希键可匹配。
            for (lk, rk) in left_keys.iter_mut().zip(right_keys.iter_mut()) {
                let lt = lk.data_type(&left_schema)?;
                let rt = rk.data_type(&right_schema)?;
                if lt != rt && lt.is_numeric() && rt.is_numeric() {
                    *lk = Expr::Cast {
                        expr: Box::new(lk.clone()),
                        to: DataType::Float64,
                    };
                    *rk = Expr::Cast {
                        expr: Box::new(rk.clone()),
                        to: DataType::Float64,
                    };
                }
            }
            let left_pe = left_keys.iter().map(compile).collect::<Result<Vec<_>>>()?;
            let right_pe = right_keys.iter().map(compile).collect::<Result<Vec<_>>>()?;
            let filter = match residual {
                Some(e) => Some(compile(&e)?),
                None => None,
            };
            Ok(Box::new(HashJoinExec::new(
                left_op,
                right_op,
                left_pe,
                right_pe,
                filter,
                Arc::clone(schema),
                *join_type,
                build_left,
                cfg.parallel,
            )))
        }
    }
}

/// 从对合并 Schema 绑定的连接条件中抽取等值键对与残余(非等值)条件。
///
/// 返回(左键表达式[左算子列空间], 右键表达式[右算子列空间], 残余过滤[合并列空间])。
fn extract_equijoin(on: &Expr, left_ncols: usize) -> (Vec<Expr>, Vec<Expr>, Option<Expr>) {
    let mut conjuncts = Vec::new();
    split_and(on, &mut conjuncts);

    let mut left_keys = Vec::new();
    let mut right_keys = Vec::new();
    let mut residual: Vec<Expr> = Vec::new();

    for c in conjuncts {
        if let Expr::Binary {
            left,
            op: BinaryOp::Eq,
            right,
        } = c
        {
            match (side_of(left, left_ncols), side_of(right, left_ncols)) {
                (Some(Side::Left), Some(Side::Right)) => {
                    left_keys.push((**left).clone());
                    right_keys.push(shift_columns(right, left_ncols));
                    continue;
                }
                (Some(Side::Right), Some(Side::Left)) => {
                    left_keys.push((**right).clone());
                    right_keys.push(shift_columns(left, left_ncols));
                    continue;
                }
                _ => {}
            }
        }
        residual.push(c.clone());
    }

    (left_keys, right_keys, combine_and(residual))
}

#[derive(PartialEq)]
enum Side {
    Left,
    Right,
}

/// 表达式引用的列全在左侧 → Left;全在右侧 → Right;无列或跨侧 → None。
fn side_of(e: &Expr, left_ncols: usize) -> Option<Side> {
    let mut cols = BTreeSet::new();
    collect_columns(e, &mut cols);
    if cols.is_empty() {
        return None;
    }
    if cols.iter().all(|&i| i < left_ncols) {
        Some(Side::Left)
    } else if cols.iter().all(|&i| i >= left_ncols) {
        Some(Side::Right)
    } else {
        None
    }
}

fn collect_columns(e: &Expr, out: &mut BTreeSet<usize>) {
    match e {
        Expr::Column(i) => {
            out.insert(*i);
        }
        Expr::Literal(_) => {}
        Expr::Alias(inner, _) => collect_columns(inner, out),
        Expr::Cast { expr, .. } | Expr::IsNull { expr, .. } | Expr::Unary { expr, .. } => {
            collect_columns(expr, out)
        }
        Expr::Binary { left, right, .. } => {
            collect_columns(left, out);
            collect_columns(right, out);
        }
        Expr::Aggregate { arg, .. } => {
            if let Some(a) = arg {
                collect_columns(a, out);
            }
        }
    }
}

/// 把表达式中的列索引整体平移 `offset`(右侧键从合并空间移到右算子空间)。
fn shift_columns(e: &Expr, offset: usize) -> Expr {
    match e {
        Expr::Column(i) => Expr::Column(i - offset),
        Expr::Literal(v) => Expr::Literal(v.clone()),
        Expr::Alias(inner, name) => {
            Expr::Alias(Box::new(shift_columns(inner, offset)), name.clone())
        }
        Expr::IsNull { expr, negated } => Expr::IsNull {
            expr: Box::new(shift_columns(expr, offset)),
            negated: *negated,
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op: *op,
            expr: Box::new(shift_columns(expr, offset)),
        },
        Expr::Binary { left, op, right } => Expr::Binary {
            left: Box::new(shift_columns(left, offset)),
            op: *op,
            right: Box::new(shift_columns(right, offset)),
        },
        Expr::Aggregate { func, arg } => Expr::Aggregate {
            func: *func,
            arg: arg.as_ref().map(|a| Box::new(shift_columns(a, offset))),
        },
        Expr::Cast { expr, to } => Expr::Cast {
            expr: Box::new(shift_columns(expr, offset)),
            to: *to,
        },
    }
}

fn split_and<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::Binary {
        left,
        op: BinaryOp::And,
        right,
    } = e
    {
        split_and(left, out);
        split_and(right, out);
    } else {
        out.push(e);
    }
}

fn combine_and(mut parts: Vec<Expr>) -> Option<Expr> {
    let mut acc = parts.pop()?;
    while let Some(p) = parts.pop() {
        acc = Expr::Binary {
            left: Box::new(p),
            op: BinaryOp::And,
            right: Box::new(acc),
        };
    }
    Some(acc)
}
