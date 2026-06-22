//! Scan 算子:从数据源逐 batch 拉取(承接下推后的列裁剪)。

use std::sync::Arc;

use crate::array::RecordBatch;
use crate::datasource::BatchIter;
use crate::error::Result;
use crate::physical::PhysicalOperator;
use crate::types::Schema;

/// 表扫描算子。
pub struct ScanExec {
    schema: Arc<Schema>,
    iter: BatchIter,
}

impl ScanExec {
    pub fn new(iter: BatchIter, schema: Arc<Schema>) -> Self {
        ScanExec { schema, iter }
    }
}

impl PhysicalOperator for ScanExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.iter.next().transpose()
    }
}
