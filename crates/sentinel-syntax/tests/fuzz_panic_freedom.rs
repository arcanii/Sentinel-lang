//! P2.5 / review F11: property-based **panic-freedom** fuzzing of the lexer and
//! parser — the most attackable surface. A compiler front end must never panic
//! (or hang) on adversarial input; it should return a clean `LexError` /
//! `ParseError` instead. This throws three kinds of input at `lex` + `parse`:
//!
//!   1. pure random printable-ASCII bytes (stresses the lexer's error paths),
//!   2. grammar-biased "token soup" (random keywords/operators/punctuation, to
//!      reach deep parser paths a random byte string rarely forms),
//!   3. mutations of real Sentinel programs (bit-flips / inserts / deletes /
//!      truncations of the seed corpus — near-valid inputs, the richest source
//!      of edge cases),
//!
//! and asserts neither function panics. The generator is a **deterministic**
//! xorshift (fixed seed), so the campaign is reproducible and CI-stable —
//! every `cargo nextest` run re-fuzzes the same sequence, and a regression
//! reports the exact failing input. (A nightly `cargo-fuzz` crate for
//! coverage-guided + differential fuzzing — generated programs compared across
//! the `snc` stage dumps and `scg` modes — is the Phase-2 deepening; this is
//! the stable, always-on Phase 1.)
//!
//! Input sizes are bounded so each call is fast and the nesting depth stays
//! well under any stack-overflow threshold (deep-nesting robustness is its own
//! probe: `parser_does_not_overflow_on_moderately_deep_nesting`).

use sentinel_syntax::{lex, parse, ParseError};
use std::panic;

/// A deterministic xorshift64 PRNG — fixed seed ⇒ reproducible campaign.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// A value in `0..n` (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Lexer-alphabet fragments — keywords, type names, operators, punctuation,
/// and literals — assembled into grammar-biased token soup.
const PALETTE: &[&str] = &[
    "fn", "main", "let", "mut", "if", "else", "while", "break", "continue", "struct", "enum",
    "match", "return", "secret", "declassify", "effect", "perform", "handle", "with", "class",
    "init", "pub", "trait", "impl", "for", "as", "use", "scope", "spawn", "await", "concurrent",
    "i64", "i32", "u8", "bool", "true", "false", "null", "x", "y", "foo", "Bar", "T", "(", ")",
    "{", "}", "[", "]", "<", ">", ":", ";", ",", ".", "::", "+", "-", "*", "/", "=", "==", "!=",
    "<=", ">=", "&&", "||", "!", "&", "|", "^", "->", "=>", "?", "@", "#", "\"hi\"", "'a'", "42",
    "0", "-1", " ", "\n",
];

/// Real Sentinel programs, mutated into near-valid adversarial inputs.
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 0 }",
    "struct P { x: i64, y: i64 } fn main() -> i64 { let p = P { x: 1, y: 2 }; p.x }",
    "enum E { A, B(i64) } fn f(e: E) -> i64 { match e { E::A => 0, E::B(n) => n } }",
    "fn main() -> i64 { let mut i: i64 = 0; while i < 10 { i = i + 1; } i }",
    "fn unwrap(s: secret i64) -> i64 { declassify(s) }",
    "effect Io { read() -> i64; } fn main() -> i64 { handle perform Io.read() with { Io.read(k) => k(42) } }",
    "class C { let x: i64; pub init(x: i64) { self.x = x; 0 } pub fn g(self: &Self) -> i64 { self.x } }",
    "fn id<T>(x: T) -> T { x } fn main() -> i64 { id(42) }",
    "use util::add; fn main() -> i64 { add(40, 2) }",
];

fn random_bytes(rng: &mut Rng, len: usize) -> String {
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        // printable ASCII (32..=127): the lexer's realistic input domain.
        s.push((rng.below(96) as u8 + 32) as char);
    }
    s
}

fn token_soup(rng: &mut Rng, n: usize) -> String {
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(PALETTE[rng.below(PALETTE.len())]);
        if rng.below(3) == 0 {
            s.push(' ');
        }
    }
    s
}

fn mutate(rng: &mut Rng, base: &str) -> String {
    let mut bytes: Vec<u8> = base.bytes().collect();
    let edits = 1 + rng.below(8);
    for _ in 0..edits {
        if bytes.is_empty() {
            break;
        }
        match rng.below(4) {
            0 => {
                let i = rng.below(bytes.len());
                bytes[i] = rng.below(96) as u8 + 32;
            }
            1 => {
                let i = rng.below(bytes.len());
                let frag = PALETTE[rng.below(PALETTE.len())].as_bytes();
                bytes.insert(i, frag[rng.below(frag.len())]);
            }
            2 => {
                let i = rng.below(bytes.len());
                bytes.remove(i);
            }
            _ => bytes.truncate(rng.below(bytes.len() + 1)),
        }
    }
    // The lexer takes `&str`; lossy decode keeps near-valid mutations intact.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `lex` + `parse` on `input`, returning `Err` iff either panicked.
fn lex_and_parse_catching(input: &str) -> Result<(), ()> {
    panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _ = lex(input);
        let _ = parse(input);
    }))
    .map_err(|_| ())
}

#[test]
fn lexer_and_parser_are_panic_free_on_fuzzed_input() {
    let mut rng = Rng(0x5e7e_1e1f_2a3b_4c5d);
    // Silence the default panic hook during the campaign; a genuine panic is
    // caught and reported with its exact (reproducible) input below.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    // 50k keeps the CI run fast (~0.1s); set `SENTINEL_FUZZ_ITERS` for a deeper
    // local campaign (the sequence is deterministic, so a higher count is a
    // strict superset that still reproduces any failure by its printed input).
    let iters: u64 = std::env::var("SENTINEL_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let mut failing: Option<String> = None;
    for k in 0..iters {
        let input = match k % 3 {
            0 => {
                let n = 1 + rng.below(256);
                random_bytes(&mut rng, n)
            }
            1 => {
                let n = 1 + rng.below(64);
                token_soup(&mut rng, n)
            }
            _ => {
                let seed = SEEDS[rng.below(SEEDS.len())];
                mutate(&mut rng, seed)
            }
        };
        if lex_and_parse_catching(&input).is_err() {
            failing = Some(input);
            break;
        }
    }

    panic::set_hook(prev);
    assert!(
        failing.is_none(),
        "lex/parse panicked on fuzzed input (reproduce by feeding this to `lex`/`parse`):\n{:?}",
        failing.unwrap()
    );
}

/// `fn main() -> i64 { (((…0…))) }` with `depth` nested parens.
fn nested_parens(depth: usize) -> String {
    let mut src = String::from("fn main() -> i64 { ");
    src.push_str(&"(".repeat(depth));
    src.push('0');
    src.push_str(&")".repeat(depth));
    src.push_str(" }");
    src
}

#[test]
fn parser_rejects_pathological_nesting_instead_of_overflowing() {
    // A recursive-descent parser stack-overflows on deeply nested input — a
    // denial-of-service on adversarial bytes (this fuzzer FOUND it: 256 nested
    // `(` aborted the process). The fix is `MAX_EXPR_DEPTH` (review F11 / P2.5):
    // extreme nesting must now be a clean `RecursionLimit` rejection, NOT a
    // crash. 5000 levels is far past any real program and past the limit.
    let src = nested_parens(5000);
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| parse(&src)));
    panic::set_hook(prev);

    let parsed = result.expect("parser overflowed/panicked instead of rejecting deep nesting");
    assert!(
        matches!(parsed, Err(ParseError::RecursionLimit { .. })),
        "expected a RecursionLimit rejection, got {parsed:?}"
    );
}

#[test]
fn parser_accepts_moderate_nesting_under_the_limit() {
    // The depth limit must not reject legitimate (if unusual) nesting. 100
    // nested parens — well under MAX_EXPR_DEPTH and far past any real code —
    // still parses cleanly to a program.
    let src = nested_parens(100);
    assert!(
        parse(&src).is_ok(),
        "moderate ({}-deep) nesting under the limit must still parse",
        100
    );
}
