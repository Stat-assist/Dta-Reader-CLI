//! Parses the CLI invocation and the individual Stata command strings passed via `-c`.
//!
//! A Stata command follows the general shape:
//!   `command [varlist] [if exp] [in range] [using filename] [, options]`
//! We split off options at the first top-level comma, recognize the (possibly two-word) command
//! name, then peel off `if` / `in` / `using` and treat the remaining leading tokens as the varlist.

use crate::model::Dataset;

//  CLI-level arguments 

#[derive(Debug, Clone)]
pub struct OutputOpts {
    /// Pretty-print JSON (default). `--compact` turns this off.
    pub pretty: bool,
    /// Stream row-oriented output (list) as one JSON object per line.
    pub jsonl: bool,
    /// Emit numeric codes instead of value-label text.
    pub nolabel: bool,
    /// Emit raw numeric values for date-formatted variables instead of rendered dates.
    pub rawdates: bool,
}

impl Default for OutputOpts {
    fn default() -> Self {
        OutputOpts {
            pretty: true,
            jsonl: false,
            nolabel: false,
            rawdates: false,
        }
    }
}

pub struct CliArgs {
    pub path: String,
    pub commands: Vec<String>,
    pub export_format: Option<String>,
    pub export_output: Option<String>,
    pub opts: OutputOpts,
}

pub enum CliOutcome {
    Run(CliArgs),
    Help,
    Version,
    Licenses,
}

pub fn parse_cli(args: &[String]) -> Result<CliOutcome, String> {
    let mut path: Option<String> = None;
    let mut commands = Vec::new();
    let mut export_format: Option<String> = None;
    let mut export_output: Option<String> = None;
    let mut opts = OutputOpts::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => return Ok(CliOutcome::Help),
            "-V" | "--version" => return Ok(CliOutcome::Version),
            "--licenses" | "--third-party-licenses" => return Ok(CliOutcome::Licenses),
            "-c" | "--command" => {
                i += 1;
                let cmd = args
                    .get(i)
                    .ok_or_else(|| "-c requires a command argument".to_string())?;
                commands.push(cmd.clone());
            }
            "-e" | "--export" => {
                i += 1;
                let fmt = args
                    .get(i)
                    .ok_or_else(|| "-e requires a format (mongodb|duckdb|parquet|sqlite)".to_string())?;
                export_format = Some(fmt.clone());
            }
            "-f" | "--output" => {
                i += 1;
                let out = args
                    .get(i)
                    .ok_or_else(|| "-f requires an output path".to_string())?;
                export_output = Some(out.clone());
            }
            "--compact" => opts.pretty = false,
            "--jsonl" => {
                opts.jsonl = true;
                opts.pretty = false;
            }
            "--nolabel" => opts.nolabel = true,
            "--rawdates" => opts.rawdates = true,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option '{}'", other));
            }
            other => {
                if path.is_none() {
                    path = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument '{}'", other));
                }
            }
        }
        i += 1;
    }
    let path = path.ok_or_else(|| "no .dta file given".to_string())?;
    Ok(CliOutcome::Run(CliArgs {
        path,
        commands,
        export_format,
        export_output,
        opts,
    }))
}

//  Stata command parsing 

#[derive(Debug)]
pub struct Cmd {
    /// Canonical command name, e.g. "describe", "label list", "export delimited".
    pub name: String,
    /// Raw varlist tokens (names, wildcards, ranges) before if/in/using.
    pub varlist_tokens: Vec<String>,
    pub if_expr: Option<String>,
    pub in_spec: Option<String>,
    pub using: Option<String>,
    pub options: Vec<(String, Option<String>)>,
}

impl Cmd {
    pub fn has_option(&self, name: &str) -> bool {
        self.options.iter().any(|(k, _)| k == name)
    }

    pub fn option_value(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// Split a command line into (main, options) at the first top-level comma (not inside parens or
/// quotes), then parse each part.
pub fn parse_command(line: &str) -> Result<Cmd, String> {
    let (main, options_str) = split_options(line);
    let tokens = tokenize_respecting_quotes(&main);
    if tokens.is_empty() {
        return Err("empty command".to_string());
    }

    let (name, rest) = canonical_name(&tokens)?;

    // Peel off if / in / using, leaving the varlist tokens.
    let mut varlist_tokens = Vec::new();
    let mut if_expr: Option<String> = None;
    let mut in_spec: Option<String> = None;
    let mut using: Option<String> = None;
    let mut j = 0;
    while j < rest.len() {
        match rest[j].as_str() {
            "if" => {
                let expr = rest[j + 1..]
                    .iter()
                    .take_while(|t| t.as_str() != "in" && t.as_str() != "using")
                    .cloned()
                    .collect::<Vec<_>>();
                let consumed = expr.len();
                if_expr = Some(expr.join(" "));
                j += 1 + consumed;
            }
            "in" => {
                in_spec = rest.get(j + 1).cloned();
                j += 2;
            }
            "using" => {
                using = rest.get(j + 1).cloned();
                j += 2;
            }
            _ => {
                varlist_tokens.push(rest[j].clone());
                j += 1;
            }
        }
    }

    Ok(Cmd {
        name,
        varlist_tokens,
        if_expr,
        in_spec,
        using,
        options: parse_options(&options_str),
    })
}

/// Recognize the (possibly two-word) command name and return the remaining tokens.
fn canonical_name(tokens: &[String]) -> Result<(String, Vec<String>), String> {
    let first = tokens[0].to_lowercase();
    let second = tokens.get(1).map(|s| s.to_lowercase());

    // Two-word commands.
    if first == "label" && second.as_deref() == Some("list") {
        return Ok(("label list".to_string(), tokens[2..].to_vec()));
    }
    if first == "notes" {
        // "notes" and "notes list" are equivalent for our read-only listing.
        let rest = if second.as_deref() == Some("list") {
            tokens[2..].to_vec()
        } else {
            tokens[1..].to_vec()
        };
        return Ok(("notes list".to_string(), rest));
    }
    if first == "misstable" {
        // Only "summarize" (default) is supported.
        let rest = if second.as_deref() == Some("summarize") || second.as_deref() == Some("sum") {
            tokens[2..].to_vec()
        } else {
            tokens[1..].to_vec()
        };
        return Ok(("misstable summarize".to_string(), rest));
    }
    if first == "export" {
        if second.as_deref() == Some("delimited") || second.as_deref() == Some("delim") {
            return Ok(("export delimited".to_string(), tokens[2..].to_vec()));
        }
        return Err("only 'export delimited' is supported".to_string());
    }

    // One-word commands, with common abbreviations mapped to canonical names.
    let canonical = match first.as_str() {
        "d" | "des" | "desc" | "descr" | "describe" => "describe",
        "l" | "li" | "lis" | "list" => "list",
        "su" | "sum" | "summ" | "summa" | "summarize" => "summarize",
        "tab" | "ta" | "tabulate" | "tab1" => "tabulate",
        "inspect" | "insp" | "ins" => "inspect",
        "count" | "cou" | "coun" => "count",
        "ds" => "ds",
        "lookfor" => "lookfor",
        "order" | "ord" => "order",
        "assert" | "asse" | "asser" => "assert",
        other => return Err(format!("unsupported command '{}'", other)),
    };
    Ok((canonical.to_string(), tokens[1..].to_vec()))
}

/// Split at the first comma that is not inside parentheses or a quoted string.
fn split_options(line: &str) -> (String, String) {
    let chars: Vec<char> = line.chars().collect();
    let mut depth = 0i32;
    let mut in_quote = false;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => in_quote = !in_quote,
            '(' | '[' if !in_quote => depth += 1,
            ')' | ']' if !in_quote => depth -= 1,
            ',' if !in_quote && depth == 0 => {
                let main: String = chars[..i].iter().collect();
                let opts: String = chars[i + 1..].iter().collect();
                return (main.trim().to_string(), opts.trim().to_string());
            }
            _ => {}
        }
    }
    (line.trim().to_string(), String::new())
}

/// Whitespace tokenizer that keeps `"quoted strings"` (including their spaces) as single tokens.
fn tokenize_respecting_quotes(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has_content = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has_content = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has_content {
                    tokens.push(std::mem::take(&mut cur));
                    has_content = false;
                }
            }
            c => {
                cur.push(c);
                has_content = true;
            }
        }
    }
    if has_content {
        tokens.push(cur);
    }
    tokens
}

/// Parse an option string like `fullnames nolabel has(type int)` into (name, value) pairs.
/// A value in parentheses is captured verbatim (spaces preserved).
fn parse_options(s: &str) -> Vec<(String, Option<String>)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        // Read the option name (up to whitespace or '(').
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '(' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        let mut value = None;
        if i < chars.len() && chars[i] == '(' {
            let mut depth = 0;
            let vstart = i + 1;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let v: String = chars[vstart..i].iter().collect();
            value = Some(v);
            i += 1; // consume ')'
        }
        if !name.is_empty() {
            out.push((name.to_lowercase(), value));
        }
    }
    out
}

//  varlist resolution 

/// Resolve raw varlist tokens to column indices. Empty (or `_all`/`*`) means all variables.
/// Supports exact names, unique-prefix abbreviation, `a-b` ranges, and `*`/`?` wildcards.
pub fn resolve_varlist(tokens: &[String], ds: &Dataset) -> Result<Vec<usize>, String> {
    if tokens.is_empty() {
        return Ok((0..ds.nvars()).collect());
    }
    let mut out: Vec<usize> = Vec::new();
    for tok in tokens {
        if tok == "_all" || tok == "*" {
            out.extend(0..ds.nvars());
            continue;
        }
        if tok.contains('-') && !tok.starts_with('-') {
            // Range a-b (variable names cannot contain '-').
            if let Some((a, b)) = tok.split_once('-') {
                let ia = var_by_name_or_prefix(a, ds)?;
                let ib = var_by_name_or_prefix(b, ds)?;
                let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                out.extend(lo..=hi);
                continue;
            }
        }
        if tok.contains('*') || tok.contains('?') {
            let matched: Vec<usize> = (0..ds.nvars())
                .filter(|&i| glob_match(tok, &ds.variables[i].name))
                .collect();
            if matched.is_empty() {
                return Err(format!("no variables match '{}'", tok));
            }
            out.extend(matched);
            continue;
        }
        out.push(var_by_name_or_prefix(tok, ds)?);
    }
    // De-duplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|&i| seen.insert(i));
    Ok(out)
}

fn var_by_name_or_prefix(name: &str, ds: &Dataset) -> Result<usize, String> {
    if let Some(i) = ds.var_index(name) {
        return Ok(i);
    }
    let matches: Vec<usize> = (0..ds.nvars())
        .filter(|&i| ds.variables[i].name.starts_with(name))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!("variable {} not found", name)),
        _ => Err(format!("variable abbreviation {} is ambiguous", name)),
    }
}

/// Simple glob matching supporting `*` (any run) and `?` (single char).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => m(&p[1..], t) || (!t.is_empty() && m(p, &t[1..])),
            '?' => !t.is_empty() && m(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
        }
    }
    m(&p, &t)
}
