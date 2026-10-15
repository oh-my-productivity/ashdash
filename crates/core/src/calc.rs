//! A small arithmetic expression evaluator (the reference launcher's calculator feature).

use std::f64::consts::{E, PI, TAU};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.' || cs[i] == '_') {
                i += 1;
            }
            // exponent such as 1e10 / 2.5e-3
            if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') {
                let mut j = i + 1;
                if j < cs.len() && (cs[j] == '+' || cs[j] == '-') {
                    j += 1;
                }
                if j < cs.len() && cs[j].is_ascii_digit() {
                    while j < cs.len() && cs[j].is_ascii_digit() {
                        j += 1;
                    }
                    i = j;
                }
            }
            let text: String = cs[start..i].iter().filter(|c| **c != '_').collect();
            out.push(Tok::Num(text.parse().ok()?));
        } else if c.is_alphabetic() {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(cs[start..i].iter().collect::<String>().to_lowercase()));
        } else {
            match c {
                '+' | '-' | '*' | '/' | '^' | '%' => out.push(Tok::Op(c)),
                '×' | '·' => out.push(Tok::Op('*')),
                '÷' => out.push(Tok::Op('/')),
                '(' => out.push(Tok::LParen),
                ')' => out.push(Tok::RParen),
                ',' => out.push(Tok::Comma),
                _ => return None,
            }
            i += 1;
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if c == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op(c @ ('*' | '/' | '%'))) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    v = match c {
                        '*' => v * r,
                        '/' => v / r,
                        _ => v % r,
                    };
                }
                // implicit multiplication: 2(3+4), 2pi
                Some(Tok::LParen) | Some(Tok::Ident(_)) => {
                    let r = self.unary()?;
                    v *= r;
                }
                _ => break,
            }
        }
        Some(v)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?; // right associative
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.next()? {
            Tok::Num(n) => Some(n),
            Tok::LParen => {
                let v = self.expr()?;
                match self.next()? {
                    Tok::RParen => Some(v),
                    _ => None,
                }
            }
            Tok::Ident(name) => {
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if let Some(Tok::RParen) = self.peek() {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.expr()?);
                            match self.next()? {
                                Tok::Comma => continue,
                                Tok::RParen => break,
                                _ => return None,
                            }
                        }
                    }
                    call(&name, &args)
                } else {
                    match name.as_str() {
                        "pi" => Some(PI),
                        "e" => Some(E),
                        "tau" => Some(TAU),
                        _ => None,
                    }
                }
            }
            _ => None,
        }
    }
}

fn call(name: &str, a: &[f64]) -> Option<f64> {
    let one = |f: fn(f64) -> f64| (a.len() == 1).then(|| f(a[0]));
    match name {
        "sqrt" => one(f64::sqrt),
        "cbrt" => one(f64::cbrt),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "ln" => one(f64::ln),
        "log" | "log10" => one(f64::log10),
        "log2" => one(f64::log2),
        "exp" => one(f64::exp),
        "abs" => one(f64::abs),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "round" => one(f64::round),
        "pow" => (a.len() == 2).then(|| a[0].powf(a[1])),
        "min" => a.iter().copied().reduce(f64::min),
        "max" => a.iter().copied().reduce(f64::max),
        _ => None,
    }
}

/// Evaluate an arithmetic expression.
pub fn eval(s: &str) -> Option<f64> {
    let toks = lex(s)?;
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0 };
    let v = p.expr()?;
    (p.pos == p.toks.len() && v.is_finite()).then_some(v)
}

/// True when plain text should be treated as a calculation without an
/// explicit `=` prefix: it evaluates, contains a digit, and contains an
/// operator or call (so that a bare number or word never hijacks results).
pub fn looks_like_expression(s: &str) -> bool {
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_op = s.chars().any(|c| "+-*/^%()×÷".contains(c));
    has_digit && has_op && eval(s).is_some()
}

pub fn format(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.len() > 20 || (v != 0.0 && v.abs() < 1e-9) {
        return format!("{:e}", v);
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        assert_eq!(eval("2+3*4"), Some(14.0));
        assert_eq!(eval("(2+3)*4"), Some(20.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("-2^2"), Some(-4.0));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(eval("sqrt(16)"), Some(4.0));
        assert!((eval("2pi").unwrap() - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(eval("max(1,5,3)"), Some(5.0));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(eval("2+"), None);
        assert_eq!(eval("safari"), None);
        assert_eq!(eval("1/0"), None);
    }

    #[test]
    fn expression_heuristic() {
        assert!(looks_like_expression("12*3"));
        assert!(!looks_like_expression("12"));
        assert!(!looks_like_expression("pi"));
    }

    #[test]
    fn formatting() {
        assert_eq!(format(36.0), "36");
        assert_eq!(format(0.1 + 0.2), "0.3");
        assert_eq!(format(1.0 / 3.0), "0.3333333333");
    }
}
