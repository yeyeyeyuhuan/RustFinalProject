//! 列式内存模型:统一的 `Array` 枚举、`RecordBatch` 列批,以及 filter/take/concat 原语。
//!
//! ## 所有权策略
//! 列以 `ArrayRef = Arc<Array>` 共享。批之间传递列时**共享底层 buffer 而非深拷贝**;
//! filter/take 等产出**新** `Arc<Array>`(因为行集变了,无法借用原 buffer)。
//! 这样既享受零拷贝传递,又保持不可变共享的安全性(由 `Arc` 与 Rust 类型系统保证)。

mod batch;
mod bitmap;
mod compute;
mod primitive;

pub use batch::{DEFAULT_BATCH_SIZE, RecordBatch};
pub use bitmap::Validity;
pub use compute::{concat, filter, take};
pub use primitive::{BoolArray, Date32Array, Float64Array, Int64Array, PrimitiveArray, Utf8Array};

use std::sync::Arc;

use crate::types::{DataType, ScalarValue};

/// 统一的列式数组抽象。类型集合封闭固定,用 `enum` 分发(无虚表开销,`match` 最直观)。
#[derive(Debug, Clone)]
pub enum Array {
    Int64(Int64Array),
    Float64(Float64Array),
    Utf8(Utf8Array),
    Boolean(BoolArray),
    Date(Date32Array),
}

/// 共享列引用。
pub type ArrayRef = Arc<Array>;

impl Array {
    /// 行数。
    pub fn len(&self) -> usize {
        match self {
            Array::Int64(a) => a.len(),
            Array::Float64(a) => a.len(),
            Array::Utf8(a) => a.len(),
            Array::Boolean(a) => a.len(),
            Array::Date(a) => a.len(),
        }
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 列的数据类型。
    pub fn data_type(&self) -> DataType {
        match self {
            Array::Int64(_) => DataType::Int64,
            Array::Float64(_) => DataType::Float64,
            Array::Utf8(_) => DataType::Utf8,
            Array::Boolean(_) => DataType::Boolean,
            Array::Date(_) => DataType::Date,
        }
    }

    /// 位置 `i` 是否有效(非 NULL)。
    pub fn is_valid(&self, i: usize) -> bool {
        match self {
            Array::Int64(a) => a.validity.is_valid(i),
            Array::Float64(a) => a.validity.is_valid(i),
            Array::Utf8(a) => a.validity.is_valid(i),
            Array::Boolean(a) => a.validity.is_valid(i),
            Array::Date(a) => a.validity.is_valid(i),
        }
    }

    /// 取位置 `i` 的标量值(NULL 返回带类型的 `ScalarValue::Null`)。
    pub fn value(&self, i: usize) -> ScalarValue {
        if !self.is_valid(i) {
            return ScalarValue::Null(self.data_type());
        }
        match self {
            Array::Int64(a) => ScalarValue::Int64(a.data[i]),
            Array::Float64(a) => ScalarValue::Float64(a.data[i]),
            Array::Utf8(a) => ScalarValue::Utf8(a.data[i].clone()),
            Array::Boolean(a) => ScalarValue::Boolean(a.data[i]),
            Array::Date(a) => ScalarValue::Date(a.data[i]),
        }
    }

    /// 数值视图:Int64/Float64 取为 `f64`,NULL 或非数值返回 None。
    pub fn f64_at(&self, i: usize) -> Option<f64> {
        if !self.is_valid(i) {
            return None;
        }
        match self {
            Array::Int64(a) => Some(a.data[i] as f64),
            Array::Float64(a) => Some(a.data[i]),
            _ => None,
        }
    }

    /// 由一组标量值构造指定类型的列(用于聚合结果、常量列等)。
    pub fn from_scalars(data_type: DataType, values: &[ScalarValue]) -> ArrayRef {
        let n = values.len();
        let mut validity = Validity::with_capacity(n);
        Arc::new(match data_type {
            DataType::Int64 => {
                let mut data = Vec::with_capacity(n);
                for v in values {
                    match v {
                        ScalarValue::Int64(x) => {
                            data.push(*x);
                            validity.push(true);
                        }
                        _ => {
                            data.push(0);
                            validity.push(false);
                        }
                    }
                }
                Array::Int64(Int64Array::new(data, validity))
            }
            DataType::Float64 => {
                let mut data = Vec::with_capacity(n);
                for v in values {
                    match v.as_f64() {
                        Some(x) if !v.is_null() => {
                            data.push(x);
                            validity.push(true);
                        }
                        _ => {
                            data.push(0.0);
                            validity.push(false);
                        }
                    }
                }
                Array::Float64(Float64Array::new(data, validity))
            }
            DataType::Utf8 => {
                let mut data = Vec::with_capacity(n);
                for v in values {
                    match v {
                        ScalarValue::Utf8(s) => {
                            data.push(s.clone());
                            validity.push(true);
                        }
                        _ => {
                            data.push(String::new());
                            validity.push(false);
                        }
                    }
                }
                Array::Utf8(Utf8Array::new(data, validity))
            }
            DataType::Boolean => {
                let mut data = Vec::with_capacity(n);
                for v in values {
                    match v {
                        ScalarValue::Boolean(b) => {
                            data.push(*b);
                            validity.push(true);
                        }
                        _ => {
                            data.push(false);
                            validity.push(false);
                        }
                    }
                }
                Array::Boolean(BoolArray::new(data, validity))
            }
            DataType::Date => {
                let mut data = Vec::with_capacity(n);
                for v in values {
                    match v {
                        ScalarValue::Date(d) => {
                            data.push(*d);
                            validity.push(true);
                        }
                        _ => {
                            data.push(0);
                            validity.push(false);
                        }
                    }
                }
                Array::Date(Date32Array::new(data, validity))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_and_null() {
        let a = Array::Int64(Int64Array::new(vec![1, 0, 3], {
            let mut v = Validity::new();
            v.push(true);
            v.push(false);
            v.push(true);
            v
        }));
        assert_eq!(a.len(), 3);
        assert_eq!(a.value(0), ScalarValue::Int64(1));
        assert!(a.value(1).is_null());
        assert_eq!(a.f64_at(2), Some(3.0));
        assert_eq!(a.f64_at(1), None);
    }

    #[test]
    fn from_scalars_roundtrip() {
        let vals = vec![
            ScalarValue::Float64(1.5),
            ScalarValue::Null(DataType::Float64),
            ScalarValue::Int64(2),
        ];
        let arr = Array::from_scalars(DataType::Float64, &vals);
        assert_eq!(arr.f64_at(0), Some(1.5));
        assert!(!arr.is_valid(1));
        assert_eq!(arr.f64_at(2), Some(2.0));
    }
}
