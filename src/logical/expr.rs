//! 绑定后的逻辑表达式:列引用以**位置索引**表示,携带可推导的类型信息。

use crate::error::{QueryError, Result};
use crate::sql::ast::{AggregateFunc, BinaryOp, UnaryOp};
use crate::types::{DataType, ScalarValue, Schema};

/// 绑定后、带类型信息的表达式树。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 列引用(在所属计划节点输入 Schema 中的位置索引)。
    Column(usize),
    /// 字面量常量。
    Literal(ScalarValue),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    /// `expr IS [NOT] NULL`。
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    /// 聚合函数。`arg` 为 None 表示 `COUNT(*)`。
    Aggregate {
        func: AggregateFunc,
        arg: Option<Box<Expr>>,
    },
    /// 输出列别名(仅影响输出列名,不影响求值)。
    Alias(Box<Expr>, String),
    /// 类型转换(隐式提升用,如 Int64 → Float64)。
    Cast {
        expr: Box<Expr>,
        to: DataType,
    },
}

impl Expr {
    /// 在给定输入 Schema 下推导结果类型。
    pub fn data_type(&self, schema: &Schema) -> Result<DataType> {
        match self {
            Expr::Column(i) => Ok(schema.field(*i).data_type),
            Expr::Literal(v) => Ok(v.data_type()),
            Expr::Alias(e, _) => e.data_type(schema),
            Expr::Cast { to, .. } => Ok(*to),
            Expr::IsNull { .. } => Ok(DataType::Boolean),
            Expr::Unary { op, expr } => match op {
                UnaryOp::Neg => expr.data_type(schema),
                UnaryOp::Not => Ok(DataType::Boolean),
            },
            Expr::Binary { left, op, right } => {
                if op.is_comparison() || op.is_logical() {
                    Ok(DataType::Boolean)
                } else if *op == BinaryOp::Div {
                    Ok(DataType::Float64)
                } else {
                    let lt = left.data_type(schema)?;
                    let rt = right.data_type(schema)?;
                    DataType::numeric_result(lt, rt).ok_or_else(|| {
                        QueryError::type_err(format!("算术运算类型非法: {lt} {} {rt}", op.symbol()))
                    })
                }
            }
            Expr::Aggregate { func, arg } => match func {
                AggregateFunc::Count => Ok(DataType::Int64),
                AggregateFunc::Avg => Ok(DataType::Float64),
                AggregateFunc::Sum | AggregateFunc::Min | AggregateFunc::Max => {
                    let a = arg
                        .as_ref()
                        .ok_or_else(|| QueryError::bind(format!("{} 需要参数", func.name())))?;
                    a.data_type(schema)
                }
            },
        }
    }

    /// 结果是否可能为 NULL(保守估计)。
    pub fn nullable(&self, schema: &Schema) -> bool {
        match self {
            Expr::Column(i) => schema.field(*i).nullable,
            Expr::Literal(v) => v.is_null(),
            Expr::Alias(e, _) => e.nullable(schema),
            Expr::Cast { expr, .. } => expr.nullable(schema),
            Expr::IsNull { .. } => false,
            Expr::Unary { expr, .. } => expr.nullable(schema),
            Expr::Binary { left, right, .. } => left.nullable(schema) || right.nullable(schema),
            Expr::Aggregate { func, .. } => !matches!(func, AggregateFunc::Count),
        }
    }

    /// 收集表达式引用到的所有列索引。
    pub fn collect_columns(&self, out: &mut std::collections::BTreeSet<usize>) {
        match self {
            Expr::Column(i) => {
                out.insert(*i);
            }
            Expr::Literal(_) => {}
            Expr::Alias(inner, _) | Expr::Cast { expr: inner, .. } => inner.collect_columns(out),
            Expr::IsNull { expr, .. } | Expr::Unary { expr, .. } => expr.collect_columns(out),
            Expr::Binary { left, right, .. } => {
                left.collect_columns(out);
                right.collect_columns(out);
            }
            Expr::Aggregate { arg, .. } => {
                if let Some(a) = arg {
                    a.collect_columns(out);
                }
            }
        }
    }

    /// 按映射重写列索引(列裁剪后位置变化时用)。映射中缺失的列将 panic(调用方需保证完整)。
    pub fn remap_columns(&self, map: &std::collections::HashMap<usize, usize>) -> Expr {
        match self {
            Expr::Column(i) => Expr::Column(*map.get(i).expect("remap 缺少列映射")),
            Expr::Literal(v) => Expr::Literal(v.clone()),
            Expr::Alias(inner, name) => {
                Expr::Alias(Box::new(inner.remap_columns(map)), name.clone())
            }
            Expr::Cast { expr, to } => Expr::Cast {
                expr: Box::new(expr.remap_columns(map)),
                to: *to,
            },
            Expr::IsNull { expr, negated } => Expr::IsNull {
                expr: Box::new(expr.remap_columns(map)),
                negated: *negated,
            },
            Expr::Unary { op, expr } => Expr::Unary {
                op: *op,
                expr: Box::new(expr.remap_columns(map)),
            },
            Expr::Binary { left, op, right } => Expr::Binary {
                left: Box::new(left.remap_columns(map)),
                op: *op,
                right: Box::new(right.remap_columns(map)),
            },
            Expr::Aggregate { func, arg } => Expr::Aggregate {
                func: *func,
                arg: arg.as_ref().map(|a| Box::new(a.remap_columns(map))),
            },
        }
    }

    /// 是否为 NULL 字面量(类型检查时对其放宽)。
    pub fn is_null_literal(&self) -> bool {
        matches!(self, Expr::Literal(ScalarValue::Null(_)))
    }
}
