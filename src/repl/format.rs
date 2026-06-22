//! 结果表格化输出。

use comfy_table::Table;

use crate::array::RecordBatch;
use crate::types::Schema;

/// 把结果批渲染为对齐表格 + 行数统计。
pub fn format_batches(batches: &[RecordBatch]) -> String {
    let Some(first) = batches.first() else {
        return "(0 行)".to_string();
    };
    let schema = &first.schema;

    let mut table = Table::new();
    table.set_header(
        schema
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>(),
    );

    let mut total = 0;
    for batch in batches {
        for row in 0..batch.num_rows() {
            let cells: Vec<String> = batch
                .columns
                .iter()
                .map(|c| c.value(row).to_string())
                .collect();
            table.add_row(cells);
            total += 1;
        }
    }

    format!("{table}\n({total} 行)")
}

/// 渲染表结构。
pub fn format_schema(name: &str, schema: &Schema) -> String {
    let mut table = Table::new();
    table.set_header(vec!["列", "类型", "可空"]);
    for f in &schema.fields {
        table.add_row(vec![
            f.name.clone(),
            f.data_type.to_string(),
            if f.nullable { "是" } else { "否" }.to_string(),
        ]);
    }
    format!("表 {name}:\n{table}")
}
