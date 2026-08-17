//! Implementations of the supported Stata commands. Each produces a JSON `Outcome`.
//!
//! Output philosophy: emit the *critical information* each Stata command conveys, as structured
//! JSON (see README.md for the per-command schema), rather than reproducing Stata's ASCII layout.

use crate::expr::{self, Condition};
use crate::format;
use crate::json::Json;
use crate::model::{missing, Characteristic, Dataset, Value, VarType, DATASET_SCOPE};
use crate::parser::{self, Cmd};
use std::io::Write;

pub struct Outcome {
    /// The JSON document to print (the envelope).
    pub value: Json,
    /// When `Some` and `--jsonl` is set, print these one compact object per line instead of `value`.
    pub jsonl_rows: Option<Vec<Json>>,
    pub exit_code: u8,
}

impl Outcome {
    fn json(value: Json) -> Outcome {
        Outcome {
            value,
            jsonl_rows: None,
            exit_code: 0,
        }
    }
}

pub struct OutputOpts {
    pub nolabel: bool,
    pub rawdates: bool,
}

pub fn execute(cmd: &Cmd, ds: &mut Dataset, opts: &OutputOpts) -> Result<Outcome, String> {
    match cmd.name.as_str() {
        "describe" => describe(cmd, ds),
        "list" => list(cmd, ds, opts),
        "summarize" => summarize(cmd, ds),
        "tabulate" => tabulate(cmd, ds, opts),
        "inspect" => inspect(cmd, ds),
        "count" => count(cmd, ds),
        "ds" => ds_cmd(cmd, ds),
        "lookfor" => lookfor(cmd, ds),
        "order" => order(cmd, ds),
        "label list" => label_list(cmd, ds),
        "notes list" => notes_list(ds),
        "misstable summarize" => misstable_summarize(cmd, ds),
        "assert" => assert(cmd, ds),
        "export delimited" => export_delimited(cmd, ds, opts),
        other => Err(format!("command '{}' is not implemented", other)),
    }
}

// 
// Shared helpers
// 

/// Rows selected by the command's `in` range and `if` expression, as 0-based indices.
fn selected_rows(cmd: &Cmd, ds: &Dataset) -> Result<Vec<usize>, String> {
    let mut rows: Vec<usize> = match &cmd.in_spec {
        Some(spec) => {
            let (a, b) = expr::parse_in_range(spec, ds.nobs)?;
            (a..=b).collect()
        }
        None => (0..ds.nobs).collect(),
    };
    if let Some(src) = &cmd.if_expr {
        let cond = Condition::parse(src, ds)?;
        let mut kept = Vec::with_capacity(rows.len());
        for r in rows {
            if cond.matches(ds, r)? {
                kept.push(r);
            }
        }
        rows = kept;
    }
    Ok(rows)
}

fn var_meta_json(ds: &Dataset, col: usize) -> Json {
    let v = &ds.variables[col];
    Json::Object(vec![
        ("name".into(), Json::str(&v.name)),
        ("type".into(), Json::str(v.vtype.name())),
        ("format".into(), Json::str(&v.format)),
        ("value_label".into(), Json::str(&v.value_label_name)),
        ("variable_label".into(), Json::str(&v.label)),
    ])
}

/// Encode one cell as JSON for `list`/data extraction (see README for the rules).
fn cell_json(ds: &Dataset, col: usize, row: usize, opts: &OutputOpts) -> Json {
    let var = &ds.variables[col];
    match &ds.columns[col][row] {
        Value::Text(s) => Json::Str(s.clone()),
        Value::Binary(_) => Json::Null,
        Value::Alias => Json::Null,
        Value::Num(x) => {
            let t = &var.vtype;
            if missing::is_missing(t, *x) {
                return Json::Null;
            }
            if !opts.nolabel && var.has_value_label() {
                if let Some(vl) = ds.value_label(&var.value_label_name) {
                    if let Some(txt) = vl.get(*x as i32) {
                        return Json::Str(txt.clone());
                    }
                }
            }
            if !opts.rawdates {
                if let Some((dt, details)) = format::parse_date_format(&var.format) {
                    if let Some(s) = format::render_date(dt, &details, *x) {
                        return Json::Str(s);
                    }
                }
            }
            Json::Num(format::number(*x, t))
        }
    }
}

/// The nonmissing numeric values of a variable over the given rows.
fn nonmissing_values(ds: &Dataset, col: usize, rows: &[usize]) -> Vec<f64> {
    let t = &ds.variables[col].vtype;
    rows.iter()
        .filter_map(|&r| match &ds.columns[col][r] {
            Value::Num(x) if !missing::is_missing(t, *x) => Some(*x),
            _ => None,
        })
        .collect()
}

// 
// describe
// 

fn describe(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let varlist: Vec<Json> = cols.iter().map(|&c| var_meta_json(ds, c)).collect();
    let sorted_by: Vec<Json> = ds
        .sort_order
        .iter()
        .map(|&i| Json::str(&ds.variables[i].name))
        .collect();
    let has_notes = ds
        .characteristics
        .iter()
        .any(|c| c.varname == DATASET_SCOPE && c.charname == "note0");

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("describe")),
        ("file".into(), Json::str(&ds.source_path)),
        ("label".into(), Json::str(&ds.dataset_label)),
        ("observations".into(), Json::int(ds.nobs as i64)),
        ("variables".into(), Json::int(ds.nvars() as i64)),
        ("timestamp".into(), Json::str(&ds.timestamp)),
        ("release".into(), Json::int(ds.source_release as i64)),
        ("sorted_by".into(), Json::Array(sorted_by)),
        ("has_notes".into(), Json::Bool(has_notes)),
        ("varlist".into(), Json::Array(varlist)),
    ])))
}

// 
// list
// 

fn list(cmd: &Cmd, ds: &Dataset, opts: &OutputOpts) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let rows = selected_rows(cmd, ds)?;

    let mut row_objects: Vec<Json> = Vec::with_capacity(rows.len());
    for &r in &rows {
        let mut fields: Vec<(String, Json)> = Vec::with_capacity(cols.len() + 1);
        fields.push(("_n".into(), Json::int((r + 1) as i64)));
        for &c in &cols {
            fields.push((ds.variables[c].name.clone(), cell_json(ds, c, r, opts)));
        }
        row_objects.push(Json::Object(fields));
    }

    let var_names: Vec<Json> = cols.iter().map(|&c| Json::str(&ds.variables[c].name)).collect();
    let value = Json::Object(vec![
        ("command".into(), Json::str("list")),
        ("variables".into(), Json::Array(var_names)),
        ("observations".into(), Json::int(rows.len() as i64)),
        ("data".into(), Json::Array(row_objects.clone())),
    ]);
    Ok(Outcome {
        value,
        jsonl_rows: Some(row_objects),
        exit_code: 0,
    })
}

// 
// summarize
// 

fn summarize(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let rows = selected_rows(cmd, ds)?;
    let detail = cmd.has_option("detail");

    let mut var_stats = Vec::new();
    for &c in &cols {
        let var = &ds.variables[c];
        if !var.is_numeric() {
            // Stata prints Obs=0 and blanks for a string variable under summarize.
            var_stats.push(Json::Object(vec![
                ("variable".into(), Json::str(&var.name)),
                ("obs".into(), Json::int(0)),
                ("mean".into(), Json::Null),
                ("sd".into(), Json::Null),
                ("min".into(), Json::Null),
                ("max".into(), Json::Null),
            ]));
            continue;
        }
        let vals = nonmissing_values(ds, c, &rows);
        let n = vals.len();
        if n == 0 {
            var_stats.push(Json::Object(vec![
                ("variable".into(), Json::str(&var.name)),
                ("obs".into(), Json::int(0)),
                ("mean".into(), Json::Null),
                ("sd".into(), Json::Null),
                ("min".into(), Json::Null),
                ("max".into(), Json::Null),
            ]));
            continue;
        }
        let mean = vals.iter().sum::<f64>() / n as f64;
        let variance = if n > 1 {
            vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)
        } else {
            0.0
        };
        let sd = variance.sqrt();
        let mut sorted = vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = sorted[0];
        let max = sorted[n - 1];

        let mut fields = vec![
            ("variable".into(), Json::str(&var.name)),
            ("obs".into(), Json::int(n as i64)),
            ("mean".into(), Json::Num(format::stat_number(mean))),
            ("sd".into(), Json::Num(format::stat_number(sd))),
            ("min".into(), Json::Num(format::number(min, &var.vtype))),
            ("max".into(), Json::Num(format::number(max, &var.vtype))),
        ];
        if detail {
            let pct = |p: f64| Json::Num(format::stat_number(percentile(&sorted, p)));
            let percentiles = Json::Object(vec![
                ("1".into(), pct(1.0)),
                ("5".into(), pct(5.0)),
                ("10".into(), pct(10.0)),
                ("25".into(), pct(25.0)),
                ("50".into(), pct(50.0)),
                ("75".into(), pct(75.0)),
                ("90".into(), pct(90.0)),
                ("95".into(), pct(95.0)),
                ("99".into(), pct(99.0)),
            ]);
            let (skew, kurt) = skewness_kurtosis(&vals, mean);
            fields.push(("variance".into(), Json::Num(format::stat_number(variance))));
            fields.push(("skewness".into(), Json::Num(format::stat_number(skew))));
            fields.push(("kurtosis".into(), Json::Num(format::stat_number(kurt))));
            fields.push(("sum".into(), Json::Num(format::stat_number(vals.iter().sum()))));
            fields.push(("percentiles".into(), percentiles));
        }
        var_stats.push(Json::Object(fields));
    }

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("summarize")),
        ("observations_in_scope".into(), Json::int(rows.len() as i64)),
        ("variables".into(), Json::Array(var_stats)),
    ])))
}

/// Stata's percentile method on already-sorted data (1-based indexing in the formula).
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    let q = n as f64 * p / 100.0;
    let qf = q.floor();
    if (q - qf).abs() < 1e-9 {
        let i = qf as usize; // 1-based
        if i >= n {
            sorted[n - 1]
        } else {
            (sorted[i - 1] + sorted[i]) / 2.0
        }
    } else {
        let i = qf as usize + 1; // 1-based
        sorted[(i - 1).min(n - 1)]
    }
}

fn skewness_kurtosis(vals: &[f64], mean: f64) -> (f64, f64) {
    let n = vals.len() as f64;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let m2 = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    let m3 = vals.iter().map(|v| (v - mean).powi(3)).sum::<f64>() / n;
    let m4 = vals.iter().map(|v| (v - mean).powi(4)).sum::<f64>() / n;
    let skew = if m2 > 0.0 { m3 / m2.powf(1.5) } else { 0.0 };
    let kurt = if m2 > 0.0 { m4 / (m2 * m2) } else { 0.0 };
    (skew, kurt)
}

//
// tabulate (one-way and two-way)
//

fn tabulate(cmd: &Cmd, ds: &Dataset, opts: &OutputOpts) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    match cols.len() {
        1 => tabulate_oneway(cmd, ds, opts, cols[0]),
        2 => tabulate_twoway(cmd, ds, opts, cols[0], cols[1]),
        _ => Err("tabulate takes one variable (one-way) or two variables (two-way)".to_string()),
    }
}

/// A distinct tabulation category for one variable. Only one variant is ever populated for a
/// given variable (string variables produce `Str`, numeric produce `Int`/`FloatBits`), matching
/// the .dta storage type being fixed per variable.
#[derive(Clone, PartialEq, Eq, Hash)]
enum CatKey {
    Int(i64),
    FloatBits(u64),
    Str(String),
}

/// The category a cell falls into, or `None` for cells with no tabulatable value (strL binary
/// blobs, alias cells) — such observations are excluded from tabulation entirely, same as the
/// original one-way implementation.
fn categorize(ds: &Dataset, col: usize, row: usize) -> Option<CatKey> {
    let var = &ds.variables[col];
    match &ds.columns[col][row] {
        Value::Text(s) => Some(CatKey::Str(s.clone())),
        Value::Num(x) => {
            let is_int_like = matches!(var.vtype, VarType::Byte | VarType::Int | VarType::Long);
            if is_int_like || x.fract() == 0.0 {
                Some(CatKey::Int(*x as i64))
            } else {
                Some(CatKey::FloatBits(x.to_bits()))
            }
        }
        Value::Binary(_) | Value::Alias => None,
    }
}

/// Whether the cell at (col, row) counts as missing for tabulation purposes: Stata's missing
/// numeric codes, or an empty string for string variables.
fn is_missing_cell(ds: &Dataset, col: usize, row: usize) -> bool {
    match &ds.columns[col][row] {
        Value::Num(x) => missing::is_missing(&ds.variables[col].vtype, *x),
        Value::Text(s) => s.is_empty(),
        Value::Binary(_) | Value::Alias => false,
    }
}

/// The JSON value (and value-label text, if applicable) to display for one category of `col`.
fn cat_display(ds: &Dataset, col: usize, key: &CatKey, opts: &OutputOpts) -> (Json, Option<String>) {
    let var = &ds.variables[col];
    match key {
        CatKey::Str(s) => (Json::Str(s.clone()), None),
        CatKey::FloatBits(bits) => (
            Json::Num(format::number(f64::from_bits(*bits), &var.vtype)),
            None,
        ),
        CatKey::Int(k) => {
            let is_miss = missing::is_missing(&var.vtype, *k as f64);
            let value_json = if is_miss {
                Json::str(missing::code_to_display(missing::missing_code(&var.vtype, *k as f64)))
            } else {
                Json::Num(format::number(*k as f64, &var.vtype))
            };
            let label = if !opts.nolabel && var.has_value_label() && !is_miss {
                ds.value_label(&var.value_label_name)
                    .and_then(|vl| vl.get(*k as i32).cloned())
            } else {
                None
            };
            (value_json, label)
        }
    }
}

/// Ascending order for the categories of a single axis. Only ever compares same-variant keys in
/// practice (see `CatKey`'s doc comment); mixed comparisons fall back to `Equal`.
fn cat_key_cmp(a: &CatKey, b: &CatKey) -> std::cmp::Ordering {
    match (a, b) {
        (CatKey::Int(x), CatKey::Int(y)) => x.cmp(y),
        (CatKey::FloatBits(x), CatKey::FloatBits(y)) => f64::from_bits(*x)
            .partial_cmp(&f64::from_bits(*y))
            .unwrap_or(std::cmp::Ordering::Equal),
        (CatKey::Str(x), CatKey::Str(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

fn tabulate_twoway(
    cmd: &Cmd,
    ds: &Dataset,
    opts: &OutputOpts,
    row_col: usize,
    col_col: usize,
) -> Result<Outcome, String> {
    use std::collections::{HashMap, HashSet};

    let rows_sel = selected_rows(cmd, ds)?;
    let include_missing = cmd.has_option("missing");

    let mut counts: HashMap<(CatKey, CatKey), usize> = HashMap::new();
    let mut row_totals: HashMap<CatKey, usize> = HashMap::new();
    let mut col_totals: HashMap<CatKey, usize> = HashMap::new();
    let mut row_keys: HashSet<CatKey> = HashSet::new();
    let mut col_keys: HashSet<CatKey> = HashSet::new();
    let mut total = 0usize;

    for &r in &rows_sel {
        let (Some(rk), Some(ck)) = (categorize(ds, row_col, r), categorize(ds, col_col, r)) else {
            continue;
        };
        let is_missing = is_missing_cell(ds, row_col, r) || is_missing_cell(ds, col_col, r);
        if is_missing && !include_missing {
            continue;
        }
        *counts.entry((rk.clone(), ck.clone())).or_insert(0) += 1;
        *row_totals.entry(rk.clone()).or_insert(0) += 1;
        *col_totals.entry(ck.clone()).or_insert(0) += 1;
        row_keys.insert(rk);
        col_keys.insert(ck);
        total += 1;
    }

    let mut row_order: Vec<CatKey> = row_keys.into_iter().collect();
    row_order.sort_by(cat_key_cmp);
    let mut col_order: Vec<CatKey> = col_keys.into_iter().collect();
    col_order.sort_by(cat_key_cmp);

    let columns_json: Vec<Json> = col_order
        .iter()
        .map(|ck| {
            let (value, label) = cat_display(ds, col_col, ck, opts);
            let mut o = vec![("value".into(), value)];
            if let Some(l) = label {
                o.push(("label".into(), Json::Str(l)));
            }
            Json::Object(o)
        })
        .collect();

    let rows_json: Vec<Json> = row_order
        .iter()
        .map(|rk| {
            let (value, label) = cat_display(ds, row_col, rk, opts);
            let cell_counts: Vec<Json> = col_order
                .iter()
                .map(|ck| Json::int(*counts.get(&(rk.clone(), ck.clone())).unwrap_or(&0) as i64))
                .collect();
            let mut o = vec![("value".into(), value)];
            if let Some(l) = label {
                o.push(("label".into(), Json::Str(l)));
            }
            o.push(("counts".into(), Json::Array(cell_counts)));
            o.push((
                "total".into(),
                Json::int(*row_totals.get(rk).unwrap_or(&0) as i64),
            ));
            Json::Object(o)
        })
        .collect();

    let column_totals_json: Vec<Json> = col_order
        .iter()
        .map(|ck| Json::int(*col_totals.get(ck).unwrap_or(&0) as i64))
        .collect();

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("tabulate")),
        ("row_variable".into(), Json::str(&ds.variables[row_col].name)),
        ("column_variable".into(), Json::str(&ds.variables[col_col].name)),
        ("columns".into(), Json::Array(columns_json)),
        ("rows".into(), Json::Array(rows_json)),
        ("column_totals".into(), Json::Array(column_totals_json)),
        ("total".into(), Json::int(total as i64)),
    ])))
}

fn tabulate_oneway(cmd: &Cmd, ds: &Dataset, opts: &OutputOpts, col: usize) -> Result<Outcome, String> {
    let var = &ds.variables[col];
    let rows = selected_rows(cmd, ds)?;
    let include_missing = cmd.has_option("missing");

    // Count occurrences per distinct value, preserving numeric order.
    use std::collections::BTreeMap;
    let mut numeric_counts: BTreeMap<i64, usize> = BTreeMap::new();
    let mut float_counts: Vec<(f64, usize)> = Vec::new();
    let mut string_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let is_int_like = matches!(
        var.vtype,
        VarType::Byte | VarType::Int | VarType::Long
    );

    for &r in &rows {
        match &ds.columns[col][r] {
            Value::Num(x) => {
                let x = *x;
                if missing::is_missing(&var.vtype, x) && !include_missing {
                    continue;
                }
                total += 1;
                if is_int_like || x.fract() == 0.0 {
                    *numeric_counts.entry(x as i64).or_insert(0) += 1;
                } else {
                    match float_counts.iter_mut().find(|(v, _)| *v == x) {
                        Some((_, c)) => *c += 1,
                        None => float_counts.push((x, 1)),
                    }
                }
            }
            Value::Text(s) => {
                if s.is_empty() && !include_missing {
                    continue;
                }
                total += 1;
                *string_counts.entry(s.clone()).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    // Build ordered rows.
    let mut entries: Vec<(Json, Option<String>, usize)> = Vec::new(); // (value_json, label, freq)
    if var.is_string() {
        for (k, c) in string_counts {
            entries.push((Json::Str(k), None, c));
        }
    } else {
        for (k, c) in numeric_counts {
            let is_miss = missing::is_missing(&var.vtype, k as f64);
            let value_json = if is_miss {
                Json::str(missing::code_to_display(missing::missing_code(&var.vtype, k as f64)))
            } else {
                Json::Num(format::number(k as f64, &var.vtype))
            };
            let label = if !opts.nolabel && var.has_value_label() && !is_miss {
                ds.value_label(&var.value_label_name)
                    .and_then(|vl| vl.get(k as i32).cloned())
            } else {
                None
            };
            entries.push((value_json, label, c));
        }
        float_counts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (v, c) in float_counts {
            entries.push((Json::Num(format::number(v, &var.vtype)), None, c));
        }
    }

    let mut cum = 0usize;
    let row_json: Vec<Json> = entries
        .into_iter()
        .map(|(value, label, freq)| {
            cum += freq;
            let pct = 100.0 * freq as f64 / total as f64;
            let cum_pct = 100.0 * cum as f64 / total as f64;
            let mut o = vec![
                ("value".into(), value),
                ("freq".into(), Json::int(freq as i64)),
                ("percent".into(), Json::Num(format::stat_number(round2(pct)))),
                ("cum".into(), Json::Num(format::stat_number(round2(cum_pct)))),
            ];
            if let Some(l) = label {
                o.insert(1, ("label".into(), Json::Str(l)));
            }
            Json::Object(o)
        })
        .collect();

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("tabulate")),
        ("variable".into(), Json::str(&var.name)),
        ("rows".into(), Json::Array(row_json)),
        ("total".into(), Json::int(total as i64)),
    ])))
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

// 
// inspect
// 

fn inspect(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let rows = selected_rows(cmd, ds)?;

    let mut results = Vec::new();
    for &c in &cols {
        let var = &ds.variables[c];
        if !var.is_numeric() {
            continue; // inspect only reports numeric variables
        }
        let t = &var.vtype;
        let (mut neg_i, mut neg_n, mut zero, mut pos_i, mut pos_n, mut miss) =
            (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut uniq = std::collections::HashSet::new();
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &r in &rows {
            if let Value::Num(x) = &ds.columns[c][r] {
                let x = *x;
                if missing::is_missing(t, x) {
                    miss += 1;
                    continue;
                }
                uniq.insert(x.to_bits());
                min = min.min(x);
                max = max.max(x);
                let is_int = x.fract() == 0.0;
                if x < 0.0 {
                    if is_int {
                        neg_i += 1
                    } else {
                        neg_n += 1
                    }
                } else if x == 0.0 {
                    zero += 1;
                } else if is_int {
                    pos_i += 1
                } else {
                    pos_n += 1
                }
            }
        }
        let total_int = neg_i + zero + pos_i;
        let total_non = neg_n + pos_n;
        let total = total_int + total_non;
        let group = |t: usize, i: usize, n: usize| {
            Json::Object(vec![
                ("total".into(), Json::int(t as i64)),
                ("integers".into(), Json::int(i as i64)),
                ("nonintegers".into(), Json::int(n as i64)),
            ])
        };
        results.push(Json::Object(vec![
            ("variable".into(), Json::str(&var.name)),
            ("label".into(), Json::str(&var.label)),
            ("negative".into(), group(neg_i + neg_n, neg_i, neg_n)),
            ("zero".into(), group(zero, zero, 0)),
            ("positive".into(), group(pos_i + pos_n, pos_i, pos_n)),
            ("total".into(), group(total, total_int, total_non)),
            ("missing".into(), Json::int(miss as i64)),
            ("unique_values".into(), Json::int(uniq.len() as i64)),
            (
                "min".into(),
                if min.is_finite() {
                    Json::Num(format::number(min, t))
                } else {
                    Json::Null
                },
            ),
            (
                "max".into(),
                if max.is_finite() {
                    Json::Num(format::number(max, t))
                } else {
                    Json::Null
                },
            ),
        ]));
    }

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("inspect")),
        ("variables".into(), Json::Array(results)),
    ])))
}

// 
// count
// 

fn count(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let rows = selected_rows(cmd, ds)?;
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("count")),
        ("count".into(), Json::int(rows.len() as i64)),
    ])))
}

// 
// ds
// 

fn ds_cmd(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let mut cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;

    if let Some(spec) = cmd.option_value("has") {
        // Supported: has(type <storage>|numeric|string).
        let spec = spec.trim();
        if let Some(rest) = spec.strip_prefix("type") {
            let want = rest.trim();
            cols.retain(|&c| {
                let t = &ds.variables[c].vtype;
                match want {
                    "numeric" => t.is_numeric(),
                    "string" => t.is_string(),
                    other => t.name() == other,
                }
            });
        } else {
            return Err(format!("unsupported ds option has({})", spec));
        }
    }

    let names: Vec<Json> = cols.iter().map(|&c| Json::str(&ds.variables[c].name)).collect();
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("ds")),
        ("variables".into(), Json::Array(names)),
    ])))
}

// 
// lookfor
// 

fn lookfor(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    if cmd.varlist_tokens.is_empty() {
        return Err("lookfor requires one or more search terms".to_string());
    }
    let terms: Vec<String> = cmd
        .varlist_tokens
        .iter()
        .map(|t| t.to_lowercase())
        .collect();
    let mut matched = Vec::new();
    for c in 0..ds.nvars() {
        let v = &ds.variables[c];
        let hay = format!("{} {}", v.name.to_lowercase(), v.label.to_lowercase());
        if terms.iter().any(|t| hay.contains(t)) {
            matched.push(var_meta_json(ds, c));
        }
    }
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("lookfor")),
        (
            "terms".into(),
            Json::Array(cmd.varlist_tokens.iter().map(Json::str).collect()),
        ),
        ("matches".into(), Json::Array(matched)),
    ])))
}

// 
// order (mutates the in-memory variable order)
// 

fn order(cmd: &Cmd, ds: &mut Dataset) -> Result<Outcome, String> {
    let moving = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    if moving.is_empty() {
        return Err("order requires a varlist".to_string());
    }
    let moving_set: std::collections::HashSet<usize> = moving.iter().copied().collect();
    let rest: Vec<usize> = (0..ds.nvars()).filter(|c| !moving_set.contains(c)).collect();

    // Determine the new index order.
    let new_order: Vec<usize> = if cmd.has_option("last") {
        rest.iter().chain(moving.iter()).copied().collect()
    } else if let Some(anchor) = cmd.option_value("before").or(cmd.option_value("after")) {
        let anchor_idx = ds
            .var_index(anchor.trim())
            .ok_or_else(|| format!("variable {} not found", anchor.trim()))?;
        let after = cmd.option_value("after").is_some();
        let mut result = Vec::new();
        for &c in &rest {
            if c == anchor_idx && !after {
                result.extend(moving.iter().copied());
            }
            result.push(c);
            if c == anchor_idx && after {
                result.extend(moving.iter().copied());
            }
        }
        result
    } else {
        // Default: move the listed variables to the front.
        moving.iter().chain(rest.iter()).copied().collect()
    };

    apply_variable_permutation(ds, &new_order);

    let names: Vec<Json> = ds.variables.iter().map(|v| Json::str(&v.name)).collect();
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("order")),
        ("variables".into(), Json::Array(names)),
    ])))
}

/// Reorder `variables` and `columns` by `new_order` (a permutation of 0..K), fixing sort indices.
fn apply_variable_permutation(ds: &mut Dataset, new_order: &[usize]) {
    let old_vars = std::mem::take(&mut ds.variables);
    let old_cols = std::mem::take(&mut ds.columns);
    let mut vars: Vec<_> = old_vars.into_iter().map(Some).collect();
    let mut cols: Vec<_> = old_cols.into_iter().map(Some).collect();
    let mut new_index_of = vec![0usize; new_order.len()];
    for (new_i, &old_i) in new_order.iter().enumerate() {
        new_index_of[old_i] = new_i;
    }
    let mut new_vars = Vec::with_capacity(new_order.len());
    let mut new_cols = Vec::with_capacity(new_order.len());
    for &old_i in new_order {
        new_vars.push(vars[old_i].take().unwrap());
        new_cols.push(cols[old_i].take().unwrap());
    }
    ds.variables = new_vars;
    ds.columns = new_cols;
    ds.sort_order = ds.sort_order.iter().map(|&i| new_index_of[i]).collect();
}

// 
// label list
// 

fn label_list(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let wanted: Option<Vec<String>> = if cmd.varlist_tokens.is_empty() {
        None
    } else {
        Some(cmd.varlist_tokens.clone())
    };
    let mut labels = Vec::new();
    for vl in &ds.value_labels {
        if let Some(names) = &wanted {
            if !names.contains(&vl.name) {
                continue;
            }
        }
        let entries: Vec<Json> = vl
            .entries
            .iter()
            .map(|(k, v)| {
                Json::Object(vec![
                    ("value".into(), Json::int(*k as i64)),
                    ("label".into(), Json::str(v)),
                ])
            })
            .collect();
        labels.push(Json::Object(vec![
            ("name".into(), Json::str(&vl.name)),
            ("entries".into(), Json::Array(entries)),
        ]));
    }
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("label list")),
        ("labels".into(), Json::Array(labels)),
    ])))
}

// 
// notes list
// 

fn notes_list(ds: &Dataset) -> Result<Outcome, String> {
    // Notes are stored as characteristics: <scope>[note0] holds the count, <scope>[noteK] the text.
    let mut notes: Vec<(String, i64, &Characteristic)> = Vec::new();
    for c in &ds.characteristics {
        if let Some(num) = c.charname.strip_prefix("note") {
            if let Ok(k) = num.parse::<i64>() {
                if k >= 1 {
                    notes.push((c.varname.clone(), k, c));
                }
            }
        }
    }
    notes.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let arr: Vec<Json> = notes
        .into_iter()
        .map(|(scope, k, c)| {
            Json::Object(vec![
                ("scope".into(), Json::str(&scope)),
                ("index".into(), Json::int(k)),
                ("text".into(), Json::str(&c.contents)),
            ])
        })
        .collect();
    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("notes list")),
        ("notes".into(), Json::Array(arr)),
    ])))
}

// 
// misstable summarize
// 

fn misstable_summarize(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let rows = selected_rows(cmd, ds)?;

    let mut result = Vec::new();
    for &c in &cols {
        let var = &ds.variables[c];
        if !var.is_numeric() {
            continue; // misstable considers numeric variables only
        }
        let t = &var.vtype;
        let (mut eq_dot, mut gt_dot) = (0usize, 0usize);
        let mut nonmiss = Vec::new();
        for &r in &rows {
            if let Value::Num(x) = &ds.columns[c][r] {
                let x = *x;
                if missing::is_missing(t, x) {
                    if missing::missing_code(t, x) == 0 {
                        eq_dot += 1;
                    } else {
                        gt_dot += 1;
                    }
                } else {
                    nonmiss.push(x);
                }
            }
        }
        if eq_dot + gt_dot == 0 {
            continue; // misstable lists only variables that have missing values
        }
        let mut uniq = std::collections::HashSet::new();
        for &v in &nonmiss {
            uniq.insert(v.to_bits());
        }
        let min = nonmiss.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = nonmiss.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        result.push(Json::Object(vec![
            ("variable".into(), Json::str(&var.name)),
            ("obs_eq_dot".into(), Json::int(eq_dot as i64)),
            ("obs_gt_dot".into(), Json::int(gt_dot as i64)),
            ("obs_lt_dot".into(), Json::int(nonmiss.len() as i64)),
            ("unique_values".into(), Json::int(uniq.len() as i64)),
            (
                "min".into(),
                if min.is_finite() {
                    Json::Num(format::number(min, t))
                } else {
                    Json::Null
                },
            ),
            (
                "max".into(),
                if max.is_finite() {
                    Json::Num(format::number(max, t))
                } else {
                    Json::Null
                },
            ),
        ]));
    }

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("misstable summarize")),
        ("variables".into(), Json::Array(result)),
    ])))
}

// 
// assert
// 

fn assert(cmd: &Cmd, ds: &Dataset) -> Result<Outcome, String> {
    // The assertion expression is the main clause (the tokens before any if/in). Any `if`/`in`
    // qualifiers restrict the observations over which the assertion must hold (via selected_rows).
    if cmd.varlist_tokens.is_empty() {
        return Err("assert requires an expression".to_string());
    }
    let expr_src = cmd.varlist_tokens.join(" ");
    let cond = Condition::parse(&expr_src, ds)?;

    let rows = selected_rows(cmd, ds)?;
    let mut contradictions = 0usize;
    for &r in &rows {
        if !cond.matches(ds, r)? {
            contradictions += 1;
        }
    }
    let passed = contradictions == 0;
    Ok(Outcome {
        value: Json::Object(vec![
            ("command".into(), Json::str("assert")),
            ("expression".into(), Json::str(&expr_src)),
            ("passed".into(), Json::Bool(passed)),
            ("contradictions".into(), Json::int(contradictions as i64)),
            ("total".into(), Json::int(rows.len() as i64)),
        ]),
        jsonl_rows: None,
        exit_code: if passed { 0 } else { 1 },
    })
}

// 
// export delimited
// 

fn export_delimited(cmd: &Cmd, ds: &Dataset, opts: &OutputOpts) -> Result<Outcome, String> {
    let path = cmd
        .using
        .as_ref()
        .ok_or_else(|| "export delimited requires 'using <file>'".to_string())?;
    let cols = parser::resolve_varlist(&cmd.varlist_tokens, ds)?;
    let rows = selected_rows(cmd, ds)?;
    let delimiter = cmd.option_value("delimiter").unwrap_or(",").to_string();
    let novarnames = cmd.has_option("novarnames");
    let nolabel = cmd.has_option("nolabel") || opts.nolabel;

    if std::path::Path::new(path).exists() && !cmd.has_option("replace") {
        return Err(format!(
            "file {} already exists; specify the 'replace' option to overwrite",
            path
        ));
    }

    let mut out = String::new();
    if !novarnames {
        let header: Vec<String> = cols
            .iter()
            .map(|&c| csv_field(&ds.variables[c].name, &delimiter))
            .collect();
        out.push_str(&header.join(&delimiter));
        out.push('\n');
    }
    for &r in &rows {
        let fields: Vec<String> = cols
            .iter()
            .map(|&c| csv_cell(ds, c, r, nolabel, opts.rawdates, &delimiter))
            .collect();
        out.push_str(&fields.join(&delimiter));
        out.push('\n');
    }

    let mut file =
        std::fs::File::create(path).map_err(|e| format!("could not write {}: {}", path, e))?;
    file.write_all(out.as_bytes())
        .map_err(|e| format!("could not write {}: {}", path, e))?;

    Ok(Outcome::json(Json::Object(vec![
        ("command".into(), Json::str("export delimited")),
        ("file".into(), Json::str(path)),
        ("observations".into(), Json::int(rows.len() as i64)),
        ("variables".into(), Json::int(cols.len() as i64)),
        ("format".into(), Json::str("csv")),
    ])))
}

/// Render one cell for CSV export, applying Stata's rules: value labels by default, empty for
/// missing, date/time formats rendered (like Stata), other numerics at full precision with the
/// leading zero dropped. (Stata renders dates but not ordinary numeric formats such as %8.0gc.)
fn csv_cell(ds: &Dataset, col: usize, row: usize, nolabel: bool, rawdates: bool, delim: &str) -> String {
    let var = &ds.variables[col];
    let raw = match &ds.columns[col][row] {
        Value::Text(s) => s.clone(),
        Value::Binary(_) => String::new(),
        Value::Alias => String::new(),
        Value::Num(x) => {
            let t = &var.vtype;
            let date_rendered = if rawdates {
                None
            } else {
                format::parse_date_format(&var.format)
                    .and_then(|(dt, details)| format::render_date(dt, &details, *x))
            };
            if missing::is_missing(t, *x) {
                String::new()
            } else if !nolabel && var.has_value_label() {
                match ds
                    .value_label(&var.value_label_name)
                    .and_then(|vl| vl.get(*x as i32))
                {
                    Some(txt) => txt.clone(),
                    None => format::drop_leading_zero(&format::number(*x, t)),
                }
            } else if let Some(date) = date_rendered {
                date
            } else {
                format::drop_leading_zero(&format::number(*x, t))
            }
        }
    };
    csv_field(&raw, delim)
}

/// Quote a CSV field if it contains the delimiter, a quote, or a newline (RFC-4180 style).
fn csv_field(s: &str, delim: &str) -> String {
    let needs_quote = s.contains('"')
        || s.contains('\n')
        || s.contains('\r')
        || (!delim.is_empty() && s.contains(delim));
    if needs_quote {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
