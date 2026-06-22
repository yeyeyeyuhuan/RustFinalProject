//! 查询优化器:一组可插拔的规则 + 不动点驱动器。
//!
//! 每条规则是 `OptimizerRule`:输入一棵逻辑计划,输出**等价但更优**的逻辑计划
//! (不可变重写——返回新树而非原地改,体现所有权转移)。驱动器反复套用全部规则
//! 直到计划不再变化(不动点)。

mod column_pruning;
mod constant_folding;
mod expr_simplify;
mod predicate_pushdown;
mod projection_pushdown;

use column_pruning::ColumnPruning;
use constant_folding::ConstantFolding;
use expr_simplify::ExprSimplify;
use predicate_pushdown::PredicatePushdown;
use projection_pushdown::ProjectionPushdown;

use crate::error::Result;
use crate::logical::{Expr, LogicalPlan, SortExpr, explain};

/// 一条优化规则。
pub trait OptimizerRule {
    fn name(&self) -> &str;
    /// 消费旧计划,返回等价的新计划。
    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan>;
}

/// 优化器驱动器。
pub struct Optimizer {
    rules: Vec<Box<dyn OptimizerRule>>,
}

impl Optimizer {
    pub fn new() -> Self {
        Optimizer {
            rules: vec![
                Box::new(ConstantFolding),
                Box::new(ExprSimplify),
                Box::new(PredicatePushdown),
                Box::new(ProjectionPushdown),
                Box::new(ColumnPruning),
            ],
        }
    }

    /// 反复套用全部规则直到不动点(或达到迭代上限)。
    pub fn optimize(&self, mut plan: LogicalPlan) -> Result<LogicalPlan> {
        const MAX_ITER: usize = 10;
        for _ in 0..MAX_ITER {
            let before = explain(&plan);
            for rule in &self.rules {
                plan = rule.rewrite(plan)?;
            }
            if explain(&plan) == before {
                break;
            }
        }
        Ok(plan)
    }
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

// ---- 共享辅助 ----

/// 对每个子计划套用 `f` 并重建节点(单层,不递归)。
pub(crate) fn map_children<F>(plan: LogicalPlan, mut f: F) -> Result<LogicalPlan>
where
    F: FnMut(LogicalPlan) -> Result<LogicalPlan>,
{
    Ok(match plan {
        LogicalPlan::Scan { .. } => plan,
        LogicalPlan::Filter { predicate, input } => LogicalPlan::Filter {
            predicate,
            input: Box::new(f(*input)?),
        },
        LogicalPlan::Projection {
            exprs,
            schema,
            input,
        } => LogicalPlan::Projection {
            exprs,
            schema,
            input: Box::new(f(*input)?),
        },
        LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema,
            input,
        } => LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema,
            input: Box::new(f(*input)?),
        },
        LogicalPlan::Sort { exprs, input } => LogicalPlan::Sort {
            exprs,
            input: Box::new(f(*input)?),
        },
        LogicalPlan::Limit { skip, fetch, input } => LogicalPlan::Limit {
            skip,
            fetch,
            input: Box::new(f(*input)?),
        },
        LogicalPlan::Join {
            left,
            right,
            on,
            join_type,
            schema,
        } => LogicalPlan::Join {
            left: Box::new(f(*left)?),
            right: Box::new(f(*right)?),
            on,
            join_type,
            schema,
        },
    })
}

/// 对计划中所有表达式套用 `f`(并递归子计划)。用于纯表达式重写规则。
pub(crate) fn map_plan_exprs(plan: LogicalPlan, f: &dyn Fn(&Expr) -> Expr) -> LogicalPlan {
    let plan = match plan {
        LogicalPlan::Scan { .. } => plan,
        LogicalPlan::Filter { predicate, input } => LogicalPlan::Filter {
            predicate: f(&predicate),
            input,
        },
        LogicalPlan::Projection {
            exprs,
            schema,
            input,
        } => LogicalPlan::Projection {
            exprs: exprs.iter().map(f).collect(),
            schema,
            input,
        },
        LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema,
            input,
        } => LogicalPlan::Aggregate {
            group_expr: group_expr.iter().map(f).collect(),
            aggr_expr: aggr_expr.iter().map(f).collect(),
            schema,
            input,
        },
        LogicalPlan::Sort { exprs, input } => LogicalPlan::Sort {
            exprs: exprs
                .into_iter()
                .map(|s| SortExpr {
                    expr: f(&s.expr),
                    asc: s.asc,
                })
                .collect(),
            input,
        },
        LogicalPlan::Limit { skip, fetch, input } => LogicalPlan::Limit { skip, fetch, input },
        LogicalPlan::Join {
            left,
            right,
            on,
            join_type,
            schema,
        } => LogicalPlan::Join {
            left,
            right,
            on: f(&on),
            join_type,
            schema,
        },
    };
    // 递归子计划
    map_children(plan, |c| Ok(map_plan_exprs(c, f))).unwrap()
}
