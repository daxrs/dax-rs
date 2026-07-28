// Benchmark harness: a query failure here is a setup bug that should panic loudly.
#![allow(clippy::unwrap_used)]
use criterion::{criterion_group, criterion_main, Criterion};
use dax_rs::engine::Engine;
use dax_rs::storage::{build_operator, BackendConfig};
use once_cell::sync::Lazy;
use std::hint::black_box;

// SportRetailer: FactSales (~500K rows) joined to DimProduct via ProductKey.
// demodata/ holds both the TMSL and the parquet files, relative to the project
// root (the working directory for `cargo bench`). Building operators here
// (rather than Engine::from_tmsl_file, which is shared with unrelated
// tests/fixtures/*.json using full self-contained paths) lets us root them at
// "demodata" to match the new dataSource convention of bare filenames.
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

// PowerBI matrix-visual query: rows = DimProduct[Category], columns =
// FactSales[Country], values = [Net Sales], with grand-total row/column via
// ROLLUPADDISSUBTOTAL and a two-step TOPN/SUBSTITUTEWITHINDEX pipeline to
// build column indices for the matrix body.
const MATRIX_QUERY: &str = r#"
DEFINE
    VAR __DS0Core =
        SUMMARIZECOLUMNS(
            ROLLUPADDISSUBTOTAL('DimProduct'[Category], "IsGrandTotalRowTotal"),
            ROLLUPADDISSUBTOTAL('FactSales'[Country], "IsGrandTotalColumnTotal"),
            "Net_Sales", 'FactSales'[Net Sales]
        )

    VAR __DS0PrimaryWindowed =
        TOPN(
            102,
            SUMMARIZE(__DS0Core, 'DimProduct'[Category], [IsGrandTotalRowTotal]),
            [IsGrandTotalRowTotal],
            0,
            'DimProduct'[Category],
            1
        )

    VAR __DS0SecondaryBase =
        SUMMARIZE(__DS0Core, 'FactSales'[Country], [IsGrandTotalColumnTotal])

    VAR __DS0Secondary =
        TOPN(102, __DS0SecondaryBase, [IsGrandTotalColumnTotal], 1, 'FactSales'[Country], 1)

    VAR __DS0BodyLimited =
        NATURALLEFTOUTERJOIN(
            __DS0PrimaryWindowed,
            SUBSTITUTEWITHINDEX(
                __DS0Core,
                "ColumnIndex",
                __DS0Secondary,
                [IsGrandTotalColumnTotal],
                ASC,
                'FactSales'[Country],
                ASC
            )
        )

EVALUATE
    __DS0Secondary

ORDER BY
    [IsGrandTotalColumnTotal], 'FactSales'[Country]

EVALUATE
    __DS0BodyLimited

ORDER BY
    [IsGrandTotalRowTotal] DESC, 'DimProduct'[Category], [ColumnIndex]
"#;

fn matrix_query_currency_benchmark(c: &mut Criterion) {
    Lazy::force(&ENGINE); // pay parquet-loading cost once, outside the timed loop
    c.bench_function("powerbi_matrix_category_country_net_sales", |b| {
        b.iter(|| ENGINE.evaluate_query(black_box(MATRIX_QUERY)).unwrap())
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = matrix_query_currency_benchmark
}
criterion_main!(benches);
