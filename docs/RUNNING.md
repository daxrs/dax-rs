---
title: "Running the DAX & XMLA Server"
description: "How to run the dax-rs DAX/XMLA server — locally, in Docker, or with S3 datasets. CLI flags, demo data, and health checks."
weight: 1
icon: "download"
status: "Latest Release"
lastUpdated: "July 2026"
---

## Run server

The server scans `--models-root` for every `*.json` TMSL file and loads each as a separate model. With no flags at all, it defaults to `./demodata` for both models and datasets — generate the demo dataset first with `cargo run --bin generate-demodata`, then:
```
cargo run --bin dax-rs
```

Or point it at your own models directory:
```
cargo run --bin dax-rs -- --models-root /path/to/models --datasets-root /path/to/data
```

Key flags:
- `--models-root <dir>` — directory scanned for `*.json` TMSL model files, each loaded as a separate model (default: `./demodata`)
- `--datasets-root <dir>` — directory containing parquet data files (default: `./demodata`)
- `--port <n>` — listen port (default: `3000`)
- `--config <file>` — YAML config file path (default: `server.yaml` if present)

## Docker

Build the image (stable Rust, no extra polars perf flags):
```
docker build -t dax-rs .
```

Build with polars' `performant` feature (more fast paths, slower compile — recommended for production):
```
docker build -t dax-rs --build-arg POLARS_FEATURES=performant .
```

Build a nightly image (SIMD + specialization; requires the nightly toolchain, pulled in automatically via `RUST_CHANNEL`). `performant` and `nightly` are independent — combine them explicitly:
```
docker build -t dax-rs:rust-nightly \
  --build-arg RUST_CHANNEL=nightly \
  --build-arg POLARS_FEATURES=performant,nightly \
  .
```

Run with a directory of models (every `*.json` file found under `--models-root`/`DAX_MODELS_ROOT` is loaded as a separate model):
```
docker run -p 3000:3000 \
  -v /path/to/models:/models \
  -e DAX_MODELS_ROOT=/models \
  -e DAX_DATASETS_ROOT=/models \
  dax-rs
```

Run with S3 datasets:
```
docker run -p 3000:3000 \
  -v /path/to/models:/models \
  -e DAX_MODELS_ROOT=/models \
  -e DAX_DATASETS_TYPE=s3 \
  -e DAX_DATASETS_BUCKET=my-bucket \
  -e DAX_DATASETS_REGION=eu-west-1 \
  -e DAX_DATASETS_ACCESS_KEY_ID=AKIA... \
  -e DAX_DATASETS_SECRET_ACCESS_KEY=... \
  dax-rs
```

The server exposes `GET /health` which returns `{"status":"ok"}` and is used as the Docker healthcheck. The dashboard is available at `http://localhost:3000/`.
