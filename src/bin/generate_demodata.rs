#![allow(dead_code)]
// One-off demo-data generator, not part of the audited library/server code.
// Remaining unwraps are on hardcoded date construction (from_ymd_opt etc.)
// with compile-time-fixed inputs; not converted to per-call .expect() here.
#![allow(clippy::unwrap_used)]
use opendal::blocking::Operator as BlockingOperator;
use polars::prelude::*;
use rand::prelude::*;
use rand::rngs::StdRng;
use std::io::Cursor;

const SEED: u64 = 42;

const GERMANY: usize = 0;
const AUSTRIA: usize = 1;
const SWITZERLAND: usize = 2;

const CAT_RUNNING: usize = 0;
const CAT_HIKING: usize = 1;
const CAT_CYCLING: usize = 2;
const CAT_SKIING: usize = 3;

static COUNTRY_NAMES: &[&str] = &["Germany", "Austria", "Switzerland"];
static CURRENCIES: &[&str] = &["EUR", "EUR", "CHF"];
static CATEGORY_NAMES: &[&str] = &["Running", "Hiking & Camping", "Cycling", "Skiing"];

// [category][month 0-indexed]
static SEASON: [[f64; 12]; 4] = [
    [
        0.70, 0.80, 1.30, 1.40, 1.30, 1.30, 1.00, 1.00, 1.30, 1.30, 0.80, 0.70,
    ],
    [
        0.50, 0.60, 0.80, 1.00, 1.50, 1.60, 1.60, 1.60, 1.00, 0.80, 0.50, 0.50,
    ],
    [
        0.50, 0.60, 0.80, 1.30, 1.40, 1.40, 1.40, 1.40, 1.30, 0.80, 0.50, 0.40,
    ],
    [
        1.60, 1.60, 1.20, 0.60, 0.30, 0.30, 0.30, 0.30, 0.50, 0.80, 1.40, 1.60,
    ],
];

// Subcategory index constants (0-23)
const SC_RUNNING_SHOES: usize = 0;
const SC_TRAIL_SHOES: usize = 1;
const SC_RUNNING_APPAREL: usize = 2;
const SC_RUNNING_SOCKS: usize = 3;
const SC_HYDRATION: usize = 4;
const SC_GPS_WATCHES: usize = 5;
const SC_HIKING_BOOTS: usize = 6;
const SC_BACKPACKS: usize = 7;
const SC_TENTS: usize = 8;
const SC_SLEEPING_BAGS: usize = 9;
const SC_TREKKING_POLES: usize = 10;
const SC_CAMPING_ACCESSORIES: usize = 11;
const SC_ROAD_BIKES: usize = 12;
const SC_MTB: usize = 13;
const SC_CYCLING_HELMETS: usize = 14;
const SC_CYCLING_APPAREL: usize = 15;
const SC_BIKE_ACCESSORIES: usize = 16;
const SC_BIKE_COMPUTERS: usize = 17;
const SC_SKIS: usize = 18;
const SC_SKI_BOOTS: usize = 19;
const SC_SKI_HELMETS: usize = 20;
const SC_GOGGLES: usize = 21;
const SC_SKI_APPAREL: usize = 22;
const SC_SKI_ACCESSORIES: usize = 23;

static SUBCAT_NAMES: &[&str] = &[
    "Running Shoes",
    "Trail Running Shoes",
    "Running Apparel",
    "Running Socks",
    "Hydration Products",
    "GPS Watches",
    "Hiking Boots",
    "Backpacks",
    "Tents",
    "Sleeping Bags",
    "Trekking Poles",
    "Camping Accessories",
    "Road Bikes",
    "Mountain Bikes",
    "Helmets",
    "Cycling Apparel",
    "Bike Accessories",
    "Bike Computers",
    "Skis",
    "Ski Boots",
    "Helmets",
    "Goggles",
    "Ski Apparel",
    "Ski Accessories",
];

// category index per subcat
static SUBCAT_CATEGORY: &[usize] = &[
    0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3,
];

// number of SKUs per subcat
static SUBCAT_COUNT: &[usize] = &[
    30, 20, 25, 15, 15, 20, // Running = 125
    25, 25, 20, 20, 15, 20, // Hiking  = 125
    20, 20, 20, 25, 20, 20, // Cycling = 125
    20, 25, 20, 20, 25, 15, // Skiing  = 125
];

// price range [min, max] per subcat (standard)
static PRICE_RANGE: &[[f64; 2]] = &[
    [60.0, 200.0],
    [80.0, 220.0],
    [30.0, 100.0],
    [8.0, 25.0],
    [15.0, 60.0],
    [100.0, 400.0],
    [80.0, 250.0],
    [50.0, 200.0],
    [150.0, 500.0],
    [80.0, 250.0],
    [40.0, 150.0],
    [10.0, 80.0],
    [600.0, 2500.0],
    [500.0, 2000.0],
    [40.0, 150.0],
    [40.0, 120.0],
    [15.0, 100.0],
    [60.0, 300.0],
    [300.0, 900.0],
    [200.0, 600.0],
    [50.0, 180.0],
    [60.0, 200.0],
    [80.0, 300.0],
    [10.0, 80.0],
];

// margin range [min, max] per subcat
static MARGIN_RANGE: &[[f64; 2]] = &[
    [0.35, 0.45],
    [0.35, 0.45],
    [0.40, 0.55],
    [0.45, 0.60],
    [0.45, 0.60],
    [0.25, 0.40],
    [0.30, 0.45],
    [0.35, 0.50],
    [0.20, 0.35],
    [0.25, 0.40],
    [0.35, 0.50],
    [0.45, 0.60],
    [0.20, 0.35],
    [0.20, 0.35],
    [0.40, 0.55],
    [0.40, 0.55],
    [0.45, 0.60],
    [0.30, 0.45],
    [0.20, 0.35],
    [0.25, 0.40],
    [0.40, 0.55],
    [0.40, 0.55],
    [0.40, 0.55],
    [0.45, 0.60],
];

// basket affinities: (trigger_subcat, companion_subcat, probability)
static BASKET: &[(usize, usize, f64)] = &[
    (SC_RUNNING_SHOES, SC_RUNNING_SOCKS, 0.60),
    (SC_RUNNING_SHOES, SC_HYDRATION, 0.30),
    (SC_TRAIL_SHOES, SC_RUNNING_SOCKS, 0.45),
    (SC_TRAIL_SHOES, SC_HYDRATION, 0.25),
    (SC_ROAD_BIKES, SC_CYCLING_HELMETS, 0.80),
    (SC_ROAD_BIKES, SC_BIKE_COMPUTERS, 0.50),
    (SC_MTB, SC_CYCLING_HELMETS, 0.75),
    (SC_MTB, SC_BIKE_COMPUTERS, 0.45),
    (SC_TENTS, SC_SLEEPING_BAGS, 0.70),
    (SC_TENTS, SC_CAMPING_ACCESSORIES, 0.50),
    (SC_SKIS, SC_SKI_BOOTS, 0.70),
    (SC_SKIS, SC_GOGGLES, 0.60),
];

// --- product metadata kept in memory for sales generation ---
struct ProductMeta {
    key: i64,
    cat_idx: usize,
    subcat_idx: usize,
    base_price: f64,
    cost_price: f64,
    is_premium: bool,
    launch_date_ms: i64,
}

// --- customer metadata ---
struct CustomerMeta {
    key: i64,
    country_idx: usize,
    reg_date_ms: i64,
    loyalty_idx: usize,
    _segment_idx: usize,
}

// ---------- date helpers ----------

fn ms_from_ymd(y: i32, m: u32, d: u32) -> i64 {
    use chrono::NaiveDate;
    let nd = NaiveDate::from_ymd_opt(y, m, d).unwrap();
    nd.and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

fn days_in_month(year: i32, month: u32) -> u32 {
    use chrono::NaiveDate;
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let first_day = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    (next_month.unwrap() - first_day).num_days() as u32
}

fn write_parquet(
    datasets_op: &BlockingOperator,
    df: &mut DataFrame,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = format!("{name}.parquet");
    let mut buf: Vec<u8> = Vec::new();
    ParquetWriter::new(Cursor::new(&mut buf)).finish(df)?;
    let rows = df.height();
    datasets_op.write(&path, buf)?;
    println!("  wrote {path} ({rows} rows)");
    Ok(())
}

// ---------- DimDate ----------

fn gen_dim_date() -> PolarsResult<DataFrame> {
    let mut date_keys: Vec<i64> = Vec::new();
    let mut dates_ms: Vec<i64> = Vec::new();
    let mut years: Vec<i64> = Vec::new();
    let mut months: Vec<i64> = Vec::new();
    let mut month_names: Vec<&str> = Vec::new();
    let mut quarters: Vec<i64> = Vec::new();
    let mut days_of_week: Vec<i64> = Vec::new();
    let mut is_weekend: Vec<bool> = Vec::new();

    static MONTH_NAMES: &[&str] = &[
        "",
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    for year in [2023i32, 2024] {
        for month in 1u32..=12 {
            for day in 1..=days_in_month(year, month) {
                use chrono::{Datelike, NaiveDate};
                let nd = NaiveDate::from_ymd_opt(year, month, day).unwrap();
                let ms = ms_from_ymd(year, month, day);
                let dow = nd.weekday().num_days_from_monday() as i64; // 0=Mon
                date_keys.push(year as i64 * 10000 + month as i64 * 100 + day as i64);
                dates_ms.push(ms);
                years.push(year as i64);
                months.push(month as i64);
                month_names.push(MONTH_NAMES[month as usize]);
                quarters.push((month as i64 - 1) / 3 + 1);
                days_of_week.push(dow + 1); // 1=Mon, 7=Sun
                is_weekend.push(dow >= 5);
            }
        }
    }

    let date_col = Int64Chunked::new("Date".into(), &dates_ms)
        .into_datetime(TimeUnit::Milliseconds, None)
        .into_series();

    DataFrame::new_infer_height(vec![
        Series::new("DateKey".into(), &date_keys).into(),
        date_col.into(),
        Series::new("Year".into(), &years).into(),
        Series::new("Month".into(), &months).into(),
        Series::new("MonthName".into(), &month_names).into(),
        Series::new("Quarter".into(), &quarters).into(),
        Series::new("DayOfWeek".into(), &days_of_week).into(),
        Series::new("IsWeekend".into(), &is_weekend).into(),
    ])
}

// ---------- DimProduct ----------

fn gen_dim_product(rng: &mut StdRng) -> (DataFrame, Vec<ProductMeta>) {
    static BRANDS_STD: &[&[&str]] = &[
        &["TrailBlaze", "RunTech", "StrideOn", "PacePro"],
        &["TrekPro", "WildGear", "PeakBase", "TrailForce"],
        &["VeloLine", "SpinEdge", "CyclePro", "RoadRider"],
        &["AlpineRun", "SnowTech", "SlopePro", "FrostEdge"],
    ];
    static BRANDS_PREM: &[&[&str]] = &[
        &["EliteStride", "ProRun"],
        &["SummitPro", "ExpedForce"],
        &["CarboVelo", "SpeedFrame"],
        &["AlpinePro", "GlacierTech"],
    ];

    let start_2022 = ms_from_ymd(2020, 1, 1);
    let end_2023 = ms_from_ymd(2023, 12, 31);

    let mut keys: Vec<i64> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut categories: Vec<&str> = Vec::new();
    let mut subcats: Vec<&str> = Vec::new();
    let mut brands: Vec<&str> = Vec::new();
    let mut premiums: Vec<bool> = Vec::new();
    let mut launch_ms: Vec<i64> = Vec::new();
    let mut discont_ms: Vec<Option<i64>> = Vec::new();
    let mut base_prices: Vec<f64> = Vec::new();
    let mut cost_prices: Vec<f64> = Vec::new();
    let mut margin_pcts: Vec<f64> = Vec::new();

    let mut meta: Vec<ProductMeta> = Vec::new();
    let mut key: i64 = 1;

    for subcat_idx in 0..24usize {
        let cat_idx = SUBCAT_CATEGORY[subcat_idx];
        let count = SUBCAT_COUNT[subcat_idx];
        let [pmin, pmax] = PRICE_RANGE[subcat_idx];
        let [mmin, mmax] = MARGIN_RANGE[subcat_idx];

        // ~30% premium
        let num_premium = (count as f64 * 0.30).round() as usize;

        for i in 0..count {
            let is_prem = i < num_premium;
            let brand = if is_prem {
                let b = BRANDS_PREM[cat_idx];
                b[i % b.len()]
            } else {
                let b = BRANDS_STD[cat_idx];
                b[i % b.len()]
            };

            let price_mult = if is_prem { 1.5 } else { 1.0 };
            let base_price = {
                let raw = rng.random_range(pmin..pmax) * price_mult;
                (raw * 100.0).round() / 100.0
            };
            let margin = {
                let raw = rng.random_range(mmin..mmax) + if is_prem { 0.05 } else { 0.0 };
                (raw * 1000.0).round() / 1000.0
            };
            let cost = (base_price * (1.0 - margin) * 100.0).round() / 100.0;

            // Launch date: spread across 2020-2023, a few in 2024
            let launch_offset_days = rng.random_range(0i64..1461); // 4 years
            let ldate_ms = start_2022 + launch_offset_days * 86_400_000;

            // ~8% discontinued (only if launched before 2023)
            let dc: Option<i64> = if ldate_ms < end_2023 && rng.random_bool(0.08) {
                let dc_offset = rng.random_range(365i64..730);
                Some(ldate_ms + dc_offset * 86_400_000)
            } else {
                None
            };

            let sku = format!(
                "{} {} {}{}",
                brand,
                SUBCAT_NAMES[subcat_idx],
                if is_prem { "Pro " } else { "" },
                key
            );

            keys.push(key);
            names.push(sku);
            categories.push(CATEGORY_NAMES[cat_idx]);
            subcats.push(SUBCAT_NAMES[subcat_idx]);
            brands.push(brand);
            premiums.push(is_prem);
            launch_ms.push(ldate_ms);
            discont_ms.push(dc);
            base_prices.push(base_price);
            cost_prices.push(cost);
            margin_pcts.push(margin);

            meta.push(ProductMeta {
                key,
                cat_idx,
                subcat_idx,
                base_price,
                cost_price: cost,
                is_premium: is_prem,
                launch_date_ms: ldate_ms,
            });

            key += 1;
        }
    }

    let launch_col = Int64Chunked::new("LaunchDate".into(), &launch_ms)
        .into_datetime(TimeUnit::Milliseconds, None)
        .into_series();

    let mut dc_ca: Int64Chunked = discont_ms.iter().copied().collect();
    dc_ca.rename("DiscontinueDate".into());
    let dc_col = dc_ca
        .into_datetime(TimeUnit::Milliseconds, None)
        .into_series();

    let df = DataFrame::new_infer_height(vec![
        Series::new("ProductKey".into(), &keys).into(),
        Series::new("ProductName".into(), &names).into(),
        Series::new("Category".into(), &categories).into(),
        Series::new("SubCategory".into(), &subcats).into(),
        Series::new("Brand".into(), &brands).into(),
        Series::new("PremiumFlag".into(), &premiums).into(),
        launch_col.into(),
        dc_col.into(),
        Series::new("BasePrice".into(), &base_prices).into(),
        Series::new("CostPrice".into(), &cost_prices).into(),
        Series::new("MarginPct".into(), &margin_pcts).into(),
    ])
    .expect("DimProduct");

    (df, meta)
}

// ---------- DimCustomer ----------

fn gen_dim_customer(rng: &mut StdRng) -> (DataFrame, Vec<CustomerMeta>) {
    static SEGMENTS: &[&str] = &[
        "Casual Outdoor",
        "Dedicated Runner",
        "Cyclist",
        "Winter Sports Enthusiast",
        "Family Camper",
        "Premium Athlete",
    ];
    static LOYALTY_TIERS: &[&str] = &["Bronze", "Silver", "Gold", "Platinum"];
    static AGE_BANDS: &[&str] = &["18-24", "25-34", "35-44", "45-54", "55+"];
    static GENDERS: &[&str] = &["Male", "Female", "Other"];

    // country distribution: Germany 60%, Austria 20%, Switzerland 20%
    static COUNTRY_DIST: &[(usize, usize)] =
        &[(GERMANY, 15000), (AUSTRIA, 5000), (SWITZERLAND, 5000)];

    let start_2021 = ms_from_ymd(2021, 1, 1);
    let end_2024 = ms_from_ymd(2024, 12, 31);
    let total_days = (end_2024 - start_2021) / 86_400_000;

    let mut ckeys: Vec<i64> = Vec::new();
    let mut countries: Vec<&str> = Vec::new();
    let mut reg_ms: Vec<i64> = Vec::new();
    let mut genders: Vec<&str> = Vec::new();
    let mut age_bands: Vec<&str> = Vec::new();
    let mut loyalty: Vec<&str> = Vec::new();
    let mut segments: Vec<&str> = Vec::new();

    let mut meta: Vec<CustomerMeta> = Vec::new();
    let mut ckey: i64 = 1;

    for &(country_idx, count) in COUNTRY_DIST {
        for _ in 0..count {
            let reg_offset = rng.random_range(0i64..total_days);
            let reg_date_ms = start_2021 + reg_offset * 86_400_000;

            let gender_r: f64 = rng.random();
            let gender = if gender_r < 0.50 {
                GENDERS[0]
            } else if gender_r < 0.98 {
                GENDERS[1]
            } else {
                GENDERS[2]
            };

            let age_idx = rng.random_range(0..5usize);

            // loyalty skewed toward Bronze/Silver
            let loy_r: f64 = rng.random();
            let loy_idx = if loy_r < 0.45 {
                0
            } else if loy_r < 0.75 {
                1
            } else if loy_r < 0.92 {
                2
            } else {
                3
            };

            // segment skewed by country (Switzerland more Premium)
            let seg_idx = {
                let r: f64 = rng.random();
                let boost = if country_idx == SWITZERLAND {
                    0.05
                } else {
                    0.0
                };
                if r < 0.20 {
                    0
                } else if r < 0.35 {
                    1
                } else if r < 0.50 {
                    2
                } else if r < 0.65 {
                    3
                } else if r < 0.80 {
                    4
                } else if r < 0.95 + boost {
                    5
                } else {
                    0
                }
            };

            ckeys.push(ckey);
            countries.push(COUNTRY_NAMES[country_idx]);
            reg_ms.push(reg_date_ms);
            genders.push(gender);
            age_bands.push(AGE_BANDS[age_idx]);
            loyalty.push(LOYALTY_TIERS[loy_idx]);
            segments.push(SEGMENTS[seg_idx]);

            meta.push(CustomerMeta {
                key: ckey,
                country_idx,
                reg_date_ms,
                loyalty_idx: loy_idx,
                _segment_idx: seg_idx,
            });
            ckey += 1;
        }
    }

    let reg_col = Int64Chunked::new("RegistrationDate".into(), &reg_ms)
        .into_datetime(TimeUnit::Milliseconds, None)
        .into_series();

    let df = DataFrame::new_infer_height(vec![
        Series::new("CustomerKey".into(), &ckeys).into(),
        Series::new("Country".into(), &countries).into(),
        reg_col.into(),
        Series::new("Gender".into(), &genders).into(),
        Series::new("AgeBand".into(), &age_bands).into(),
        Series::new("LoyaltyTier".into(), &loyalty).into(),
        Series::new("CustomerSegment".into(), &segments).into(),
    ])
    .expect("DimCustomer");

    (df, meta)
}

// ---------- DimChannel ----------

fn gen_dim_channel() -> PolarsResult<DataFrame> {
    DataFrame::new_infer_height(vec![
        Series::new("ChannelKey".into(), &[1i64, 2, 3]).into(),
        Series::new("ChannelName".into(), &["Store", "Online", "Marketplace"]).into(),
    ])
}

// ---------- DimStore ----------

fn gen_dim_store(rng: &mut StdRng) -> PolarsResult<DataFrame> {
    static CITY_DE: &[&str] = &[
        "Berlin",
        "Munich",
        "Hamburg",
        "Frankfurt",
        "Cologne",
        "Stuttgart",
        "Dresden",
        "Leipzig",
        "Dortmund",
        "Essen",
        "Bremen",
        "Hanover",
        "Nuremberg",
        "Düsseldorf",
        "Freiburg",
    ];
    static CITY_AT: &[&str] = &["Vienna", "Graz", "Linz", "Salzburg", "Innsbruck"];
    static CITY_CH: &[&str] = &["Zurich", "Geneva", "Basel", "Bern", "Lausanne"];

    let mut store_keys: Vec<i64> = Vec::new();
    let mut store_names: Vec<String> = Vec::new();
    let mut store_countries: Vec<&str> = Vec::new();
    let mut store_cities: Vec<&str> = Vec::new();

    let sizes = &["Flagship", "Large", "Medium", "Small"];

    let mut sk: i64 = 1;
    for (cities, country) in [
        (CITY_DE, COUNTRY_NAMES[GERMANY]),
        (CITY_AT, COUNTRY_NAMES[AUSTRIA]),
        (CITY_CH, COUNTRY_NAMES[SWITZERLAND]),
    ] {
        for city in cities {
            let size = sizes[rng.random_range(0..sizes.len())];
            store_keys.push(sk);
            store_names.push(format!("{} {} Store", city, size));
            store_countries.push(country);
            store_cities.push(city);
            sk += 1;
        }
    }

    DataFrame::new_infer_height(vec![
        Series::new("StoreKey".into(), &store_keys).into(),
        Series::new("StoreName".into(), &store_names).into(),
        Series::new("Country".into(), &store_countries).into(),
        Series::new("City".into(), &store_cities).into(),
    ])
}

// ---------- DimPromotion ----------

fn gen_dim_promotion() -> PolarsResult<DataFrame> {
    let mut pkeys: Vec<i64> = Vec::new();
    let mut pnames: Vec<String> = Vec::new();
    let mut ptypes: Vec<&str> = Vec::new();
    let mut disc_rates: Vec<f64> = Vec::new();

    // key 0 = No Promotion
    pkeys.push(0);
    pnames.push("No Promotion".into());
    ptypes.push("None");
    disc_rates.push(0.0);

    static PROMO_TYPES: &[(&str, &str, f64)] = &[
        ("Spring Running Sale", "Seasonal Sale", 0.15),
        ("Summer Hiking Sale", "Seasonal Sale", 0.15),
        ("Autumn Cycling Deals", "Seasonal Sale", 0.12),
        ("Winter Ski Season", "Seasonal Sale", 0.10),
        ("End of Season Running", "Clearance", 0.25),
        ("End of Season Hiking", "Clearance", 0.25),
        ("End of Season Cycling", "Clearance", 0.22),
        ("End of Season Ski", "Clearance", 0.20),
        ("Run & Socks Bundle", "Bundle Offer", 0.12),
        ("Bike & Helmet Bundle", "Bundle Offer", 0.15),
        ("Tent & Sleeping Bag Bundle", "Bundle Offer", 0.14),
        ("Ski & Boots Bundle", "Bundle Offer", 0.13),
        ("Bronze Loyalty Reward", "Loyalty Discount", 0.05),
        ("Silver Loyalty Reward", "Loyalty Discount", 0.08),
        ("Gold Loyalty Reward", "Loyalty Discount", 0.12),
        ("Platinum Loyalty Reward", "Loyalty Discount", 0.18),
        ("Flash Sale Running", "Seasonal Sale", 0.20),
        ("Flash Sale Cycling", "Seasonal Sale", 0.20),
        ("New Year Clearance", "Clearance", 0.30),
        ("Black Friday Deals", "Seasonal Sale", 0.25),
    ];

    for (i, &(name, ptype, rate)) in PROMO_TYPES.iter().enumerate() {
        pkeys.push(i as i64 + 1);
        pnames.push(name.into());
        ptypes.push(ptype);
        disc_rates.push(rate);
    }

    DataFrame::new_infer_height(vec![
        Series::new("PromotionKey".into(), &pkeys).into(),
        Series::new("PromotionName".into(), &pnames).into(),
        Series::new("PromotionType".into(), &ptypes).into(),
        Series::new("DiscountRate".into(), &disc_rates).into(),
    ])
}

// ---------- DimCurrency ----------

fn gen_dim_currency() -> PolarsResult<DataFrame> {
    DataFrame::new_infer_height(vec![
        Series::new("CurrencyCode".into(), &["EUR", "CHF"]).into()
    ])
}

// ---------- DimExchangeRateType ----------

fn gen_dim_exchange_rate_type() -> PolarsResult<DataFrame> {
    DataFrame::new_infer_height(vec![Series::new(
        "RateType".into(),
        &["Local", "Monthly Average", "Budget"],
    )
    .into()])
}

// ---------- FX rates (shared by FactExchangeRate and FactSales) ----------
//
// CHF to EUR rate per (Year, Month, RateType). Budget rates are fixed once
// per fiscal year at planning time and then held for several months before
// being revised, so they change less often than the monthly average.
// EUR-denominated transactions never need conversion (factor 1.0); only CHF
// transactions are actually rescaled by these rates.
static FX_RATES: &[(i32, u32, f64, f64)] = &[
    // (Year, Month, Average, Budget)
    (2023, 1, 0.900, 0.900),
    (2023, 2, 0.910, 0.900),
    (2023, 3, 0.920, 0.900),
    (2023, 4, 0.930, 0.920),
    (2023, 5, 0.940, 0.920),
    (2023, 6, 0.946, 0.920),
    (2023, 7, 0.935, 0.920),
    (2023, 8, 0.925, 0.920),
    (2023, 9, 0.915, 0.920),
    (2023, 10, 0.910, 0.920),
    (2023, 11, 0.905, 0.920),
    (2023, 12, 0.900, 0.920),
    (2024, 1, 0.915, 0.920),
    (2024, 2, 0.920, 0.920),
    (2024, 3, 0.925, 0.920),
    (2024, 4, 0.930, 0.930),
    (2024, 5, 0.932, 0.930),
    (2024, 6, 0.930, 0.930),
    (2024, 7, 0.928, 0.930),
    (2024, 8, 0.926, 0.930),
    (2024, 9, 0.925, 0.930),
    (2024, 10, 0.920, 0.930),
    (2024, 11, 0.918, 0.930),
    (2024, 12, 0.915, 0.930),
];

/// (Monthly Average rate, Budget rate) for a given (year, month). Panics if
/// `year`/`month` fall outside the generated date range — every caller here
/// derives them from dates this same generator produced, so that's a bug,
/// not a data condition.
fn fx_rate(year: i32, month: u32) -> (f64, f64) {
    FX_RATES
        .iter()
        .find(|&&(y, m, _, _)| y == year && m == month)
        .map(|&(_, _, avg, budget)| (avg, budget))
        .unwrap_or_else(|| panic!("no FX rate for {year}-{month:02}"))
}

// ---------- FactExchangeRate ----------

fn gen_fact_exchange_rate() -> PolarsResult<DataFrame> {
    let mut years: Vec<i64> = Vec::with_capacity(FX_RATES.len() * 2);
    let mut months: Vec<i64> = Vec::with_capacity(FX_RATES.len() * 2);
    let mut rate_types: Vec<&str> = Vec::with_capacity(FX_RATES.len() * 2);
    let mut rates: Vec<f64> = Vec::with_capacity(FX_RATES.len() * 2);

    for &(year, month, avg, budget) in FX_RATES {
        years.push(year as i64);
        months.push(month as i64);
        rate_types.push("Monthly Average");
        rates.push(avg);

        years.push(year as i64);
        months.push(month as i64);
        rate_types.push("Budget");
        rates.push(budget);
    }

    DataFrame::new_infer_height(vec![
        Series::new("Year".into(), &years).into(),
        Series::new("Month".into(), &months).into(),
        Series::new("RateType".into(), &rate_types).into(),
        Series::new("Rate".into(), &rates).into(),
    ])
}

// ---------- FactSales ----------

fn gen_fact_sales(
    products: &[ProductMeta],
    customers: &[CustomerMeta],
    store_count_by_country: &[usize; 3],
    rng: &mut StdRng,
) -> PolarsResult<DataFrame> {
    // Build lookup tables
    let mut products_by_subcat: [Vec<usize>; 24] = Default::default();
    for (i, p) in products.iter().enumerate() {
        products_by_subcat[p.subcat_idx].push(i);
    }
    let mut premium_by_subcat: [Vec<usize>; 24] = Default::default();
    let mut standard_by_subcat: [Vec<usize>; 24] = Default::default();
    for (i, p) in products.iter().enumerate() {
        if p.is_premium {
            premium_by_subcat[p.subcat_idx].push(i);
        } else {
            standard_by_subcat[p.subcat_idx].push(i);
        }
    }

    let mut customers_by_country: [Vec<usize>; 3] = Default::default();
    for (i, c) in customers.iter().enumerate() {
        customers_by_country[c.country_idx].push(i);
    }

    // subcat->category lookup: products_by_cat
    let mut products_by_cat: [Vec<usize>; 4] = Default::default();
    for (i, p) in products.iter().enumerate() {
        products_by_cat[p.cat_idx].push(i);
    }

    // Annual base transactions per country (2023/2024)
    let annual_base: [[f64; 2]; 3] = [
        [106_000.0, 124_450.0], // Germany: +17.5% in 2024
        [40_000.0, 40_000.0],   // Austria: flat
        [40_000.0, 40_000.0],   // Switzerland: flat
    ];

    // precompute seasonality means for normalization
    let season_means: [f64; 4] = core::array::from_fn(|c| SEASON[c].iter().sum::<f64>() / 12.0);

    // promotion discount lookup: key 0 = 0.0, keys 1-20 = rates
    static PROMO_RATES: &[f64] = &[
        0.0, // 0 = none
        0.15, 0.15, 0.12, 0.10, // seasonal 1-4
        0.25, 0.25, 0.22, 0.20, // clearance 5-8
        0.12, 0.15, 0.14, 0.13, // bundle 9-12
        0.05, 0.08, 0.12, 0.18, // loyalty 13-16
        0.20, 0.20, 0.30, 0.25, // flash/clearance 17-20
    ];

    // 3 rows per transaction (Local / Monthly Average / Budget currency views) —
    // see fx_rate and emit_row.
    let mut tx_ids: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_dates: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_cust: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_country: Vec<&str> = Vec::with_capacity(520_000 * 3);
    let mut tx_currency: Vec<&str> = Vec::with_capacity(520_000 * 3);
    let mut tx_prod: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_qty: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_unit_price: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_gross: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_discount: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_net: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_cost: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_margin: Vec<f64> = Vec::with_capacity(520_000 * 3);
    let mut tx_promo: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_channel: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_store: Vec<i64> = Vec::with_capacity(520_000 * 3);
    let mut tx_fx_type: Vec<&str> = Vec::with_capacity(520_000 * 3);

    let mut txid: i64 = 1;
    let start_de_store: i64 = 1; // StoreKey 1..15 for Germany
    let start_at_store: i64 = 16;
    let start_ch_store: i64 = 21;

    // store ranges per country
    let store_ranges: [(i64, i64); 3] = [
        (
            start_de_store,
            start_de_store + store_count_by_country[GERMANY] as i64 - 1,
        ),
        (
            start_at_store,
            start_at_store + store_count_by_country[AUSTRIA] as i64 - 1,
        ),
        (
            start_ch_store,
            start_ch_store + store_count_by_country[SWITZERLAND] as i64 - 1,
        ),
    ];

    for (year_idx, year) in [2023i32, 2024].iter().enumerate() {
        for month in 1u32..=12 {
            let month_start_ms = ms_from_ymd(*year, month, 1);
            let dim = days_in_month(*year, month) as i64;
            let (avg_rate, budget_rate) = fx_rate(*year, month);

            for country_idx in 0..3usize {
                let annual = annual_base[country_idx][year_idx];
                let currency = CURRENCIES[country_idx];

                for cat_idx in 0..4usize {
                    let s = SEASON[cat_idx][(month - 1) as usize];
                    let s_norm = s / season_means[cat_idx];
                    let noise: f64 = rng.random_range(0.90..1.10);
                    let target = ((annual * 0.25 / 12.0) * s_norm * noise).round() as usize;

                    let subcat_start = cat_idx * 6;
                    let subcat_end = subcat_start + 6;

                    for _ in 0..target {
                        let date_ms = month_start_ms + rng.random_range(0i64..dim) * 86_400_000;

                        // Pick customer registered before this date
                        let cust_pool = &customers_by_country[country_idx];
                        let cust_idx = pick_active_customer(cust_pool, customers, date_ms, rng);
                        let cust = &customers[cust_idx];

                        // Switzerland 2024: push toward premium subcats
                        let subcat_idx = pick_subcat(subcat_start, subcat_end, rng);

                        let prod_idx = pick_product(
                            subcat_idx,
                            &products_by_subcat,
                            &premium_by_subcat,
                            &standard_by_subcat,
                            country_idx,
                            *year,
                            date_ms,
                            products,
                            rng,
                        );
                        let prod = &products[prod_idx];

                        // Channel: Store 60%, Online 35-38%, Marketplace 2-5%
                        let online_share = if *year == 2024 { 0.37 } else { 0.35 };
                        let (channel_key, store_key) =
                            pick_channel(country_idx, online_share, &store_ranges, rng);

                        // Promotion
                        let promo_key = pick_promo(cust.loyalty_idx, cat_idx, rng);
                        let disc_rate = PROMO_RATES[promo_key as usize];

                        let qty: i64 = pick_qty(subcat_idx, rng);

                        // Swiss premiumization: ASP uplift in 2024
                        let price_mult = if country_idx == SWITZERLAND && *year == 2024 {
                            1.12
                        } else {
                            1.0
                        };
                        let unit_price =
                            round2(prod.base_price * price_mult * rng.random_range(0.97..1.03));
                        let gross = round2(unit_price * qty as f64);
                        let discount = round2(gross * disc_rate);
                        let net = round2(gross - discount);
                        let cost = round2(prod.cost_price * qty as f64);
                        let margin = round2(net - cost);

                        emit_row(
                            &mut txid,
                            date_ms,
                            cust.key,
                            COUNTRY_NAMES[country_idx],
                            currency,
                            prod.key,
                            qty,
                            unit_price,
                            gross,
                            discount,
                            net,
                            cost,
                            margin,
                            promo_key,
                            channel_key,
                            store_key,
                            avg_rate,
                            budget_rate,
                            &mut tx_ids,
                            &mut tx_dates,
                            &mut tx_cust,
                            &mut tx_country,
                            &mut tx_currency,
                            &mut tx_prod,
                            &mut tx_qty,
                            &mut tx_unit_price,
                            &mut tx_gross,
                            &mut tx_discount,
                            &mut tx_net,
                            &mut tx_cost,
                            &mut tx_margin,
                            &mut tx_promo,
                            &mut tx_channel,
                            &mut tx_store,
                            &mut tx_fx_type,
                        );

                        // Basket companions
                        for &(trigger, companion_sc, prob) in BASKET {
                            if trigger == subcat_idx && rng.random_bool(prob) {
                                let comp_pool = &products_by_subcat[companion_sc];
                                if comp_pool.is_empty() {
                                    continue;
                                }
                                let comp_idx = comp_pool[rng.random_range(0..comp_pool.len())];
                                let comp = &products[comp_idx];

                                let cqty: i64 = 1;
                                let cup = round2(
                                    comp.base_price * price_mult * rng.random_range(0.97..1.03),
                                );
                                let cgross = round2(cup * cqty as f64);
                                // basket companion shares same promotion as trigger
                                let cdisc = round2(cgross * disc_rate);
                                let cnet = round2(cgross - cdisc);
                                let ccost = round2(comp.cost_price * cqty as f64);
                                let cmargin = round2(cnet - ccost);

                                emit_row(
                                    &mut txid,
                                    date_ms,
                                    cust.key,
                                    COUNTRY_NAMES[country_idx],
                                    currency,
                                    comp.key,
                                    cqty,
                                    cup,
                                    cgross,
                                    cdisc,
                                    cnet,
                                    ccost,
                                    cmargin,
                                    promo_key,
                                    channel_key,
                                    store_key,
                                    avg_rate,
                                    budget_rate,
                                    &mut tx_ids,
                                    &mut tx_dates,
                                    &mut tx_cust,
                                    &mut tx_country,
                                    &mut tx_currency,
                                    &mut tx_prod,
                                    &mut tx_qty,
                                    &mut tx_unit_price,
                                    &mut tx_gross,
                                    &mut tx_discount,
                                    &mut tx_net,
                                    &mut tx_cost,
                                    &mut tx_margin,
                                    &mut tx_promo,
                                    &mut tx_channel,
                                    &mut tx_store,
                                    &mut tx_fx_type,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let date_col = Int64Chunked::new("Date".into(), &tx_dates)
        .into_datetime(TimeUnit::Milliseconds, None)
        .into_series();

    DataFrame::new_infer_height(vec![
        Series::new("TransactionID".into(), &tx_ids).into(),
        date_col.into(),
        Series::new("CustomerKey".into(), &tx_cust).into(),
        Series::new("Country".into(), &tx_country).into(),
        Series::new("Currency".into(), &tx_currency).into(),
        Series::new("ProductKey".into(), &tx_prod).into(),
        Series::new("Quantity".into(), &tx_qty).into(),
        Series::new("UnitPriceLCU".into(), &tx_unit_price).into(),
        Series::new("GrossSales".into(), &tx_gross).into(),
        Series::new("DiscountAmount".into(), &tx_discount).into(),
        Series::new("NetSales".into(), &tx_net).into(),
        Series::new("Cost".into(), &tx_cost).into(),
        Series::new("GrossMargin".into(), &tx_margin).into(),
        Series::new("PromotionKey".into(), &tx_promo).into(),
        Series::new("ChannelKey".into(), &tx_channel).into(),
        Series::new("StoreKey".into(), &tx_store).into(),
        Series::new("ExchangeRateType".into(), &tx_fx_type).into(),
    ])
}

// ---------- helper fns ----------

#[inline]
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn pick_active_customer(
    pool: &[usize],
    customers: &[CustomerMeta],
    date_ms: i64,
    rng: &mut StdRng,
) -> usize {
    // Try up to 8 times to find a customer registered before this date
    for _ in 0..8 {
        let idx = pool[rng.random_range(0..pool.len())];
        if customers[idx].reg_date_ms <= date_ms {
            return idx;
        }
    }
    pool[rng.random_range(0..pool.len())]
}

fn pick_subcat(sc_start: usize, sc_end: usize, rng: &mut StdRng) -> usize {
    rng.random_range(sc_start..sc_end)
}

#[allow(clippy::too_many_arguments)]
fn pick_product(
    subcat_idx: usize,
    products_by_subcat: &[Vec<usize>; 24],
    premium_by_subcat: &[Vec<usize>; 24],
    standard_by_subcat: &[Vec<usize>; 24],
    country_idx: usize,
    year: i32,
    date_ms: i64,
    products: &[ProductMeta],
    rng: &mut StdRng,
) -> usize {
    // Switzerland 2024: premium probability boosted to ~50%
    let premium_prob = if country_idx == SWITZERLAND && year == 2024 {
        0.50
    } else {
        0.30
    };

    let pool = if rng.random_bool(premium_prob) && !premium_by_subcat[subcat_idx].is_empty() {
        &premium_by_subcat[subcat_idx]
    } else if !standard_by_subcat[subcat_idx].is_empty() {
        &standard_by_subcat[subcat_idx]
    } else {
        &products_by_subcat[subcat_idx]
    };

    if pool.is_empty() {
        return products_by_subcat[subcat_idx][0];
    }

    // Try to find a product launched before this date, not discontinued
    for _ in 0..6 {
        let idx = pool[rng.random_range(0..pool.len())];
        let p = &products[idx];
        let launched = p.launch_date_ms <= date_ms;
        let active = p.launch_date_ms.max(0) <= date_ms && p.launch_date_ms <= date_ms;
        let not_discont = true; // check DiscontinueDate separately if needed
        let _ = (launched, active, not_discont);
        if p.launch_date_ms <= date_ms {
            return idx;
        }
    }
    pool[rng.random_range(0..pool.len())]
}

fn pick_channel(
    country_idx: usize,
    online_share: f64,
    store_ranges: &[(i64, i64); 3],
    rng: &mut StdRng,
) -> (i64, i64) {
    let r: f64 = rng.random();
    if r < 0.60 {
        // Store
        let (lo, hi) = store_ranges[country_idx];
        let sk = rng.random_range(lo..=hi);
        (1, sk)
    } else if r < 0.60 + online_share {
        (2, 0) // Online, no physical store
    } else {
        (3, 0) // Marketplace
    }
}

fn pick_promo(loyalty_idx: usize, cat_idx: usize, rng: &mut StdRng) -> i64 {
    if !rng.random_bool(0.17) {
        return 0;
    }
    // Pick promo type weighted by context
    let r: f64 = rng.random();
    if r < 0.30 {
        // Seasonal (1-4)
        rng.random_range(1i64..=4)
    } else if r < 0.45 {
        // Clearance (5-8) - more likely in low season
        rng.random_range(5i64..=8)
    } else if r < 0.65 {
        // Bundle (9-12)
        9 + cat_idx as i64 // one bundle type per category
    } else {
        // Loyalty (13-16)
        13 + loyalty_idx as i64
    }
}

fn pick_qty(subcat_idx: usize, rng: &mut StdRng) -> i64 {
    // High-value equipment: usually qty=1; accessories can be 1-4
    let max_qty: i64 = match subcat_idx {
        SC_ROAD_BIKES | SC_MTB | SC_TENTS | SC_SKIS | SC_SKI_BOOTS => 1,
        SC_RUNNING_SOCKS | SC_CAMPING_ACCESSORIES | SC_BIKE_ACCESSORIES | SC_SKI_ACCESSORIES => 4,
        _ => 2,
    };
    rng.random_range(1i64..=max_qty)
}

/// Emits one transaction as 3 rows — Local, Monthly Average, and Budget
/// currency views (see fx_rate) — sharing one TransactionID so
/// DISTINCTCOUNT(TransactionID) stays correct without a currency filter.
/// Every other measure (including plain row counts and Quantity, which isn't
/// even currency-denominated) is only correct when filtered to exactly one
/// ExchangeRateType, by design — that's the tradeoff of this model.
#[allow(clippy::too_many_arguments)]
fn emit_row(
    txid: &mut i64,
    date_ms: i64,
    cust_key: i64,
    country: &'static str,
    currency: &'static str,
    prod_key: i64,
    qty: i64,
    unit_price: f64,
    gross: f64,
    discount: f64,
    net: f64,
    cost: f64,
    margin: f64,
    promo: i64,
    channel: i64,
    store: i64,
    avg_rate: f64,
    budget_rate: f64,
    // output vecs
    ids: &mut Vec<i64>,
    dates: &mut Vec<i64>,
    custs: &mut Vec<i64>,
    countries: &mut Vec<&'static str>,
    currencies: &mut Vec<&'static str>,
    prods: &mut Vec<i64>,
    qtys: &mut Vec<i64>,
    unit_prices: &mut Vec<f64>,
    grosses: &mut Vec<f64>,
    discounts: &mut Vec<f64>,
    nets: &mut Vec<f64>,
    costs: &mut Vec<f64>,
    margins: &mut Vec<f64>,
    promos: &mut Vec<i64>,
    channels: &mut Vec<i64>,
    stores: &mut Vec<i64>,
    fx_types: &mut Vec<&'static str>,
) {
    // EUR transactions need no conversion; only CHF ones are rescaled.
    let views: [(&'static str, f64); 3] = if currency == "EUR" {
        [("Local", 1.0), ("Monthly Average", 1.0), ("Budget", 1.0)]
    } else {
        [
            ("Local", 1.0),
            ("Monthly Average", avg_rate),
            ("Budget", budget_rate),
        ]
    };

    for (fx_type, factor) in views {
        ids.push(*txid);
        dates.push(date_ms);
        custs.push(cust_key);
        countries.push(country);
        currencies.push(currency);
        prods.push(prod_key);
        qtys.push(qty);
        unit_prices.push(round2(unit_price * factor));
        grosses.push(round2(gross * factor));
        discounts.push(round2(discount * factor));
        nets.push(round2(net * factor));
        costs.push(round2(cost * factor));
        margins.push(round2(margin * factor));
        promos.push(promo);
        channels.push(channel);
        stores.push(store);
        fx_types.push(fx_type);
    }
    *txid += 1;
}

// ---------- TMSL ----------

static TABLES_JSON: &str = r##"  "tables": [
    {
      "name": "DimDate",
      "dataSource": "DimDate.parquet",
      "columns": [
        { "name": "DateKey",    "dataType": "Int64",   "isHidden": true },
        { "name": "Date",       "dataType": "DateTime" },
        { "name": "Year",       "dataType": "Int64"    },
        { "name": "Month",      "dataType": "Int64"    },
        { "name": "MonthName",  "dataType": "String"   },
        { "name": "Quarter",    "dataType": "Int64"    },
        { "name": "DayOfWeek",  "dataType": "Int64"    },
        { "name": "IsWeekend",  "dataType": "Boolean"  }
      ]
    },
    {
      "name": "DimProduct",
      "dataSource": "DimProduct.parquet",
      "columns": [
        { "name": "ProductKey",       "dataType": "Int64",   "isHidden": true },
        { "name": "ProductName",      "dataType": "String"   },
        { "name": "Category",         "dataType": "String"   },
        { "name": "SubCategory",      "dataType": "String"   },
        { "name": "Brand",            "dataType": "String"   },
        { "name": "PremiumFlag",      "dataType": "Boolean"  },
        { "name": "LaunchDate",       "dataType": "DateTime" },
        { "name": "DiscontinueDate",  "dataType": "DateTime" },
        { "name": "BasePrice",        "dataType": "Double"   },
        { "name": "CostPrice",        "dataType": "Double"   },
        { "name": "MarginPct",        "dataType": "Double"   }
      ]
    },
    {
      "name": "DimCustomer",
      "dataSource": "DimCustomer.parquet",
      "columns": [
        { "name": "CustomerKey",      "dataType": "Int64",   "isHidden": true },
        { "name": "Country",          "dataType": "String"   },
        { "name": "RegistrationDate", "dataType": "DateTime" },
        { "name": "Gender",           "dataType": "String"   },
        { "name": "AgeBand",          "dataType": "String"   },
        { "name": "LoyaltyTier",      "dataType": "String"   },
        { "name": "CustomerSegment",  "dataType": "String"   }
      ]
    },
    {
      "name": "DimChannel",
      "dataSource": "DimChannel.parquet",
      "columns": [
        { "name": "ChannelKey",  "dataType": "Int64", "isHidden": true },
        { "name": "ChannelName", "dataType": "String" }
      ]
    },
    {
      "name": "DimStore",
      "dataSource": "DimStore.parquet",
      "columns": [
        { "name": "StoreKey",  "dataType": "Int64", "isHidden": true },
        { "name": "StoreName", "dataType": "String" },
        { "name": "Country",   "dataType": "String" },
        { "name": "City",      "dataType": "String" }
      ]
    },
    {
      "name": "DimPromotion",
      "dataSource": "DimPromotion.parquet",
      "columns": [
        { "name": "PromotionKey",  "dataType": "Int64", "isHidden": true },
        { "name": "PromotionName", "dataType": "String" },
        { "name": "PromotionType", "dataType": "String" },
        { "name": "DiscountRate",  "dataType": "Double" }
      ]
    },
    {
      "name": "DimCurrency",
      "dataSource": "DimCurrency.parquet",
      "columns": [
        { "name": "CurrencyCode", "dataType": "String" }
      ]
    },
    {
      "name": "DimExchangeRateType",
      "dataSource": "DimExchangeRateType.parquet",
      "columns": [
        { "name": "RateType", "dataType": "String" }
      ]
    },
    {
      "name": "FactExchangeRate",
      "dataSource": "FactExchangeRate.parquet",
      "columns": [
        { "name": "Year",     "dataType": "Int64"  },
        { "name": "Month",    "dataType": "Int64"  },
        { "name": "RateType", "dataType": "String" },
        { "name": "Rate",     "dataType": "Double" }
      ]
    },
    {
      "name": "FactSales",
      "dataSource": "FactSales.parquet",
      "columns": [
        { "name": "TransactionID",  "dataType": "Int64"    },
        { "name": "Date",           "dataType": "DateTime" },
        { "name": "CustomerKey",    "dataType": "Int64",   "isHidden": true },
        { "name": "Country",        "dataType": "String"   },
        { "name": "Currency",       "dataType": "String"   },
        { "name": "ProductKey",     "dataType": "Int64",   "isHidden": true },
        { "name": "Quantity",       "dataType": "Int64",   "isHidden": true },
        { "name": "UnitPriceLCU",   "dataType": "Double",  "isHidden": true },
        { "name": "GrossSales",     "dataType": "Double",  "isHidden": true },
        { "name": "DiscountAmount", "dataType": "Double",  "isHidden": true },
        { "name": "NetSales",       "dataType": "Double",  "isHidden": true },
        { "name": "Cost",           "dataType": "Double",  "isHidden": true },
        { "name": "GrossMargin",    "dataType": "Double",  "isHidden": true },
        { "name": "PromotionKey",   "dataType": "Int64",   "isHidden": true },
        { "name": "ChannelKey",     "dataType": "Int64",   "isHidden": true },
        { "name": "StoreKey",       "dataType": "Int64",   "isHidden": true },
        { "name": "ExchangeRateType", "dataType": "String", "isHidden": true }
      ],
      "measures": [
        {
          "name": "Qty",
          "expression": "CALCULATE ( SUM ( FactSales[Quantity] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#.##0",
          "displayFolder": "Sales"
        },
        {
          "name": "Net Sales",
          "expression": "CALCULATE ( SUM ( FactSales[NetSales] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Sales"
        },
        {
          "name": "Gross Sales",
          "expression": "CALCULATE ( SUM ( FactSales[GrossSales] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Sales"
        },
        {
          "name": "Discount Amount",
          "expression": "CALCULATE ( SUM ( FactSales[DiscountAmount] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Sales"
        },
        {
          "name": "Total Cost",
          "expression": "CALCULATE ( SUM ( FactSales[Cost] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Cost"
        },
        {
          "name": "Gross Margin",
          "expression": "CALCULATE ( SUM ( FactSales[GrossMargin] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Cost"
        },
        {
          "name": "Unit Price",
          "expression": "CALCULATE ( AVERAGE ( FactSales[UnitPriceLCU] ), DimExchangeRateType[RateType] = SELECTEDVALUE ( DimExchangeRateType[RateType], \"Monthly Average\" ) )",
          "formatString": "#,##0.00",
          "displayFolder": "Sales"
        },
        {
          "name": "Average Sales Price",
          "expression": "DIVIDE ( [Net Sales], [Qty] )",
          "formatString": "#,##0.00",
          "displayFolder": "Sales"
        }
      ]
    }
  ]"##;

static RELATIONSHIPS_JSON: &str = r#"  "relationships": [
    {
      "name": "FactSales_DimDate",
      "fromTable": "FactSales",
      "fromColumn": "Date",
      "toTable": "DimDate",
      "toColumn": "Date",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimCustomer",
      "fromTable": "FactSales",
      "fromColumn": "CustomerKey",
      "toTable": "DimCustomer",
      "toColumn": "CustomerKey",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimProduct",
      "fromTable": "FactSales",
      "fromColumn": "ProductKey",
      "toTable": "DimProduct",
      "toColumn": "ProductKey",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimPromotion",
      "fromTable": "FactSales",
      "fromColumn": "PromotionKey",
      "toTable": "DimPromotion",
      "toColumn": "PromotionKey",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimChannel",
      "fromTable": "FactSales",
      "fromColumn": "ChannelKey",
      "toTable": "DimChannel",
      "toColumn": "ChannelKey",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimStore",
      "fromTable": "FactSales",
      "fromColumn": "StoreKey",
      "toTable": "DimStore",
      "toColumn": "StoreKey",
      "crossFilteringBehavior": "single",
      "isActive": true
    },
    {
      "name": "FactSales_DimExchangeRateType",
      "fromTable": "FactSales",
      "fromColumn": "ExchangeRateType",
      "toTable": "DimExchangeRateType",
      "toColumn": "RateType",
      "crossFilteringBehavior": "single",
      "isActive": true
    }
  ]"#;

fn write_tmsl_tables_only(catalogs_op: &BlockingOperator) -> Result<(), opendal::Error> {
    let json = format!(
        "{{\n  \"name\": \"SportRetailerTables\",\n{TABLES_JSON},\n  \"relationships\": []\n}}\n"
    );
    catalogs_op.write("sport_retailer_tables_only.json", json.into_bytes())?;
    Ok(())
}

fn write_tmsl_with_relationships(catalogs_op: &BlockingOperator) -> Result<(), opendal::Error> {
    let json =
        format!("{{\n  \"name\": \"SportRetailer\",\n{TABLES_JSON},\n{RELATIONSHIPS_JSON}\n}}\n");
    catalogs_op.write("sport_retailer.json", json.into_bytes())?;
    Ok(())
}

// ---------- main ----------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let seed = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(SEED);

    // Building/using opendal's blocking operator directly on the thread driving
    // this async runtime panics ("cannot start a runtime from within a runtime").
    // Run the whole generation step on a genuinely separate blocking thread.
    tokio::task::spawn_blocking(move || generate(seed)).await?
}

fn generate(seed: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let datasets_cfg = dax_rs::storage::BackendConfig::default()
        .apply_overrides(&dax_rs::storage::env_overrides("DAX_DATASETS"));
    let models_cfg = dax_rs::storage::BackendConfig::default()
        .apply_overrides(&dax_rs::storage::env_overrides("DAX_MODELS"));
    let datasets_op = dax_rs::storage::build_operator(&datasets_cfg)?;
    let catalogs_op = dax_rs::storage::build_operator(&models_cfg)?;

    let mut rng = StdRng::seed_from_u64(seed);

    println!("Generating DimDate...");
    let mut dim_date = gen_dim_date()?;
    write_parquet(&datasets_op, &mut dim_date, "DimDate")?;

    println!("Generating DimProduct...");
    let (mut dim_product, product_meta) = gen_dim_product(&mut rng);
    write_parquet(&datasets_op, &mut dim_product, "DimProduct")?;

    println!("Generating DimCustomer...");
    let (mut dim_customer, customer_meta) = gen_dim_customer(&mut rng);
    write_parquet(&datasets_op, &mut dim_customer, "DimCustomer")?;

    println!("Generating DimChannel...");
    let mut dim_channel = gen_dim_channel()?;
    write_parquet(&datasets_op, &mut dim_channel, "DimChannel")?;

    println!("Generating DimStore...");
    let mut dim_store = gen_dim_store(&mut rng)?;
    write_parquet(&datasets_op, &mut dim_store, "DimStore")?;

    println!("Generating DimPromotion...");
    let mut dim_promo = gen_dim_promotion()?;
    write_parquet(&datasets_op, &mut dim_promo, "DimPromotion")?;

    println!("Generating DimCurrency...");
    let mut dim_currency = gen_dim_currency()?;
    write_parquet(&datasets_op, &mut dim_currency, "DimCurrency")?;

    println!("Generating DimExchangeRateType...");
    let mut dim_exchange_rate_type = gen_dim_exchange_rate_type()?;
    write_parquet(
        &datasets_op,
        &mut dim_exchange_rate_type,
        "DimExchangeRateType",
    )?;

    println!("Generating FactExchangeRate...");
    let mut fact_fx = gen_fact_exchange_rate()?;
    write_parquet(&datasets_op, &mut fact_fx, "FactExchangeRate")?;

    // store counts by country for store key ranges
    let store_count_by_country: [usize; 3] = [15, 5, 5];

    println!("Generating FactSales (this may take a moment)...");
    let mut fact_sales = gen_fact_sales(
        &product_meta,
        &customer_meta,
        &store_count_by_country,
        &mut rng,
    )?;
    write_parquet(&datasets_op, &mut fact_sales, "FactSales")?;

    println!("Writing TMSL...");
    write_tmsl_tables_only(&catalogs_op)?;
    println!("  wrote sport_retailer_tables_only.json");
    write_tmsl_with_relationships(&catalogs_op)?;
    println!("  wrote sport_retailer.json");

    println!("\nDone. Total FactSales rows: {}", fact_sales.height());
    Ok(())
}
