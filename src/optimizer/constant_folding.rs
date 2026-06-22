//! 常量折叠:编译期算掉两侧均为字面量的子表达式。

use super::{OptimizerRule, map_plan_exprs};
use crate::error::Result;
use crate::logical::{Expr, LogicalPlan};
use crate::sql::ast::{BinaryOp, UnaryOp};
use crate::types::{DataType, ScalarValue};

pub struct ConstantFolding;

impl OptimizerRule for ConstantFolding {
    fn name(&self) -> &str {
        "ConstantFolding"
    }

    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan> {
        Ok(map_plan_exprs(plan, &fold_expr))
    }
}

/// 自底向上折叠表达式。
fn fold_expr(e: &Expr) -> Expr {
    match e {
        Expr::Binary { left, op, right } => {
            let l = fold_expr(left);
            let r = fold_expr(right);
            if let (Expr::Literal(lv), Expr::Literal(rv)) = (&l, &r)
                && let Some(res) = eval_binary(*op, lv, rv)
            {
                return Expr::Literal(res);
            }
            Expr::Binary {
                left: Box::new(l),
                op: *op,
                right: Box::new(r),
            }
        }
        Expr::Unary { op, expr } => {
            let inner = fold_expr(expr);
            if let Expr::Literal(v) = &inner
                && let Some(res) = eval_unary(*op, v)
            {
                return Expr::Literal(res);
            }
            Expr::Unary {
                op: *op,
                expr: Box::new(inner),
            }
        }
        Expr::IsNull { expr, negated } => {
            let inner = fold_expr(expr);
            if let Expr::Literal(v) = &inner {
                let is_null = v.is_null();
                return Expr::Literal(ScalarValue::Boolean(if *negated {
                    !is_null
                } else {
                    is_null
                }));
            }
            Expr::IsNull {
                expr: Box::new(inner),
                negated: *negated,
            }
        }
        Expr::Cast { expr, to } => Expr::Cast {
            expr: Box::new(fold_expr(expr)),
            to: *to,
        },
        Expr::Alias(inner, name) => Expr::Alias(Box::new(fold_expr(inner)), name.clone()),
        Expr::Aggregate { func, arg } => Expr::Aggregate {
            func: *func,
            arg: arg.as_ref().map(|a| Box::new(fold_expr(a))),
        },
        Expr::Column(_) | Expr::Literal(_) => e.clone(),
    }
}

/// 对两个非空字面量求值二元运算。无法折叠(含 NULL / 类型非法)返回 None。
fn eval_binary(op: BinaryOp, l: &ScalarValue, r: &ScalarValue) -> Option<ScalarValue> {
    if l.is_null() || r.is_null() {
        return None;
    }
    if op.is_logical() {
        let (lb, rb) = (as_bool(l)?, as_bool(r)?);
        return Some(ScalarValue::Boolean(match op {
            BinaryOp::And => lb && rb,
            BinaryOp::Or => lb || rb,
            _ => unreachable!(),
        }));
    }
    if op.is_comparison() {
        let ord = l.order_cmp(r);
        use std::cmp::Ordering::*;
        return Some(ScalarValue::Boolean(match op {
            BinaryOp::Eq => ord == Equal,
            BinaryOp::NotEq => ord != Equal,
            BinaryOp::Lt => ord == Less,
            BinaryOp::LtEq => ord != Greater,
            BinaryOp::Gt => ord == Greater,
            BinaryOp::GtEq => ord != Less,
            _ => unreachable!(),
        }));
    }
    // 算术:仅数值
    let (la, ra) = (l.as_f64()?, r.as_f64()?);
    let both_int = l.data_type() == DataType::Int64 && r.data_type() == DataType::Int64;
    if both_int && op != BinaryOp::Div {
        let (li, ri) = (la as i64, ra as i64);
        Some(ScalarValue::Int64(match op {
            BinaryOp::Add => li.wrapping_add(ri),
            BinaryOp::Sub => li.wrapping_sub(ri),
            BinaryOp::Mul => li.wrapping_mul(ri),
            _ => unreachable!(),
        }))
    } else {
        if op == BinaryOp::Div && ra == 0.0 {
            return None; // 不折叠除零
        }
        Some(ScalarValue::Float64(match op {
            BinaryOp::Add => la + ra,
            BinaryOp::Sub => la - ra,
            BinaryOp::Mul => la * ra,
            BinaryOp::Div => la / ra,
            _ => unreachable!(),
        }))
    }
}

fn eval_unary(op: UnaryOp, v: &ScalarValue) -> Option<ScalarValue> {
    if v.is_null() {
        return None;
    }
    match op {
        UnaryOp::Not => Some(ScalarValue::Boolean(!as_bool(v)?)),
        UnaryOp::Neg => match v {
            ScalarValue::Int64(x) => Some(ScalarValue::Int64(x.wrapping_neg())),
            ScalarValue::Float64(x) => Some(ScalarValue::Float64(-x)),
            _ => None,
        },
    }
}

fn as_bool(v: &ScalarValue) -> Option<bool> {
    match v {
        ScalarValue::Boolean(b) => Some(*b),
        _ => None,
    }
}
