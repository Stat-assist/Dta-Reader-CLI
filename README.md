# searchlight_cli

A command-line tool that reads Stata `.dta` files and either (1) runs a subset of Stata's
inspection commands, emitting **JSON** (or **JSONL**) instead of Stata's text tables, or
(2) exports the whole file into SQLite, DuckDB, Parquet, or MongoDB's dump format.

The point is to let *other programs* read Stata data and extract it — using familiar Stata command
syntax — **without needing a Stata license or installation**. Call it from Python (or any language)
with `subprocess`, parse the JSON, done. Or hand it straight to whatever database you're already
using via `-e`.

It is the CLI companion to [Searchlight](https://github.com/YOUR-USERNAME/Searchlight); the `.dta`
binary-decoding logic is a
direct Rust port of that project's Java `core` engine, and covers `.dta` format versions **117–121**
(Stata 13 through 19), both byte orders.

---

## Why JSON instead of Stata's tables?

Stata prints human-readable ASCII boxes. Those are painful to parse programmatically. This tool
emits the *same critical information* as structured JSON, so a consumer never has to scrape columns
or box-drawing characters. For the row-oriented `list` command there is also a JSONL mode (one JSON
object per observation) that streams cleanly for large datasets.

---

## Building

Requires a Rust toolchain (1.85+) **and a C/C++ compiler** (needed to build the bundled SQLite and
DuckDB engines). On Windows this means the MSVC toolchain (`rustup default
stable-x86_64-pc-windows-msvc`, plus the Visual Studio Build Tools' C++ workload); on macOS, Xcode
Command Line Tools; on Linux, `gcc`/`clang` (usually already present).

```sh
cargo build --release
```

The binary is written to `target/release/searchlight_cli` (`.exe` on Windows). It is fully
self-contained: no external database, server, or CLI tool is needed at runtime for any `-e` export
format — SQLite and DuckDB's own C/C++ engines (DuckDB's build includes its native Parquet
extension) are compiled directly into the binary, the same technique used by all `bundled`-feature
Rust database wrappers.

**Dependencies:** the core reader and `-c` commands are zero-dependency (std only). The `-e` export
flag links `rusqlite` (bundled SQLite), `duckdb` (bundled DuckDB engine, including its Parquet
writer), and `bson` (pure Rust, for the MongoDB dump format) — see
[Database export](#database-export--e) below, and [Third-party licenses](#third-party-licenses) for
how their licenses are attributed.

> **Windows antivirus note:** some AV products quarantine freshly-compiled, unsigned executables —
> and, with these dependencies, the intermediate `build-script-build.exe` files cargo generates
> mid-build — on first run. If a build fails with "Access is denied," or a freshly built exe won't
> launch, unblock the flagged file in your AV and re-run (`cargo build` picks up right where it left
> off). Adding the `target/` directory (and, if installing dev tools like `cargo-about`, a
> `--target-dir` pointed elsewhere than the system temp dir) to your AV's exclusions avoids this
> entirely.

---

## Usage

```
searchlight_cli <file.dta> -c "<command>" [-c "<command>" ...] [options]
```

- The first positional argument is the path to the `.dta` file. **Windows and Unix paths both
  work** (use quotes if the path contains spaces).
- `-c` / `--command` supplies a Stata command. It is **repeatable**; commands run in order against
  the same in-memory dataset (so `order` can precede a `list`, etc.).

### Options

| Option | Effect |
|---|---|
| `-c, --command <CMD>` | A command to run (repeatable). |
| `--compact` | Emit single-line JSON instead of pretty-printed. |
| `--jsonl` | For `list`, stream one JSON object per observation (implies `--compact`). |
| `--nolabel` | Emit numeric codes instead of value-label text. |
| `--rawdates` | Emit raw numeric values for date variables instead of rendered dates. |
| `-h, --help` | Show help. |
| `-V, --version` | Show version. |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | An `assert` command's condition was false for at least one observation (mirrors Stata's `r(9)`). |
| `2` | Usage error, unreadable/unparseable file, or a command error. Details are printed as a JSON `{"error": ...}` on **stderr**. |

### Example

```sh
searchlight_cli "C:\Program Files\Stata19\auto.dta" -c "list make price in 1/5"
```

```json
{
  "command": "list",
  "variables": ["make", "price"],
  "observations": 5,
  "data": [
    { "_n": 1, "make": "AMC Concord", "price": 4099 },
    { "_n": 2, "make": "AMC Pacer",   "price": 4749 },
    { "_n": 3, "make": "AMC Spirit",  "price": 3799 },
    { "_n": 4, "make": "Buick Century","price": 4816 },
    { "_n": 5, "make": "Buick Electra","price": 7827 }
  ]
}
```

---

## Database export (`-e`)

Separate from `-c` commands, `-e <format>` converts the whole `.dta` file into another database
format in one shot:

```
searchlight_cli <file.dta> -e <sqlite|duckdb|mongodb|parquet> [-f <output-path>]
```

`-e` and `-c` are independent modes — don't combine them in one invocation. On success, prints a
plain-text confirmation line (`Success: Exported N observations to <path>...`) and exits `0`. On
failure, prints a JSON `{"error": ...}` to stderr and exits `2`, same as command mode.

If `-f` is omitted, the output is written next to the input `.dta` file, with the same base name
(and the format's extension, for the three formats that produce a single file).

Every export writes **raw storage values**, not value labels — the exported table has the same
numbers you'd see in Stata's Data Editor, not the label text `list` shows by default. (Use
`-c "label list"` separately to recover the label definitions if needed.) Missing values become
SQL `NULL` / BSON `null` in every format. `float`/`double` cells are stored at Stata's own faithful
precision (see **Value encoding rules** above) rather than the raw IEEE64 widening of a 32-bit
float, so e.g. a `float` holding `3.58` is stored as exactly `3.5799999`, matching what Stata itself
would show — not as `3.5799999237060547`.

| Format | `-f` expects | External tool required? |
|---|---|---|
| `sqlite` | a file path | No — SQLite's engine is compiled directly into this binary. |
| `duckdb` | a file path | No — DuckDB's engine is compiled directly into this binary. |
| `parquet` | a file path | No — written by DuckDB's own bundled Parquet writer (see below). |
| `mongodb` | a **directory** path | No — the BSON dump format is written directly; no `mongod` needed. |

All four formats are produced entirely in-process, with no external database, server, or CLI tool
required at runtime, and no text-format round-trip of any value (every cell goes from the parsed
`.dta` value straight into a bound SQL parameter or a BSON field).

### `sqlite`

The `rusqlite` crate's `bundled` feature compiles SQLite's own C source directly into this binary
(a C/C++ compiler is required at *build* time for this, not at run time — see **Building** above).
Writes a single table named `data` with one column per Stata variable, typed `TEXT` / `INTEGER` /
`REAL` to match the variable's storage type, via parameterized `INSERT` statements.

```sh
searchlight_cli auto.dta -e sqlite -f auto.sqlite
```

### `duckdb`

The `duckdb` crate's `bundled` feature compiles DuckDB's own C++ engine directly into this binary —
the same technique as `sqlite` above, just for DuckDB. No external `duckdb` binary, server, or PATH
entry is needed. Writes a single table named `data`, populated the same way as the `sqlite` export
(parameterized `INSERT`s, not a CSV or text intermediate).

```sh
searchlight_cli auto.dta -e duckdb -f auto.duckdb
```

### `mongodb`

**No MongoDB installation is required to produce this export.** `mongodump` (the obvious tool for
"export a database to BSON") only works against an already-*running* `mongod` server — it cannot
write a dump from arbitrary data on its own, and standing up a temporary local server just to dump
through it would be a heavy, fragile dependency for a CLI export flag. Instead, searchlight_cli
**writes the BSON dump format directly**, byte-for-byte matching what `mongodump` itself produces:

```
<output-dir>/
  <name>.bson            raw BSON documents, concatenated (one per observation)
  <name>.metadata.json    {"options": {}, "indexes": [{"v":2,"key":{"_id":1},"name":"_id_"}]}
```

`<name>` is the `.dta` file's base name (e.g. `auto.dta` → `auto.bson`). This folder is a valid
`mongorestore` source — no server is needed to *create* it, only to actually load it:

```sh
searchlight_cli auto.dta -e mongodb -f auto_dump
mongorestore --db=mydb --collection=auto "auto_dump/auto.bson"
```

Verified against the real `bsondump` and `mongorestore` tools from MongoDB's Database Tools
package: `bsondump` parses every document with correct types and correct missing-value handling;
`mongorestore` accepts the file and metadata and proceeds straight to (and only fails at) the
network connection step when no `mongod` is reachable — i.e. it never rejects the file itself.

**Known limitation:** a `strL` cell holding *binary* content (an embedded NUL byte — images, raw
byte blobs, etc., as opposed to ordinary text) exports as `null`. The `.dta` reader currently
records only the byte length of such blobs, not their content, since no other command
(`list`, `export delimited`, ...) needed to display binary content either. Text strLs export
normally.

### `parquet`

Also uses the bundled DuckDB engine: the data is staged into an in-memory DuckDB table (via the
same parameterized inserts as the `duckdb` export above), then DuckDB's own native Parquet writer —
compiled directly into the binary via the crate's `parquet` feature — flushes it with
`COPY data TO '<path>' (FORMAT PARQUET)`. No `arrow`/`parquet` Rust crate is linked at all; DuckDB's
C++ engine handles the Parquet format itself, the same as when you use DuckDB's own CLI.

```sh
searchlight_cli auto.dta -e parquet -f auto.parquet
```

Verified against DuckDB's own `read_parquet()`: correct row count, correct schema, faithful
precision, and proper `NULL` for missing values.

---

## Calling from Python (the primary use case)

```python
import json, subprocess

def stata(dta_path, command, *flags):
    result = subprocess.run(
        ["searchlight_cli", dta_path, *flags, "-c", command],
        capture_output=True, text=True,
    )
    if result.returncode == 2:
        raise RuntimeError(json.loads(result.stderr)["error"])
    return result.stdout

# Extract data as a list of dicts:
out = json.loads(stata("auto.dta", "list"))
rows = out["data"]                       # [{'_n':1,'make':'AMC Concord',...}, ...]

# Stream large data via JSONL (one object per line):
text = stata("big.dta", "list", "--jsonl")
records = [json.loads(line) for line in text.splitlines()]

# Get summary statistics:
summ = json.loads(stata("auto.dta", "summarize price mpg"))
for v in summ["variables"]:
    print(v["variable"], v["mean"], v["sd"])
```

Loading straight into pandas:

```python
import pandas as pd, json
df = pd.DataFrame(json.loads(stata("auto.dta", "list"))["data"]).drop(columns="_n")
```

---

## Value encoding rules

How a cell value becomes JSON (used by `list`; the CSV rules for `export delimited` are analogous
and match Stata byte-for-byte):

| Cell | JSON |
|---|---|
| String (`str#` / `strL`) | JSON string |
| Numeric, missing (`.`, `.a`–`.z`) | `null` |
| Numeric with a value label (default) | the label **string** (use `--nolabel` for the code) |
| Numeric with a date/time format (default) | the **rendered date** string, e.g. `"02jan2001"`, `"1967q1"` (use `--rawdates` for the number) |
| Numeric, plain | a JSON **number** at faithful precision |
| `strL` binary blob / alias variable | `null` |

**Numeric precision** matches Stata's own `export delimited`: integer storage types are exact;
`float` uses 8 significant digits (so `3.58` stored as a float prints `3.5799999`, exactly as Stata
does); `double` uses 16. JSON numbers keep the leading zero (`0.5`); the CSV export drops it (`.5`)
to match Stata.

---

## Supported commands

Each command produces one JSON object (the "envelope") with a `"command"` field plus the fields
below. Commands accept the usual Stata qualifiers where meaningful: `[varlist]`, `if <exp>`,
`in <range>`, and `, options`.

### `describe [varlist] [, fullnames]`
Dataset metadata and per-variable info.
```json
{ "command":"describe", "file":"...", "label":"1978 automobile data",
  "observations":74, "variables":12, "timestamp":"13 Apr 2024 17:45",
  "release":118, "sorted_by":["foreign"], "has_notes":true,
  "varlist":[ {"name":"make","type":"str18","format":"%-18s",
               "value_label":"","variable_label":"Make and model"}, ... ] }
```
(`fullnames` is accepted for compatibility; JSON always uses full names.)

### `list [varlist] [if] [in]`
Observations as data. `data` is an array of row objects; each row has `_n` (the 1-based
observation number) plus one field per variable. With `--jsonl`, the row objects are streamed one
per line and the envelope is omitted.
```json
{ "command":"list", "variables":[...], "observations":5, "data":[ {"_n":1, ...}, ... ] }
```

### `summarize [varlist] [if] [in] [, detail]`
Per-variable statistics (`obs`, `mean`, `sd`, `min`, `max`). With `detail`, adds `variance`,
`skewness`, `kurtosis`, `sum`, and a `percentiles` object (1/5/10/25/50/75/90/95/99). String
variables report `obs:0` and `null` stats, as in Stata. Statistics are full precision.

### `tabulate <var> [if] [in] [, missing nolabel]` (one-way) / `tabulate <var1> <var2> [...]` (two-way)
**One-way** (one variable): a frequency table. Each row: `value` (label applied unless
`--nolabel`), optional `label`, `freq`, `percent`, `cum`.
```json
{ "command":"tabulate", "variable":"foreign",
  "rows":[ {"value":0,"label":"Domestic","freq":52,"percent":70.27,"cum":70.27}, ... ],
  "total":74 }
```
**Two-way** (two variables): a cross-tabulation. `columns` lists the column categories in order;
each entry in `rows` has a `counts` array aligned to that same column order, plus its own row
`total`; `column_totals` aligns the same way.
```json
{ "command":"tabulate", "row_variable":"rep78", "column_variable":"foreign",
  "columns":[ {"value":0,"label":"Domestic"}, {"value":1,"label":"Foreign"} ],
  "rows":[
    {"value":1,"counts":[2,0],"total":2},
    {"value":3,"counts":[27,3],"total":30}
  ],
  "column_totals":[48,21], "total":69 }
```
In both forms, missing is excluded unless `missing` is given (two-way: an observation is dropped
if *either* variable is missing on it, matching Stata). Statistical options (`chi2`, `V`, `gamma`,
row/column/cell percentages, etc.) and `tab2`/`tabi` are not implemented — only the base frequency
cross-tab.

### `inspect [varlist] [if] [in]`
For each numeric variable, the counts Stata's `inspect` reports (the ASCII histogram is omitted):
`negative` / `zero` / `positive` / `total` each split into `{total, integers, nonintegers}`, plus
`missing`, `unique_values`, `min`, `max`.

### `count [if] [in]`
`{ "command":"count", "count": N }`.

### `ds [varlist] [, has(type <t>)]`
Variable names. `has(type int|byte|long|float|double|str#|numeric|string)` filters by storage type.

### `lookfor <term> [term ...]`
Variables whose name or variable label contains any term (case-insensitive). Returns matching
variable metadata.

### `order <varlist> [, last before(var) after(var)]`
Reorders variables in memory (affects later `-c` commands in the same invocation). Default moves
the listed variables to the front. Returns the new full variable order.

### `label list [names]`
Value-label definitions: `[{ "name":"origin", "entries":[{"value":0,"label":"Domestic"}, ...] }]`.

### `notes list`
Dataset and variable notes: `[{ "scope":"_dta", "index":1, "text":"..." }]`.

### `misstable summarize [varlist] [if] [in]`
Lists only variables that have missing values, with `obs_eq_dot` (system missing `.`),
`obs_gt_dot` (extended missing `.a`–`.z`), `obs_lt_dot` (nonmissing), `unique_values`, `min`, `max`.

### `assert <exp> [if] [in]`
`{ "command":"assert", "expression":"...", "passed":true|false, "contradictions":N, "total":N }`.
Sets exit code `1` when the assertion fails. As in Stata, a missing value compares larger than any
number, so `assert rep78 < 6` fails on missing `rep78`.

### `export delimited [varlist] [if] [in] using <file> [, replace nolabel novarnames delimiter(<d>)]`
Writes a CSV file and prints a confirmation object. **The CSV is byte-for-byte identical to Stata's
`export delimited`**: value labels applied by default (`nolabel` to disable), dates rendered,
missing written as empty, LF line endings, RFC-4180 quoting, full numeric precision. Refuses to
overwrite an existing file unless `replace` is given.

---

## `if` expression support

Used by `if` qualifiers and `assert`. Supports:

- variable references, numeric literals, string literals (`"..."`), missing literals (`.`, `.a`–`.z`)
- arithmetic `+ - * /`, relational `< <= > >=`, equality `== !=` (and `~=`), logical `& | && || !`
- parentheses
- functions: `missing()`/`mi()`, `inrange()`, `inlist()`, `abs()`, `int()`, `float()`, `round()`

Missing-value semantics follow Stata: missing is greater than every nonmissing number, and
`. < .a < .b < ... < .z`. So `x < .` selects nonmissing observations.

---

## Fidelity & testing

- **`export delimited` is verified byte-for-byte** against Stata's own output across a suite of
  datasets covering integers, floats, doubles, value labels, and date/time variables.
- `describe`, `list`, `count`, `summarize` (incl. `detail`), `tabulate`, `inspect`, `ds`, `assert`,
  and the others were checked against Stata's output on `auto.dta` and other datasets.
- `-e sqlite` / `-e duckdb` / `-e parquet` are automatically tested by `cargo test`: each export is
  reopened with its own bundled engine and checked for correct row counts, correct column types,
  faithful float precision, and missing values as `NULL`.
- `-e mongodb`'s BSON output is automatically tested by parsing the raw BSON documents back with
  the `bson` crate, and was additionally verified against the real `bsondump` and `mongorestore`
  tools from MongoDB's Database Tools package: `bsondump` parses every document with correct types
  and correct missing-value handling; `mongorestore` accepts the file/metadata and proceeds to (and
  only fails at) the "no server reachable" step — i.e. it never rejects the file itself.
- `cargo test` runs unit tests (number formatting, date rendering, `in`-range parsing, missing-value
  ordering) and integration tests against a bundled `auto.dta` fixture, including all four export
  formats.

### Known scope limits

- `tabulate` supports one-way and two-way (base frequency cross-tab only — no `chi2`/`V`/`gamma`/
  row-col-cell percentages, and no `tab2`/`tabi`).
- `ds` supports `has(type ...)`; other `ds` selection options are not implemented.
- Date rendering covers `%tc %tC %td %tw %tm %tq %th %ty` with the full detail-code language;
  `%tC` is treated as `%tc` (no leap seconds), and business-calendar (`%tb`) / generic (`%tg`)
  formats fall back to the raw number.
- The tool is read-only: it never modifies the input `.dta` file. `order` changes only the
  in-memory view for the current invocation.
- `-e mongodb` exports binary `strL` cell content (as opposed to text) as `null` — see
  [Database export](#database-export--e) for why.

---

## Third-party licenses

This binary statically links several third-party Rust crates, most significantly the bundled
SQLite and DuckDB engines (`rusqlite`, `duckdb`) and `bson`. Their licenses (MIT, BSD, Apache-2.0,
and others) require the license text and copyright notice to be preserved and distributed alongside
the software. [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md) satisfies that: it lists every
dependency in the compiled binary's actual dependency graph, its license, and the full license text.

That file is also **embedded directly into the compiled binary**, so it travels with the executable
even if it's distributed apart from this repository:

```sh
searchlight_cli --licenses
```

It's generated with [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) from `Cargo.lock`
and the project's `about.toml` / `about.hbs`. To regenerate after a dependency change:

```sh
cargo about generate about.hbs -o THIRD_PARTY_LICENSES.md
```

This project's own code carries no license declaration.

