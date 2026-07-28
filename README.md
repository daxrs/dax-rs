# dax-rs

![status](https://img.shields.io/badge/status-alpha-orange)

A Rust implementation of a DAX engine and XMLA/SOAP server, compatible with **Power BI** and **Analysis Services** live connections. Query engine is backed by [Polars](https://pola.rs/); models are described using **TMSL** (Tabular Model Scripting Language), the same JSON format Power BI Desktop uses internally.

Point Power BI Desktop's "Analysis Services" live connection (or Excel, or any XMLA client) at a running `dax-rs` server, and it queries your TMSL model + Parquet data through the real DAX engine — no Analysis Services or Fabric capacity required.

## Features

- **DAX engine** — 122 of 211 DAX functions implemented (see [`docs/DAX_Functions.md`](docs/DAX_Functions.md) for the current compliance table), backed by Polars for vectorized evaluation over Parquet data.
- **MDX-to-DAX translation** — Excel speaks MDX over XMLA; `dax-rs` translates incoming MDX queries to DAX before evaluating them. [WIP]
- **TMSL models** — define tables, columns, measures, and relationships as JSON; see [`docs/TMSL.md`](docs/TMSL.md).
- **Pluggable storage** — models and datasets are each configured independently and can live on local disk, S3 (or any S3-compatible store), Azure Blob Storage, or Google Cloud Storage.
- **REST API** — inspect and edit loaded models over HTTP; see [`docs/models_api.md`](docs/models_api.md).
- **Demo dataset generator** — a synthetic multi-year retail dataset (products, customers, sales, promotions, currency conversion) for trying the engine without bringing your own model; see [`demodata/aispec.md`](demodata/aispec.md).

## Quickstart

```sh
# Generate the demo dataset (writes to ./demodata by default)
cargo run --bin generate-demodata

# Run the server — with no flags, it loads every *.json model found in
# ./demodata and reads their parquet data from the same place
cargo run --bin dax-rs
```

The server listens on `http://localhost:3000`:
- `GET /health` — `{"status":"ok"}` once models are loaded
- `GET /` — a small dashboard listing loaded models
- `POST /xmla` — the XMLA/SOAP endpoint Power BI, Excel, and other Analysis Services clients connect to

In Power BI Desktop: **Get Data → Analysis Services**, connect live to `http://localhost:3000/xmla`.

## Running with your own model

```sh
cargo run --bin dax-rs -- --models-root /path/to/models --datasets-root /path/to/data
```

Every `*.json` TMSL file found under `--models-root` is loaded as a separate model. Full flag/environment-variable/YAML reference (including S3/Azure/GCS storage config) is in [`docs/server_config.md`](docs/server_config.md).

## Docker

```sh
docker build -t dax-rs .
docker run -p 3000:3000 \
  -v /path/to/models:/models \
  -e DAX_MODELS_ROOT=/models \
  -e DAX_DATASETS_ROOT=/models \
  dax-rs
```

See [`docs/RUNNING.md`](docs/RUNNING.md) for build variants (`performant`/`nightly` Polars features) and S3-backed examples.

## Documentation

| Doc | Covers |
|---|---|
| [`docs/RUNNING.md`](docs/RUNNING.md) | Running locally and via Docker |
| [`docs/server_config.md`](docs/server_config.md) | Full config reference — YAML, env vars, CLI flags, storage backends |
| [`docs/TMSL.md`](docs/TMSL.md) | The TMSL model format `dax-rs` reads and writes |
| [`docs/models_api.md`](docs/models_api.md) | REST API for inspecting and editing loaded models |
| [`docs/DAX_Functions.md`](docs/DAX_Functions.md) | DAX function implementation status |

## License

[GNU AGPL v3.0 or later](LICENSE).
