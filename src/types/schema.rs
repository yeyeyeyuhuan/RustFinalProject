//! Schema:有序的(列名, 类型, 是否可空)序列。支持按名 / 按位查列与列名歧义检测。

use std::sync::Arc;

use super::datatype::DataType;
use crate::error::{QueryError, Result};

/// 一列的元信息。`qualifier` 为可选的表限定(如表别名),用于 JOIN 后区分同名列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub qualifier: Option<String>,
}

impl Field {
    /// 构造无表限定的列。
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Self {
        Field {
            name: name.into(),
            data_type,
            nullable,
            qualifier: None,
        }
    }

    /// 设定表限定(返回带限定的新 Field)。
    pub fn with_qualifier(mut self, qualifier: impl Into<String>) -> Self {
        self.qualifier = Some(qualifier.into());
        self
    }
}

/// 一组有序列构成的表结构。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    pub fields: Vec<Field>,
}

impl Schema {
    /// 由列序列构造 Schema。
    pub fn new(fields: Vec<Field>) -> Self {
        Schema { fields }
    }

    /// 空 Schema。
    pub fn empty() -> Self {
        Schema { fields: Vec::new() }
    }

    /// 列数。
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// 是否无列。
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// 按位置取列。
    pub fn field(&self, index: usize) -> &Field {
        &self.fields[index]
    }

    /// 按列名(大小写不敏感)定位列位置。可带表限定 `qualifier`。
    ///
    /// 命中 0 个 → 列不存在;命中多个 → 列名歧义,均返回 `QueryError::Bind`。
    pub fn index_of(&self, qualifier: Option<&str>, name: &str) -> Result<usize> {
        let matches: Vec<usize> = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.name.eq_ignore_ascii_case(name)
                    && match qualifier {
                        None => true,
                        Some(q) => f
                            .qualifier
                            .as_deref()
                            .is_some_and(|fq| fq.eq_ignore_ascii_case(q)),
                    }
            })
            .map(|(i, _)| i)
            .collect();

        match matches.as_slice() {
            [] => Err(QueryError::bind(format!(
                "列 `{}` 不存在",
                qualified_name(qualifier, name)
            ))),
            [i] => Ok(*i),
            _ => Err(QueryError::bind(format!(
                "列 `{}` 有歧义,匹配到 {} 列",
                qualified_name(qualifier, name),
                matches.len()
            ))),
        }
    }

    /// 投影出指定位置的列构成的新 Schema。
    pub fn project(&self, indices: &[usize]) -> Schema {
        Schema {
            fields: indices.iter().map(|&i| self.fields[i].clone()).collect(),
        }
    }

    /// 合并两个 Schema(JOIN 用):左列在前,右列在后。
    pub fn merge(&self, other: &Schema) -> Schema {
        let mut fields = self.fields.clone();
        fields.extend(other.fields.iter().cloned());
        Schema { fields }
    }

    /// 便捷包装为 `Arc`。
    pub fn into_ref(self) -> Arc<Schema> {
        Arc::new(self)
    }
}

fn qualified_name(qualifier: Option<&str>, name: &str) -> String {
    match qualifier {
        Some(q) => format!("{q}.{name}"),
        None => name.to_string(),
    }
}
