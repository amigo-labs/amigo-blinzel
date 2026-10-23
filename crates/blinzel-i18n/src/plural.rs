//! CLDR plural categories for the integer counts the UI shows.

/// Index into the per-message plural row: zero, one, two, few, many, other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Zero = 0,
    One = 1,
    Two = 2,
    Few = 3,
    Many = 4,
    Other = 5,
}

/// Plural rule families (CLDR, integers only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Families are used as soon as a language with that rule ships.
#[allow(dead_code)]
pub(crate) enum Rule {
    /// English, German, Dutch, Italian, Spanish, Swedish, …: 1 → one.
    OneOther,
    /// French, Portuguese: 0 and 1 → one.
    ZeroOneOther,
    /// Russian, Ukrainian, Belarusian.
    EastSlavic,
    Polish,
    /// Czech, Slovak.
    Czech,
    /// Japanese, Chinese, Korean, …: no plural forms.
    None,
}

impl Rule {
    pub(crate) fn category(self, n: u64) -> Category {
        let (m10, m100) = (n % 10, n % 100);
        match self {
            Rule::OneOther => {
                if n == 1 {
                    Category::One
                } else {
                    Category::Other
                }
            }
            Rule::ZeroOneOther => {
                if n <= 1 {
                    Category::One
                } else {
                    Category::Other
                }
            }
            Rule::EastSlavic => {
                if m10 == 1 && m100 != 11 {
                    Category::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    Category::Few
                } else {
                    Category::Many
                }
            }
            Rule::Polish => {
                if n == 1 {
                    Category::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    Category::Few
                } else {
                    Category::Many
                }
            }
            Rule::Czech => match n {
                1 => Category::One,
                2..=4 => Category::Few,
                _ => Category::Other,
            },
            Rule::None => Category::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert_eq!(Rule::OneOther.category(1), Category::One);
        assert_eq!(Rule::OneOther.category(0), Category::Other);
        assert_eq!(Rule::OneOther.category(21), Category::Other);
        assert_eq!(Rule::ZeroOneOther.category(0), Category::One);
        assert_eq!(Rule::ZeroOneOther.category(2), Category::Other);
        assert_eq!(Rule::EastSlavic.category(21), Category::One);
        assert_eq!(Rule::EastSlavic.category(11), Category::Many);
        assert_eq!(Rule::EastSlavic.category(23), Category::Few);
        assert_eq!(Rule::EastSlavic.category(13), Category::Many);
        assert_eq!(Rule::Polish.category(1), Category::One);
        assert_eq!(Rule::Polish.category(22), Category::Few);
        assert_eq!(Rule::Polish.category(21), Category::Many);
        assert_eq!(Rule::Czech.category(3), Category::Few);
        assert_eq!(Rule::Czech.category(5), Category::Other);
        assert_eq!(Rule::None.category(1), Category::Other);
    }
}
