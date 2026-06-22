//! 物理表达式:把逻辑 `Expr` 编译为可对 `RecordBatch` **整批向量化**求值的形式。
//!
//! 求值一次处理一整列,全程正确传播 NULL(任一操作数为 NULL → 结果 NULL;
//! 逻辑运算采用 SQL 三值逻辑)。聚合不在此处求值,由 HashAggregate 算子处理。

use std::cmp::Ordering;
use std::sync::Arc;

use crate::array::{Array, ArrayRef, BoolArray, Float64Array, Int64Array, RecordBatch, Validity};
use crate::error::{QueryError, Result};
use crate::logical::Expr;
use crate::sql::ast::{BinaryOp, UnaryOp};
use crate::types::{DataType, ScalarValue};

/// 编译后的物理表达式。
#[derive(Debug, Clone)]
pub enum PhysicalExpr {
    Column(usize),
    Literal(ScalarValue),
    Binary {
        left: Box<PhysicalExpr>,
        op: BinaryOp,
        right: Box<PhysicalExpr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<PhysicalExpr>,
    },
    IsNull {
        expr: Box<PhysicalExpr>,
        negated: bool,
    },
    Cast {
        expr: Box<PhysicalExpr>,
        to: DataType,
    },
}

/// 把逻辑表达式编译为物理表达式(`Alias` 透明剥离,聚合报错)。
pub fn compile(e: &Expr) -> Result<PhysicalExpr> {
    match e {
        Expr::Column(i) => Ok(PhysicalExpr::Column(*i)),
        Expr::Literal(v) => Ok(PhysicalExpr::Literal(v.clone())),
        Expr::Alias(inner, _) => compile(inner),
        Expr::Binary { left, op, right } => Ok(PhysicalExpr::Binary {
            left: Box::new(compile(left)?),
            op: *op,
            right: Box::new(compile(right)?),
        }),
        Expr::Unary { op, expr } => Ok(PhysicalExpr::Unary {
            op: *op,
            expr: Box::new(compile(expr)?),
        }),
        Expr::IsNull { expr, negated } => Ok(PhysicalExpr::IsNull {
            expr: Box::new(compile(expr)?),
            negated: *negated,
        }),
        Expr::Cast { expr, to } => Ok(PhysicalExpr::Cast {
            expr: Box::new(compile(expr)?),
            to: *to,
        }),
        Expr::Aggregate { .. } => Err(QueryError::exec("聚合表达式不能直接编译为物理表达式")),
    }
}

impl PhysicalExpr {
    /// 对一批数据整列求值,产出结果列。
    pub fn evaluate(&self, batch: &RecordBatch) -> Result<ArrayRef> {
        match self {
            PhysicalExpr::Column(i) => Ok(Arc::clone(batch.column(*i))),
            PhysicalExpr::Literal(v) => {
                let n = batch.num_rows();
                let vals = vec![v.clone(); n];
                Ok(Array::from_scalars(v.data_type(), &vals))
            }
            PhysicalExpr::Binary { left, op, right } => {
                let l = left.evaluate(batch)?;
                let r = right.evaluate(batch)?;
                if op.is_arithmetic() {
                    arithmetic(*op, &l, &r)
                } else if op.is_comparison() {
                    comparison(*op, &l, &r)
                } else {
                    logical(*op, &l, &r)
                }
            }
            PhysicalExpr::Unary { op, expr } => {
                let a = expr.evaluate(batch)?;
                match op {
                    UnaryOp::Neg => negate(&a),
                    UnaryOp::Not => not_eval(&a),
                }
            }
            PhysicalExpr::IsNull { expr, negated } => {
                let a = expr.evaluate(batch)?;
                Ok(is_null_eval(&a, *negated))
            }
            PhysicalExpr::Cast { expr, to } => {
                let a = expr.evaluate(batch)?;
                cast(&a, *to)
            }
        }
    }
}

/// 列类型转换(仅支持数值互转与到字符串;同类型直接共享)。
fn cast(a: &Array, to: DataType) -> Result<ArrayRef> {
    if a.data_type() == to {
        return Ok(Arc::new(a.clone()));
    }
    let n = a.len();
    match to {
        DataType::Float64 => {
            let mut data = Vec::with_capacity(n);
            let mut validity = Validity::with_capacity(n);
            for i in 0..n {
                match a.f64_at(i) {
                    Some(v) => {
                        data.push(v);
                        validity.push(true);
                    }
                    None => {
                        data.push(0.0);
                        validity.push(false);
                    }
                }
            }
            Ok(Arc::new(Array::Float64(Float64Array::new(data, validity))))
        }
        DataType::Int64 => {
            let mut data = Vec::with_capacity(n);
            let mut validity = Validity::with_capacity(n);
            for i in 0..n {
                match a.f64_at(i) {
                    Some(v) => {
                        data.push(v as i64);
                        validity.push(true);
                    }
                    None => {
                        data.push(0);
                        validity.push(false);
                    }
                }
            }
            Ok(Arc::new(Array::Int64(Int64Array::new(data, validity))))
        }
        DataType::Utf8 => {
            let mut data = Vec::with_capacity(n);
            let mut validity = Validity::with_capacity(n);
            for i in 0..n {
                if a.is_valid(i) {
                    data.push(a.value(i).to_string());
                    validity.push(true);
                } else {
                    data.push(String::new());
                    validity.push(false);
                }
            }
            Ok(Arc::new(Array::Utf8(crate::array::Utf8Array::new(
                data, validity,
            ))))
        }
        DataType::Date => Err(QueryError::type_err("不支持转换为 Date")),
        DataType::Boolean => Err(QueryError::type_err("不支持转换为 Boolean")),
    }
}

// ---- 算术 ----

fn arithmetic(op: BinaryOp, l: &Array, r: &Array) -> Result<ArrayRef> {
    let n = l.len();
    if l.data_type() == DataType::Int64 && r.data_type() == DataType::Int64 && op != BinaryOp::Div {
        let mut data = Vec::with_capacity(n);
        let mut validity = Validity::with_capacity(n);
        for i in 0..n {
            match (int_at(l, i), int_at(r, i)) {
                (Some(a), Some(b)) => {
                    data.push(apply_int(op, a, b));
                    validity.push(true);
                }
                _ => {
                    data.push(0);
                    validity.push(false);
                }
            }
        }
        Ok(Arc::new(Array::Int64(Int64Array::new(data, validity))))
    } else {
        let mut data = Vec::with_capacity(n);
        let mut validity = Validity::with_capacity(n);
        for i in 0..n {
            match (l.f64_at(i), r.f64_at(i)) {
                (Some(a), Some(b)) => {
                    if op == BinaryOp::Div && b == 0.0 {
                        data.push(0.0);
                        validity.push(false);
                    } else {
                        data.push(apply_float(op, a, b));
                        validity.push(true);
                    }
                }
                _ => {
                    data.push(0.0);
                    validity.push(false);
                }
            }
        }
        Ok(Arc::new(Array::Float64(Float64Array::new(data, validity))))
    }
}

fn apply_int(op: BinaryOp, a: i64, b: i64) -> i64 {
    match op {
        BinaryOp::Add => a.wrapping_add(b),
        BinaryOp::Sub => a.wrapping_sub(b),
        BinaryOp::Mul => a.wrapping_mul(b),
        _ => 0,
    }
}

fn apply_float(op: BinaryOp, a: f64, b: f64) -> f64 {
    match op {
        BinaryOp::Add => a + b,
        BinaryOp::Sub => a - b,
        BinaryOp::Mul => a * b,
        BinaryOp::Div => a / b,
        _ => 0.0,
    }
}

// ---- 比较 ----

fn comparison(op: BinaryOp, l: &Array, r: &Array) -> Result<ArrayRef> {
    let n = l.len();
    let (lt, rt) = (l.data_type(), r.data_type());
    let mut data = Vec::with_capacity(n);
    let mut validity = Validity::with_capacity(n);

    for i in 0..n {
        let res: Option<bool> = if !l.is_valid(i) || !r.is_valid(i) {
            None
        } else if lt.is_numeric() && rt.is_numeric() {
            let a = l.f64_at(i).unwrap();
            let b = r.f64_at(i).unwrap();
            Some(cmp_apply(op, a.partial_cmp(&b)))
        } else if lt == DataType::Utf8 && rt == DataType::Utf8 {
            Some(cmp_apply(op, Some(str_at(l, i).cmp(str_at(r, i)))))
        } else if lt == DataType::Boolean && rt == DataType::Boolean {
            Some(cmp_apply(op, Some(bool_at(l, i).cmp(&bool_at(r, i)))))
        } else if lt == DataType::Date && rt == DataType::Date {
            Some(cmp_apply(op, Some(l.value(i).order_cmp(&r.value(i)))))
        } else {
            return Err(QueryError::type_err(format!(
                "比较类型不匹配: {lt} vs {rt}"
            )));
        };
        match res {
            Some(b) => {
                data.push(b);
                validity.push(true);
            }
            None => {
                data.push(false);
                validity.push(false);
            }
        }
    }
    Ok(Arc::new(Array::Boolean(BoolArray::new(data, validity))))
}

fn cmp_apply(op: BinaryOp, ord: Option<Ordering>) -> bool {
    match ord {
        None => false, // NaN 等不可比 → false
        Some(o) => match op {
            BinaryOp::Eq => o == Ordering::Equal,
            BinaryOp::NotEq => o != Ordering::Equal,
            BinaryOp::Lt => o == Ordering::Less,
            BinaryOp::LtEq => o != Ordering::Greater,
            BinaryOp::Gt => o == Ordering::Greater,
            BinaryOp::GtEq => o != Ordering::Less,
            _ => false,
        },
    }
}

// ---- 逻辑(三值)----

fn logical(op: BinaryOp, l: &Array, r: &Array) -> Result<ArrayRef> {
    if l.data_type() != DataType::Boolean || r.data_type() != DataType::Boolean {
        return Err(QueryError::type_err("逻辑运算需要布尔列"));
    }
    let n = l.len();
    let mut data = Vec::with_capacity(n);
    let mut validity = Validity::with_capacity(n);

    for i in 0..n {
        let a = if l.is_valid(i) {
            Some(bool_at(l, i))
        } else {
            None
        };
        let b = if r.is_valid(i) {
            Some(bool_at(r, i))
        } else {
            None
        };
        let res = match op {
            BinaryOp::And => match (a, b) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            BinaryOp::Or => match (a, b) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            _ => None,
        };
        match res {
            Some(x) => {
                data.push(x);
                validity.push(true);
            }
            None => {
                data.push(false);
                validity.push(false);
            }
        }
    }
    Ok(Arc::new(Array::Boolean(BoolArray::new(data, validity))))
}

// ---- 一元 ----

fn negate(a: &Array) -> Result<ArrayRef> {
    match a {
        Array::Int64(p) => {
            let mut data = Vec::with_capacity(p.data.len());
            let mut validity = Validity::with_capacity(p.data.len());
            for i in 0..p.data.len() {
                data.push(p.data[i].wrapping_neg());
                validity.push(p.validity.is_valid(i));
            }
            Ok(Arc::new(Array::Int64(Int64Array::new(data, validity))))
        }
        Array::Float64(p) => {
            let mut data = Vec::with_capacity(p.data.len());
            let mut validity = Validity::with_capacity(p.data.len());
            for i in 0..p.data.len() {
                data.push(-p.data[i]);
                validity.push(p.validity.is_valid(i));
            }
            Ok(Arc::new(Array::Float64(Float64Array::new(data, validity))))
        }
        _ => Err(QueryError::type_err("一元负号需要数值列")),
    }
}

fn not_eval(a: &Array) -> Result<ArrayRef> {
    let Array::Boolean(p) = a else {
        return Err(QueryError::type_err("NOT 需要布尔列"));
    };
    let mut data = Vec::with_capacity(p.data.len());
    let mut validity = Validity::with_capacity(p.data.len());
    for i in 0..p.data.len() {
        if p.validity.is_valid(i) {
            data.push(!p.data[i]);
            validity.push(true);
        } else {
            data.push(false);
            validity.push(false);
        }
    }
    Ok(Arc::new(Array::Boolean(BoolArray::new(data, validity))))
}

fn is_null_eval(a: &Array, negated: bool) -> ArrayRef {
    let n = a.len();
    let mut data = Vec::with_capacity(n);
    for i in 0..n {
        let is_null = !a.is_valid(i);
        data.push(if negated { !is_null } else { is_null });
    }
    Arc::new(Array::Boolean(BoolArray::new(data, Validity::all_valid(n))))
}

// ---- 取值辅助 ----

fn int_at(a: &Array, i: usize) -> Option<i64> {
    match a {
        Array::Int64(p) if p.validity.is_valid(i) => Some(p.data[i]),
        _ => None,
    }
}

fn bool_at(a: &Array, i: usize) -> bool {
    match a {
        Array::Boolean(p) => p.data[i],
        _ => false,
    }
}

fn str_at(a: &Array, i: usize) -> &str {
    match a {
        Array::Utf8(p) => &p.data[i],
        _ => "",
    }
}
