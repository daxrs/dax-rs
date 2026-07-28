# Models REST API

Base URL: `http://localhost:3000`

All responses are JSON. Model and table IDs are their names as defined in the TMSL file, percent-encoded when used in a URL path (e.g. a model named `Sales Model` becomes `Sales%20Model`).

---

## Endpoints

### List models

```
GET /models
```

Returns all loaded models.

**Response `200 OK`**

```json
[
  {
    "id": "AdventureWorks",
    "name": "AdventureWorks"
  }
]
```

---

### Get model

```
GET /models/{id}
```

Returns metadata for a single model.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Response `200 OK`**

```json
{
  "id": "AdventureWorks",
  "name": "AdventureWorks",
  "culture": "en-US",
  "collation": "Latin1_General_100_BIN2",
  "compatibility_level": 1500,
  "default_mode": "Import",
  "state": "FullyProcessed",
  "read_write_mode": "ReadOnly",
  "created_timestamp": "2026-05-21T09:14:00",
  "last_schema_update": "2026-05-21T09:14:00",
  "last_refreshed": "2026-05-21T09:14:00"
}
```

All three timestamps are formatted as `YYYY-MM-DDTHH:MM:SS` (UTC). `created_timestamp` and `last_schema_update` are set at server start; `last_refreshed` is updated each time `POST /models/{id}/refreshdata` completes successfully.

**Response `404 Not Found`** — model ID does not exist.

---

### List tables

```
GET /models/{id}/tables
```

Returns all tables in the model, each with their full column list.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Response `200 OK`**

```json
[
  {
    "id": "Sales",
    "name": "Sales",
    "is_hidden": false,
    "columns": [
      {
        "name": "OrderDate",
        "data_type": "dateTime",
        "summarize_by": "None",
        "is_hidden": false,
        "is_key": false,
        "is_nullable": true,
        "is_unique": false,
        "format_string": "dd/MM/yyyy"
      }
    ]
  }
]
```

Optional column fields (`format_string`, `display_folder`, `data_category`, `description`, `sort_by_column`) are omitted when not set.

Optional table fields (`data_category`, `description`) are omitted when not set.

**Column `data_type` values:** `string`, `integer`, `double`, `boolean`, `dateTime`, `base64Binary`, `unsignedLong`

**Column `summarize_by` values:** `None`, `Sum`, `Min`, `Max`, `Count`, `Average`, `DistinctCount`

**Response `404 Not Found`** — model ID does not exist.

---

### Get table

```
GET /models/{id}/tables/{tableId}
```

Returns a single table with its full column list.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |
| `tableId` | Table name |

**Response `200 OK`** — same shape as a single entry from [List tables](#list-tables).

**Response `404 Not Found`** — model ID or table ID does not exist.

---

### List relationships

```
GET /models/{id}/relationships
```

Returns all relationships defined in the model.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Response `200 OK`**

```json
[
  {
    "name": "Sales_Product",
    "from_table": "Sales",
    "from_column": "ProductKey",
    "to_table": "Product",
    "to_column": "ProductKey",
    "is_active": true,
    "bidirectional": false
  }
]
```

**Response `404 Not Found`** — model ID does not exist.

---

### List measures

```
GET /models/{id}/measures
```

Returns all measures defined in the model.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Response `200 OK`**

```json
[
  {
    "name": "Total Sales",
    "table": "Sales",
    "expression": "SUM(Sales[Amount])",
    "is_hidden": false,
    "format_string": "#,##0.00"
  }
]
```

Optional fields (`format_string`, `display_folder`, `description`) are omitted when not set.

**Response `404 Not Found`** — model ID does not exist.

---

### Get object dependencies

```
GET /models/{id}/dependencies/{object}
```

Returns the lineage graph for a measure or column — its upstream inputs, direct downstream consumers, and the full set of transitively impacted measures.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |
| `object` | Measure name **or** column reference in `Table[Column]` format, percent-encoded |

Column references must be percent-encoded: `Sales[Amount]` → `Sales%5BAmount%5D`. Measure lookup is case-insensitive.

**Response `200 OK` — measure**

```json
{
  "object": "YTD Revenue",
  "object_type": "measure",
  "table": "Sales",
  "upstream": {
    "measures": ["Revenue"],
    "columns": []
  },
  "downstream": {
    "measures": ["Revenue Growth %"]
  },
  "impacted_measures": ["Revenue Growth %"]
}
```

**Response `200 OK` — column**

```json
{
  "object": "Sales[Amount]",
  "object_type": "column",
  "table": "Sales",
  "upstream": {
    "measures": [],
    "columns": []
  },
  "downstream": {
    "measures": ["Revenue"]
  },
  "impacted_measures": ["Revenue", "YTD Revenue", "Revenue Growth %"]
}
```

**Field definitions**

| Field | Description |
|---|---|
| `upstream.measures` | Measures directly referenced in this measure's expression |
| `upstream.columns` | Columns directly referenced in this measure's expression. Always empty for column objects |
| `downstream.measures` | Measures that directly reference this object |
| `impacted_measures` | All measures reachable transitively downstream — the full blast radius of a change to this object. Sorted alphabetically |

**Response `404 Not Found`** — model ID does not exist, or `object` is not a known measure or column.

---

### Evaluate DAX

```
POST /models/{id}/dax/evaluate
Content-Type: text/plain
```

Executes a DAX query against the model and returns the tabular result. A query may contain multiple `EVALUATE` statements; each produces one result table in the response array.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Request body**

Raw DAX query text:

```dax
EVALUATE
SUMMARIZECOLUMNS(
    'Product'[Color],
    "Revenue", [Total Sales]
)
```

**Response `200 OK`**

An array — one entry per `EVALUATE` statement in the query:

```json
[
  {
    "columns": [
      { "name": "Product[Color]", "data_type": "string" },
      { "name": "Revenue",        "data_type": "double" }
    ],
    "rows": [
      ["Blue", "1234.50"],
      ["Red",  null],
      ["Green","780.00"]
    ]
  }
]
```

All values are serialised as strings. `null` indicates a blank or missing cell. Column `data_type` values match those used by the [List tables](#list-tables) endpoint (`string`, `integer`, `double`, `boolean`, `dateTime`, etc.).

**Response `400 Bad Request`**

Returned when the DAX engine rejects the query (parse failure, evaluation error, etc.):

```json
{ "error": "Unknown column: Sales.Amountt" }
```

**Response `404 Not Found`** — model ID does not exist.

---

### Validate DAX

```
POST /models/{id}/dax/validate
Content-Type: text/plain
```

Validates a DAX query against the model without executing it. Three phases run in order — a later phase only runs if the earlier one passes:

1. **Syntax** — the query must be parseable DAX (`EVALUATE ...`, optionally with a `DEFINE` block).
2. **Semantic** — all column references (`Table[Column]`), table references, and function names must exist in the model.
3. **Type** — argument types must satisfy each function's signature.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Request body**

Raw DAX query text, for example:

```dax
EVALUATE
SUMMARIZECOLUMNS(
    'Product'[Color],
    "Revenue", [Total Sales]
)
```

**Response `200 OK` — valid query**

```json
{
  "valid": true,
  "errors": []
}
```

**Response `200 OK` — invalid query**

```json
{
  "valid": false,
  "errors": [
    {
      "kind": "syntax",
      "message": " --> 1:9\n  |\n1 | EVALUTE ...\n  |         ^---\n  |\n  = expected ..."
    },
    {
      "kind": "semantic",
      "message": "Unknown column: Sales.Amountt"
    },
    {
      "kind": "type",
      "message": "Type error: ..."
    }
  ]
}
```

The endpoint always returns `200 OK`. Errors are reported in the body, not via HTTP status codes. Multiple errors can be returned when more than one expression fails to bind.

**Error `kind` values**

| Value | Meaning |
|---|---|
| `syntax` | The query could not be parsed. Only one error is returned and later phases are skipped |
| `semantic` | An unknown column, table, function, or identifier. May appear multiple times |
| `type` | A function received arguments of incompatible types. May appear multiple times |

**Response `404 Not Found`** — model ID does not exist.

---

### Refresh data

```
POST /models/{id}/refreshdata
```

Reloads all Parquet data sources for the model into memory. Safe to call while queries are in-flight — new data is loaded into a temporary buffer first and swapped in atomically after all tables succeed. If any table fails to load, the existing in-memory data is left untouched and an error is returned.

Updates the `last_refreshed` timestamp on the model. Does **not** affect `last_schema_update`.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Response `204 No Content`** — all tables reloaded successfully.

**Response `500 Internal Server Error`** — plain-text error message describing which table failed and why.

**Response `404 Not Found`** — model ID does not exist.

---

### Run commands

```
POST /models/{id}/commands
```

Applies a list of schema-mutation commands to the model. Commands execute sequentially; on the first error, execution stops and the error is returned.

**Path parameters**

| Parameter | Description |
|---|---|
| `id` | Model name |

**Request body**

```json
{
  "dryRun": false,
  "commands": [
    { "type": "...", ... }
  ]
}
```

Set `dryRun: true` to validate and simulate commands without persisting changes.

**Response `200 OK`**

```json
{
  "dryRun": false,
  "applied": 2,
  "errors": []
}
```

`applied` is the number of commands that succeeded before the first error (or the total if all succeeded). `errors` contains at most one entry (the first failure).

**Response `404 Not Found`** — model ID does not exist.

---

## Commands reference

All commands share a `"type"` discriminator field. Unrecognised types are rejected at parse time.

### Model commands

#### `RenameModel`

```json
{ "type": "RenameModel", "newName": "NewModelName" }
```

#### `SetModelProperty`

```json
{ "type": "SetModelProperty", "property": { "culture": "fr-FR" } }
{ "type": "SetModelProperty", "property": { "collation": "Latin1_General_100_BIN2" } }
{ "type": "SetModelProperty", "property": { "defaultMode": "DirectQuery" } }
```

The `property` field is an externally-tagged union — exactly one key naming the property, with its typed value.

---

### Table commands

#### `CreateTable`

```json
{ "type": "CreateTable", "name": "NewTable" }
```

#### `DeleteTable`

```json
{ "type": "DeleteTable", "table": "Sales" }
```

#### `RenameTable`

```json
{ "type": "RenameTable", "table": "Sales", "newName": "Orders" }
```

#### `SetTableProperty`

```json
{ "type": "SetTableProperty", "table": "Sales", "property": { "isHidden": true } }
{ "type": "SetTableProperty", "table": "Sales", "property": { "description": "Main fact table" } }
{ "type": "SetTableProperty", "table": "Sales", "property": { "description": null } }
{ "type": "SetTableProperty", "table": "Sales", "property": { "dataCategory": "Time" } }
{ "type": "SetTableProperty", "table": "Sales", "property": { "dataCategory": null } }
```

Pass `null` for optional string/category properties to clear them.

---

### Column commands

#### `AddColumn`

```json
{
  "type": "AddColumn",
  "table": "Sales",
  "name": "Amount",
  "dataType": "double",
  "isHidden": false,
  "isNullable": true,
  "isKey": false,
  "isUnique": false,
  "summarizeBy": "Sum",
  "formatString": "#,##0.00",
  "displayFolder": "Financials",
  "description": "Sale amount in base currency"
}
```

Optional fields: `summarizeBy` (default `None`), `formatString`, `displayFolder`, `description`.

**`dataType` values:** `string`, `int64`, `double`, `decimal`, `dateTime`, `boolean`, `binary`

#### `DeleteColumn`

```json
{ "type": "DeleteColumn", "table": "Sales", "column": "Amount" }
```

#### `RenameColumn`

```json
{ "type": "RenameColumn", "table": "Sales", "column": "Amt", "newName": "Amount" }
```

#### `ChangeColumnType`

```json
{ "type": "ChangeColumnType", "table": "Sales", "column": "Amount", "dataType": "double" }
```

#### `SetColumnProperty`

```json
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "isHidden": true } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "description": "Sale amount" } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "description": null } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "formatString": "#,##0.00" } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "formatString": null } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "displayFolder": "Financials" } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "displayFolder": null } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "dataCategory": "WebUrl" } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "dataCategory": null } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "sortByColumn": "SortKey" } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "sortByColumn": null } }
{ "type": "SetColumnProperty", "table": "Sales", "column": "Amount", "property": { "summarizeBy": "Sum" } }
```

Pass `null` for optional properties to clear them. `isHidden` and `summarizeBy` do not accept `null`.

**`summarizeBy` values:** `none`, `sum`, `min`, `max`, `count`, `average`, `distinctCount`

**`dataCategory` values:** `Address`, `City`, `Continent`, `Country`, `County`, `Latitude`, `Longitude`, `Place`, `PostalCode`, `StateOrProvince`, `WebUrl`, `ImageUrl`, `Barcode`, `PhoneNumber`, `Organization`, `FaceUri`, or any custom string.

---

### Measure commands

#### `CreateMeasure`

```json
{
  "type": "CreateMeasure",
  "table": "Sales",
  "name": "Total Sales",
  "expression": "SUM(Sales[Amount])",
  "formatString": "#,##0.00",
  "displayFolder": "KPIs",
  "description": "Sum of all sale amounts",
  "isHidden": false
}
```

Optional fields: `formatString`, `displayFolder`, `description`, `isHidden` (default `false`).

#### `DeleteMeasure`

```json
{ "type": "DeleteMeasure", "name": "Total Sales" }
```

#### `RenameMeasure`

```json
{ "type": "RenameMeasure", "name": "Total Sales", "newName": "Revenue" }
```

#### `UpdateMeasureExpression`

```json
{ "type": "UpdateMeasureExpression", "name": "Total Sales", "expression": "SUMX(Sales, Sales[Amount])" }
```

#### `SetMeasureFormatString`

```json
{ "type": "SetMeasureFormatString", "name": "Total Sales", "formatString": "#,##0.00" }
```

---

### Relationship commands

#### `CreateRelationship`

```json
{
  "type": "CreateRelationship",
  "name": "Sales_Product",
  "fromTable": "Sales",
  "fromColumn": "ProductKey",
  "toTable": "Product",
  "toColumn": "ProductKey",
  "isActive": true,
  "bidirectional": false
}
```

`isActive` defaults to `true`, `bidirectional` defaults to `false`.

#### `DeleteRelationship`

```json
{ "type": "DeleteRelationship", "name": "Sales_Product" }
```

#### `UpdateRelationship`

```json
{ "type": "UpdateRelationship", "name": "Sales_Product", "isActive": false }
{ "type": "UpdateRelationship", "name": "Sales_Product", "bidirectional": true }
{ "type": "UpdateRelationship", "name": "Sales_Product", "isActive": true, "bidirectional": false }
```

All fields except `name` are optional — omit to leave unchanged.
