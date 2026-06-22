//! Filter 算子:对每 batch 求值布尔条件,按 mask 过滤产出新 batch。

use std::sync::Arc;

use crate::array::{Array, RecordBatch};
use crate::error::{QueryError, Result};
use crate::physical::{PhysicalExpr, PhysicalOperator};
use crate::types::Schema;

/// 过滤算子。
pub struct FilterExec {
    input: Box<dyn PhysicalOperator>,
    predicate: PhysicalExpr,
    schema: Arc<Schema>,
}

impl FilterExec {
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        predicate: PhysicalExpr,
        schema: Arc<Schema>,
    ) -> Self {
        FilterExec {
            input,
            predicate,
            schema,
        }
    }
}

impl PhysicalOperator for FilterExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            let batch = match self.input.next_batch()? {
                Some(b) => b,
                None => return Ok(None),
            };
            let mask = self.predicate.evaluate(&batch)?;
            let bool_arr = match mask.as_ref() {
                Array::Boolean(b) => b,
                _ => return Err(QueryError::exec("WHERE 条件未求值为布尔列")),
            };
            let out = batch.filter(bool_arr);
            if out.num_rows() > 0 {
                return Ok(Some(out));
            }
            // 整批被过滤掉则继续拉取下一批,避免产出空批。
        }
    }
}
