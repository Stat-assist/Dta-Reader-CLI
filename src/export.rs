//! Export a Dataset to various formats: SQLite, DuckDB, Parquet, MongoDB.

use crate::format;
use crate::model::{missing, Dataset, Value, VarType};
use std::path::Path;

pub fn export(ds: &Dataset, format: &str, output_path: Option<&str>) -> Result<String, String> {
    match format {
        "sqlite" => export_sqlite(ds, output_path),
        "duckdb" => export_duckdb(ds, output_path),
        "parquet" => export_parquet(ds, output_path),
        "mongodb" => export_mongodb(ds, output_path),
        other => Err(format!("unsupported export format '{}'", other)),
    }
}

/// A `float`/`double` cell's value, rounded to Stata's own significant-digit precision (8 for
/// float, 16 for double) and round-tripped back through `f64::from_str`. Without this, the raw f64
/// widening of an on-disk float32 prints as e.g. `3.5799999237060547` instead of the clean
/// `3.5799999` Stata itself would show for the same cell — technically the same bits, but a
/// misleading decimal expansion. Storing the rounded value keeps every export faithful to Stata.
fn faithful_f64(value: f64, vtype: &VarType) -> f64 {
    match vtype {
        VarType::Float | VarType::Double => {
            format::number(value, vtype).parse().unwrap_or(value)
        }
        _ => value,
    }
}

fn default_output_path(dta_path: &str, extension: &str) -> String {
    let p = Path::new(dta_path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    parent
        .join(format!("{}.{}", stem, extension))
        .to_string_lossy()
        .to_string()
}

//   SQLite  

fn export_sqlite(ds: &Dataset, output_path: Option<&str>) -> Result<String, String> {
    let default_path = default_output_path(&ds.source_path, "sqlite");
    let path = output_path.unwrap_or(&default_path);

    // Remove any existing file to start fresh
    let _ = std::fs::remove_file(&path);

    let conn = rusqlite::Connection::open(&path)
        .map_err(|e| format!("could not create sqlite: {}", e))?;

    // Build CREATE TABLE statement (quote column names to avoid reserved words)
    let mut col_defs = Vec::new();
    for v in &ds.variables {
        let sql_type = match &v.vtype {
            VarType::Str(_) | VarType::StrL => "TEXT",
            VarType::Byte | VarType::Int | VarType::Long => "INTEGER",
            VarType::Float | VarType::Double => "REAL",
            VarType::Alias => "TEXT",
        };
        col_defs.push(format!("\"{}\" {}", v.name, sql_type));
    }

    let type_list = col_defs.join(", ");
    let create_sql = format!("CREATE TABLE data ({});", type_list);

    conn.execute(&create_sql, [])
        .map_err(|e| format!("could not create table: {}", e))?;

    // Insert rows (use quoted column names)
    let col_names: Vec<String> = ds.variables.iter().map(|v| format!("\"{}\"", v.name)).collect();
    let col_list = col_names.join(", ");
    let placeholders = (0..ds.nvars()).map(|_| "?").collect::<Vec<_>>().join(", ");
    let insert_sql = format!("INSERT INTO data ({}) VALUES ({});", col_list, placeholders);

    for row in 0..ds.nobs {
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| format!("could not prepare insert: {}", e))?;

        for (col, _) in ds.variables.iter().enumerate() {
            match &ds.columns[col][row] {
                Value::Text(s) => {
                    stmt.raw_bind_parameter(col + 1, s.as_str())
                        .map_err(|e| format!("could not bind text: {}", e))?;
                }
                Value::Num(n) => {
                    let vtype = &ds.variables[col].vtype;
                    if missing::is_missing(vtype, *n) {
                        stmt.raw_bind_parameter(col + 1, rusqlite::types::Null)
                            .map_err(|e| format!("could not bind null: {}", e))?;
                    } else {
                        match vtype {
                            VarType::Byte | VarType::Int | VarType::Long => {
                                stmt.raw_bind_parameter(col + 1, *n as i64)
                                    .map_err(|e| format!("could not bind int: {}", e))?;
                            }
                            _ => {
                                stmt.raw_bind_parameter(col + 1, faithful_f64(*n, vtype))
                                    .map_err(|e| format!("could not bind float: {}", e))?;
                            }
                        }
                    }
                }
                Value::Binary(_) | Value::Alias => {
                    stmt.raw_bind_parameter(col + 1, rusqlite::types::Null)
                        .map_err(|e| format!("could not bind null: {}", e))?;
                }
            }
        }

        stmt.raw_execute()
            .map_err(|e| format!("could not execute insert: {}", e))?;
    }

    Ok(format!("Exported {} observations to {}", ds.nobs, path))
}

//  DuckDB / Parquet (bundled DuckDB engine, compiled directly into this binary) 
//
// The `duckdb` crate's `bundled` feature compiles DuckDB's own C++ amalgamation into the binary
// (the same technique `rusqlite`'s `bundled` feature uses for SQLite) — no external `duckdb`
// binary or server is needed at runtime. Its `parquet` feature additionally compiles DuckDB's own
// native Parquet reader/writer extension directly in, so `-e parquet` needs no `arrow`/`parquet`
// Rust crate at all: it just runs `COPY ... TO ... (FORMAT PARQUET)` through the same embedded
// engine used for `-e duckdb`, executed entirely in-process via parameterized INSERTs (no CSV
// intermediate, no text round-trip of any value).

fn export_duckdb(ds: &Dataset, output_path: Option<&str>) -> Result<String, String> {
    let default_path = default_output_path(&ds.source_path, "duckdb");
    let path = output_path.unwrap_or(&default_path);
    // DuckDB refuses to overwrite a live database file; start fresh.
    let _ = std::fs::remove_file(path);

    let conn = duckdb::Connection::open(path)
        .map_err(|e| format!("could not create duckdb file: {}", e))?;
    populate_duckdb_table(ds, &conn, "data")?;

    Ok(format!("Exported {} observations to {}", ds.nobs, path))
}

fn export_parquet(ds: &Dataset, output_path: Option<&str>) -> Result<String, String> {
    let default_path = default_output_path(&ds.source_path, "parquet");
    let path = output_path.unwrap_or(&default_path);
    let _ = std::fs::remove_file(path);

    // Stage the data in an in-memory DuckDB table, then have DuckDB's own Parquet writer flush it.
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| format!("could not open in-memory duckdb: {}", e))?;
    populate_duckdb_table(ds, &conn, "data")?;

    let copy_sql = format!("COPY data TO '{}' (FORMAT PARQUET);", escape_sql_literal(path));
    conn.execute_batch(&copy_sql)
        .map_err(|e| format!("could not write parquet file: {}", e))?;

    Ok(format!("Exported {} observations to {}", ds.nobs, path))
}

/// Creates `table` in a DuckDB connection and inserts every observation via bound parameters
/// (never through text), mirroring `export_sqlite`'s logic exactly but against `duckdb::Connection`.
fn populate_duckdb_table(ds: &Dataset, conn: &duckdb::Connection, table: &str) -> Result<(), String> {
    let mut col_defs = Vec::new();
    for v in &ds.variables {
        let duck_type = match &v.vtype {
            VarType::Str(_) | VarType::StrL | VarType::Alias => "VARCHAR",
            VarType::Byte | VarType::Int | VarType::Long => "BIGINT",
            VarType::Float | VarType::Double => "DOUBLE",
        };
        col_defs.push(format!("\"{}\" {}", v.name, duck_type));
    }
    let create_sql = format!("CREATE TABLE \"{}\" ({});", table, col_defs.join(", "));
    conn.execute_batch(&create_sql)
        .map_err(|e| format!("could not create table: {}", e))?;

    let col_names: Vec<String> = ds.variables.iter().map(|v| format!("\"{}\"", v.name)).collect();
    let placeholders = (0..ds.nvars()).map(|_| "?").collect::<Vec<_>>().join(", ");
    let insert_sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({});",
        table,
        col_names.join(", "),
        placeholders
    );

    for row in 0..ds.nobs {
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| format!("could not prepare insert: {}", e))?;

        for col in 0..ds.nvars() {
            match &ds.columns[col][row] {
                Value::Text(s) => {
                    stmt.raw_bind_parameter(col + 1, s.as_str())
                        .map_err(|e| format!("could not bind text: {}", e))?;
                }
                Value::Num(n) => {
                    let vtype = &ds.variables[col].vtype;
                    if missing::is_missing(vtype, *n) {
                        stmt.raw_bind_parameter(col + 1, duckdb::types::Null)
                            .map_err(|e| format!("could not bind null: {}", e))?;
                    } else {
                        match vtype {
                            VarType::Byte | VarType::Int | VarType::Long => {
                                stmt.raw_bind_parameter(col + 1, *n as i64)
                                    .map_err(|e| format!("could not bind int: {}", e))?;
                            }
                            _ => {
                                stmt.raw_bind_parameter(col + 1, faithful_f64(*n, vtype))
                                    .map_err(|e| format!("could not bind float: {}", e))?;
                            }
                        }
                    }
                }
                Value::Binary(_) | Value::Alias => {
                    stmt.raw_bind_parameter(col + 1, duckdb::types::Null)
                        .map_err(|e| format!("could not bind null: {}", e))?;
                }
            }
        }

        stmt.raw_execute()
            .map_err(|e| format!("could not execute insert: {}", e))?;
    }

    Ok(())
}

/// Escape a value for embedding in a single-quoted SQL string literal.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''").replace('\\', "\\\\")
}

//   MongoDB  

/// A directory path (no extension) alongside the source .dta file, matching its stem. Used as the
/// default dump directory since a mongodump-style export is a folder of files, not one file.
fn default_output_dir(dta_path: &str) -> String {
    let p = Path::new(dta_path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    parent.join(stem).to_string_lossy().to_string()
}

/// Writes a `mongodump`-format dump directory (`<collection>.bson` + `<collection>.metadata.json`)
/// without needing a running `mongod`: the raw BSON documents are hand-written directly, byte-for-
/// byte in the same layout `mongodump` itself produces, and are restorable with `mongorestore`.
///
/// Known limitation: strL cells holding *binary* content (embedded NUL bytes) are exported as
/// `null` because the .dta reader currently only records their byte length, not their bytes (the
/// commands that needed strLs so far — `list`, `export delimited` — never display binary content
/// either). Text strLs and all other types export their real values.
fn export_mongodb(ds: &Dataset, output_path: Option<&str>) -> Result<String, String> {
    use crate::json::Json;
    use bson::{Bson, Document};
    use std::fs::File;

    let default_dir = default_output_dir(&ds.source_path);
    let dir = output_path.unwrap_or(&default_dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create directory {}: {}", dir, e))?;

    let collection = Path::new(&ds.source_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data");

    let bson_path = Path::new(dir).join(format!("{}.bson", collection));
    let metadata_path = Path::new(dir).join(format!("{}.metadata.json", collection));

    let mut file =
        File::create(&bson_path).map_err(|e| format!("could not create {:?}: {}", bson_path, e))?;

    for row in 0..ds.nobs {
        let mut doc = Document::new();
        for col in 0..ds.nvars() {
            let name = ds.variables[col].name.clone();
            let vtype = &ds.variables[col].vtype;
            let value = match &ds.columns[col][row] {
                Value::Text(s) => Bson::String(s.clone()),
                Value::Num(n) => {
                    if missing::is_missing(vtype, *n) {
                        Bson::Null
                    } else {
                        match vtype {
                            VarType::Byte | VarType::Int | VarType::Long => Bson::Int64(*n as i64),
                            _ => Bson::Double(faithful_f64(*n, vtype)),
                        }
                    }
                }
                // Binary strL content and alias cells carry no exportable value (see doc comment).
                Value::Binary(_) | Value::Alias => Bson::Null,
            };
            doc.insert(name, value);
        }
        doc.to_writer(&mut file)
            .map_err(|e| format!("could not write BSON document: {}", e))?;
    }

    // Minimal metadata.json: an empty options block plus the default _id index every real
    // MongoDB collection has. mongorestore accepts this and (re)creates that index on restore;
    // _id itself is populated automatically by the server-side insert path during restore.
    let metadata = Json::Object(vec![
        ("options".into(), Json::Object(vec![])),
        (
            "indexes".into(),
            Json::Array(vec![Json::Object(vec![
                ("v".into(), Json::int(2)),
                ("key".into(), Json::Object(vec![("_id".into(), Json::int(1))])),
                ("name".into(), Json::str("_id_")),
            ])]),
        ),
    ]);
    std::fs::write(&metadata_path, metadata.to_string(true))
        .map_err(|e| format!("could not write {:?}: {}", metadata_path, e))?;

    Ok(format!(
        "Exported {} observations to {} (collection '{}'). Restore with: mongorestore --db=<yourdb> --collection={} \"{}\"",
        ds.nobs,
        dir,
        collection,
        collection,
        bson_path.display(),
    ))
}
