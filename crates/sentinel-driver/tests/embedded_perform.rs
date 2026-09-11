//! Register D69 / ADR 0072 A1: the effecting-fn body shapes, fail-closed.
//!
//! `snc build` lowers an effecting fn whose tail holds one `perform` by running the
//! `perform` FIRST, in the fn, and replaying the rest of the tail in a resumer (the embedded
//! shape); a `let` bound to a `perform` or an effecting call suspends there and replays the
//! rest (the let and chained shapes). ADR 0072 A1 names rules each needs, and each refused
//! body below breaks one: built, it ran the handler on a path that never performs,
//! reordered or dropped an effect, read a stale, garbage or out-of-slot value, leaked the
//! continuation frame, or panicked — or it was refused only by an accident of types or file
//! order (a few, like `loop_cond`, happened to print the right answer and are refused
//! because the rule is conservative). Each accepted body is built and run, and must print
//! what its source means. The programs live here rather than in `tests/pass` or `tests/ui`
//! because the text back ends do not apply the rules — the oracle hoists or refuses several
//! of these and scg emits invalid IR for others (the rest of register D69, and D71) — so the
//! stage differentials cannot hold them.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .parent()
        .expect("crates/ has a parent")
        .to_path_buf()
}

/// The program around `body`, `e1`'s body. `Io.read` prints 777 and resumes with 5;
/// `Io.echo(x)` prints `x` and resumes with it. `main` prints `run(0)`, then `run(3)`.
///
/// The ORDER is load-bearing: `mk`, a plain fn, sits directly before `e1`, so a resumer that
/// took a stale fn id (register D60) would take `mk`'s ABI and fail the build of
/// `perform_then_return` and `return_perform`. `g`, effecting, comes after `run` for the
/// same reason.
fn program(body: &str) -> String {
    format!(
        "effect Io {{\n    read() -> i64;\n    echo(x: i64) -> i64;\n}}\n\n\
         enum E {{\n    A,\n    B(i64),\n}}\n\n\
         fn p(x: i64) -> i64 {{\n    print(x);\n    x\n}}\n\n\
         fn bump(x: &mut i64) -> i64 {{\n    *x = *x + 10;\n    *x\n}}\n\n\
         fn val(e: E) -> i64 {{\n    match e {{\n        E::A => 0,\n        E::B(v) => v,\n    }}\n}}\n\n\
         fn mk(n: i64) -> E {{\n    if n == 0 {{\n        E::A\n    }} else {{\n        E::B(n)\n    }}\n}}\n\n\
         fn e1(mut n: i64, b: u8) -> i64 ! {{ Io }} {{\n    {body}\n}}\n\n\
         fn run(n: i64) -> i64 {{\n    handle e1(n, 42 as u8) with {{\n        \
         Io.read(k) => {{\n            print(777);\n            k(5)\n        }},\n        \
         Io.echo(x, k) => {{\n            print(x);\n            k(x)\n        }}\n    }}\n}}\n\n\
         fn g(n: i64) -> i64 ! {{ Io }} {{\n    perform Io.read()\n}}\n\n\
         fn main() -> i64 {{\n    print(run(0));\n    print(run(3));\n    0\n}}\n"
    )
}

/// Build `src` as `name`; the build's output, and the executable's path.
fn build(name: &str, src: &str) -> (std::process::Output, PathBuf) {
    let dir = workspace_root().join("target/sentinel-embedded");
    std::fs::create_dir_all(&dir).expect("create build dir");
    let path = dir.join(format!("{name}.sentinel"));
    std::fs::write(&path, src).expect("write the program");
    let exe = dir.join(name);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .env("NO_COLOR", "1")
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    (out, exe)
}

/// Assert `src` is refused with diagnostic `code`, for `reason`.
fn assert_refused(name: &str, src: &str, code: &str, reason: &str) {
    let (out, _) = build(name, src);
    // miette wraps the message behind a `│` gutter; compare with the wrapping undone.
    let stderr = String::from_utf8_lossy(&out.stderr)
        .replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !out.status.success() && stderr.contains(code) && stderr.contains(reason),
        "`{name}` must be refused ({code}), because {reason}; got status {} and:\n{stderr}",
        out.status
    );
}

const NOT_DIRECT: &str = "sentinel::codegen::effecting_fn_body_not_direct";

#[test]
fn embedded_perform_refuses_unfaithful_tails() {
    const GUARDED: &str = "the `perform` in its tail is guarded";
    const OBSERVED: &str = "something its tail evaluates before the `perform`";
    const ARGS: &str = "the `perform`'s arguments call an effecting fn, write a variable";
    const SECOND: &str = "its tail has a second suspension point";
    const CAPTURED: &str = "`b` is captured across the continuation";
    const BINDS: &str = "its tail binds a name";
    const LET: &str = "writes a variable or `return`s before it suspends";
    const SUSPARGS: &str = "of a call to an effecting fn in it suspend themselves";
    let cases: &[(&str, &str, &str)] = &[
        // Guarded: the `perform` runs on some paths only, or more than once, and the shape
        // would run it once, first, on every path.
        ("if_arm", "if n > 0 { perform Io.read() } else { 42 }", GUARDED),
        ("other_arm_returns", "(if n == 0 { perform Io.read() } else { return 42 }) + 1", GUARDED),
        ("match_arm", "(match mk(n) { E::A => perform Io.read(), _ => 42 }) + 1", GUARDED),
        ("or_right", "if n == 0 || perform Io.read() == 0 { 42 } else { 1 }", GUARDED),
        ("and_right", "if n != 0 && perform Io.read() == 5 { 42 } else { 1 }", GUARDED),
        ("loop_body", "{ while n > 100 { perform Io.read(); } 42 }", GUARDED),
        ("loop_cond", "{ while perform Io.read() > 100 { 0; } 42 }", GUARDED),
        ("return_perform_in_arm", "if n == 0 { return perform Io.read() } else { n + 1 }", GUARDED),
        // A `perform` inside a local `handle` belongs to it, not to the fn's handler.
        ("local_handle", "(handle perform Io.read() with { Io.read(k) => k(7) }) + n", GUARDED),
        ("handler_arm", "(handle n with { Io.read(k) => k(perform Io.read()) }) + 1", GUARDED),
        ("return_arm", "(handle n with { Io.read(k) => k(1), return v => v + perform Io.read() }) + 1", GUARDED),
        // Evaluated before the `perform`, so it would run after it: a `return`, a `print`,
        // a division that can trap, an index that can abort.
        ("return_first", "(if n == 0 { return 42 } else { n }) + perform Io.read()", OBSERVED),
        ("print_first", "p(1) + perform Io.read()", OBSERVED),
        ("print_first_then_return", "p(1) + perform Io.read() + (if n == 0 { return 9 } else { n })", OBSERVED),
        ("div_first", "10 / n + perform Io.read()", OBSERVED),
        ("index_first", "[1, 2][n] + perform Io.read()", OBSERVED),
        // The fn fills the continuation frame before evaluating the arguments: a write is
        // read back stale, a `return` leaks the frame, a suspension hands back a pointer.
        ("arg_writes", "perform Io.echo({ n = n + 10; n }) + n", ARGS),
        // The borrow checker admits this one (the borrow ends with its statement), and HEAD
        // read `n` back stale: only the `&mut` rule refuses it.
        ("arg_borrows_mut", "perform Io.echo({ bump(&mut n); n }) + n", ARGS),
        ("arg_returns", "perform Io.echo(if n == 0 { return 9 } else { n }) + n", ARGS),
        ("arg_reads_local", "{ let a = n; perform Io.echo(a) } + 1", ARGS),
        ("arg_suspends", "perform Io.echo(g(n)) + 1", ARGS),
        // A `perform` or effecting call whose own argument suspends, in any shape: the inner
        // continuation came back as data. A local `handle` there is refused conservatively.
        ("direct_arg_suspends", "perform Io.echo(if n >= 0 { g(n) } else { 0 })", SUSPARGS),
        ("direct_call_arg_suspends", "g(if n >= 0 { g(n) } else { 0 })", SUSPARGS),
        ("arg_local_handle", "perform Io.echo(handle n with { Io.read(k) => k(1) })", SUSPARGS),
        // The replay would suspend again: `g`'s effect would reach no handler, and a
        // `perform` under a `match` or an enum construction would still be there.
        ("effecting_call_after", "perform Io.read() + { g(n); 0 }", SECOND),
        ("under_match", "match mk(perform Io.read()) { E::A => 1, _ => n + 5 }", SECOND),
        ("under_enum", "val(E::B(perform Io.read())) + n", SECOND),
        // `b` is a `u8`: the fn would copy 8 bytes out of its 1-byte slot.
        ("narrow_capture", "perform Io.read() + (b as i64)", CAPTURED),
        ("local_after", "perform Io.read() + { let a = n; a }", BINDS),
        // The let and chained shapes fill their frame before the RHS, too.
        ("let_rhs_writes", "let x = perform Io.echo({ n = n + 10; n });\n    x + n", LET),
        ("let_rhs_returns", "let x = perform Io.echo(if n == 0 { return 9 } else { n });\n    x + n", LET),
        ("chained_rhs_writes", "let a = perform Io.read();\n    let c = perform Io.echo({ n = n + a; n });\n    a + c + n", LET),
        ("let_arg_suspends", "let x = perform Io.echo(if n >= 0 { g(n) } else { 0 });\n    x + n", SUSPARGS),
    ];
    for (name, body, reason) in cases {
        assert_refused(name, &program(body), NOT_DIRECT, reason);
    }
}

#[test]
fn embedded_perform_builds_faithful_tails() {
    let cases: &[(&str, &str, &str)] = &[
        // The `perform` first, a `return` after it: `run(0)` returns 9, `run(3)` gives 5 + 3.
        ("perform_then_return", "perform Io.read() + (if n == 0 { return 9 } else { n })", "777\n9\n777\n8\n"),
        // An `if`'s condition is on the unconditional path.
        ("perform_in_cond", "if perform Io.read() == 5 { 42 } else { n }", "777\n42\n777\n42\n"),
        ("return_perform", "return perform Io.read() + n", "777\n5\n777\n8\n"),
        // An effect AFTER the `perform` runs after it, in the replay.
        ("print_after", "perform Io.read() + p(1)", "777\n1\n6\n777\n1\n6\n"),
        // A `perform`'s argument runs where the `perform` is, plain calls included.
        ("echo_arg", "perform Io.echo(n + 1) * 2", "1\n2\n4\n8\n"),
        ("echo_arg_call", "perform Io.echo(p(n)) + 1", "0\n0\n1\n3\n3\n4\n"),
        // Declined by the embedded shape, then built in order by the direct shape: `run(0)`
        // returns 42 without performing, and `p(1)` prints before the handler.
        ("block_tail_return_first", "{ if n == 0 { return 42 } else { 0 }; perform Io.read() }", "42\n777\n5\n"),
        ("block_tail_print_first", "{ p(1); perform Io.read() }", "1\n777\n5\n1\n777\n5\n"),
    ];
    for (name, body, want) in cases {
        let (out, exe) = build(name, &program(body));
        assert!(
            out.status.success(),
            "`{body}` must build; got:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let run = Command::new(&exe).output().expect("run the program");
        let got = String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n");
        assert_eq!(got, *want, "`{body}` printed the wrong thing");
        assert_eq!(run.status.code(), Some(0), "`{body}` did not exit cleanly");
    }
}

/// Programs outside the template: a tail whose value is not an `i64`, an operation with two
/// parameters, and generic effecting fns (register D70) — whose instances were lowered as
/// plain fns, so `if n > 0 { perform Io.read() } else { 42 }` answered the continuation's
/// address for `n > 0`, and a let-bound call panicked the compiler.
#[test]
fn embedded_perform_refuses_other_unlowerable_bodies() {
    const GENERIC: &str = "it is generic, and a generic effecting fn does not get";
    let head = "effect Io {\n    read() -> i64;\n}\n\n";
    let handled = "fn main() -> i64 {\n    print(handle c(3) with {\n        Io.read(k) => k(5)\n    });\n    0\n}\n";
    let cases: &[(&str, String, &str, &str)] = &[
        (
            "value_u8",
            format!("{head}fn c(n: i64) -> u8 ! {{ Io }} {{\n    (perform Io.read() + n) as u8\n}}\n\nfn main() -> i64 {{\n    let r: u8 = handle c(3) with {{\n        Io.read(k) => k(5)\n    }};\n    r as i64\n}}\n"),
            NOT_DIRECT,
            "its tail answers `u8`",
        ),
        (
            "two_param_op",
            "effect Io {\n    put(a: i64, b: i64) -> i64;\n}\n\nfn c(n: i64) -> i64 ! { Io } {\n    perform Io.put(n, n + 1) + 1\n}\n\nfn main() -> i64 {\n    handle c(3) with {\n        Io.put(a, b, k) => k(a)\n    }\n}\n".to_string(),
            "sentinel::codegen::operation_arity_not_supported",
            "takes 2 parameters",
        ),
        (
            "generic_guarded",
            format!("{head}fn c<T>(n: i64, t: T) -> i64 ! {{ Io }} {{\n    if n > 0 {{ perform Io.read() }} else {{ 42 }}\n}}\n\nfn main() -> i64 {{\n    print(handle c(3, 0) with {{\n        Io.read(k) => k(5)\n    }});\n    0\n}}\n"),
            NOT_DIRECT,
            GENERIC,
        ),
        (
            "generic_let_caller",
            format!("{head}fn g<T>(n: i64, t: T) -> i64 ! {{ Io }} {{\n    perform Io.read() + n\n}}\n\nfn c(n: i64) -> i64 ! {{ Io }} {{\n    let s: i64 = g(n, 0);\n    s + 1\n}}\n\n{handled}"),
            NOT_DIRECT,
            GENERIC,
        ),
    ];
    for (name, src, code, reason) in cases {
        assert_refused(name, src, code, reason);
    }
}

/// A method call in a let-bound value writes no captured variable — a method's receiver is
/// a class or a struct, which no frame slot holds — so the let shape still lowers it.
#[test]
fn embedded_perform_builds_let_with_method_call() {
    let src = "effect Io {\n    echo(x: i64) -> i64;\n}\n\n\
               class Pt {\n    let x: i64;\n    pub init(x: i64) {\n        self.x = x;\n        0\n    }\n    \
               pub fn get(self: &Self) -> i64 {\n        self.x\n    }\n}\n\n\
               fn e2(n: i64, q: Pt) -> i64 ! { Io } {\n    let x = perform Io.echo(q.get());\n    x + n\n}\n\n\
               fn run(n: i64) -> i64 {\n    handle e2(n, Pt::init(n + 100)) with {\n        \
               Io.echo(x, k) => {\n            print(x);\n            k(x)\n        }\n    }\n}\n\n\
               fn main() -> i64 {\n    print(run(0));\n    print(run(3));\n    0\n}\n";
    let (out, exe) = build("let_method", src);
    assert!(out.status.success(), "must build; got:\n{}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&exe).output().expect("run the program");
    let got = String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n");
    assert_eq!(got, "100\n100\n103\n106\n");
    assert_eq!(run.status.code(), Some(0));
}
