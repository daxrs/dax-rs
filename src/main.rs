use std::sync::Arc;

use dax_rs::server;
use dax_rs::server::config::CliArgs;
use dax_rs::server::dax_provider::{DaxDatabaseProvider, DaxServerProvider};
use dax_rs::storage::build_operator;

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

    let level_filter = args
        .config
        .log_level
        .parse::<tracing_subscriber::filter::LevelFilter>()
        .unwrap_or_else(|_| {
            eprintln!(
                "Warning: invalid log_level '{}', defaulting to 'info'",
                args.config.log_level
            );
            tracing_subscriber::filter::LevelFilter::INFO
        });

    let file_appender = tracing_appender::rolling::never(".", "xmla.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
        .with(level_filter)
        .init();

    let datasets_op = build_operator(&args.storage.datasets)
        .unwrap_or_else(|e| panic!("Failed to initialise datasets storage: {e}"));

    let models_op = build_operator(&args.storage.models)
        .unwrap_or_else(|e| panic!("Failed to initialise models storage: {e}"));

    let timezone = args.config.timezone.clone();

    let db_providers: Vec<DaxDatabaseProvider> = tokio::task::spawn_blocking(move || {
        let mut files: Vec<String> = models_op
            .list("/")
            .unwrap_or_else(|e| panic!("Cannot list models storage: {e}"))
            .into_iter()
            .filter(|entry| entry.path().ends_with(".json"))
            .map(|entry| entry.name().to_string())
            .collect();
        files.sort();

        if files.is_empty() {
            panic!("No model files found in the models storage");
        }

        let entries: Vec<(String, Arc<opendal::blocking::Operator>)> =
            files.into_iter().map(|f| (f, Arc::clone(&models_op))).collect();

        struct LoadedModel {
            name: String,
            tmsl_path: String,
            catalogs_op: Arc<opendal::blocking::Operator>,
            engine: dax_rs::engine::Engine,
        }

        let mut loaded: Vec<LoadedModel> = Vec::new();

        for (tmsl_path, catalogs_op) in entries {
            let mut engine = dax_rs::engine::Engine::from_storage(&catalogs_op, &tmsl_path, Arc::clone(&datasets_op))
                .unwrap_or_else(|e| panic!("Failed to load model '{tmsl_path}': {e}"));

            if let Err(e) = engine.set_timezone(timezone.as_deref()) {
                eprintln!("Warning: {e}, defaulting to system local timezone");
            }

            engine.warmup();

            let name = engine.ctx().catalog.model_name.clone()
                .or_else(|| {
                    std::path::Path::new(&tmsl_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "Model".to_string());

            loaded.push(LoadedModel { name, tmsl_path, catalogs_op, engine });
        }

        let mut name_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for m in &loaded {
            *name_counts.entry(m.name.to_ascii_lowercase()).or_insert(0) += 1;
        }
        for m in &mut loaded {
            if name_counts[&m.name.to_ascii_lowercase()] > 1 {
                let original = m.name.clone();
                m.name = format!("{} ({})", m.name, m.tmsl_path);
                tracing::warn!(
                    original_name = %original,
                    renamed_to = %m.name,
                    path = %m.tmsl_path,
                    "duplicate catalog name across models — renamed to disambiguate"
                );
            }
        }

        let mut db_providers: Vec<DaxDatabaseProvider> = Vec::new();

        for m in loaded {
            let table_count   = m.engine.ctx().catalog.table_order.len();
            let measure_count = m.engine.ctx().catalog.measure_order.len();
            tracing::info!(model = %m.name, tables = table_count, measures = measure_count, "model loaded");

            db_providers.push(
                DaxDatabaseProvider::new(m.name, m.engine)
                    .with_catalogs_storage(m.catalogs_op, &m.tmsl_path),
            );
        }

        db_providers
    })
    .await
    .expect("model-loading task panicked");

    let provider = Arc::new(DaxServerProvider::new(db_providers));
    server::run(provider, args.config).await;
}
