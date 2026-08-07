---
title: "DAX REST API"
description: "Execute and validate DAX queries against your models over plain HTTP — a self-hosted DAX API with no Power BI or XMLA client required."
weight: 3
icon: "bolt"
status: "Latest Release"
lastUpdated: "August 2026"
---

Base URL: `http://localhost:3000`

These endpoints let you run and validate DAX queries directly over HTTP — no Power BI, Excel, or XMLA client in the loop. Point any HTTP client at your model and get results back as JSON.

---

## Evaluate DAX

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

All values are serialised as strings. `null` indicates a blank or missing cell. Column `data_type` values match those used by the [List tables](/docs/models_api/#list-tables) endpoint (`string`, `integer`, `double`, `boolean`, `dateTime`, etc.).

**Response `400 Bad Request`**

Returned when the DAX engine rejects the query (parse failure, evaluation error, etc.):

```json
{ "error": "Unknown column: Sales.Amountt" }
```

**Response `404 Not Found`** — model ID does not exist.

---

## Validate DAX

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
