//! A small JSON reader for the GitHub release API, so the updater needs no
//! serialization framework: objects, arrays, strings (with escapes and
//! surrogate pairs), numbers, booleans and null. Nesting is limited, so
//! hostile input cannot exhaust the stack.

/// A parsed JSON value. Objects keep their key order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

const MAX_DEPTH: usize = 64;

impl Json {
    /// Parses a complete document; `None` on any syntax error.
    pub fn parse(text: &str) -> Option<Json> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value(0)?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }

    /// The value of `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self
            .s
            .get(self.i)
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> Option<()> {
        self.s[self.i..]
            .starts_with(lit)
            .then(|| self.i += lit.len())
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Json::Str),
            b't' => self.eat(b"true").map(|()| Json::Bool(true)),
            b'f' => self.eat(b"false").map(|()| Json::Bool(false)),
            b'n' => self.eat(b"null").map(|()| Json::Null),
            _ => self.number(),
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut kv = Vec::new();
        self.ws();
        if self.eat(b"}").is_some() {
            return Some(Json::Obj(kv));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b":")?;
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws();
            if self.eat(b",").is_some() {
                continue;
            }
            self.eat(b"}")?;
            return Some(Json::Obj(kv));
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut a = Vec::new();
        self.ws();
        if self.eat(b"]").is_some() {
            return Some(Json::Arr(a));
        }
        loop {
            a.push(self.value(depth + 1)?);
            self.ws();
            if self.eat(b",").is_some() {
                continue;
            }
            self.eat(b"]")?;
            return Some(Json::Arr(a));
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b"\"")?;
        let mut out = String::new();
        loop {
            // Copy the run up to the next quote or escape in one go.
            let run = self.s[self.i..]
                .iter()
                .position(|&c| c == b'"' || c == b'\\' || c < 0x20)?;
            out.push_str(std::str::from_utf8(&self.s[self.i..self.i + run]).ok()?);
            self.i += run;
            match self.s[self.i] {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                b'\\' => {
                    let e = *self.s.get(self.i + 1)?;
                    self.i += 2;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                self.eat(b"\\u")?;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(code)?
                        }
                        _ => return None,
                    };
                    out.push(c);
                }
                // Raw control characters are not allowed in strings.
                _ => return None,
            }
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        if t.is_empty() {
            return None;
        }
        t.parse().ok().map(Json::Num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_release_like_document() {
        let j = Json::parse(
            r#" { "tag_name": "v1.2.3", "draft": false, "n": -1.5e2, "x": null,
                "assets": [ {"name": "a\"b\\cä😀", "size": 3} ], "e": {} } "#,
        )
        .unwrap();
        assert_eq!(j.get("tag_name").and_then(Json::as_str), Some("v1.2.3"));
        assert_eq!(j.get("draft").and_then(Json::as_bool), Some(false));
        assert_eq!(j.get("n"), Some(&Json::Num(-150.0)));
        assert_eq!(j.get("x"), Some(&Json::Null));
        let a = j.get("assets").and_then(Json::as_array).unwrap();
        assert_eq!(a[0].get("name").and_then(Json::as_str), Some("a\"b\\cä😀"));
        assert_eq!(j.get("e"), Some(&Json::Obj(Vec::new())));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "\"unterminated",
            "\"bad \\q escape\"",
            "\"\\ud800 lone\"",
            "tru",
            "1 2",
            "\"tab\there\"",
        ] {
            assert_eq!(Json::parse(bad), None, "{bad:?}");
        }
        // Deep nesting is refused instead of overflowing the stack.
        let deep = "[".repeat(10_000) + &"]".repeat(10_000);
        assert_eq!(Json::parse(&deep), None);
    }
}
