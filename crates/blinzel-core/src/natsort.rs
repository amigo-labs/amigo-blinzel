//! Natural ("logical") file name order like Windows Explorer: digit runs
//! compare as numbers, letters case-insensitively, punctuation before digits
//! before letters. Keys are computed once per file so sorting 10k names is a
//! plain slice comparison, no locale calls.

use std::cmp::Ordering;

const SYMBOL: u32 = 0x1000_0000;
const NUMBER: u32 = 0x2000_0000;
const LETTER: u32 = 0x3000_0000;

/// ASCII punctuation in the order Windows' default sort uses for file
/// names (symbols before digits, digits before letters).
const ASCII_SYMBOL_ORDER: &[u8] = b" !\"#$%&()*,./:;?@[\\]^_`{|}~+<=>'-";

/// Precomputed sort key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SortKey {
    tokens: Box<[u32]>,
}

impl SortKey {
    pub fn new(name: &str) -> SortKey {
        let mut tokens = Vec::with_capacity(name.len() + 4);
        let bytes = name.as_bytes();
        let mut chars = name.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c.is_ascii_digit() {
                // Whole digit run; leading zeros do not count.
                let mut end = i + 1;
                while let Some(&(j, d)) = chars.peek() {
                    if d.is_ascii_digit() {
                        end = j + 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                let run = &bytes[i..end];
                let sig = match run.iter().position(|&b| b != b'0') {
                    Some(p) => &run[p..],
                    None => &run[run.len() - 1..],
                };
                tokens.push(NUMBER | sig.len() as u32);
                tokens.extend(sig.iter().map(|&d| NUMBER | (d - b'0') as u32));
            } else if c.is_alphabetic() {
                for l in c.to_lowercase() {
                    for base in fold(l) {
                        tokens.push(LETTER | base as u32);
                    }
                }
            } else if c.is_ascii() {
                let rank = ASCII_SYMBOL_ORDER
                    .iter()
                    .position(|&s| s == c as u8)
                    .unwrap_or(0x7F);
                tokens.push(SYMBOL | rank as u32);
            } else {
                // Other symbols after the ASCII ones, by code point.
                tokens.push(SYMBOL | 0x100 | c as u32);
            }
        }
        SortKey {
            tokens: tokens.into_boxed_slice(),
        }
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tokens.cmp(&other.tokens)
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Latin letters with diacritics sort with their base letter (ä with a,
/// ß as ss), like Windows' linguistic order; exact ties are broken later.
fn fold(c: char) -> impl Iterator<Item = char> {
    const TABLE: &[(&str, &str)] = &[
        ("àáâãäåāăą", "a"),
        ("æ", "ae"),
        ("çćĉċč", "c"),
        ("ďđ", "d"),
        ("èéêëēĕėęě", "e"),
        ("ĝğġģ", "g"),
        ("ĥħ", "h"),
        ("ìíîïĩīĭįı", "i"),
        ("ĵ", "j"),
        ("ķ", "k"),
        ("ĺļľŀł", "l"),
        ("ñńņň", "n"),
        ("òóôõöøōŏő", "o"),
        ("œ", "oe"),
        ("ŕŗř", "r"),
        ("śŝşš", "s"),
        ("ß", "ss"),
        ("ţťŧ", "t"),
        ("ùúûüũūŭůűų", "u"),
        ("ŵ", "w"),
        ("ýÿŷ", "y"),
        ("źżž", "z"),
    ];
    let folded = if (c as u32) < 0xC0 {
        None
    } else {
        TABLE
            .iter()
            .find(|(set, _)| set.contains(c))
            .map(|(_, base)| *base)
    };
    let mut buf = [c, '\0'];
    let len = match folded {
        Some(b) => {
            let mut it = b.chars();
            buf[0] = it.next().unwrap();
            if let Some(second) = it.next() {
                buf[1] = second;
                2
            } else {
                1
            }
        }
        None => 1,
    };
    buf.into_iter().take(len)
}

/// Natural comparison of two names, with a deterministic tie-break.
pub fn compare(a: &str, b: &str) -> Ordering {
    SortKey::new(a).cmp(&SortKey::new(b)).then_with(|| a.cmp(b))
}

/// Sorts names naturally (keys computed once).
pub fn sort(names: &mut [String]) {
    let mut keyed: Vec<(SortKey, String)> = names
        .iter_mut()
        .map(|n| (SortKey::new(n), std::mem::take(n)))
        .collect();
    keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (slot, (_, n)) in names.iter_mut().zip(keyed) {
        *slot = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(v: &[&str]) -> Vec<String> {
        let mut s: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        sort(&mut s);
        s
    }

    #[test]
    fn numbers_compare_numerically() {
        assert_eq!(
            sorted(&["img10.jpg", "img2.jpg", "img1.jpg", "img100.jpg"]),
            ["img1.jpg", "img2.jpg", "img10.jpg", "img100.jpg"]
        );
        assert_eq!(
            sorted(&["IMG_0100.JPG", "IMG_0009.JPG", "IMG_0010.JPG"]),
            ["IMG_0009.JPG", "IMG_0010.JPG", "IMG_0100.JPG"]
        );
        assert_eq!(compare("photo (2).jpg", "photo (10).jpg"), Ordering::Less);
        // Arbitrarily long numbers.
        assert_eq!(
            compare("a99999999999999999999999", "a100000000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn case_insensitive_with_stable_tiebreak() {
        assert_eq!(
            sorted(&["b.jpg", "A.jpg", "a.jpg", "C.jpg"]),
            ["A.jpg", "a.jpg", "b.jpg", "C.jpg"]
        );
        assert_eq!(compare("Äpfel", "zebra"), Ordering::Less);
        assert_eq!(compare("Straße", "Strasse1"), Ordering::Less);
        assert_eq!(sorted(&["b", "ä", "a"]), ["a", "ä", "b"]);
    }

    #[test]
    fn symbols_before_digits_before_letters() {
        assert_eq!(
            sorted(&["a.jpg", "1.jpg", "_x.jpg", " y.jpg"]),
            [" y.jpg", "_x.jpg", "1.jpg", "a.jpg"]
        );
        assert_eq!(compare("a b", "ab"), Ordering::Less);
        assert_eq!(compare("a.jpg", "a1.jpg"), Ordering::Less);
    }
}
