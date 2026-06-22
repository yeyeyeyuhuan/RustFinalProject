//! Limit 算子:取前 N 行,可带跳过;取满后短路提前结束。

use std::sync::Arc;

use crate::array::RecordBatch;
use crate::error::Result;
use crate::physical::PhysicalOperator;
use crate::types::Schema;

/// Limit 算子。
pub struct LimitExec {
    input: Box<dyn PhysicalOperator>,
    skip: usize,
    fetch: Option<usize>,
    skipped: usize,
    emitted: usize,
    schema: Arc<Schema>,
}

impl LimitExec {
    pub fn new(input: Box<dyn PhysicalOperator>, skip: usize, fetch: Option<usize>) -> Self {
        let schema = input.schema();
        LimitExec {
            input,
            skip,
            fetch,
            skipped: 0,
            emitted: 0,
            schema,
        }
    }
}

impl PhysicalOperator for LimitExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            let mut batch = match self.input.next_batch()? {
                Some(b) => b,
                None => return Ok(None),
            };
            let mut rows = batch.num_rows();
            if rows == 0 {
                continue;
            }

            // 跳过阶段
            if self.skipped < self.skip {
                let to_skip = (self.skip - self.skipped).min(rows);
                self.skipped += to_skip;
                if to_skip == rows {
                    continue;
                }
                batch = batch.slice(to_skip, rows - to_skip);
                rows = batch.num_rows();
            }

            // 取数阶段
            if let Some(f) = self.fetch {
                if self.emitted >= f {
                    return Ok(None);
                }
                let remaining = f - self.emitted;
                if rows > remaining {
                    batch = batch.slice(0, remaining);
                }
            }

            self.emitted += batch.num_rows();
            return Ok(Some(batch));
        }
    }
}
