//! 引擎支持的数据类型与类型推导规则。

use std::fmt;

/// 引擎支持的数据类型(刻意保持封闭固定的小集合,便于 `enum Array` 分发)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Int64,
    Float64,
    Utf8,
    Boolean,
    /// 日期(内部存储为距 1970-01-01 的天数,i32)。
    Date,
}

impl DataType {
    /// 是否为数值类型(Int64 / Float64)。
    pub fn is_numeric(&self) -> bool {
        matches!(self, DataType::Int64 | DataType::Float64)
    }

    /// 类型名(用于报错与表头打印)。
    pub fn name(&self) -> &'static str {
        match self {
            DataType::Int64 => "Int64",
            DataType::Float64 => "Float64",
            DataType::Utf8 => "Utf8",
            DataType::Boolean => "Boolean",
            DataType::Date => "Date",
        }
    }

    /// `from` 是否能隐式提升到 `to`。
    ///
    /// 规则:同类型恒可;Int64 可提升到 Float64(数值放宽);其余不可。
    pub fn can_coerce(from: DataType, to: DataType) -> bool {
        from == to || (from == DataType::Int64 && to == DataType::Float64)
    }

    /// 两个数值类型做算术的结果类型:任一为 Float64 则结果 Float64,否则 Int64。
    pub fn numeric_result(lhs: DataType, rhs: DataType) -> Option<DataType> {
        match (lhs, rhs) {
            (DataType::Int64, DataType::Int64) => Some(DataType::Int64),
            (l, r) if l.is_numeric() && r.is_numeric() => Some(DataType::Float64),
            _ => None,
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
