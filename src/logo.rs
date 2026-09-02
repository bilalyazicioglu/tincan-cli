//! The tin can telephone.
//!
//! Two cans and a string is the whole idea of the program, so it is drawn rather than
//! spelled: here for the terminal before the interface takes over, and again — live,
//! with the real round trip time written on the string — on the empty chat screen.

/// Plain, for `--help` and anywhere the output might be piped.
pub const BANNER: &str = "
   ( o )                            ( o )
  ┌─────┐                          ┌─────┐
  │     │╌╌╌╌╌╌╌╌ t i n c a n ╌╌╌╌╌│     │
  └─────┘                          └─────┘
";

const BANNER_ASCII: &str = "
   ( o )                            ( o )
  +-----+                          +-----+
  |     |--------- t i n c a n ----|     |
  +-----+                          +-----+
";

const TIN: &str = "\x1b[38;5;252m";
const ZINC: &str = "\x1b[38;5;245m";
const PATINA: &str = "\x1b[1;38;5;80m";
const BRASS: &str = "\x1b[38;5;179m";
const OFF: &str = "\x1b[0m";

/// Prints the drawing at start-up, in colour where the terminal wants colour.
pub fn print_banner() {
    if plain() {
        println!("{}", banner_text());
        return;
    }
    let art = banner_text();
    for row in art.lines() {
        // The string is brass, the cans are bare metal, and the name is the one
        // patina-green thing — the same green the interface uses for what is live.
        let painted = match row.split_once("t i n c a n") {
            Some((head, tail)) => format!("{BRASS}{head}{PATINA}t i n c a n{BRASS}{tail}{OFF}"),
            None => format!("{TIN}{row}{OFF}"),
        };
        println!("{painted}");
    }
}

/// A heading for the plain-terminal output before the interface starts.
pub fn heading(text: &str) -> String {
    if plain() {
        return text.to_string();
    }
    format!("{ZINC}{text}{OFF}")
}

/// The invite code, or anything else that is the next thing to act on.
pub fn code(text: &str) -> String {
    if plain() {
        return text.to_string();
    }
    format!("{BRASS}{text}{OFF}")
}

fn banner_text() -> &'static str {
    if ascii() { BANNER_ASCII } else { BANNER }
}

fn plain() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

fn ascii() -> bool {
    std::env::var_os("TINCAN_ASCII").is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ascii_banner_is_actually_ascii() {
        assert!(BANNER_ASCII.is_ascii());
    }

    #[test]
    fn both_banners_are_the_same_shape() {
        assert_eq!(BANNER.lines().count(), BANNER_ASCII.lines().count());
        for (fancy, plain) in BANNER.lines().zip(BANNER_ASCII.lines()) {
            assert_eq!(
                fancy.chars().count(),
                plain.chars().count(),
                "the fallback must not shift the drawing:\n{fancy}\n{plain}"
            );
        }
    }

    #[test]
    fn both_banners_carry_the_name() {
        assert!(BANNER.contains("t i n c a n"));
        assert!(BANNER_ASCII.contains("t i n c a n"));
    }
}
