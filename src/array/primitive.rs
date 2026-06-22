//! 各具体类型的列式底层存储。NULL 位置在 `data` 中保留占位值,以 `validity` 为准。

use super::bitmap::Validity;

/// 定长基本类型列(整型 / 浮点),`data[i]` 在 `validity` 标记无效时为占位值。
#[derive(Debug, Clone)]
pub struct PrimitiveArray<T> {
    pub data: Vec<T>,
    pub validity: Validity,
}

impl<T> PrimitiveArray<T> {
    pub fn new(data: Vec<T>, validity: Validity) -> Self {
        PrimitiveArray { data, validity }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 64 位整型列。
pub type Int64Array = PrimitiveArray<i64>;
/// 64 位浮点列。
pub type Float64Array = PrimitiveArray<f64>;
/// 日期列(距 1970-01-01 的天数)。
pub type Date32Array = PrimitiveArray<i32>;

/// 变长字符串列。
#[derive(Debug, Clone)]
pub struct Utf8Array {
    pub data: Vec<String>,
    pub validity: Validity,
}

impl Utf8Array {
    pub fn new(data: Vec<String>, validity: Validity) -> Self {
        Utf8Array { data, validity }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 布尔列。
#[derive(Debug, Clone)]
pub struct BoolArray {
    pub data: Vec<bool>,
    pub validity: Validity,
}

impl BoolArray {
    pub fn new(data: Vec<bool>, validity: Validity) -> Self {
        BoolArray { data, validity }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
