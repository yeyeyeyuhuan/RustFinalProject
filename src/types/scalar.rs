//! 标量值:表示单个带类型的值,可表示 NULL。用于字面量、聚合中间结果、单格输出等。

use std::cmp::Ordering;
use std::fmt;

use super::datatype::DataType;

/// 单个标量值,带类型信息且能表示 NULL。
#[derive(Debug, Clone)]
pub enum ScalarValue {
    /// 带类型的 NULL(保留类型便于构造同类型列)。
    Null(DataType),
    Int64(i64),
    Float64(f64),
    Utf8(String),
    Boolean(bool),
    /// 日期(距 1970-01-01 的天数)。
    Date(i32),
}

impl ScalarValue {
    /// 该标量值的数据类型。
    pub fn data_type(&self) -> DataType {
        match self {
            ScalarValue::Null(dt) => *dt,
            ScalarValue::Int64(_) => DataType::Int64,
            ScalarValue::Float64(_) => DataType::Float64,
            ScalarValue::Utf8(_) => DataType::Utf8,
            ScalarValue::Boolean(_) => DataType::Boolean,
            ScalarValue::Date(_) => DataType::Date,
        }
    }

    /// 是否为 NULL。
    pub fn is_null(&self) -> bool {
        matches!(self, ScalarValue::Null(_))
    }

    /// 取数值视图(Int64/Float64 → f64),NULL 或非数值返回 None。用于跨数值类型比较。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ScalarValue::Int64(v) => Some(*v as f64),
            ScalarValue::Float64(v) => Some(*v),
            _ => None,
        }
    }

    /// 排序 / 分组用的全序比较:NULL 视为小于任何非 NULL 值,两 NULL 相等。
    /// 数值跨 Int64/Float64 统一按 f64 比较;字符串按字典序;布尔 false < true。
    pub fn order_cmp(&self, other: &ScalarValue) -> Ordering {
        match (self.is_null(), other.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => match (self, other) {
                (ScalarValue::Utf8(a), ScalarValue::Utf8(b)) => a.cmp(b),
                (ScalarValue::Boolean(a), ScalarValue::Boolean(b)) => a.cmp(b),
                (ScalarValue::Date(a), ScalarValue::Date(b)) => a.cmp(b),
                _ => {
                    let a = self.as_f64().unwrap_or(f64::NAN);
                    let b = other.as_f64().unwrap_or(f64::NAN);
                    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
                }
            },
        }
    }
}

impl PartialEq for ScalarValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ScalarValue::Null(a), ScalarValue::Null(b)) => a == b,
            (ScalarValue::Int64(a), ScalarValue::Int64(b)) => a == b,
            (ScalarValue::Float64(a), ScalarValue::Float64(b)) => a == b,
            (ScalarValue::Utf8(a), ScalarValue::Utf8(b)) => a == b,
            (ScalarValue::Boolean(a), ScalarValue::Boolean(b)) => a == b,
            (ScalarValue::Date(a), ScalarValue::Date(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for ScalarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScalarValue::Null(_) => f.write_str("NULL"),
            ScalarValue::Int64(v) => write!(f, "{v}"),
            ScalarValue::Float64(v) => write!(f, "{v}"),
            ScalarValue::Utf8(v) => f.write_str(v),
            ScalarValue::Boolean(v) => write!(f, "{v}"),
            ScalarValue::Date(days) => f.write_str(&super::date::format_date(*days)),
        }
    }
}
