//! Restricted integer expressions from the upstream recipe data. This parser
//! has no function calls, arbitrary attributes, or access to process state.

use super::Environment;
use crate::{Error, Result};
use serde_json::{json, Value};

pub(super) fn number(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_bool().map(i64::from))
        .ok_or_else(|| Error::Invalid(format!("expected integer, got {value}")))
}

pub(super) fn compare(op: &str, left: &Value, right: &Value) -> Result<bool> {
    if op == "==" {
        return Ok(left == right
            || number(left)
                .ok()
                .zip(number(right).ok())
                .is_some_and(|(a, b)| a == b));
    }
    if op == "!=" {
        return Ok(!compare("==", left, right)?);
    }
    let (left, right) = (number(left)?, number(right)?);
    match op {
        "<" => Ok(left < right),
        ">" => Ok(left > right),
        "<=" => Ok(left <= right),
        ">=" => Ok(left >= right),
        _ => Err(Error::Invalid(format!("unsupported comparison {op}"))),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(i64),
    Name(String),
    Op(String),
    End,
}

fn tokenize(source: &str) -> Result<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let text = &source[start..i];
            let value =
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    i64::from_str_radix(hex, 16)
                } else {
                    text.parse()
                }
                .map_err(|_| Error::Invalid(format!("invalid number {text}")))?;
            tokens.push(Token::Number(value));
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || b"_.".contains(&bytes[i]))
            {
                i += 1;
            }
            tokens.push(Token::Name(source[start..i].into()));
        } else if b"()+-*/%<>=!&|^~[]".contains(&bytes[i]) {
            i += 1;
            if i < bytes.len()
                && bytes[i].is_ascii()
                && ["<<", ">>", "<=", ">=", "==", "!=", "//"].contains(&&source[start..i + 1])
            {
                i += 1;
            }
            tokens.push(Token::Op(source[start..i].into()));
        } else {
            return Err(Error::Invalid(format!("unsupported expression: {source}")));
        }
    }
    tokens.push(Token::End);
    Ok(tokens)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    cursor: usize,
    env: &'a Environment,
}

impl Parser<'_> {
    fn pop(&mut self) -> Token {
        let token = self.tokens[self.cursor].clone();
        self.cursor += 1;
        token
    }
    fn expect(&mut self, expected: &str) -> Result<()> {
        if self.tokens[self.cursor] != Token::Op(expected.into()) {
            return Err(Error::Invalid(format!("expected {expected} in expression")));
        }
        self.cursor += 1;
        Ok(())
    }

    fn expression(&mut self, minimum: u8) -> Result<Value> {
        let mut left = match self.pop() {
            Token::Number(n) => json!(n),
            Token::Name(name) => {
                if name == "buf" || name == "response.arguments" {
                    if self.tokens[self.cursor] == Token::Op("[".into()) {
                        self.cursor += 1;
                        let index = number(&self.expression(0)?)?;
                        self.expect("]")?;
                        if index < 0 {
                            return Err(Error::Invalid("negative recipe offset".into()));
                        }
                        json!(if name == "buf" {
                            self.env.buf_byte(index as usize)
                        } else {
                            self.env.response_arg(index as usize)
                        })
                    } else if name == "response.arguments" {
                        json!(self
                            .env
                            .response
                            .as_ref()
                            .map(|r| r.arguments.to_vec())
                            .unwrap_or_default())
                    } else {
                        return Err(Error::Invalid("unindexed recipe buffer".into()));
                    }
                } else if name == "count" {
                    json!(self.env.buf.len())
                } else {
                    self.env
                        .vars
                        .get(&name)
                        .cloned()
                        .ok_or_else(|| Error::Invalid(format!("unbound variable: {name}")))?
                }
            }
            Token::Op(op) if op == "(" => {
                let value = self.expression(0)?;
                self.expect(")")?;
                value
            }
            Token::Op(op) if ["!", "-", "~", "+"].contains(&op.as_str()) => {
                let value = number(&self.expression(9)?)?;
                json!(match op.as_str() {
                    "!" => i64::from(value == 0),
                    "~" => !value,
                    "+" => value,
                    _ => value
                        .checked_neg()
                        .ok_or_else(|| Error::Invalid("integer overflow".into()))?,
                })
            }
            _ => return Err(Error::Invalid("unsupported expression".into())),
        };
        while let Token::Op(op) = &self.tokens[self.cursor] {
            let precedence = match op.as_str() {
                "==" | "!=" | "<" | ">" | "<=" | ">=" => 1,
                "|" => 2,
                "^" => 3,
                "&" => 4,
                "<<" | ">>" => 5,
                "+" | "-" => 6,
                "*" | "/" | "//" | "%" => 7,
                _ => break,
            };
            if precedence < minimum {
                break;
            }
            let op = op.clone();
            self.cursor += 1;
            let right = self.expression(precedence + 1)?;
            if precedence == 1 {
                left = json!(compare(&op, &left, &right)?);
                continue;
            }
            let (a, b) = (number(&left)?, number(&right)?);
            let value = match op.as_str() {
                "+" => a.checked_add(b),
                "-" => a.checked_sub(b),
                "*" => a.checked_mul(b),
                "/" | "//" => a.checked_div(b).map(|q| {
                    if a % b != 0 && (a < 0) != (b < 0) {
                        q - 1
                    } else {
                        q
                    }
                }),
                "%" => a.checked_rem(b).map(|r| {
                    if r != 0 && (r < 0) != (b < 0) {
                        r + b
                    } else {
                        r
                    }
                }),
                "<<" => u32::try_from(b).ok().and_then(|b| a.checked_shl(b)),
                ">>" => u32::try_from(b).ok().and_then(|b| a.checked_shr(b)),
                "&" => Some(a & b),
                "|" => Some(a | b),
                "^" => Some(a ^ b),
                _ => None,
            }
            .ok_or_else(|| Error::Invalid(format!("invalid arithmetic {a} {op} {b}")))?;
            left = json!(value);
        }
        Ok(left)
    }
}

pub(super) fn raw(source: &str, env: &Environment) -> Result<Value> {
    let source = source.trim();
    if source.starts_with('"') {
        return Ok(serde_json::from_str::<Value>(source)?);
    }
    // These two forms represent pointer arithmetic, not numeric addition.
    if let Some((base, offset)) = source.split_once('+') {
        if let Ok(offset) = offset.trim().parse::<usize>() {
            let data = match base.trim() {
                "buf" => Some(env.buf.as_slice()),
                "response.arguments" => Some(
                    env.response
                        .as_ref()
                        .map(|r| r.arguments.as_slice())
                        .unwrap_or(&[]),
                ),
                _ => None,
            };
            if let Some(data) = data {
                return Ok(json!(data.get(offset..).unwrap_or(&[])));
            }
        }
    }
    let mut parser = Parser {
        tokens: tokenize(source)?,
        cursor: 0,
        env,
    };
    let value = parser.expression(0)?;
    if parser.tokens[parser.cursor] != Token::End {
        return Err(Error::Invalid(format!("unsupported expression: {source}")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arithmetic_and_reads_preserve_precedence() {
        let mut env = Environment::new(vec![7, 8], Default::default());
        env.vars.insert("state".into(), json!(1));
        env.vars.insert("start_col".into(), json!(2));
        env.vars.insert("stop_col".into(), json!(9));
        assert_eq!(
            raw("(buf[0] << 8) | (buf[1] & 0xFF)", &env).unwrap(),
            json!(0x0708)
        );
        assert_eq!(raw("(stop_col - start_col) + 1", &env).unwrap(), json!(8));
        assert_eq!(raw("!state", &env).unwrap(), json!(0));
        assert_eq!(raw("-7 / 3", &env).unwrap(), json!(-3));
        assert_eq!(raw("7 % -3", &env).unwrap(), json!(-2));
        assert_eq!(raw("buf + 1", &env).unwrap(), json!([8]));
    }
    #[test]
    fn unsupported_or_malformed_expressions_are_errors() {
        let env = Environment::default();
        for source in [
            "__import__(\"os\").system(\"echo hi\")",
            "dev_get_drvdata(dev)",
            "missing",
            "1 / 0",
            "3 << -1",
            "(",
            "1 +",
            "",
            "3 *** 2",
            "+中",
        ] {
            assert!(raw(source, &env).is_err(), "{source}");
        }
    }
}
