//! Our own minimal JSON parser — no crates (serde), std only.
//! Enough to parse registry packuments. Recursive descent over characters.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(HashMap<String, Json>),
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Json::Num(n) = self {
            Some(*n)
        } else {
            None
        }
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        if let Json::Obj(m) = self {
            m.get(key)
        } else {
            None
        }
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Json>> {
        if let Json::Obj(m) = self {
            Some(m)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[Json]> {
        if let Json::Arr(items) = self {
            Some(items)
        } else {
            None
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Json::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
}

/// Parse a JSON string. Err with a position — on a syntax error.
pub fn parse(input: &str) -> Result<Json, String> {
    // Swallow a UTF-8 BOM (U+FEFF) at the start: editors and PowerShell like to add
    // it (`echo ... > file.json`), and in JSON it's not whitespace and broke parsing.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let chars: Vec<char> = input.chars().collect();
    let mut p = Parser { chars, pos: 0 };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return Err(format!("Extra characters after JSON at position {}", p.pos));
    }
    Ok(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => Ok(Json::Str(self.parse_string()?)),
            Some('t') | Some('f') => self.parse_bool(),
            Some('n') => self.parse_null(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(format!("Unexpected character '{c}' at position {}", self.pos)),
            None => Err("Unexpected end of JSON".to_string()),
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("Expected '{c}' at position {}", self.pos))
        }
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut map = HashMap::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(format!("Expected ',' or '}}' at position {}", self.pos)),
            }
        }
        Ok(Json::Obj(map))
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(format!("Expected ',' or ']' at position {}", self.pos)),
            }
        }
        Ok(Json::Arr(items))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err("Unterminated string".to_string()),
                Some('"') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some('"') => s.push('"'),
                        Some('\\') => s.push('\\'),
                        Some('/') => s.push('/'),
                        Some('b') => s.push('\u{0008}'),
                        Some('f') => s.push('\u{000C}'),
                        Some('n') => s.push('\n'),
                        Some('r') => s.push('\r'),
                        Some('t') => s.push('\t'),
                        Some('u') => {
                            let cp = self.parse_hex4()?;
                            // A surrogate pair (emoji, etc.).
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.peek() == Some('\\') {
                                    self.pos += 1;
                                    self.expect('u')?;
                                    let low = self.parse_hex4()?;
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                                    s.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                                } else {
                                    s.push('\u{FFFD}');
                                }
                                continue;
                            }
                            s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(format!("Invalid escape at position {}", self.pos)),
                    }
                    self.pos += 1;
                }
                Some(c) => {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
        Ok(s)
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        self.pos += 1; // skip 'u'
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self.peek().ok_or("Truncated \\u escape")?;
            let d = c.to_digit(16).ok_or("Invalid hex digit in \\u")?;
            value = value * 16 + d;
            self.pos += 1;
        }
        self.pos -= 1; // compensate for the caller's +1
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice: String = self.chars[start..self.pos].iter().collect();
        slice
            .parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("Invalid number \"{slice}\""))
    }

    fn parse_bool(&mut self) -> Result<Json, String> {
        if self.matches("true") {
            Ok(Json::Bool(true))
        } else if self.matches("false") {
            Ok(Json::Bool(false))
        } else {
            Err(format!("Invalid literal at position {}", self.pos))
        }
    }

    fn parse_null(&mut self) -> Result<Json, String> {
        if self.matches("null") {
            Ok(Json::Null)
        } else {
            Err(format!("Invalid literal at position {}", self.pos))
        }
    }

    fn matches(&mut self, word: &str) -> bool {
        let end = self.pos + word.len();
        if end <= self.chars.len() && self.chars[self.pos..end].iter().collect::<String>() == word {
            self.pos = end;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization (Json -> string). Needed for lock, db, config.
// ---------------------------------------------------------------------------

/// Escape a string per JSON rules.
fn escape_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64) // an integer without ".0"
    } else {
        format!("{n}")
    }
}

/// Compact serialization (one line).
pub fn stringify(value: &Json) -> String {
    let mut out = String::new();
    write_value(value, &mut out, None, 0);
    out
}

/// Serialization with indentation (like JSON.stringify(v, null, 2)). Object keys
/// are sorted — stable (byte-reproducible) output.
pub fn stringify_pretty(value: &Json) -> String {
    let mut out = String::new();
    write_value(value, &mut out, Some(2), 0);
    out
}

fn write_value(value: &Json, out: &mut String, indent: Option<usize>, depth: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(&fmt_num(*n)),
        Json::Str(s) => escape_str(s, out),
        Json::Arr(items) => write_seq(out, indent, depth, '[', ']', items.len(), |out, i| {
            write_value(&items[i], out, indent, depth + 1);
        }),
        Json::Obj(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort(); // stable order
            write_seq(out, indent, depth, '{', '}', keys.len(), |out, i| {
                escape_str(keys[i], out);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(&map[keys[i]], out, indent, depth + 1);
            })
        }
    }
}

fn write_seq(
    out: &mut String,
    indent: Option<usize>,
    depth: usize,
    open: char,
    close: char,
    len: usize,
    mut write_item: impl FnMut(&mut String, usize),
) {
    out.push(open);
    if len == 0 {
        out.push(close);
        return;
    }
    let pad = |out: &mut String, d: usize| {
        if let Some(step) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(step * d));
        }
    };
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }
        pad(out, depth + 1);
        write_item(out, i);
    }
    pad(out, depth);
    out.push(close);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("42").unwrap(), Json::Num(42.0));
        assert_eq!(parse("-3.5").unwrap(), Json::Num(-3.5));
        assert_eq!(parse("1.5e3").unwrap(), Json::Num(1500.0));
        assert_eq!(
            parse("\"hello\"").unwrap(),
            Json::Str("hello".to_string())
        );
    }

    #[test]
    fn strings_with_escapes() {
        assert_eq!(parse(r#""a\nb""#).unwrap(), Json::Str("a\nb".to_string()));
        assert_eq!(
            parse(r#""quote\"here""#).unwrap(),
            Json::Str("quote\"here".to_string())
        );
        assert_eq!(
            parse(r#""tab\tend""#).unwrap(),
            Json::Str("tab\tend".to_string())
        );
        assert_eq!(parse(r#""A""#).unwrap(), Json::Str("A".to_string()));
    }

    #[test]
    fn arrays_and_objects() {
        let v = parse(r#"[1, 2, 3]"#).unwrap();
        assert_eq!(
            v,
            Json::Arr(vec![Json::Num(1.0), Json::Num(2.0), Json::Num(3.0)])
        );

        let o = parse(r#"{"name": "express", "deps": {"ms": "^2.0.0"}}"#).unwrap();
        assert_eq!(o.get("name").and_then(Json::as_str), Some("express"));
        assert_eq!(
            o.get("deps")
                .and_then(|d| d.get("ms"))
                .and_then(Json::as_str),
            Some("^2.0.0")
        );
    }

    #[test]
    fn nested_and_empty() {
        assert_eq!(parse("{}").unwrap(), Json::Obj(HashMap::new()));
        assert_eq!(parse("[]").unwrap(), Json::Arr(vec![]));
        let v = parse(r#"{"a": [{"b": true}, null]}"#).unwrap();
        let arr = v.get("a").unwrap();
        assert_eq!(
            arr,
            &Json::Arr(vec![
                Json::Obj(HashMap::from([("b".to_string(), Json::Bool(true))])),
                Json::Null
            ])
        );
    }

    #[test]
    fn parse_skips_utf8_bom() {
        // PowerShell `echo > file.json` appends a UTF-8 BOM — must not break parsing.
        let with_bom = "\u{feff}{\"name\":\"test\"}";
        let v = parse(with_bom).unwrap();
        assert_eq!(v.get("name").and_then(Json::as_str), Some("test"));
    }

    #[test]
    fn errors() {
        assert!(parse("{").is_err());
        assert!(parse("[1, 2").is_err());
        assert!(parse("nul").is_err());
        assert!(parse("\"unclosed").is_err());
        assert!(parse("").is_err());
    }
}
