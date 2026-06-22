//! 哈希分组 / join 键:把一行键值编码为可哈希、可比较相等的形式(f64 用位模式)。
//! HashAggregate 与 HashJoin 共用。

use crate::types::ScalarValue;

/// 单个键单元。
#[derive(Hash, PartialEq, Eq, Clone)]
pub(crate) enum KeyValue {
    Null,
    Int(i64),
    Float(u64),
    Str(String),
    Bool(bool),
    Date(i32),
}

impl KeyValue {
    pub(crate) fn from_scalar(v: &ScalarValue) -> Self {
        match v {
            ScalarValue::Null(_) => KeyValue::Null,
            ScalarValue::Int64(x) => KeyValue::Int(*x),
            ScalarValue::Float64(x) => KeyValue::Float(x.to_bits()),
            ScalarValue::Utf8(s) => KeyValue::Str(s.clone()),
            ScalarValue::Boolean(b) => KeyValue::Bool(*b),
            ScalarValue::Date(d) => KeyValue::Date(*d),
        }
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self, KeyValue::Null)
    }
}

/// 把一组标量编码为整行键。
pub(crate) fn row_key(values: &[ScalarValue]) -> Vec<KeyValue> {
    values.iter().map(KeyValue::from_scalar).collect()
}

/// 按键哈希计算所属分区(partitioned 并行聚合 / 连接用)。
pub(crate) fn partition_of(key: &[KeyValue], parts: usize) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    (h.finish() % parts as u64) as usize
}
