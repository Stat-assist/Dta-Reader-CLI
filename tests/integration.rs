//! End-to-end tests over the bundled `auto.dta` fixture: parse the file, run commands through the
//! same code path the CLI uses, and check the JSON envelopes carry the right values.

use searchlight_cli::commands::{self, OutputOpts};
use searchlight_cli::json::Json;
use searchlight_cli::model::Dataset;
use searchlight_cli::parser;
use searchlight_cli::reader;

fn load() -> Dataset {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/auto.dta"))
        .expect("fixture auto.dta present");
    reader::read_dta(&bytes, "auto.dta").expect("auto.dta parses")
}

/// Run a single command and return its result envelope, pretty-printed for easy substring checks.
fn run(ds: &mut Dataset, line: &str) -> (String, u8) {
    let cmd = parser::parse_command(line).expect("command parses");
    let opts = OutputOpts {
        nolabel: false,
        rawdates: false,
    };
    let outcome = commands::execute(&cmd, ds, &opts).expect("command executes");
    (outcome.value.to_string(false), outcome.exit_code)
}

#[test]
fn reads_expected_shape() {
    let ds = load();
    assert_eq!(ds.nobs, 74);
    assert_eq!(ds.nvars(), 12);
    assert_eq!(ds.source_release, 118);
    assert_eq!(ds.dataset_label, "1978 automobile data");
    assert_eq!(ds.variables[0].name, "make");
    assert_eq!(ds.variables[11].name, "foreign");
    assert_eq!(ds.variables[11].value_label_name, "origin");
}

#[test]
fn count_and_if() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "count if foreign==1");
    assert!(out.contains("\"count\":22"), "got: {}", out);
    let (out, _) = run(&mut ds, "count if mpg>20");
    assert!(out.contains("\"count\":36"), "got: {}", out);
}

#[test]
fn list_applies_labels_and_missing() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "list foreign rep78 in 1/3");
    assert!(out.contains("\"foreign\":\"Domestic\""), "labels: {}", out);
    // Row 3 (AMC Spirit) has a missing rep78, which must serialize as JSON null.
    assert!(out.contains("\"rep78\":null"), "missing null: {}", out);
}

#[test]
fn summarize_matches_stata() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "summarize price");
    assert!(out.contains("\"obs\":74"), "{}", out);
    assert!(out.contains("\"min\":3291"), "{}", out);
    assert!(out.contains("\"max\":15906"), "{}", out);
    // Mean 6165.2567...; check the leading digits are present at full precision.
    assert!(out.contains("\"mean\":6165.25"), "{}", out);
}

#[test]
fn assert_sets_exit_code() {
    let mut ds = load();
    let (_, code) = run(&mut ds, "assert mpg > 0");
    assert_eq!(code, 0);
    let (out, code) = run(&mut ds, "assert rep78 < 6");
    assert_eq!(code, 1, "missing rep78 rows are contradictions");
    assert!(out.contains("\"contradictions\":5"), "{}", out);
}

#[test]
fn tabulate_percentages() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "tabulate foreign");
    assert!(out.contains("\"freq\":52"), "{}", out);
    assert!(out.contains("\"freq\":22"), "{}", out);
    assert!(out.contains("\"total\":74"), "{}", out);
}

#[test]
fn tabulate_twoway_cross_tab() {
    let mut ds = load();
    // Verified against Stata's own `tabulate rep78 foreign`: rows 1-5 with counts
    // (2,0) (8,0) (27,3) (9,9) (2,9), column totals 48/21, grand total 69 (5 missing rep78 dropped).
    let (out, _) = run(&mut ds, "tabulate rep78 foreign");
    assert!(out.contains("\"row_variable\":\"rep78\""), "{}", out);
    assert!(out.contains("\"column_variable\":\"foreign\""), "{}", out);
    assert!(out.contains("\"counts\":[27,3]"), "{}", out);
    assert!(out.contains("\"counts\":[9,9]"), "{}", out);
    assert!(out.contains("\"column_totals\":[48,21]"), "{}", out);
    assert!(out.contains("\"total\":69"), "{}", out);

    // With `missing`, the dropped rep78==. row reappears: (4,1), totals become 52/22/74.
    let (out, _) = run(&mut ds, "tabulate rep78 foreign, missing");
    assert!(out.contains("\"value\":\".\""), "missing row: {}", out);
    assert!(out.contains("\"counts\":[4,1]"), "{}", out);
    assert!(out.contains("\"column_totals\":[52,22]"), "{}", out);
    assert!(out.contains("\"total\":74"), "{}", out);
}

#[test]
fn tabulate_rejects_three_variables() {
    let mut ds = load();
    let cmd = parser::parse_command("tabulate rep78 foreign make").expect("command parses");
    let opts = OutputOpts {
        nolabel: false,
        rawdates: false,
    };
    match commands::execute(&cmd, &mut ds, &opts) {
        Err(e) => assert!(e.contains("one variable") || e.contains("two variables"), "{}", e),
        Ok(_) => panic!("expected an error for three tabulate variables"),
    }
}

#[test]
fn order_moves_variables_to_front() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "order foreign make");
    // The order command reports the new full variable order; foreign then make lead it.
    let expected_prefix = "\"variables\":[\"foreign\",\"make\",\"price\"";
    assert!(out.contains(expected_prefix), "{}", out);
    // And the mutation persists on the dataset for subsequent commands.
    assert_eq!(ds.variables[0].name, "foreign");
    assert_eq!(ds.variables[1].name, "make");
}

#[test]
fn ds_filters_by_type() {
    let mut ds = load();
    let (out, _) = run(&mut ds, "ds, has(type int)");
    assert!(out.contains("\"price\""), "{}", out);
    assert!(!out.contains("\"make\""), "make is a string, excluded: {}", out);
}

#[test]
fn json_string_escaping() {
    // Ensure control characters and quotes are escaped so output is always valid JSON.
    let j = Json::str("a\"b\\c\n");
    assert_eq!(j.to_string(false), "\"a\\\"b\\\\c\\n\"");
}

/// A scratch path under the OS temp dir, unique per test process so parallel `cargo test` runs
/// never collide.
fn scratch_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("searchlight_cli_test_{}_{}", std::process::id(), name))
}

#[test]
fn sqlite_export_roundtrips_values_and_missing() {
    use searchlight_cli::export;

    let ds = load();
    let path = scratch_path("auto.sqlite");
    let path_str = path.to_str().unwrap();
    let _ = std::fs::remove_file(&path);

    let msg = export::export(&ds, "sqlite", Some(path_str)).expect("sqlite export succeeds");
    assert!(msg.contains("74"), "export message: {}", msg);

    let conn = rusqlite::Connection::open(&path).expect("reopen exported sqlite file");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM data", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 74);

    // Row 3 (AMC Spirit) has missing rep78; row 1's gear_ratio (float) must be stored at Stata's
    // faithful precision (3.5799999), not the raw f32->f64 widening (3.5799999237060547...).
    let (rep78_row3, gear_ratio_row1): (Option<i64>, f64) = conn
        .query_row(
            "SELECT (SELECT rep78 FROM data LIMIT 1 OFFSET 2), \
                    (SELECT gear_ratio FROM data LIMIT 1 OFFSET 0)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rep78_row3, None, "missing rep78 must be SQL NULL");
    assert_eq!(gear_ratio_row1, 3.5799999);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn mongodb_export_writes_valid_bson_dump() {
    use searchlight_cli::export;

    let ds = load();
    let dir = scratch_path("auto_mongo_dump");
    let _ = std::fs::remove_dir_all(&dir);

    let msg =
        export::export(&ds, "mongodb", Some(dir.to_str().unwrap())).expect("mongodb export succeeds");
    assert!(msg.contains("74"), "export message: {}", msg);

    let bson_path = dir.join("auto.bson");
    let metadata_path = dir.join("auto.metadata.json");
    assert!(bson_path.exists(), "expected {:?} to exist", bson_path);
    assert!(metadata_path.exists(), "expected {:?} to exist", metadata_path);

    let bytes = std::fs::read(&bson_path).expect("read bson dump");
    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let mut docs = Vec::new();
    while (cursor.position() as usize) < bytes.len() {
        docs.push(bson::Document::from_reader(&mut cursor).expect("valid BSON document"));
    }
    assert_eq!(docs.len(), 74, "expected one BSON document per observation");

    assert_eq!(
        docs[0].get_str("make").unwrap(),
        "AMC Concord",
        "doc: {:?}",
        docs[0]
    );
    assert!(
        matches!(docs[2].get("rep78"), Some(bson::Bson::Null)),
        "missing rep78 must serialize as BSON null: {:?}",
        docs[2]
    );
    assert_eq!(docs[0].get_f64("gear_ratio").unwrap(), 3.5799999);

    let _ = std::fs::remove_dir_all(&dir);
}

/// DuckDB export is fully self-contained (the `duckdb` crate's `bundled` feature compiles DuckDB
/// directly into this binary), so — unlike an external-tool dependency — this test always runs; it
/// reopens the exported file with the same bundled engine and checks contents directly, the same
/// rigor as the sqlite/mongodb export tests above.
#[test]
fn duckdb_export_roundtrips_values_and_missing() {
    use searchlight_cli::export;

    let ds = load();
    let path = scratch_path("auto.duckdb");
    let _ = std::fs::remove_file(&path);

    let msg = export::export(&ds, "duckdb", Some(path.to_str().unwrap())).expect("duckdb export succeeds");
    assert!(msg.contains("74"), "export message: {}", msg);

    let conn = duckdb::Connection::open(&path).expect("reopen exported duckdb file");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM data", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 74);

    let (rep78_row3, gear_ratio_row1): (Option<i64>, f64) = conn
        .query_row(
            "SELECT (SELECT rep78 FROM data LIMIT 1 OFFSET 2), \
                    (SELECT gear_ratio FROM data LIMIT 1 OFFSET 0)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rep78_row3, None, "missing rep78 must be SQL NULL");
    assert_eq!(gear_ratio_row1, 3.5799999, "faithful precision, not raw f32->f64 widening");

    let _ = std::fs::remove_file(&path);
}

/// Parquet export also goes through the bundled DuckDB engine (its native Parquet writer), so this
/// reopens the file the same way — via DuckDB's `read_parquet` — rather than needing a separate
/// Parquet-reading crate.
#[test]
fn parquet_export_roundtrips_values_and_missing() {
    use searchlight_cli::export;

    let ds = load();
    let path = scratch_path("auto.parquet");
    let _ = std::fs::remove_file(&path);

    let msg =
        export::export(&ds, "parquet", Some(path.to_str().unwrap())).expect("parquet export succeeds");
    assert!(msg.contains("74"), "export message: {}", msg);

    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb to read parquet back");
    let path_str = path.to_str().unwrap().replace('\'', "''");
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{}')", path_str),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 74);

    let (rep78_row3, gear_ratio_row1): (Option<i64>, f64) = conn
        .query_row(
            &format!(
                "SELECT (SELECT rep78 FROM read_parquet('{p}') LIMIT 1 OFFSET 2), \
                        (SELECT gear_ratio FROM read_parquet('{p}') LIMIT 1 OFFSET 0)",
                p = path_str
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rep78_row3, None, "missing rep78 must be NULL in the parquet file");
    assert_eq!(gear_ratio_row1, 3.5799999, "faithful precision, not raw f32->f64 widening");

    let _ = std::fs::remove_file(&path);
}
