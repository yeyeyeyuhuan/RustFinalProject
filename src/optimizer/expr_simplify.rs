//! 表达式化简:布尔恒等式(`x AND true`、`x OR false`、恒真恒假)与双重否定消除。

use super::{OptimizerRule, map_plan_exprs};
use crate::error::Result;
use crate::logical::Expr;
use crate::logical::LogicalPlan;
use crate::sql::ast::{BinaryOp, UnaryOp};
use crate::types::ScalarValue;

pub struct ExprSimplify;

impl OptimizerRule for ExprSimplify {
    fn name(&self) -> &str {
        "ExprSimplify"
    }

    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan> {
        Ok(map_plan_exprs(plan, &simplify_expr))
    }
}

fn simplify_expr(e: &Expr) -> Expr {
    match e {
        Expr::Binary { left, op, right } => {
            let l = simplify_expr(left);
            let r = simplify_expr(right);
            match op {
                BinaryOp::And => {
                    if is_true(&l) {
                        return r;
                    }
                    if is_true(&r) {
                        return l;
                    }
                    if is_false(&l) || is_false(&r) {
                        return lit_bool(false);
                    }
                }
                BinaryOp::Or => {
                    if is_false(&l) {
                        return r;
                    }
                    if is_false(&r) {
                        return l;
                    }
                    if is_true(&l) || is_true(&r) {
                        return lit_bool(true);
                    }
                }
                _ => {}
            }
            Expr::Binary {
                left: Box::new(l),
                op: *op,
                right: Box::new(r),
            }
        }
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
        } => {
            let inner = simplify_expr(expr);
            // NOT NOT x → x
            if let Expr::Unary {
                op: UnaryOp::Not,
                expr: inner2,
            } = inner
            {
                return *inner2;
            }
            Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(inner),
            }
        }
        Expr::Unary { op, expr } => Expr::Unary {
            op: *op,
            expr: Box::new(simplify_expr(expr)),
        },
        Expr::IsNull { expr, negated } => Expr::IsNull {
            expr: Box::new(simplify_expr(expr)),
            negated: *negated,
        },
        Expr::Cast { expr, to } => Expr::Cast {
            expr: Box::new(simplify_expr(expr)),
            to: *to,
        },
        Expr::Alias(inner, name) => Expr::Alias(Box::new(simplify_expr(inner)), name.clone()),
        Expr::Aggregate { func, arg } => Expr::Aggregate {
            func: *func,
            arg: arg.as_ref().map(|a| Box::new(simplify_expr(a))),
        },
        Expr::Column(_) | Expr::Literal(_) => e.clone(),
    }
}

fn is_true(e: &Expr) -> bool {
    matches!(e, Expr::Literal(ScalarValue::Boolean(true)))
}

fn is_false(e: &Expr) -> bool {
    matches!(e, Expr::Literal(ScalarValue::Boolean(false)))
}

fn lit_bool(b: bool) -> Expr {
    Expr::Literal(ScalarValue::Boolean(b))
}
