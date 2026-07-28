# Server Configuration

The server reads configuration from three sources in increasing priority order:

1. `server.yaml` (or the file given by `--config` / `$DAX_CONFIG`)
2. Environment variables
3. CLI flags

## YAML file

By default the server looks for `server.yaml` in the working directory. An alternative path can be supplied with `--config <path>` or the `DAX_CONFIG` environment variable. If the file is absent it is silently skipped.

```yaml
server_name: dax-rs
hostname: localhost
port: 3000
locale_identifier: 1033
log_level: info
concurrency:
  max_concurrency: 32
  max_wait_secs: 30
storage:
  models:
    type: local
    root: ./models
  datasets:
    type: local
    root: ./data
```

The XMLA endpoint URL (`http://<hostname>:<port>/xmla`) and the MSOLAP connection string are derived automatically from `hostname` and `port`.

| Key                 | Type    | Default       | Description                                                               |
|---------------------|---------|---------------|---------------------------------------------------------------------------|
| `server_name`       | string  | `"dax-rs"`    | Display name returned in `DISCOVER_DATASOURCES`.                          |
| `hostname`          | string  | `"localhost"` | Hostname advertised to clients in the endpoint URL and connection string. |
| `port`              | integer | `3000`        | Port the server listens on and advertises to clients.                     |
| `locale_identifier` | integer | `1033`        | LCID advertised in discovery responses. `1033` = English (United States). |
| `log_level`         | string  | `"info"`      | Minimum log level emitted. Accepted values: `error`, `warn`, `info`, `debug`, `trace`. |
| `timezone`          | string  | absent        | IANA timezone name (e.g. `"Europe/Copenhagen"`) used by `NOW()` and `TODAY()`. If absent, the system's local timezone is used — see [Timezone](#timezone) below. |

### Timezone

`NOW()`/`TODAY()` resolve time in one of two ways:

1. **`timezone` config set** (`server.yaml` key, `DAX_TIMEZONE` env var, or `--timezone` flag) — the named IANA timezone is used directly, resolved from a timezone database bundled in the binary. This does not depend on anything installed in the container and is the recommended way to pin a specific timezone regardless of deployment environment.
2. **`timezone` unset** — falls back to the process's system-local timezone, which on Linux is controlled by the standard `TZ` environment variable (and requires the `tzdata` package to be installed — the published Docker image includes it and defaults to `ENV TZ=Etc/UTC`, overridable with `docker run -e TZ=Region/City ...`).

An invalid IANA name in `timezone` is logged as a warning at startup and falls back to the system-local timezone rather than failing to start.

### `[concurrency]`

Controls the optional concurrency limiter. When enabled, requests that arrive while `max_concurrency` requests are already in flight are queued. Requests that wait longer than `max_wait_secs` are rejected with `429 Too Many Requests`.

| Key               | Type    | Default | Description                                                          |
|-------------------|---------|---------|----------------------------------------------------------------------|
| `max_concurrency` | integer | `0`     | Maximum simultaneous requests. `0` disables the limiter entirely.    |
| `max_wait_secs`   | integer | `30`    | Seconds a queued request will wait for a slot before being rejected. |

### `[storage]`

Controls where the server reads TMSL model files and parquet dataset files from. Two independent backends are configured: `models` (TMSL files) and `datasets` (parquet files). If the `storage` section is omitted both default to the local filesystem with root `./demodata`, which is where the demo dataset (`generate-demodata`) lands by default.

Each backend is identified by a `type` key. The `data_source` field in each TMSL table is always interpreted as a **path relative to the `datasets` root**.

#### Local filesystem

```yaml
storage:
  models:
    type: local
    root: ./models        # directory containing TMSL files
  datasets:
    type: local
    root: ./data          # directory containing parquet files
```

| Key    | Type   | Default        | Description                                                                           |
|--------|--------|----------------|-----------------------------------------------------------------------------------------|
| `root` | string | `"./demodata"` | Absolute or working-directory-relative path that all file paths are resolved against. |

#### Amazon S3 (and S3-compatible stores)

```yaml
storage:
  datasets:
    type: s3
    bucket: my-data-bucket
    region: eu-west-1
    root: /datasets               # key prefix within the bucket
    endpoint: http://localhost:9000  # optional — omit for AWS, set for MinIO etc.
    access_key_id: AKIA...           # optional — falls back to env / instance role
    secret_access_key: "..."         # optional — falls back to env / instance role
```

| Key                 | Type   | Required | Description                                           |
|---------------------|--------|----------|-------------------------------------------------------|
| `bucket`            | string | yes      | S3 bucket name.                                       |
| `region`            | string | yes      | AWS region (e.g. `eu-west-1`).                        |
| `root`              | string | no       | Key prefix prepended to all paths. Defaults to `"./demodata"`. |
| `endpoint`          | string | no       | Custom endpoint URL. Use for MinIO, LocalStack, or any S3-compatible store. |
| `access_key_id`     | string | no       | AWS access key ID. If omitted the SDK credential chain is used (env vars, instance metadata, etc.). |
| `secret_access_key` | string | no       | AWS secret access key. If omitted the SDK credential chain is used. |

#### Azure Blob Storage

```yaml
storage:
  datasets:
    type: azblob
    account_name: myaccount
    container: my-container
    root: /datasets
    account_key: "..."      # optional — one of account_key or sas_token
    sas_token: "?sv=..."    # optional
```

| Key            | Type   | Required | Description                                                |
|----------------|--------|----------|------------------------------------------------------------|
| `account_name` | string | yes      | Azure storage account name.                                |
| `container`    | string | yes      | Blob container name.                                       |
| `root`         | string | no       | Blob path prefix. Defaults to `"./demodata"`.                       |
| `account_key`  | string | no       | Base64-encoded storage account key.                        |
| `sas_token`    | string | no       | Shared Access Signature token (including the leading `?`). |

#### Google Cloud Storage

```yaml
storage:
  datasets:
    type: gcs
    bucket: my-gcs-bucket
    root: /datasets
    credential_path: /run/secrets/sa.json   # optional
```

| Key               | Type   | Required | Description                            |
|-------------------|--------|----------|----------------------------------------|
| `bucket`          | string | yes      | GCS bucket name.                       |
| `root`            | string | no       | Object path prefix. Defaults to `"./demodata"`. |
| `credential_path` | string | no       | Path to a service account JSON key file. If omitted, Application Default Credentials are used. |

#### Mixed example — local models, S3 datasets

```yaml
storage:
  models:
    type: local
    root: ./models
  datasets:
    type: s3
    bucket: analytics-data
    region: eu-west-1
    root: /parquets
```

### Serving multiple models

The server always scans the `models` storage root (`storage.models` / `DAX_MODELS_*` / `--models-*`) for every `*.json` TMSL file and loads each one as a separate model — this works uniformly whether the backend is local disk, S3, Azure, or GCS.

```yaml
storage:
  models:
    type: local
    root: ./models   # every *.json file here is loaded as a separate model
  datasets:
    type: local
    root: ./data
```

Each model's database name is taken from the `name` field inside the TMSL file; if absent, the file stem is used (e.g. `sales.json` → `Sales`). To serve a single model, put just that one TMSL file in the `models` root.

## Environment variables

| Variable                | Equivalent key                |
|-------------------------|-------------------------------|
| `DAX_CONFIG`            | Path to the config file       |
| `DAX_SERVER_NAME`       | `server_name`                 |
| `DAX_HOSTNAME`          | `hostname`                    |
| `DAX_PORT`              | `port`                        |
| `DAX_LOCALE_IDENTIFIER` | `locale_identifier`           |
| `DAX_MAX_CONCURRENCY`   | `concurrency.max_concurrency` |
| `DAX_MAX_WAIT_SECS`     | `concurrency.max_wait_secs`   |
| `DAX_LOG_LEVEL`         | `log_level`                   |
| `DAX_TIMEZONE`          | `timezone`                    |

### Storage environment variables

Env vars are the recommended way to supply credentials — they do not appear in process lists or shell history. All storage fields are available as env vars; credentials are **only** available as env vars (not CLI flags).

Each variable exists in both a `DAX_MODELS_*` and a `DAX_DATASETS_*` form.

| Variable                                                          | Field                                   |
|--------------------------------------------------------------------|-----------------------------------------|
| `DAX_MODELS_TYPE` / `DAX_DATASETS_TYPE`                           | `type` (`local`, `s3`, `azblob`, `gcs`) |
| `DAX_MODELS_ROOT` / `DAX_DATASETS_ROOT`                           | `root`                                  |
| `DAX_MODELS_BUCKET` / `DAX_DATASETS_BUCKET`                       | `bucket` (S3, GCS)                      |
| `DAX_MODELS_REGION` / `DAX_DATASETS_REGION`                       | `region` (S3)                           |
| `DAX_MODELS_ENDPOINT` / `DAX_DATASETS_ENDPOINT`                   | `endpoint` (S3)                         |
| `DAX_MODELS_ACCOUNT_NAME` / `DAX_DATASETS_ACCOUNT_NAME`           | `account_name` (Azure Blob)             |
| `DAX_MODELS_CONTAINER` / `DAX_DATASETS_CONTAINER`                 | `container` (Azure Blob)                |
| `DAX_MODELS_ACCESS_KEY_ID` / `DAX_DATASETS_ACCESS_KEY_ID`         | `access_key_id` (S3) — credential       |
| `DAX_MODELS_SECRET_ACCESS_KEY` / `DAX_DATASETS_SECRET_ACCESS_KEY` | `secret_access_key` (S3) — credential   |
| `DAX_MODELS_ACCOUNT_KEY` / `DAX_DATASETS_ACCOUNT_KEY`             | `account_key` (Azure Blob) — credential |
| `DAX_MODELS_SAS_TOKEN` / `DAX_DATASETS_SAS_TOKEN`                 | `sas_token` (Azure Blob) — credential   |
| `DAX_MODELS_CREDENTIAL_PATH` / `DAX_DATASETS_CREDENTIAL_PATH`     | `credential_path` (GCS) — credential    |

## CLI flags

| Flag                          | Equivalent key                |
|-------------------------------|-------------------------------|
| `--config <path>`             | Path to the config file       |
| `--server-name <value>`       | `server_name`                 |
| `--hostname <value>`          | `hostname`                    |
| `--port <value>`              | `port`                        |
| `--locale-identifier <value>` | `locale_identifier`           |
| `--max-concurrency <value>`   | `concurrency.max_concurrency` |
| `--max-wait-secs <value>`     | `concurrency.max_wait_secs`   |
| `--log-level <value>`         | `log_level`                   |
| `--timezone <value>`          | `timezone`                    |

### Storage CLI flags

Each flag exists in both a `--models-*` and a `--datasets-*` form. Credentials are not available as CLI flags — use environment variables instead.

| Flag                                              | Field                                   |
|----------------------------------------------------|-----------------------------------------|
| `--models-type` / `--datasets-type`                | `type` (`local`, `s3`, `azblob`, `gcs`) |
| `--models-root` / `--datasets-root`                | `root`                                  |
| `--models-bucket` / `--datasets-bucket`            | `bucket` (S3, GCS)                      |
| `--models-region` / `--datasets-region`            | `region` (S3)                           |
| `--models-endpoint` / `--datasets-endpoint`        | `endpoint` (S3)                         |
| `--models-account-name` / `--datasets-account-name`| `account_name` (Azure Blob)             |
| `--models-container` / `--datasets-container`      | `container` (Azure Blob)                |

Both `--flag value` and `--flag=value` forms are accepted.

## Usage

```
cargo run --bin dax-rs --models-root ./models [--config server.yaml] [flags...]
```

Every `*.json` file found in the `models` storage root is loaded as a separate model.
