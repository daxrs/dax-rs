use crate::server::storage::{BackendOverrides, StorageConfig};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct ConcurrencyConfig {
    /// 0 = disabled (no limit). Any positive value enables the limiter.
    #[serde(default)]
    pub max_concurrency: usize,
    #[serde(default = "default_max_wait_secs")]
    pub max_wait_secs: u64,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self { max_concurrency: 0, max_wait_secs: default_max_wait_secs() }
    }
}

fn default_max_wait_secs() -> u64 {
    30
}

pub struct ServerConfig {
    pub server_name: String,
    pub hostname: String,
    pub port: u16,
    pub locale_identifier: u32,
    pub concurrency: ConcurrencyConfig,
    pub log_level: String,
    pub timezone: Option<String>,
}

impl ServerConfig {
    pub fn xmla_url(&self) -> String {
        format!("http://{}:{}/xmla", self.hostname, self.port)
    }

    pub fn data_source_info(&self) -> String {
        format!("Provider=MSOLAP;Data Source={}", self.xmla_url())
    }

    pub fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server_name: "dax-rs".into(),
            hostname: "localhost".into(),
            port: 3000,
            locale_identifier: 1033,
            concurrency: ConcurrencyConfig::default(),
            log_level: "info".into(),
            timezone: None,
        }
    }
}

#[derive(Deserialize, Default)]
struct ConfigFile {
    server_name: Option<String>,
    hostname: Option<String>,
    port: Option<u16>,
    locale_identifier: Option<u32>,
    concurrency: Option<ConcurrencyConfig>,
    storage: Option<StorageConfig>,
    log_level: Option<String>,
    timezone: Option<String>,
}

pub struct CliArgs {
    pub config: ServerConfig,
    pub storage: StorageConfig,
}

impl CliArgs {
    pub fn parse() -> Self {
        let raw: Vec<String> = std::env::args().skip(1).collect();

        let mut config_path: Option<String> = std::env::var("DAX_CONFIG").ok();
        let mut cli_server_name: Option<String> = None;
        let mut cli_hostname: Option<String> = None;
        let mut cli_port: Option<u16> = None;
        let mut cli_locale_identifier: Option<u32> = None;
        let mut cli_max_concurrency: Option<usize> = None;
        let mut cli_max_wait_secs: Option<u64> = None;
        let mut cli_log_level: Option<String> = None;
        let mut cli_timezone: Option<String> = None;
        let mut cli_models = BackendOverrides::default();
        let mut cli_datasets = BackendOverrides::default();

        let mut i = 0;
        while i < raw.len() {
            let arg = raw[i].as_str();
            if let Some(val) = flag_value(arg, "--config", &raw, &mut i) {
                config_path = Some(val);
            } else if let Some(val) = flag_value(arg, "--server-name", &raw, &mut i) {
                cli_server_name = Some(val);
            } else if let Some(val) = flag_value(arg, "--hostname", &raw, &mut i) {
                cli_hostname = Some(val);
            } else if let Some(val) = flag_value(arg, "--port", &raw, &mut i) {
                cli_port = val.parse().ok();
            } else if let Some(val) = flag_value(arg, "--locale-identifier", &raw, &mut i) {
                cli_locale_identifier = val.parse().ok();
            } else if let Some(val) = flag_value(arg, "--max-concurrency", &raw, &mut i) {
                cli_max_concurrency = val.parse().ok();
            } else if let Some(val) = flag_value(arg, "--max-wait-secs", &raw, &mut i) {
                cli_max_wait_secs = val.parse().ok();
            } else if let Some(val) = flag_value(arg, "--log-level", &raw, &mut i) {
                cli_log_level = Some(val);
            } else if let Some(val) = flag_value(arg, "--timezone", &raw, &mut i) {
                cli_timezone = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-type", &raw, &mut i) {
                cli_models.type_ = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-root", &raw, &mut i) {
                cli_models.root = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-bucket", &raw, &mut i) {
                cli_models.bucket = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-region", &raw, &mut i) {
                cli_models.region = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-endpoint", &raw, &mut i) {
                cli_models.endpoint = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-account-name", &raw, &mut i) {
                cli_models.account_name = Some(val);
            } else if let Some(val) = flag_value(arg, "--models-container", &raw, &mut i) {
                cli_models.container = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-type", &raw, &mut i) {
                cli_datasets.type_ = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-root", &raw, &mut i) {
                cli_datasets.root = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-bucket", &raw, &mut i) {
                cli_datasets.bucket = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-region", &raw, &mut i) {
                cli_datasets.region = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-endpoint", &raw, &mut i) {
                cli_datasets.endpoint = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-account-name", &raw, &mut i) {
                cli_datasets.account_name = Some(val);
            } else if let Some(val) = flag_value(arg, "--datasets-container", &raw, &mut i) {
                cli_datasets.container = Some(val);
            }
            i += 1;
        }

        let mut cfg = ServerConfig::default();

        let file_path = config_path.as_deref().unwrap_or("server.yaml");
        let mut storage = StorageConfig::default();
        if let Ok(content) = std::fs::read_to_string(file_path) {
            match serde_yaml_ng::from_str::<ConfigFile>(&content) {
                Ok(file_cfg) => {
                    if let Some(v) = file_cfg.server_name {
                        cfg.server_name = v;
                    }
                    if let Some(v) = file_cfg.hostname {
                        cfg.hostname = v;
                    }
                    if let Some(v) = file_cfg.port {
                        cfg.port = v;
                    }
                    if let Some(v) = file_cfg.locale_identifier {
                        cfg.locale_identifier = v;
                    }
                    if let Some(v) = file_cfg.concurrency {
                        cfg.concurrency = v;
                    }
                    if let Some(v) = file_cfg.storage {
                        storage = v;
                    }
                    if let Some(v) = file_cfg.log_level {
                        cfg.log_level = v;
                    }
                    if let Some(v) = file_cfg.timezone {
                        cfg.timezone = Some(v);
                    }
                }
                Err(e) => eprintln!("Warning: could not parse {file_path}: {e}"),
            }
        }

        if let Ok(v) = std::env::var("DAX_SERVER_NAME") {
            cfg.server_name = v;
        }
        if let Ok(v) = std::env::var("DAX_HOSTNAME") {
            cfg.hostname = v;
        }
        if let Ok(v) = std::env::var("DAX_PORT") {
            if let Ok(n) = v.parse() {
                cfg.port = n;
            }
        }
        if let Ok(v) = std::env::var("DAX_LOCALE_IDENTIFIER") {
            if let Ok(n) = v.parse() {
                cfg.locale_identifier = n;
            }
        }
        if let Ok(v) = std::env::var("DAX_MAX_CONCURRENCY") {
            if let Ok(n) = v.parse() {
                cfg.concurrency.max_concurrency = n;
            }
        }
        if let Ok(v) = std::env::var("DAX_MAX_WAIT_SECS") {
            if let Ok(n) = v.parse() {
                cfg.concurrency.max_wait_secs = n;
            }
        }
        if let Ok(v) = std::env::var("DAX_LOG_LEVEL") {
            cfg.log_level = v;
        }
        if let Ok(v) = std::env::var("DAX_TIMEZONE") {
            cfg.timezone = Some(v);
        }

        storage.models = storage
            .models
            .apply_overrides(&crate::server::storage::env_overrides("DAX_MODELS"));
        storage.datasets = storage
            .datasets
            .apply_overrides(&crate::server::storage::env_overrides("DAX_DATASETS"));

        if let Some(v) = cli_server_name {
            cfg.server_name = v;
        }
        if let Some(v) = cli_hostname {
            cfg.hostname = v;
        }
        if let Some(v) = cli_port {
            cfg.port = v;
        }
        if let Some(v) = cli_locale_identifier {
            cfg.locale_identifier = v;
        }
        if let Some(v) = cli_max_concurrency {
            cfg.concurrency.max_concurrency = v;
        }
        if let Some(v) = cli_max_wait_secs {
            cfg.concurrency.max_wait_secs = v;
        }
        if let Some(v) = cli_log_level {
            cfg.log_level = v;
        }
        if let Some(v) = cli_timezone {
            cfg.timezone = Some(v);
        }

        storage.models = storage.models.apply_overrides(&cli_models);
        storage.datasets = storage.datasets.apply_overrides(&cli_datasets);

        Self { config: cfg, storage }
    }
}

fn flag_value(arg: &str, flag: &str, raw: &[String], i: &mut usize) -> Option<String> {
    let eq = format!("{flag}=");
    if arg == flag {
        *i += 1;
        raw.get(*i).cloned()
    } else {
        arg.strip_prefix(eq.as_str()).map(|rest| rest.to_string())
    }
}
