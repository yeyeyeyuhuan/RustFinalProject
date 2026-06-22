//! CSV 数据源:类型推断 + 行式 record → 列式 RecordBatch 装载 + 流式按 batch 产出。
//!
//! `csv` crate 只负责字符级 record 切分(引号转义等脏活);类型推断、列式装载、
//! batch 切分均自写。空串与解析失败 → NULL。

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{BatchIter, DataSource};
use crate::array::{
    Array, ArrayRef, BoolArray, DEFAULT_BATCH_SIZE, Float64Array, Int64Array, RecordBatch,
    Utf8Array, Validity,
};
use crate::error::{QueryError, Result};
use crate::types::{DataType, Field, Schema};

/// 类型推断采样行数。
const SAMPLE_SIZE: usize = 100;

/// 基于 CSV 文件的数据源。构造时推断 Schema,`scan` 时重新流式读取。
pub struct CsvSource {
    path: PathBuf,
    schema: Arc<Schema>,
    estimated_rows: Option<usize>,
}

impl CsvSource {
    /// 打开 CSV 文件并推断 Schema(扫描前若干行)。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut reader = open_reader(&path)?;

        let headers = reader
            .headers()
            .map_err(|e| QueryError::Csv(format!("读取表头失败: {e}")))?
            .clone();

        // 收集每列的采样值用于推断。
        let mut samples: Vec<Vec<String>> = vec![Vec::new(); headers.len()];
        let mut record = csv::StringRecord::new();
        let mut count = 0;
        while count < SAMPLE_SIZE {
            match reader.read_record(&mut record) {
                Ok(true) => {
                    for (i, col) in samples.iter_mut().enumerate() {
                        col.push(record.get(i).unwrap_or("").to_string());
                    }
                    count += 1;
                }
                Ok(false) => break,
                Err(e) => return Err(QueryError::Csv(format!("读取数据行失败: {e}"))),
            }
        }

        let fields = headers
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let dt = infer_column_type(&samples[i]);
                // 列是否可空:采样中出现空串则标记可空(保守起见统一可空)。
                Field::new(name, dt, true)
            })
            .collect();

        // 粗略行数估计:文件总字节 / 采样平均行字节。
        let estimated_rows = estimate_rows(&path, &samples, count, headers.len());

        Ok(CsvSource {
            path,
            schema: Arc::new(Schema::new(fields)),
            estimated_rows,
        })
    }
}

/// 由文件大小与采样行平均字节估算总行数。
fn estimate_rows(
    path: &Path,
    samples: &[Vec<String>],
    count: usize,
    ncols: usize,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let file_size = std::fs::metadata(path).ok()?.len() as usize;
    // 采样总字节 ≈ 各单元格长度之和 + 每行分隔符/换行(≈ ncols)。
    let cell_bytes: usize = samples.iter().flatten().map(|s| s.len()).sum();
    let avg_row = (cell_bytes + count * ncols.max(1)) / count;
    Some(file_size / avg_row.max(1))
}

impl DataSource for CsvSource {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn estimated_rows(&self) -> Option<usize> {
        self.estimated_rows
    }

    fn scan(&self, projection: Option<Vec<usize>>) -> Result<BatchIter> {
        let indices: Vec<usize> = match projection {
            Some(p) => p,
            None => (0..self.schema.len()).collect(),
        };
        let projected_schema = Arc::new(self.schema.project(&indices));
        let types: Vec<DataType> = indices
            .iter()
            .map(|&i| self.schema.field(i).data_type)
            .collect();
        let reader = open_reader(&self.path)?;

        Ok(Box::new(CsvBatchIter {
            reader,
            schema: projected_schema,
            src_indices: indices,
            types,
            finished: false,
        }))
    }
}

fn open_reader(path: &Path) -> Result<csv::Reader<File>> {
    let file = File::open(path)?;
    Ok(csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(file))
}

/// 流式按 batch 产出列式数据的迭代器。
struct CsvBatchIter {
    reader: csv::Reader<File>,
    schema: Arc<Schema>,
    src_indices: Vec<usize>,
    types: Vec<DataType>,
    finished: bool,
}

impl Iterator for CsvBatchIter {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let mut builders: Vec<ColumnBuilder> = self
            .types
            .iter()
            .map(|&dt| ColumnBuilder::new(dt))
            .collect();
        let mut rows = 0usize;
        let mut record = csv::StringRecord::new();

        loop {
            match self.reader.read_record(&mut record) {
                Ok(true) => {
                    for (b, &src) in builders.iter_mut().zip(self.src_indices.iter()) {
                        b.push(record.get(src).unwrap_or(""));
                    }
                    rows += 1;
                    if rows >= DEFAULT_BATCH_SIZE {
                        break;
                    }
                }
                Ok(false) => {
                    self.finished = true;
                    break;
                }
                Err(e) => return Some(Err(QueryError::Csv(format!("读取数据行失败: {e}")))),
            }
        }

        if rows == 0 {
            return None;
        }

        let columns: Vec<ArrayRef> = builders.into_iter().map(|b| b.finish()).collect();
        Some(RecordBatch::try_new(Arc::clone(&self.schema), columns))
    }
}

/// 列构造器:逐 record 累加单元格,按列类型解析。空串 / 解析失败 → NULL。
enum ColumnBuilder {
    Int64 {
        data: Vec<i64>,
        validity: Validity,
    },
    Float64 {
        data: Vec<f64>,
        validity: Validity,
    },
    Utf8 {
        data: Vec<String>,
        validity: Validity,
    },
    Boolean {
        data: Vec<bool>,
        validity: Validity,
    },
    Date {
        data: Vec<i32>,
        validity: Validity,
    },
}

impl ColumnBuilder {
    fn new(dt: DataType) -> Self {
        match dt {
            DataType::Int64 => ColumnBuilder::Int64 {
                data: Vec::new(),
                validity: Validity::new(),
            },
            DataType::Float64 => ColumnBuilder::Float64 {
                data: Vec::new(),
                validity: Validity::new(),
            },
            DataType::Utf8 => ColumnBuilder::Utf8 {
                data: Vec::new(),
                validity: Validity::new(),
            },
            DataType::Boolean => ColumnBuilder::Boolean {
                data: Vec::new(),
                validity: Validity::new(),
            },
            DataType::Date => ColumnBuilder::Date {
                data: Vec::new(),
                validity: Validity::new(),
            },
        }
    }

    fn push(&mut self, raw: &str) {
        let raw = raw.trim();
        match self {
            ColumnBuilder::Int64 { data, validity } => match raw.parse::<i64>() {
                Ok(v) if !raw.is_empty() => {
                    data.push(v);
                    validity.push(true);
                }
                _ => {
                    data.push(0);
                    validity.push(false);
                }
            },
            ColumnBuilder::Float64 { data, validity } => match raw.parse::<f64>() {
                Ok(v) if !raw.is_empty() => {
                    data.push(v);
                    validity.push(true);
                }
                _ => {
                    data.push(0.0);
                    validity.push(false);
                }
            },
            ColumnBuilder::Boolean { data, validity } => match parse_bool(raw) {
                Some(v) => {
                    data.push(v);
                    validity.push(true);
                }
                None => {
                    data.push(false);
                    validity.push(false);
                }
            },
            ColumnBuilder::Date { data, validity } => match crate::types::date::parse_date(raw) {
                Some(d) if !raw.is_empty() => {
                    data.push(d);
                    validity.push(true);
                }
                _ => {
                    data.push(0);
                    validity.push(false);
                }
            },
            ColumnBuilder::Utf8 { data, validity } => {
                if raw.is_empty() {
                    data.push(String::new());
                    validity.push(false);
                } else {
                    data.push(raw.to_string());
                    validity.push(true);
                }
            }
        }
    }

    fn finish(self) -> ArrayRef {
        Arc::new(match self {
            ColumnBuilder::Int64 { data, validity } => {
                Array::Int64(Int64Array::new(data, validity))
            }
            ColumnBuilder::Float64 { data, validity } => {
                Array::Float64(Float64Array::new(data, validity))
            }
            ColumnBuilder::Utf8 { data, validity } => Array::Utf8(Utf8Array::new(data, validity)),
            ColumnBuilder::Boolean { data, validity } => {
                Array::Boolean(BoolArray::new(data, validity))
            }
            ColumnBuilder::Date { data, validity } => {
                Array::Date(crate::array::Date32Array::new(data, validity))
            }
        })
    }
}

/// 推断一列的类型:全部非空样本能解析为 i64 → Int64;否则 f64 → Float64;
/// 否则布尔 → Boolean;否则 Utf8。全空列当作 Utf8。
fn infer_column_type(samples: &[String]) -> DataType {
    let mut all_int = true;
    let mut all_float = true;
    let mut all_bool = true;
    let mut all_date = true;
    let mut any = false;

    for s in samples {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        any = true;
        if s.parse::<i64>().is_err() {
            all_int = false;
        }
        if s.parse::<f64>().is_err() {
            all_float = false;
        }
        if parse_bool(s).is_none() {
            all_bool = false;
        }
        if crate::types::date::parse_date(s).is_none() {
            all_date = false;
        }
    }

    if !any {
        DataType::Utf8
    } else if all_int {
        DataType::Int64
    } else if all_float {
        DataType::Float64
    } else if all_bool {
        DataType::Boolean
    } else if all_date {
        DataType::Date
    } else {
        DataType::Utf8
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    if s.eq_ignore_ascii_case("true") {
        Some(true)
    } else if s.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScalarValue;

    #[test]
    fn infer_and_load_employees() {
        let src = CsvSource::open("testdata/employees.csv").unwrap();
        let schema = src.schema();
        assert_eq!(schema.len(), 6);
        assert_eq!(schema.field(0).data_type, DataType::Int64); // id
        assert_eq!(schema.field(1).data_type, DataType::Utf8); // name
        assert_eq!(schema.field(3).data_type, DataType::Float64); // salary
        assert_eq!(schema.field(5).data_type, DataType::Boolean); // active

        let mut iter = src.scan(None).unwrap();
        let batch = iter.next().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 10);
        assert_eq!(batch.column(1).value(0), ScalarValue::Utf8("Alice".into()));
        assert_eq!(batch.column(5).value(2), ScalarValue::Boolean(false));
        assert!(iter.next().is_none());
    }

    #[test]
    fn nulls_become_null() {
        let src = CsvSource::open("testdata/nulls.csv").unwrap();
        let mut iter = src.scan(None).unwrap();
        let batch = iter.next().unwrap().unwrap();
        // city 第 2 行(index 1)为空 → NULL
        assert!(!batch.column(1).is_valid(1));
        // score 第 3 行(index 2)为空 → NULL
        assert!(!batch.column(2).is_valid(2));
        // vip 第 4 行(index 3)为空 → NULL
        assert!(!batch.column(3).is_valid(3));
    }

    #[test]
    fn projection_reads_subset() {
        let src = CsvSource::open("testdata/employees.csv").unwrap();
        let mut iter = src.scan(Some(vec![1, 3])).unwrap();
        let batch = iter.next().unwrap().unwrap();
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.schema.field(0).name, "name");
        assert_eq!(batch.schema.field(1).name, "salary");
    }
}
