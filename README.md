# RustFinalProject — 列式内存分析查询引擎

一个从零实现的**列式内存分析查询引擎**(mini OLAP / DataFrame 内核,类比极简版 DuckDB / Polars)。
用户提供 CSV 数据,用 SQL 子集查询。引擎走完整的数据库查询处理链路:

```
SQL 文本
  → 词法分析 → 语法分析(AST)
  → 绑定 / 语义分析(带类型的逻辑表达式)
  → 逻辑计划
  → 查询优化(规则重写,不动点驱动)
  → 物理计划生成
  → 向量化执行(火山模型 + partitioned 并行)
  → 表格化结果输出
```

核心逻辑(SQL 解析、绑定、优化、物理算子、执行)全部自写;第三方 crate 仅用于非核心的脏活。

---

## 功能列表

- **SQL 子集**:`SELECT`(投影 / 别名 / `*`)、`FROM`(表别名)、`INNER JOIN` / `LEFT JOIN ... ON`、`WHERE`、
  `GROUP BY`、`HAVING`、`ORDER BY`(多列 / 升降序)、`LIMIT`。
- **表达式**:算术 `+ - * /`、比较 `= <> < <= > >=`、逻辑 `AND OR NOT`、`IS [NOT] NULL`、
  一元负号、括号;聚合 `COUNT(*)` / `COUNT` / `SUM` / `AVG` / `MIN` / `MAX`。运算符优先级由 **Pratt parser** 处理。
- **类型系统**:`Int64` / `Float64` / `Utf8` / `Boolean` / `Date`,隐式数值提升(Int64 → Float64)与日期字符串字面量隐式转换,
  全程正确的 **NULL 传播**(逻辑运算采用 SQL 三值逻辑)。
- **列式内存模型**:每列连续存储 + 位压缩 NULL 位图(`Vec<u64>`),`RecordBatch` 为模块间唯一数据单位。
- **查询优化器**:可插拔规则 + 不动点驱动——常量折叠、表达式化简、谓词合并、**投影下推 / 列裁剪**(把所需列下推到扫描,**支持穿过 Join**)、去冗余投影。
- **物理算子**:Scan / Filter / Projection / HashAggregate / HashJoin(INNER + LEFT)/ Sort / Limit,统一火山模型拉取接口。
- **代价优化**:基于 CSV 文件大小的行数估计,INNER Join 选**较小一侧**建哈希表。
- **并行执行**:partitioned 并行聚合(`std::thread` + `mpsc` + `Arc`)与并行连接 build(`rayon`),
  结果与单线程**等价**。
- **交互式 REPL**:表格化输出 + 耗时,`.explain` 查看优化前后计划树,`.parallel` 开关并行。

### 明确不做(控制工期)
子查询、窗口函数、CTE、事务、写回 / UPDATE / DELETE、索引、磁盘溢出、`RIGHT/FULL JOIN`。

---

## 编译与运行

```bash
cargo build --release        # 编译
cargo test                   # 运行全部测试(单元 + 端到端 + 优化器 + 并行一致性)
cargo run --bin rfp          # 启动交互式 REPL
```

REPL 会话示例:

```
sql> .load testdata/employees.csv emp
已加载表 `emp`(来自 testdata/employees.csv)
sql> .load testdata/departments.csv dept

sql> SELECT dept_id, COUNT(*), AVG(salary) FROM emp
     WHERE active = true GROUP BY dept_id HAVING COUNT(*) > 1
     ORDER BY AVG(salary) DESC LIMIT 5;
+---------+----------+-------------------+
| dept_id | COUNT(*) | AVG(salary)       |
+========================================+
| 30      | 2        | 9300              |
| 10      | 3        | 8533.333333333334 |
| 20      | 2        | 7450              |
+---------+----------+-------------------+
(3 行)
耗时 2.8ms

sql> SELECT e.name, d.name FROM emp AS e INNER JOIN dept AS d ON e.dept_id = d.id;

sql> .explain SELECT name, salary FROM emp WHERE salary > 9000 AND 1 = 1;
== 未优化逻辑计划 ==
Projection: #1, #3
  Filter: ((#3 > 9000) AND (1 = 1))
    Scan: emp
== 优化后逻辑计划 ==
Filter: (#1 > 9000)
  Scan: emp 投影列=name, salary

sql> .parallel on
并行执行:已开启
sql> .quit
```

### 元命令
| 命令 | 说明 |
| --- | --- |
| `.load <csv> <表名>` | 加载 CSV 为命名表(自动类型推断) |
| `.schema <表名>` | 查看表结构 |
| `.tables` | 列出已注册表 |
| `.explain <SQL>` | 打印优化前后逻辑计划树 |
| `.parallel <on\|off>` | 开关 partitioned 并行 |
| `.help` / `.quit` | 帮助 / 退出 |

---

## 支持的 SQL 文法

```
SELECT <select_list>
FROM <table> [AS alias]
[ [INNER] JOIN <table> [AS alias] ON <expr> ]
[ WHERE <expr> ]
[ GROUP BY <expr_list> ]
[ HAVING <expr> ]
[ ORDER BY <expr> [ASC|DESC] (, ...) ]
[ LIMIT <int> ]

select_list := '*' | (<expr> [AS alias]) (',' ...)
expr        := 字面量 | 列引用[table.col] | 一元 | 二元 | 聚合(COUNT/SUM/AVG/MIN/MAX, COUNT(*))
运算符优先级(低→高): OR, AND, NOT, (= <> < <= > >= / IS [NOT] NULL), (+ -), (* /), 一元 -
类型: Int64 / Float64 / Utf8 / Boolean
```

---

## 架构与模块

单 crate 多模块,严格单向依赖(上层依赖下层):

| 模块 | 职责 |
| --- | --- |
| `types` | 数据类型、`ScalarValue`、`Schema`(类型推导 / 列名歧义检测) |
| `array` | 列式数组(`enum Array`)、NULL 位图、`RecordBatch`、filter/take/concat 原语 |
| `error` | 统一错误类型 `QueryError`(`thiserror`) |
| `datasource` | `DataSource` trait、CSV 读取(自写类型推断 + 列式装载 + 流式 batch) |
| `sql` | 词法分析、语法分析(递归下降 + Pratt)、AST |
| `logical` | 逻辑表达式 `Expr`、逻辑计划 `LogicalPlan`、绑定 / 语义分析、计划树打印 |
| `optimizer` | `OptimizerRule` trait + 不动点驱动 + 五条规则 |
| `physical` | 物理表达式向量化求值、物理计划生成、各物理算子 |
| `execution` | 火山模型驱动、查询会话 `SessionContext` |
| `repl` | 交互式命令行 + 表格输出 |

**三大接口契约**(项目脊柱):`RecordBatch`(数据传输单位)、`Expr`(带类型逻辑表达式)、
`PhysicalOperator`(火山拉取接口)。

---

## 体现的 Rust 核心特性

- **所有权 / 借用**:`array` 层列以 `Arc<Array>` 零拷贝共享;优化器对计划树做**不可变重写**(消费旧树返回新树)。
- **enum + 模式匹配**:类型系统、AST、逻辑 / 物理计划树、`Array` 分发。
- **trait**:`DataSource` / `OptimizerRule` / `PhysicalOperator` / `Array` 操作。
- **泛型**:`PrimitiveArray<T>` 统一整型 / 浮点列;take/filter 原语。
- **生命周期**:零拷贝切片与借用式表达式求值。
- **并发**:`execution` / `physical` 层的 partitioned 并行——`std::thread::scope` + `mpsc::channel` + `Arc`(并行聚合),`rayon`(并行连接 build)。数据竞争由类型系统在**编译期**杜绝。
- **错误处理**:`Result<T, QueryError>` 贯穿全程,无随意 `unwrap`/`expect`。

---

## 依赖说明

| crate | 用途(仅限非核心脏活) |
| --- | --- |
| `thiserror` | 错误枚举脚手架 |
| `csv` | CSV 字符级 record 切分(类型推断 / 列装载自写) |
| `rayon` | 并行连接 build 的数据并行 |
| `rustyline` | REPL 行编辑 |
| `comfy-table` | 结果表格美化输出 |

零 `unsafe`。通过 `cargo fmt` 与 `cargo clippy`(0 warning)。

---

## 测试

```bash
cargo test
```

- **单元测试**:位图、列操作、CSV 类型推断、lexer、parser、各表达式 / 算子。
- **端到端**(`tests/end_to_end.rs`):投影 / 过滤 / 聚合 / 排序 / Limit / JOIN / NULL / 错误处理。
- **优化器**(`tests/optimizer.rs`):规则触发 + 优化前后结果等价。
- **并行一致性**(`tests/parallel_consistency.rs`):串行与 partitioned 并行结果集相同。

测试数据见 `testdata/`。
