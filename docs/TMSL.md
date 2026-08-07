---
title: "TMSL Model Documentation"
description: "The TMSL model structure supported by dax-rs."
weight: 6
icon: "info"
status: "Latest Release"
lastUpdated: "July 2026"
---

This document describes the subset of Tabular Model Scripting Language (TMSL) that
`dax-rs` can read and write. The format is a JSON file that describes tables,
columns, measures, and relationships. It maps directly to Power BI Desktop's
model export format.

---

## Rust API

```rust
use dax_rs::loaders::tmsl::{load_tmsl, save_tmsl, load_tmsl_from_op, save_tmsl_to_op};
use dax_rs::catalog::Catalog;

// Load from the local filesystem
let catalog: Catalog = load_tmsl("model.json")?;

// Save back to disk (round-trips all fields)
save_tmsl("model.json", &catalog)?;

// Load / save via an opendal BlockingOperator (S3, Azure Blob, etc.)
let catalog = load_tmsl_from_op(&op, "model.json")?;
save_tmsl_to_op(&op, "model.json", &catalog)?;
```

`load_tmsl` deserialises the JSON, validates the schema, and returns a `Catalog`.
It returns an error (not a panic) if the JSON is malformed or any validation rule
is violated (see [Validation](#validation)).

`save_tmsl` serialises a `Catalog` back to the same JSON structure, preserving the
original definition order of tables, columns, and measures. Fields that equal their
default value are omitted to keep the output compact.

---

## Top-level structure

```json
{
  "name": "MyModel",
  "culture": "en-US",
  "collation": "Latin1_General_100_CI_AS",
  "compatibilityLevel": 1550,
  "defaultMode": "import",
  "tables": [ ... ],
  "relationships": [ ... ]
}
```

| Field               | Type    | Required | Notes                                         |
|---------------------|---------|----------|-----------------------------------------------|
| `name`              | string  | no       | Display name of the model.                    |
| `culture`           | string  | no       | Locale string, e.g. `"en-US"`.               |
| `collation`         | string  | no       | SQL collation name.                           |
| `compatibilityLevel`| integer | no       | Numeric compatibility level, e.g. `1550`.    |
| `defaultMode`       | string  | no       | Storage mode hint, e.g. `"import"`.          |
| `tables`            | array   | yes      | One entry per table (see [Tables](#tables)).  |
| `relationships`     | array   | yes      | May be empty (`[]`).                          |

---

## Tables

```json
{
  "name": "Sales",
  "dataSource": "data/sales.parquet",
  "isHidden": false,
  "dataCategory": "Time",
  "description": "Transactional sales data",
  "columns": [ ... ],
  "measures": [ ... ]
}
```

| Field          | Type    | Required | Default  | Notes                                                         |
|----------------|---------|----------|----------|---------------------------------------------------------------|
| `name`         | string  | yes      |          | Unique table name within the model.                          |
| `dataSource`   | string  | yes      |          | Path to the backing Parquet file.                            |
| `columns`      | array   | no       | `[]`     | See [Columns](#columns).                                     |
| `measures`     | array   | no       | `[]`     | See [Measures](#measures).                                   |
| `isHidden`     | boolean | no       | `false`  | Hidden tables are excluded from client tool auto-discovery.  |
| `dataCategory` | string  | no       | absent   | Semantic category. `"Time"` is the only named variant; any other string is stored verbatim. |
| `description`  | string  | no       | absent   | Free-text description shown in client tools.                 |

---

## Columns

```json
{
  "name": "OrderDate",
  "dataType": "dateTime",
  "summarizeBy": "none",
  "isHidden": false,
  "isKey": false,
  "isNullable": true,
  "isUnique": false,
  "formatString": "Short Date",
  "displayFolder": "Dates",
  "dataCategory": "WebUrl",
  "sortByColumn": "MonthNumber",
  "description": "Date the order was placed"
}
```

| Field           | Type    | Required | Default  | Notes                                                                            |
|-----------------|---------|----------|----------|----------------------------------------------------------------------------------|
| `name`          | string  | yes      |          | Column name, unique within the table.                                            |
| `dataType`      | string  | yes      |          | See [Data types](#data-types).                                                   |
| `summarizeBy`   | string  | no       | `"none"` | See [SummarizeBy](#summarizeby).                                                 |
| `isHidden`      | boolean | no       | `false`  | Hides the column from client tools.                                              |
| `isKey`         | boolean | no       | `false`  | Marks the column as the table's primary key.                                     |
| `isNullable`    | boolean | no       | `true`   | When `false`, the column must not contain null values.                           |
| `isUnique`      | boolean | no       | `false`  | Asserts that all values in the column are distinct.                              |
| `formatString`  | string  | no       | absent   | Display format hint, e.g. `"#,##0.00"`, `"Short Date"`.                        |
| `displayFolder` | string  | no       | absent   | Folder path shown in client tools, e.g. `"Finance\\Revenue"`.                   |
| `dataCategory`  | string  | no       | absent   | Semantic hint for client tools. See [Column data categories](#column-data-categories). |
| `sortByColumn`  | string  | no       | absent   | Name of another column in the same table to sort by. The target must exist.     |
| `description`   | string  | no       | absent   | Free-text description.                                                           |

### Data types

| JSON value    | Polars type        | Notes                                                    |
|---------------|--------------------|----------------------------------------------------------|
| `"string"`    | `String`           |                                                          |
| `"int64"`     | `Int64`            |                                                          |
| `"double"`    | `Float64`          |                                                          |
| `"decimal"`   | `Float64`          | Stored as `Float64`; no fixed-precision arithmetic.      |
| `"boolean"`   | `Boolean`          |                                                          |
| `"dateTime"`  | `Datetime(ms)`     | UTC milliseconds since epoch.                            |
| `"binary"`    | `Binary`           |                                                          |
| `"automatic"` | —                  | **Rejected** at load time. Column type must be explicit. |
| `"unknown"`   | —                  | **Rejected** at load time.                               |
| `"variant"`   | —                  | **Rejected** at load time.                               |

Deserialisation is case-insensitive on the way in; serialisation always emits the
lowercase canonical form shown above.

### SummarizeBy

Controls the default aggregation Power BI applies when the column is dropped onto a
visual.

| JSON value        | Meaning                        |
|-------------------|--------------------------------|
| `"none"`          | No default aggregation (default) |
| `"sum"`           | Sum                            |
| `"min"`           | Minimum                        |
| `"max"`           | Maximum                        |
| `"count"`         | Count of non-blank values      |
| `"average"`       | Average                        |
| `"distinctCount"` | Count of distinct values       |

### Column data categories

The `dataCategory` string is a hint to client tools for semantic formatting or map
visuals. Recognised values:

`Address`, `City`, `Continent`, `Country`, `County`, `Latitude`, `Longitude`,
`Place`, `PostalCode`, `StateOrProvince`, `WebUrl`, `ImageUrl`, `Barcode`,
`PhoneNumber`, `Organization`, `FaceUri`

Any other string is accepted and stored verbatim as `Other(value)`.

---

## Measures

```json
{
  "name": "Total Sales",
  "expression": "SUM(Sales[Amount])",
  "isHidden": false,
  "formatString": "#,##0.00",
  "displayFolder": "Revenue",
  "description": "Sum of all sales amounts"
}
```

| Field           | Type    | Required | Default | Notes                                                             |
|-----------------|---------|----------|---------|-------------------------------------------------------------------|
| `name`          | string  | yes      |         | Measure name, unique across the whole model (not just the table). |
| `expression`    | string  | yes      |         | DAX expression body. May reference other measures and columns.    |
| `isHidden`      | boolean | no       | `false` | Hides the measure from client tools.                             |
| `formatString`  | string  | no       | absent  | Display format, e.g. `"#,##0.00"`, `"0%"`.                     |
| `displayFolder` | string  | no       | absent  | Folder path in client tools, e.g. `"KPIs\\Revenue"`.            |
| `description`   | string  | no       | absent  | Free-text description.                                            |

Measures belong to a table but are globally namespaced: the first occurrence of a
given measure name wins and subsequent duplicates are silently ignored.

---

## Relationships

```json
{
  "name": "Sales_Product",
  "fromTable": "Sales",
  "fromColumn": "ProductSK",
  "toTable": "Product",
  "toColumn": "ProductSK",
  "crossFilteringBehavior": "single",
  "isActive": true
}
```

| Field                    | Type    | Required | Notes                                                                                          |
|--------------------------|---------|----------|------------------------------------------------------------------------------------------------|
| `name`                   | string  | yes      | Unique relationship name within the model.                                                     |
| `fromTable`              | string  | yes      | The "many" side (fact / detail table). Holds the foreign key.                                  |
| `fromColumn`             | string  | yes      | Join column on `fromTable`. Must be declared in `columns`.                                     |
| `toTable`                | string  | yes      | The "one" side (dimension / lookup table). Holds the primary key.                              |
| `toColumn`               | string  | yes      | Join column on `toTable`. Must be declared in `columns`.                                       |
| `crossFilteringBehavior` | string  | yes      | `"single"` — filter flows from `toTable` (dim) to `fromTable` (fact) only. `"both"` — bidirectional. |
| `isActive`               | boolean | yes      | Only active relationships participate in automatic filter propagation.                         |

Both join columns must have the same `dataType`; a type mismatch is rejected at
load time.

---

## Validation

`Catalog::from_model` validates the model before returning. All errors name the
offending object so the message can be surfaced directly to users.

| Rule | Error message shape |
|------|---------------------|
| Column has `dataType` of `"automatic"`, `"unknown"`, or `"variant"` | `Column '<name>' has unsupported type …` |
| Relationship references a column not declared in `columns` (from side) | `Relationship '<name>': from-column '<table>.<col>' not declared in model` |
| Relationship references a column not declared in `columns` (to side)   | `Relationship '<name>': to-column '<table>.<col>' not declared in model`   |
| Join columns have different `dataType` values | `Relationship '<name>': join column type mismatch — '<t1>.<c1>' is … but '<t2>.<c2>' is …` |
| `sortByColumn` references a column that does not exist in the same table | `Column '<table>.<col>': sortByColumn '<target>' does not exist in table '<table>'` |

---

## Examples

### Minimal single-table model

```json
{
  "name": "SimpleModel",
  "tables": [
    {
      "name": "Sales",
      "dataSource": "data/sales.parquet",
      "columns": [
        { "name": "OrderDate", "dataType": "dateTime" },
        { "name": "Amount",    "dataType": "double",  "summarizeBy": "sum" }
      ],
      "measures": [
        { "name": "Total Sales", "expression": "SUM(Sales[Amount])" }
      ]
    }
  ],
  "relationships": []
}
```

### Star schema (fact + dimension with relationship)

```json
{
  "name": "DemoModel",
  "tables": [
    {
      "name": "Sales",
      "dataSource": "data/sales.parquet",
      "columns": [
        { "name": "ProductSK", "dataType": "Int64" },
        { "name": "Amount",    "dataType": "double", "summarizeBy": "sum" },
        { "name": "Quantity",  "dataType": "double", "summarizeBy": "sum" }
      ],
      "measures": [
        { "name": "TotalAmount",   "expression": "SUM(Sales[Amount])" },
        { "name": "AmountAbove30", "expression": "CALCULATE(SUM(Sales[Amount]), Sales[Amount] > 30)" },
        { "name": "DoubleTotal",   "expression": "[TotalAmount] * 2" }
      ]
    },
    {
      "name": "Product",
      "dataSource": "data/product.parquet",
      "columns": [
        { "name": "ProductSK",   "dataType": "Int64" },
        { "name": "ProductType", "dataType": "string" },
        { "name": "Color",       "dataType": "string" }
      ],
      "measures": []
    }
  ],
  "relationships": [
    {
      "name": "Sales_Product",
      "fromTable": "Product",
      "fromColumn": "ProductSK",
      "toTable": "Sales",
      "toColumn": "ProductSK",
      "crossFilteringBehavior": "single",
      "isActive": true
    }
  ]
}
```

The relationship direction matters: filters flow **from** `toTable` (the
dimension / "one" side) **to** `fromTable` (the fact / "many" side). In this example
`Product[Color] = "Red"` automatically restricts which `Sales` rows are visible.

### Snowflake schema (two-hop relationships)

```json
{
  "name": "SnowflakeModel",
  "tables": [
    {
      "name": "Sales",
      "dataSource": "data/sales.parquet",
      "columns": [
        { "name": "ProductSK", "dataType": "Int64" },
        { "name": "Amount",    "dataType": "double" }
      ],
      "measures": []
    },
    {
      "name": "Product",
      "dataSource": "data/product.parquet",
      "columns": [
        { "name": "ProductSK",  "dataType": "Int64" },
        { "name": "CategorySK", "dataType": "Int64" }
      ],
      "measures": []
    },
    {
      "name": "Category",
      "dataSource": "data/category.parquet",
      "columns": [
        { "name": "CategorySK",   "dataType": "Int64" },
        { "name": "CategoryName", "dataType": "string" }
      ],
      "measures": []
    }
  ],
  "relationships": [
    {
      "name": "Sales_Product",
      "fromTable": "Product",
      "fromColumn": "ProductSK",
      "toTable": "Sales",
      "toColumn": "ProductSK",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "Product_Category",
      "fromTable": "Category",
      "fromColumn": "CategorySK",
      "toTable": "Product",
      "toColumn": "CategorySK",
      "crossFilteringBehavior": "single",
      "isActive": true
    }
  ]
}
```

Filter propagation chains: a `Category[CategoryName]` filter reaches `Sales`
automatically via the two-hop path `Category → Product → Sales`.

### Column metadata (sorting, folders, categories)

```json
{
  "name": "CalendarModel",
  "tables": [
    {
      "name": "Date",
      "dataSource": "data/date.parquet",
      "dataCategory": "Time",
      "columns": [
        { "name": "DateKey",     "dataType": "Int64",    "isKey": true,   "isUnique": true, "isNullable": false },
        { "name": "MonthName",   "dataType": "string",   "sortByColumn": "MonthNumber", "displayFolder": "Month" },
        { "name": "MonthNumber", "dataType": "Int64",    "isHidden": true, "displayFolder": "Month" },
        { "name": "CalendarURL", "dataType": "string",   "dataCategory": "WebUrl" }
      ],
      "measures": []
    }
  ],
  "relationships": []
}
```
