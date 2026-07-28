// Benchmark harness: a query failure here is a setup bug that should panic loudly.
#![allow(clippy::unwrap_used)]
use criterion::{criterion_group, criterion_main, Criterion};
use dax_rs::engine::Engine;
use dax_rs::storage::{build_operator, BackendConfig};
use once_cell::sync::Lazy;
use std::hint::black_box;

// SportRetailer: FactSales (~500K rows). demodata/ holds both the TMSL and the
// parquet files, relative to the project root (the working directory for
// `cargo bench`). Building operators here (rather than Engine::from_tmsl_file,
// which is shared with unrelated tests/fixtures/*.json using full self-contained
// paths) lets us root them at "demodata" to match the new dataSource convention
// of bare filenames.
static ENGINE: Lazy<Engine> = Lazy::new(|| {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
        tokio::runtime::Runtime::new()
            .expect("failed to create runtime for blocking opendal operator")
    });
    let _guard = RUNTIME.enter();
    let root = BackendConfig::Local { root: "demodata".into() };
    let catalogs_op = build_operator(&root).expect("build catalogs operator");
    let datasets_op = build_operator(&root).expect("build datasets operator");
    Engine::from_storage(&catalogs_op, "sport_retailer.json", datasets_op)
        .expect("Failed to load SportRetailer engine from demodata/sport_retailer.json")
});

// Deliberately the "wrong" currency-conversion pattern Currency_Conversion.md
// warns against: SUMX iterates every FactSales row (~500K) and does a
// per-row LOOKUPVALUE against FactExchangeRate (24 months x 2 rate types).
// The model's own "Gross Sales" measure avoids this by pre-aggregating via
// SUMMARIZE(Currency, Year, Month) first — a few dozen groups instead of
// 500K rows. This benchmark isolates raw per-row SUMX + LOOKUPVALUE cost,
// uncorrupted by that optimization.
const SUMX_QUERY: &str = r#"
EVALUATE
    {
        SUMX(
            FactSales,
            FactSales[NetSales] *
            LOOKUPVALUE(
                FactExchangeRate[Rate],
                FactExchangeRate[Year], YEAR(FactSales[Date]),
                FactExchangeRate[Month], MONTH(FactSales[Date]),
                FactExchangeRate[RateType], "Monthly Average"
            )
        )
    }
"#;

fn sumx_currency_conversion_benchmark(c: &mut Criterion) {
    Lazy::force(&ENGINE); // pay parquet-loading cost once, outside the timed loop
    c.bench_function("sumx_fact_sales_row_by_row_fx_lookup", |b| {
        b.iter(|| ENGINE.evaluate_query(black_box(SUMX_QUERY)).unwrap())
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = sumx_currency_conversion_benchmark
}
criterion_main!(benches);
