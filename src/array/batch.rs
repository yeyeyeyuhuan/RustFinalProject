//! `RecordBatch`:模块间传输数据的唯一单位——一批若干行的多个列 + 其 Schema。

use std::sync::Arc;

use super::primitive::BoolArray;
use super::{ArrayRef, compute};
use crate::error::{QueryError, Result};
use crate::types::Schema;

/// 默认批大小(行数)。
pub const DEFAULT_BATCH_SIZE: usize = 4096;

/// 一批等长的列 + 其 Schema。所有数据源、所有算子的输入输出都是它。
#[derive(Debug, Clone)]
pub struct RecordBatch {
    pub schema: Arc<Schema>,
    pub columns: Vec<ArrayRef>,
}

impl RecordBatch {
    /// 构造批,校验列数与 Schema 一致、各列等长。
    pub fn try_new(schema: Arc<Schema>, columns: Vec<ArrayRef>) -> Result<Self> {
        if schema.len() != columns.len() {
            return Err(QueryError::exec(format!(
                "列数 {} 与 Schema 列数 {} 不一致",
                columns.len(),
                schema.len()
            )));
        }
        if let Some(first) = columns.first() {
            let len = first.len();
            for c in &columns {
                if c.len() != len {
                    return Err(QueryError::exec("RecordBatch 各列长度不一致"));
                }
            }
        }
        Ok(RecordBatch { schema, columns })
    }

    /// 行数(以第一列为准)。
    pub fn num_rows(&self) -> usize {
        self.columns.first().map(|c| c.len()).unwrap_or(0)
    }

    /// 列数。
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// 按位置取列。
    pub fn column(&self, i: usize) -> &ArrayRef {
        &self.columns[i]
    }

    /// Schema 引用克隆。
    pub fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    /// 投影出指定列(共享底层 buffer,零拷贝)。
    pub fn project(&self, indices: &[usize]) -> Result<RecordBatch> {
        let columns = indices
            .iter()
            .map(|&i| Arc::clone(&self.columns[i]))
            .collect();
        let schema = Arc::new(self.schema.project(indices));
        RecordBatch::try_new(schema, columns)
    }

    /// 按布尔 mask 过滤所有列,产出新批(Schema 不变)。
    pub fn filter(&self, mask: &BoolArray) -> RecordBatch {
        let columns = self
            .columns
            .iter()
            .map(|c| compute::filter(c, mask))
            .collect();
        RecordBatch {
            schema: self.schema(),
            columns,
        }
    }

    /// 取 `[offset, offset+len)` 行构成的新批(Schema 不变)。
    pub fn slice(&self, offset: usize, len: usize) -> RecordBatch {
        let indices: Vec<usize> = (offset..offset + len).collect();
        let columns = self
            .columns
            .iter()
            .map(|c| compute::take(c, &indices))
            .collect();
        RecordBatch {
            schema: self.schema(),
            columns,
        }
    }
}
