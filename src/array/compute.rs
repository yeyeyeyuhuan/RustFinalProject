//! 列操作原语:按布尔 mask 过滤、按下标 gather/take(排序与 join 重排用)、拼接。
//! 这些是上层算子复用的基础。

use std::sync::Arc;

use super::bitmap::Validity;
use super::primitive::{BoolArray, PrimitiveArray, Utf8Array};
use super::{Array, ArrayRef};
use crate::error::{QueryError, Result};

/// 按布尔 mask 过滤:保留 mask 有效且为 true 的行。NULL mask 视为 false。
pub fn filter(array: &Array, mask: &BoolArray) -> ArrayRef {
    let keep: Vec<usize> = (0..mask.data.len())
        .filter(|&i| mask.validity.is_valid(i) && mask.data[i])
        .collect();
    take(array, &keep)
}

/// 按下标重排 / 抽取:产出由 `indices` 指定行构成的新列。
pub fn take(array: &Array, indices: &[usize]) -> ArrayRef {
    Arc::new(match array {
        Array::Int64(a) => Array::Int64(take_primitive(a, indices)),
        Array::Float64(a) => Array::Float64(take_primitive(a, indices)),
        Array::Date(a) => Array::Date(take_primitive(a, indices)),
        Array::Utf8(a) => {
            let mut data = Vec::with_capacity(indices.len());
            let mut validity = Validity::with_capacity(indices.len());
            for &i in indices {
                data.push(a.data[i].clone());
                validity.push(a.validity.is_valid(i));
            }
            Array::Utf8(Utf8Array::new(data, validity))
        }
        Array::Boolean(a) => {
            let mut data = Vec::with_capacity(indices.len());
            let mut validity = Validity::with_capacity(indices.len());
            for &i in indices {
                data.push(a.data[i]);
                validity.push(a.validity.is_valid(i));
            }
            Array::Boolean(BoolArray::new(data, validity))
        }
    })
}

fn take_primitive<T: Copy>(a: &PrimitiveArray<T>, indices: &[usize]) -> PrimitiveArray<T> {
    let mut data = Vec::with_capacity(indices.len());
    let mut validity = Validity::with_capacity(indices.len());
    for &i in indices {
        data.push(a.data[i]);
        validity.push(a.validity.is_valid(i));
    }
    PrimitiveArray::new(data, validity)
}

/// 拼接同类型的多个列(并行结果合并、阻塞算子收集全量数据用)。
pub fn concat(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let first = arrays
        .first()
        .ok_or_else(|| QueryError::exec("concat 输入为空"))?;
    let dt = first.data_type();
    for a in arrays {
        if a.data_type() != dt {
            return Err(QueryError::exec(format!(
                "concat 类型不一致: {} vs {}",
                dt,
                a.data_type()
            )));
        }
    }

    Ok(match dt {
        crate::types::DataType::Int64 => {
            let mut data = Vec::new();
            let mut validity = Validity::new();
            for arr in arrays {
                if let Array::Int64(a) = arr.as_ref() {
                    data.extend_from_slice(&a.data);
                    validity.extend_from(&a.validity);
                }
            }
            Arc::new(Array::Int64(PrimitiveArray::new(data, validity)))
        }
        crate::types::DataType::Float64 => {
            let mut data = Vec::new();
            let mut validity = Validity::new();
            for arr in arrays {
                if let Array::Float64(a) = arr.as_ref() {
                    data.extend_from_slice(&a.data);
                    validity.extend_from(&a.validity);
                }
            }
            Arc::new(Array::Float64(PrimitiveArray::new(data, validity)))
        }
        crate::types::DataType::Utf8 => {
            let mut data = Vec::new();
            let mut validity = Validity::new();
            for arr in arrays {
                if let Array::Utf8(a) = arr.as_ref() {
                    data.extend_from_slice(&a.data);
                    validity.extend_from(&a.validity);
                }
            }
            Arc::new(Array::Utf8(Utf8Array::new(data, validity)))
        }
        crate::types::DataType::Boolean => {
            let mut data = Vec::new();
            let mut validity = Validity::new();
            for arr in arrays {
                if let Array::Boolean(a) = arr.as_ref() {
                    data.extend_from_slice(&a.data);
                    validity.extend_from(&a.validity);
                }
            }
            Arc::new(Array::Boolean(BoolArray::new(data, validity)))
        }
        crate::types::DataType::Date => {
            let mut data = Vec::new();
            let mut validity = Validity::new();
            for arr in arrays {
                if let Array::Date(a) = arr.as_ref() {
                    data.extend_from_slice(&a.data);
                    validity.extend_from(&a.validity);
                }
            }
            Arc::new(Array::Date(PrimitiveArray::new(data, validity)))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScalarValue;

    fn int_array(vals: &[Option<i64>]) -> Array {
        let mut data = Vec::new();
        let mut validity = Validity::new();
        for v in vals {
            match v {
                Some(x) => {
                    data.push(*x);
                    validity.push(true);
                }
                None => {
                    data.push(0);
                    validity.push(false);
                }
            }
        }
        Array::Int64(PrimitiveArray::new(data, validity))
    }

    #[test]
    fn filter_keeps_true_rows() {
        let a = int_array(&[Some(10), Some(20), Some(30)]);
        let mask = BoolArray::new(vec![true, false, true], Validity::all_valid(3));
        let out = filter(&a, &mask);
        assert_eq!(out.len(), 2);
        assert_eq!(out.value(0), ScalarValue::Int64(10));
        assert_eq!(out.value(1), ScalarValue::Int64(30));
    }

    #[test]
    fn take_reorders() {
        let a = int_array(&[Some(1), Some(2), Some(3)]);
        let out = take(&a, &[2, 0]);
        assert_eq!(out.value(0), ScalarValue::Int64(3));
        assert_eq!(out.value(1), ScalarValue::Int64(1));
    }

    #[test]
    fn concat_appends() {
        let a = Arc::new(int_array(&[Some(1), None]));
        let b = Arc::new(int_array(&[Some(3)]));
        let out = concat(&[a, b]).unwrap();
        assert_eq!(out.len(), 3);
        assert!(!out.is_valid(1));
        assert_eq!(out.value(2), ScalarValue::Int64(3));
    }
}
