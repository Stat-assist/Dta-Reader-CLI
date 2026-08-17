//! searchlight_cli: read Stata .dta files and run a subset of Stata inspection commands,
//! emitting JSON/JSONL. No Stata installation required. See README.md for the output contract.

use searchlight_cli::{commands, export, json::Json, parser, reader};

use parser::CliOutcome;
use std::process::ExitCode;

const HELP: &str = "\
searchlight_cli - read Stata .dta files and run Stata commands, or export to other formats.

USAGE:
    searchlight_cli <file.dta> -c \"<command>\" [-c \"<command>\" ...] [options]
    searchlight_cli <file.dta> -e <format> [-f <output-path>]

OPTIONS (commands):
    -c, --command <CMD>   A Stata command to run (repeatable; runs in order on the dataset).
        --compact         Emit single-line (unindented) JSON instead of pretty JSON.
        --jsonl           For `list`, stream one JSON object per observation (implies --compact).
        --nolabel         Emit numeric codes instead of value-label text.
        --rawdates        Emit raw numeric values for date-formatted variables (no date rendering).

OPTIONS (export, -e is independent of -c; do not combine them in one invocation):
    -e, --export <FMT>    Export format: sqlite, duckdb, parquet, or mongodb.
    -f, --output <PATH>   Output path (default: same dir/name as input file).
                          sqlite/duckdb/parquet: a file path. mongodb: a DIRECTORY path (a dump
                          folder is written, not a single file).

    All four formats are fully self-contained: no external database, server, or CLI tool is
    needed (SQLite and DuckDB's engines, including DuckDB's native Parquet writer, are compiled
    directly into this binary; MongoDB's BSON dump format is written directly, no mongod needed).

OPTIONS (general):
    -h, --help              Show this help.
    -V, --version           Show version.
        --licenses          Print third-party license attributions for every bundled dependency.

SUPPORTED COMMANDS:
    describe [, fullnames]   list      summarize [, detail]   tabulate [var2]   inspect   count
    ds [, has(type T)]       lookfor   order                  label list  notes list
    misstable summarize      assert    export delimited [using <file>] [, replace nolabel]

    tabulate takes one variable (one-way frequency table) or two (two-way cross-tab); see README.

EXAMPLES:
    searchlight_cli \"C:\\data\\auto.dta\" -c \"list make price in 1/5\"
    searchlight_cli \"C:\\data\\auto.dta\" -c \"tabulate rep78 foreign\"
    searchlight_cli \"C:\\data\\auto.dta\" -e sqlite -f \"C:\\data\\auto.sqlite\"
    searchlight_cli \"C:\\data\\auto.dta\" -e parquet -f \"C:\\data\\auto.parquet\"
    searchlight_cli \"C:\\data\\auto.dta\" -e mongodb -f \"C:\\data\\auto_dump\"
";

/// Embedded at compile time so license attributions travel with the binary itself — not just the
/// source repo — satisfying bundled dependencies' (SQLite, DuckDB, bson, ...) notice requirements
/// even when only the compiled executable is distributed.
const THIRD_PARTY_LICENSES: &str = include_str!("../THIRD_PARTY_LICENSES.md");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parser::parse_cli(&args) {
        Ok(CliOutcome::Help) => {
            print!("{}", HELP);
            ExitCode::SUCCESS
        }
        Ok(CliOutcome::Version) => {
            println!("searchlight_cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(CliOutcome::Licenses) => {
            print!("{}", THIRD_PARTY_LICENSES);
            ExitCode::SUCCESS
        }
        Ok(CliOutcome::Run(cli)) => run(cli),
        Err(e) => fail(&e),
    }
}

fn run(cli: parser::CliArgs) -> ExitCode {
    let bytes = match std::fs::read(&cli.path) {
        Ok(b) => b,
        Err(e) => return fail(&format!("could not read {}: {}", cli.path, e)),
    };
    let mut dataset = match reader::read_dta(&bytes, &cli.path) {
        Ok(d) => d,
        Err(e) => return fail(&format!("could not parse {}: {}", cli.path, e)),
    };

    // Handle export mode
    if let Some(fmt) = &cli.export_format {
        match export::export(&dataset, fmt, cli.export_output.as_deref()) {
            Ok(msg) => {
                println!("Success: {}", msg);
                return ExitCode::SUCCESS;
            }
            Err(e) => return fail(&format!("export failed: {}", e)),
        }
    }

    // Command mode
    if cli.commands.is_empty() {
        return fail("no command given; use -c \"<command>\" or -e <format> (see --help)");
    }

    let out_opts = commands::OutputOpts {
        nolabel: cli.opts.nolabel,
        rawdates: cli.opts.rawdates,
    };

    let mut exit = ExitCode::SUCCESS;
    for command in &cli.commands {
        let parsed = match parser::parse_command(command) {
            Ok(c) => c,
            Err(e) => return fail(&format!("in command '{}': {}", command, e)),
        };
        match commands::execute(&parsed, &mut dataset, &out_opts) {
            Ok(outcome) => {
                emit(&outcome, &cli.opts);
                if outcome.exit_code != 0 {
                    exit = ExitCode::from(outcome.exit_code);
                }
            }
            Err(e) => return fail(&format!("in command '{}': {}", command, e)),
        }
    }
    exit
}

fn emit(outcome: &commands::Outcome, opts: &parser::OutputOpts) {
    if opts.jsonl {
        if let Some(rows) = &outcome.jsonl_rows {
            for row in rows {
                println!("{}", row.to_string(false));
            }
            return;
        }
        // Non-row command under --jsonl: emit the envelope as one compact line.
        println!("{}", outcome.value.to_string(false));
        return;
    }
    println!("{}", outcome.value.to_string(opts.pretty));
}

fn fail(message: &str) -> ExitCode {
    let err = Json::Object(vec![("error".into(), Json::str(message))]);
    eprintln!("{}", err.to_string(true));
    ExitCode::from(2)
}
