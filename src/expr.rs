//! A small evaluator for Stata `if` expressions and `assert` conditions.
//!
//! Supports: variable references, numeric literals, string literals, the missing literals
//! (`.`, `.a`..`.z`), parentheses, unary `!`/`-`, arithmetic `+ - * /`, relational `< <= > >=`,
//! equality `== !=`, logical `& | && ||`, and the functions `missing()`/`mi()`, `inrange()`,
//! `inlist()`, `abs()`, `int()`, `float()`.
//!
//! Missing-value semantics match Stata: a missing value compares larger than every nonmissing
//! number, and `.` < `.a` < ... < `.z`. Normalizing numeric cells into `Val::Missing` (rather than
//! comparing raw sentinel magnitudes) makes `x < .` mean "x is nonmissing" regardless of storage
//! type.

use crate::model::{missing, Dataset, Value};

#[derive(Debug, Clone)]
pub enum Val {
    Num(f64),
    Missing(i32), // 0 => '.', 1..26 => '.a'..'.z'
    Str(String),
}

impl Val {
    /// Stata truthiness of an `if` expression: true iff a nonzero, nonmissing number.
    fn truthy(&self) -> bool {
        matches!(self, Val::Num(x) if *x != 0.0)
    }
}

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Missing(i32),
    Str(String),
    Var(usize),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// A compiled expression, parsed once and evaluated per observation.
pub struct Condition {
    ast: Expr,
}

impl Condition {
    /// Parse and resolve an expression against a dataset's variables.
    pub fn parse(src: &str, ds: &Dataset) -> Result<Condition, String> {
        let tokens = tokenize(src)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            ds,
        };
        let ast = parser.parse_expr()?;
        if parser.pos != parser.tokens.len() {
            return Err(format!(
                "unexpected token {:?} in expression",
                parser.tokens[parser.pos]
            ));
        }
        Ok(Condition { ast })
    }

    /// Evaluate for a given observation; returns whether the `if` matches.
    pub fn matches(&self, ds: &Dataset, row: usize) -> Result<bool, String> {
        Ok(eval(&self.ast, ds, row)?.truthy())
    }
}

//  tokenizer 

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    Missing(i32),
    Op(String),
    LParen,
    RParen,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated string literal".to_string());
                }
                i += 1; // closing quote
                toks.push(Tok::Str(s));
            }
            '=' | '!' | '<' | '>' | '&' | '|' | '~' => {
                // Two-char operators first.
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if matches!(two.as_str(), "==" | "!=" | "<=" | ">=" | "&&" | "||" | "~=") {
                    toks.push(Tok::Op(normalize_op(&two)));
                    i += 2;
                } else {
                    let one = c.to_string();
                    toks.push(Tok::Op(normalize_op(&one)));
                    i += 1;
                }
            }
            '+' | '-' | '*' | '/' => {
                toks.push(Tok::Op(c.to_string()));
                i += 1;
            }
            '.' => {
                // Missing literal: '.' or '.a'..'.z'. A '.' followed by a digit is a decimal number.
                if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    let (num, adv) = read_number(&chars, i);
                    toks.push(Tok::Num(num));
                    i = adv;
                } else if i + 1 < chars.len() && chars[i + 1].is_ascii_lowercase() {
                    let code = (chars[i + 1] as u8 - b'a' + 1) as i32;
                    toks.push(Tok::Missing(code));
                    i += 2;
                } else {
                    toks.push(Tok::Missing(0));
                    i += 1;
                }
            }
            c if c.is_ascii_digit() => {
                let (num, adv) = read_number(&chars, i);
                toks.push(Tok::Num(num));
                i = adv;
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                toks.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(format!("unexpected character '{}' in expression", other)),
        }
    }
    Ok(toks)
}

fn normalize_op(op: &str) -> String {
    match op {
        "~=" => "!=".to_string(),
        "~" => "!".to_string(),
        "&&" => "&".to_string(),
        "||" => "|".to_string(),
        other => other.to_string(),
    }
}

fn read_number(chars: &[char], start: usize) -> (f64, usize) {
    let mut i = start;
    while i < chars.len()
        && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == 'e' || chars[i] == 'E'
            || ((chars[i] == '+' || chars[i] == '-')
                && i > start
                && (chars[i - 1] == 'e' || chars[i - 1] == 'E')))
    {
        i += 1;
    }
    let s: String = chars[start..i].iter().collect();
    (s.parse().unwrap_or(f64::NAN), i)
}

//  parser (recursive descent, precedence climbing) 

struct Parser<'a> {
    tokens: Vec<Tok>,
    pos: usize,
    ds: &'a Dataset,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.match_op("|") {
            let right = self.parse_and()?;
            left = Expr::Bin(Op::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while self.match_op("&") {
            let right = self.parse_equality()?;
            left = Expr::Bin(Op::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_relational()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "==" => Op::Eq,
                Some(Tok::Op(o)) if o == "!=" => Op::Ne,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_relational()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "<" => Op::Lt,
                Some(Tok::Op(o)) if o == "<=" => Op::Le,
                Some(Tok::Op(o)) if o == ">" => Op::Gt,
                Some(Tok::Op(o)) if o == ">=" => Op::Ge,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_additive()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" => Op::Add,
                Some(Tok::Op(o)) if o == "-" => Op::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_multiplicative()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "*" => Op::Mul,
                Some(Tok::Op(o)) if o == "/" => Op::Div,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.match_op("!") {
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        if self.match_op("-") {
            return Ok(Expr::Neg(Box::new(self.parse_unary()?)));
        }
        if self.match_op("+") {
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(n))
            }
            Some(Tok::Missing(c)) => {
                self.pos += 1;
                Ok(Expr::Missing(c))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(Expr::Str(s))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.parse_expr()?;
                if !matches!(self.peek(), Some(Tok::RParen)) {
                    return Err("expected ')'".to_string());
                }
                self.pos += 1;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.parse_expr()?);
                            if matches!(self.peek(), Some(Tok::Comma)) {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        return Err(format!("expected ')' after arguments to {}()", name));
                    }
                    self.pos += 1;
                    Ok(Expr::Call(name.to_lowercase(), args))
                } else {
                    let idx = self
                        .ds
                        .var_index(&name)
                        .ok_or_else(|| format!("variable {} not found", name))?;
                    Ok(Expr::Var(idx))
                }
            }
            other => Err(format!("unexpected token {:?} in expression", other)),
        }
    }

    fn match_op(&mut self, op: &str) -> bool {
        if let Some(Tok::Op(o)) = self.peek() {
            if o == op {
                self.pos += 1;
                return true;
            }
        }
        false
    }
}

//  evaluation 

fn eval(e: &Expr, ds: &Dataset, row: usize) -> Result<Val, String> {
    match e {
        Expr::Num(n) => Ok(Val::Num(*n)),
        Expr::Missing(c) => Ok(Val::Missing(*c)),
        Expr::Str(s) => Ok(Val::Str(s.clone())),
        Expr::Var(idx) => Ok(cell_val(ds, *idx, row)),
        Expr::Neg(inner) => match eval(inner, ds, row)? {
            Val::Num(n) => Ok(Val::Num(-n)),
            _ => Ok(Val::Missing(0)),
        },
        Expr::Not(inner) => Ok(Val::Num(if eval(inner, ds, row)?.truthy() {
            0.0
        } else {
            1.0
        })),
        Expr::Bin(op, l, r) => eval_bin(*op, eval(l, ds, row)?, eval(r, ds, row)?),
        Expr::Call(name, args) => eval_call(name, args, ds, row),
    }
}

fn cell_val(ds: &Dataset, col: usize, row: usize) -> Val {
    match &ds.columns[col][row] {
        Value::Num(x) => {
            let t = &ds.variables[col].vtype;
            if missing::is_missing(t, *x) {
                Val::Missing(missing::missing_code(t, *x))
            } else {
                Val::Num(*x)
            }
        }
        Value::Text(s) => Val::Str(s.clone()),
        Value::Binary(_) => Val::Str(String::new()),
        Value::Alias => Val::Missing(0),
    }
}

/// Total order used for relational operators. Nonmissing numbers < any missing; `.` < `.a` < ...
fn eval_bin(op: Op, l: Val, r: Val) -> Result<Val, String> {
    let b = |cond: bool| Val::Num(if cond { 1.0 } else { 0.0 });
    match op {
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            if let (Val::Num(a), Val::Num(c)) = (&l, &r) {
                let v = match op {
                    Op::Add => a + c,
                    Op::Sub => a - c,
                    Op::Mul => a * c,
                    Op::Div => {
                        if *c == 0.0 {
                            return Ok(Val::Missing(0)); // Stata: division by zero -> missing
                        }
                        a / c
                    }
                    _ => unreachable!(),
                };
                Ok(Val::Num(v))
            } else {
                Ok(Val::Missing(0)) // arithmetic with missing/string propagates missing
            }
        }
        Op::And => Ok(b(l.truthy() && r.truthy())),
        Op::Or => Ok(b(l.truthy() || r.truthy())),
        Op::Eq => Ok(b(compare(&l, &r) == Some(std::cmp::Ordering::Equal))),
        Op::Ne => Ok(b(compare(&l, &r) != Some(std::cmp::Ordering::Equal))),
        Op::Lt => Ok(b(compare(&l, &r) == Some(std::cmp::Ordering::Less))),
        Op::Le => Ok(b(matches!(
            compare(&l, &r),
            Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal)
        ))),
        Op::Gt => Ok(b(compare(&l, &r) == Some(std::cmp::Ordering::Greater))),
        Op::Ge => Ok(b(matches!(
            compare(&l, &r),
            Some(std::cmp::Ordering::Greater) | Some(std::cmp::Ordering::Equal)
        ))),
    }
}

/// Stata's total order: nonmissing numbers below every missing; among missings `.` < `.a` < ... <
/// `.z`. String-vs-numeric mismatches are unordered (never equal, never less/greater).
fn compare(l: &Val, r: &Val) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (l, r) {
        (Val::Str(a), Val::Str(b)) => Some(a.cmp(b)),
        (Val::Str(_), _) | (_, Val::Str(_)) => None,
        (Val::Num(a), Val::Num(b)) => a.partial_cmp(b),
        (Val::Num(_), Val::Missing(_)) => Some(Ordering::Less),
        (Val::Missing(_), Val::Num(_)) => Some(Ordering::Greater),
        (Val::Missing(a), Val::Missing(b)) => Some(a.cmp(b)),
    }
}

fn eval_call(name: &str, args: &[Expr], ds: &Dataset, row: usize) -> Result<Val, String> {
    let vals: Result<Vec<Val>, String> = args.iter().map(|a| eval(a, ds, row)).collect();
    let vals = vals?;
    match name {
        "missing" | "mi" => {
            let any_missing = vals.iter().any(|v| {
                matches!(v, Val::Missing(_)) || matches!(v, Val::Str(s) if s.is_empty())
            });
            Ok(Val::Num(if any_missing { 1.0 } else { 0.0 }))
        }
        "abs" => num1(&vals, |x| x.abs()),
        "int" => num1(&vals, |x| x.trunc()),
        "float" => num1(&vals, |x| (x as f32) as f64),
        "round" => match vals.as_slice() {
            [Val::Num(x)] => Ok(Val::Num(x.round())),
            [Val::Num(x), Val::Num(u)] if *u != 0.0 => Ok(Val::Num((x / u).round() * u)),
            _ => Ok(Val::Missing(0)),
        },
        "inrange" => match vals.as_slice() {
            [v, lo, hi] => {
                let ge = matches!(
                    compare(v, lo),
                    Some(std::cmp::Ordering::Greater) | Some(std::cmp::Ordering::Equal)
                );
                let le = matches!(
                    compare(v, hi),
                    Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal)
                );
                Ok(Val::Num(if ge && le { 1.0 } else { 0.0 }))
            }
            _ => Err("inrange() takes 3 arguments".to_string()),
        },
        "inlist" => {
            if let Some((first, rest)) = vals.split_first() {
                let found = rest
                    .iter()
                    .any(|v| compare(first, v) == Some(std::cmp::Ordering::Equal));
                Ok(Val::Num(if found { 1.0 } else { 0.0 }))
            } else {
                Err("inlist() needs at least 2 arguments".to_string())
            }
        }
        other => Err(format!("unsupported function {}()", other)),
    }
}

fn num1(vals: &[Val], f: impl Fn(f64) -> f64) -> Result<Val, String> {
    match vals {
        [Val::Num(x)] => Ok(Val::Num(f(*x))),
        [_] => Ok(Val::Missing(0)),
        _ => Err("function takes 1 argument".to_string()),
    }
}

/// Parse an `in` range like `1/5`, `f/10`, `-5/l`, `10` into an inclusive 0-based [start,end].
pub fn parse_in_range(spec: &str, nobs: usize) -> Result<(usize, usize), String> {
    if nobs == 0 {
        return Err("no observations".to_string());
    }
    let resolve = |tok: &str| -> Result<i64, String> {
        let t = tok.trim();
        match t {
            "f" | "F" => Ok(1),
            "l" | "L" => Ok(nobs as i64),
            _ => t
                .parse::<i64>()
                .map_err(|_| format!("invalid observation number '{}'", t)),
        }
    };
    let (a, b) = match spec.split_once('/') {
        Some((lo, hi)) => (resolve(lo)?, resolve(hi)?),
        None => {
            let v = resolve(spec)?;
            (v, v)
        }
    };
    // Negative counts from the end (Stata: -1 is the last observation).
    let norm = |v: i64| -> i64 {
        if v < 0 {
            nobs as i64 + 1 + v
        } else {
            v
        }
    };
    let a = norm(a).max(1);
    let b = norm(b).min(nobs as i64);
    if a > b {
        return Err(format!("invalid range {}", spec));
    }
    Ok(((a - 1) as usize, (b - 1) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_ranges() {
        assert_eq!(parse_in_range("1/5", 74).unwrap(), (0, 4));
        assert_eq!(parse_in_range("f/10", 74).unwrap(), (0, 9));
        assert_eq!(parse_in_range("70/l", 74).unwrap(), (69, 73));
        assert_eq!(parse_in_range("-5/l", 74).unwrap(), (69, 73));
        assert_eq!(parse_in_range("3", 74).unwrap(), (2, 2));
    }

    #[test]
    fn missing_sorts_above_numbers() {
        // Relational comparison: a real number is less than any missing.
        use std::cmp::Ordering;
        assert_eq!(compare(&Val::Num(5.0), &Val::Missing(0)), Some(Ordering::Less));
        assert_eq!(
            compare(&Val::Missing(0), &Val::Missing(1)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare(&Val::Num(3.0), &Val::Num(3.0)),
            Some(Ordering::Equal)
        );
    }
}
