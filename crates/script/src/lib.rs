//! Bounded straight-line JavaScript subset, independent of the OS and DOM.
//! No loops, functions, dynamic evaluation or ambient host capabilities.
#![no_std]
extern crate alloc;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Syntax,
    Unsupported,
    Budget,
    UnknownName,
    MissingElement,
    ReadOnly,
    Type,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
}
impl Value {
    pub fn text(&self) -> String {
        match self {
            Self::Number(n) => n.to_string(),
            Self::Text(s) => s.clone(),
            Self::Bool(b) => b.to_string(),
            Self::Null => "null".into(),
        }
    }
    fn number(&self) -> Result<f64, Error> {
        match self {
            Self::Number(n) => Ok(*n),
            Self::Bool(b) => Ok(u8::from(*b) as f64),
            Self::Null => Ok(0.0),
            Self::Text(s) => s.parse().map_err(|_| Error::Type),
        }
    }
}
#[derive(Clone, Debug)]
pub enum Element {
    Id(String),
    Body,
}
#[derive(Clone, Debug)]
pub enum Target {
    Title,
    Text(Element),
    Style(Element, String),
}
/// The embedder exposes only these typed property reads/writes to the evaluator.
pub trait Host {
    fn read(&mut self, target: &Target) -> Result<Value, Error>;
    fn write(&mut self, target: &Target, value: Value) -> Result<(), Error>;
}
#[derive(Clone, Debug)]
enum Token<'a> {
    Name(&'a str),
    Number(f64),
    Text(String),
    Symbol(char),
}
fn lex(source: &str) -> Result<Vec<Token<'_>>, Error> {
    let mut tokens = Vec::new();
    let mut rest = source;
    while !rest.is_empty() {
        if tokens.len() >= 4096 {
            return Err(Error::Budget);
        }
        let ch = rest.chars().next().ok_or(Error::Syntax)?;
        if ch.is_whitespace() {
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        if rest.starts_with("//") {
            rest = rest.find('\n').map(|n| &rest[n..]).unwrap_or("");
            continue;
        }
        if rest.starts_with("/*") {
            let n = rest[2..].find("*/").ok_or(Error::Syntax)?;
            rest = &rest[n + 4..];
            continue;
        }
        if ch == '\'' || ch == '"' {
            rest = &rest[1..];
            let mut text = String::new();
            let mut closed = false;
            while let Some(c) = rest.chars().next() {
                rest = &rest[c.len_utf8()..];
                if c == ch {
                    closed = true;
                    break;
                }
                if c == '\n' || c == '\r' {
                    return Err(Error::Syntax);
                }
                if c == '\\' {
                    let c = rest.chars().next().ok_or(Error::Syntax)?;
                    rest = &rest[c.len_utf8()..];
                    text.push(match c {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '\'' => '\'',
                        '"' => '"',
                        _ => return Err(Error::Unsupported),
                    });
                } else {
                    text.push(c);
                }
                if text.len() > 8192 {
                    return Err(Error::Budget);
                }
            }
            if !closed {
                return Err(Error::Syntax);
            }
            tokens.push(Token::Text(text));
            continue;
        }
        if ch.is_ascii_digit() {
            let end = rest
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());
            let value = rest[..end].parse::<f64>().map_err(|_| Error::Syntax)?;
            if !value.is_finite() {
                return Err(Error::Type);
            }
            tokens.push(Token::Number(value));
            rest = &rest[end..];
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' || ch == '$' {
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '$')
                .unwrap_or(rest.len());
            tokens.push(Token::Name(&rest[..end]));
            rest = &rest[end..];
            continue;
        }
        if "=;.+-*/(),".contains(ch) {
            tokens.push(Token::Symbol(ch));
            rest = &rest[1..];
            continue;
        }
        return Err(Error::Unsupported);
    }
    Ok(tokens)
}
struct Variable {
    name: String,
    value: Value,
    constant: bool,
}
/// One realm per document, with an aggregate 32 KiB source/8192-token budget.
pub struct Runtime {
    vars: Vec<Variable>,
    source_bytes: usize,
    remaining: usize,
}
impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
impl Runtime {
    pub fn new() -> Self {
        Self {
            vars: Vec::new(),
            source_bytes: 0,
            remaining: 8192,
        }
    }
    /// Errors stop this script; earlier completed statements retain their effects.
    pub fn execute(&mut self, source: &str, host: &mut impl Host) -> Result<(), Error> {
        self.source_bytes = self
            .source_bytes
            .checked_add(source.len())
            .ok_or(Error::Budget)?;
        if self.source_bytes > 32768 {
            return Err(Error::Budget);
        }
        let tokens = lex(source)?;
        if tokens.len() > self.remaining {
            return Err(Error::Budget);
        }
        self.remaining -= tokens.len();
        let mut parser = Parser {
            tokens,
            at: 0,
            depth: 0,
            runtime: self,
            host,
        };
        while parser.at < parser.tokens.len() {
            parser.statement()?;
        }
        Ok(())
    }
}
struct Parser<'s, 'a, H: Host> {
    tokens: Vec<Token<'s>>,
    at: usize,
    depth: usize,
    runtime: &'a mut Runtime,
    host: &'a mut H,
}
impl<H: Host> Parser<'_, '_, H> {
    fn bump(&mut self) -> Result<Token<'_>, Error> {
        let value = self.tokens.get(self.at).cloned().ok_or(Error::Syntax)?;
        self.at += 1;
        Ok(value)
    }
    fn symbol(&mut self, c: char) -> bool {
        if matches!(self.tokens.get(self.at),Some(Token::Symbol(s)) if *s==c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: char) -> Result<(), Error> {
        if self.symbol(c) {
            Ok(())
        } else {
            Err(Error::Syntax)
        }
    }
    fn name(&mut self) -> Result<String, Error> {
        match self.bump()? {
            Token::Name(n) => Ok(n.into()),
            _ => Err(Error::Syntax),
        }
    }
    fn finish(&mut self) -> Result<(), Error> {
        if self.symbol(';') || self.at == self.tokens.len() {
            Ok(())
        } else {
            Err(Error::Syntax)
        }
    }
    fn statement(&mut self) -> Result<(), Error> {
        if self.symbol(';') {
            return Ok(());
        }
        if matches!(self.tokens.get(self.at),Some(Token::Text(s)) if s=="use strict") {
            self.at += 1;
            return self.finish();
        }
        let name = self.name()?;
        if ["let", "const", "var"].contains(&name.as_str()) {
            let variable = self.name()?;
            self.expect('=')?;
            let value = self.expression(0)?;
            if let Some(old) = self.runtime.vars.iter_mut().find(|v| v.name == variable) {
                if name != "var" || old.constant {
                    return Err(Error::ReadOnly);
                }
                old.value = value;
            } else {
                if self.runtime.vars.len() >= 128 {
                    return Err(Error::Budget);
                }
                self.runtime.vars.push(Variable {
                    name: variable,
                    value,
                    constant: name == "const",
                });
            }
            return self.finish();
        }
        if name == "document" {
            let target = self.target()?;
            self.expect('=')?;
            let value = self.expression(0)?;
            self.host.write(&target, value)?;
            return self.finish();
        }
        if !self.symbol('=') {
            return Err(Error::Unsupported);
        }
        let value = self.expression(0)?;
        let old = self
            .runtime
            .vars
            .iter_mut()
            .find(|v| v.name == name)
            .ok_or(Error::UnknownName)?;
        if old.constant {
            return Err(Error::ReadOnly);
        }
        old.value = value;
        self.finish()
    }
    fn target(&mut self) -> Result<Target, Error> {
        self.expect('.')?;
        let property = self.name()?;
        if property == "title" {
            return Ok(Target::Title);
        }
        let element = match property.as_str() {
            "body" => Element::Body,
            "getElementById" => {
                self.expect('(')?;
                let id = self.expression(0)?.text();
                self.expect(')')?;
                Element::Id(id)
            }
            _ => return Err(Error::Unsupported),
        };
        self.expect('.')?;
        match self.name()?.as_str() {
            "textContent" => Ok(Target::Text(element)),
            "style" => {
                self.expect('.')?;
                Ok(Target::Style(element, self.name()?))
            }
            _ => Err(Error::Unsupported),
        }
    }
    fn expression(&mut self, min: u8) -> Result<Value, Error> {
        if self.depth >= 32 {
            return Err(Error::Budget);
        }
        self.depth += 1;
        let result = self.expr(min);
        self.depth -= 1;
        result
    }
    fn expr(&mut self, min: u8) -> Result<Value, Error> {
        let mut left = if self.symbol('(') {
            let value = self.expression(0)?;
            self.expect(')')?;
            value
        } else if self.symbol('-') {
            Value::Number(-self.expression(3)?.number()?)
        } else if self.symbol('+') {
            Value::Number(self.expression(3)?.number()?)
        } else {
            match self.bump()? {
                Token::Number(n) => Value::Number(n),
                Token::Text(s) => Value::Text(s),
                Token::Name("true") => Value::Bool(true),
                Token::Name("false") => Value::Bool(false),
                Token::Name("null") => Value::Null,
                Token::Name("document") => {
                    let target = self.target()?;
                    self.host.read(&target)?
                }
                Token::Name(n) => {
                    let name = n.to_string();
                    self.runtime
                        .vars
                        .iter()
                        .find(|v| v.name == name)
                        .map(|v| v.value.clone())
                        .ok_or(Error::UnknownName)?
                }
                _ => return Err(Error::Syntax),
            }
        };
        loop {
            let (operator, precedence) = match self.tokens.get(self.at) {
                Some(Token::Symbol(c @ ('+' | '-'))) => (*c, 1),
                Some(Token::Symbol(c @ ('*' | '/'))) => (*c, 2),
                _ => break,
            };
            if precedence < min {
                break;
            }
            self.at += 1;
            let right = self.expression(precedence + 1)?;
            left = if operator == '+'
                && (matches!(left, Value::Text(_)) || matches!(right, Value::Text(_)))
            {
                let mut text = left.text();
                let right = right.text();
                if text.len() + right.len() > 8192 {
                    return Err(Error::Budget);
                }
                text.push_str(&right);
                Value::Text(text)
            } else {
                let a = left.number()?;
                let b = right.number()?;
                let number = match operator {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    _ => return Err(Error::Syntax),
                };
                if !number.is_finite() {
                    return Err(Error::Type);
                }
                Value::Number(number)
            };
        }
        Ok(left)
    }
}
