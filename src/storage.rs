use opendal::{blocking::Operator as BlockingOperator, services, Operator};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendConfig {
    Local {
        #[serde(default = "default_root")]
        root: String,
    },
    #[serde(rename = "s3")]
    S3 {
        bucket: String,
        region: String,
        #[serde(default = "default_root")]
        root: String,
        #[serde(default)]
        endpoint: Option<String>,
        #[serde(default)]
        access_key_id: Option<String>,
        #[serde(default)]
        secret_access_key: Option<String>,
    },
    #[serde(rename = "azblob")]
    AzureBlob {
        account_name: String,
        container: String,
        #[serde(default = "default_root")]
        root: String,
        #[serde(default)]
        account_key: Option<String>,
        #[serde(default)]
        sas_token: Option<String>,
    },
    #[serde(rename = "gcs")]
    Gcs {
        bucket: String,
        #[serde(default = "default_root")]
        root: String,
        #[serde(default)]
        credential_path: Option<String>,
    },
}

fn default_root() -> String {
    "./demodata".into()
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self::Local { root: default_root() }
    }
}

impl BackendConfig {
    fn type_str(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::S3 { .. } => "s3",
            Self::AzureBlob { .. } => "azblob",
            Self::Gcs { .. } => "gcs",
        }
    }

    pub fn apply_overrides(self, ov: &BackendOverrides) -> Self {
        let effective_type = ov
            .type_
            .clone()
            .unwrap_or_else(|| self.type_str().to_string());

        let (
            base_root,
            base_bucket,
            base_region,
            base_endpoint,
            base_access_key_id,
            base_secret_access_key,
            base_account_name,
            base_container,
            base_account_key,
            base_sas_token,
            base_credential_path,
        ) = match self {
            Self::Local { root } => (
                root, None, None, None, None, None, None, None, None, None, None,
            ),
            Self::S3 {
                root,
                bucket,
                region,
                endpoint,
                access_key_id,
                secret_access_key,
            } => (
                root,
                Some(bucket),
                Some(region),
                endpoint,
                access_key_id,
                secret_access_key,
                None,
                None,
                None,
                None,
                None,
            ),
            Self::AzureBlob { root, account_name, container, account_key, sas_token } => (
                root,
                None,
                None,
                None,
                None,
                None,
                Some(account_name),
                Some(container),
                account_key,
                sas_token,
                None,
            ),
            Self::Gcs { root, bucket, credential_path } => (
                root,
                Some(bucket),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                credential_path,
            ),
        };

        let root = ov.root.clone().unwrap_or(base_root);
        let bucket = ov.bucket.clone().or(base_bucket).unwrap_or_default();
        let region = ov.region.clone().or(base_region).unwrap_or_default();

        match effective_type.as_str() {
            "local" => Self::Local { root },
            "s3" => Self::S3 {
                bucket,
                region,
                root,
                endpoint: ov.endpoint.clone().or(base_endpoint),
                access_key_id: ov.access_key_id.clone().or(base_access_key_id),
                secret_access_key: ov.secret_access_key.clone().or(base_secret_access_key),
            },
            "azblob" => Self::AzureBlob {
                account_name: ov
                    .account_name
                    .clone()
                    .or(base_account_name)
                    .unwrap_or_default(),
                container: ov.container.clone().or(base_container).unwrap_or_default(),
                root,
                account_key: ov.account_key.clone().or(base_account_key),
                sas_token: ov.sas_token.clone().or(base_sas_token),
            },
            "gcs" => Self::Gcs {
                bucket,
                root,
                credential_path: ov.credential_path.clone().or(base_credential_path),
            },
            _ => Self::Local { root }, // unknown type — fall back to local
        }
    }
}

#[derive(Default)]
pub struct BackendOverrides {
    pub type_: Option<String>,
    pub root: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    /// Env-var only — not exposed as a CLI flag.
    pub access_key_id: Option<String>,
    /// Env-var only — not exposed as a CLI flag.
    pub secret_access_key: Option<String>,
    pub account_name: Option<String>,
    pub container: Option<String>,
    /// Env-var only — not exposed as a CLI flag.
    pub account_key: Option<String>,
    /// Env-var only — not exposed as a CLI flag.
    pub sas_token: Option<String>,
    /// Env-var only — not exposed as a CLI flag.
    pub credential_path: Option<String>,
}

pub fn env_overrides(prefix: &str) -> BackendOverrides {
    let var = |name: &str| std::env::var(format!("{prefix}_{name}")).ok();
    BackendOverrides {
        type_: var("TYPE"),
        root: var("ROOT"),
        bucket: var("BUCKET"),
        region: var("REGION"),
        endpoint: var("ENDPOINT"),
        access_key_id: var("ACCESS_KEY_ID"),
        secret_access_key: var("SECRET_ACCESS_KEY"),
        account_name: var("ACCOUNT_NAME"),
        container: var("CONTAINER"),
        account_key: var("ACCOUNT_KEY"),
        sas_token: var("SAS_TOKEN"),
        credential_path: var("CREDENTIAL_PATH"),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StorageConfig {
    #[serde(default)]
    pub models: BackendConfig,
    #[serde(default)]
    pub datasets: BackendConfig,
}

pub fn build_operator(cfg: &BackendConfig) -> Result<Arc<BlockingOperator>, opendal::Error> {
    let op: Operator = match cfg {
        BackendConfig::Local { root } => {
            Operator::new(services::Fs::default().root(root))?.finish()
        }
        BackendConfig::S3 {
            bucket,
            region,
            root,
            endpoint,
            access_key_id,
            secret_access_key,
        } => {
            let b = services::S3::default()
                .root(root)
                .bucket(bucket)
                .region(region);
            let b = match endpoint {
                Some(v) => b.endpoint(v),
                None => b,
            };
            let b = match access_key_id {
                Some(v) => b.access_key_id(v),
                None => b,
            };
            let b = match secret_access_key {
                Some(v) => b.secret_access_key(v),
                None => b,
            };
            Operator::new(b)?.finish()
        }
        BackendConfig::AzureBlob { account_name, container, root, account_key, sas_token } => {
            let b = services::Azblob::default()
                .root(root)
                .container(container)
                .account_name(account_name);
            let b = match account_key {
                Some(v) => b.account_key(v),
                None => b,
            };
            let b = match sas_token {
                Some(v) => b.sas_token(v),
                None => b,
            };
            Operator::new(b)?.finish()
        }
        BackendConfig::Gcs { bucket, root, credential_path } => {
            let b = services::Gcs::default().root(root).bucket(bucket);
            let b = match credential_path {
                Some(v) => b.credential_path(v),
                None => b,
            };
            Operator::new(b)?.finish()
        }
    };
    Ok(Arc::new(BlockingOperator::new(op)?))
}
