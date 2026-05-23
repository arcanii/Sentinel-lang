# B1.7: hand-rolled caret diagnostics.
# Zsh-paste-safe: no exit, no set -e, no unquoted parens in echoes.

run_b17() {
  cd "$(git rev-parse --show-toplevel)" || { echo 'not in a git repo'; return 1; }

  local CRATE='crates/sentinel-effects-proto'
  local DIAG="$CRATE/src/diag.rs"
  local LIB="$CRATE/src/lib.rs"

  if [ ! -f "$LIB" ]; then
    echo "missing $LIB"; return 1
  fi
  if [ -f "$DIAG" ]; then
    echo "$DIAG already exists; refusing to overwrite"; return 1
  fi

  # ---------- write src/diag.rs ----------
  cat > "$DIAG" <<'DIAG_EOF'
//! Hand-rolled caret diagnostics for Sentinel-Mini.
//!
//! Phase C will likely adopt `miette`; B1.7 stays dependency-free so we
//! can validate the *shape* of diagnostics (line/col location, caret
//! underline, source line excerpt) without committing to that crate's
//! ergonomics in the prototype.
//!
//! # Multi-line spans
//!
//! Sentinel-Mini programs are usually one-liners. When a span crosses a
//! newline we clip the caret to the first line of the span and trust
//! the message to convey the rest. A B2/Phase C diagnostic crate can
//! revisit this.

use crate::span::Span;

/// 1-indexed line/column derived from a byte offset into a source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Locate a byte offset within `source`, returning a 1-indexed line and
/// column. Offsets past the end clamp to one-past-the-last character.
pub fn locate(source: &str, offset: u32) -> LineCol {
    let off = (offset as usize).min(source.len());
    let prefix = &source[..off];
    let line = (prefix.bytes().filter(|b| *b == b'\n').count() as u32) + 1;
    let last_nl = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = (off - last_nl) as u32 + 1;
    LineCol { line, col }
}

/// Render a diagnostic with a source-line excerpt and caret underline.
///
/// If `span` is `None` (or empty), the caret block is omitted and the
/// output is a single line: `"<severity>: <message>"`.
///
/// The format follows rustc's `--> file:line:col` convention so muscle
/// memory carries over to Phase C.
pub fn render(
    source: &str,
    span: Option<Span>,
    severity: &str,
    message: &str,
) -> String {
    let span = match span {
        Some(s) if s.start < s.end || (s.start as usize) < source.len() => s,
        _ => return format!("{severity}: {message}"),
    };

    let start = locate(source, span.start);
    let line_start_byte = nth_line_start(source, start.line);
    let line_end_byte = nth_line_end(source, line_start_byte);
    let line_text = &source[line_start_byte..line_end_byte];

    // Clip the caret to the first line of the span.
    let caret_start_col = start.col as usize;
    let span_end_on_this_line =
        (span.end as usize).min(line_end_byte) - line_start_byte;
    let caret_end_col = span_end_on_this_line + 1;
    let caret_len = caret_end_col.saturating_sub(caret_start_col).max(1);

    let line_no = start.line.to_string();
    let gutter = " ".repeat(line_no.len());
    let pad_before_caret = " ".repeat(caret_start_col.saturating_sub(1));
    let carets = "^".repeat(caret_len);

    let mut out = String::new();
    out.push_str(&format!("{severity}: {message}\n"));
    out.push_str(&format!("  {gutter}--> input:{}:{}\n", start.line, start.col));
    out.push_str(&format!("  {gutter} |\n"));
    out.push_str(&format!("  {line_no} | {line_text}\n"));
    out.push_str(&format!("  {gutter} | {pad_before_caret}{carets}"));
    out
}

fn nth_line_start(source: &str, line: u32) -> usize {
    // line is 1-indexed.
    if line <= 1 {
        return 0;
    }
    let mut count = 1u32;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == line {
                return i + 1;
            }
        }
    }
    source.len()
}

fn nth_line_end(source: &str, line_start: usize) -> usize {
    match source[line_start..].find('\n') {
        Some(rel) => line_start + rel,
        None => source.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_at_start_is_line_1_col_1() {
        let lc = locate("hello", 0);
        assert_eq!(lc, LineCol { line: 1, col: 1 });
    }

    #[test]
    fn locate_after_newline_advances_line_and_resets_col() {
        let src = "abc\ndef";
        let lc = locate(src, 4); // 'd'
        assert_eq!(lc, LineCol { line: 2, col: 1 });
    }

    #[test]
    fn locate_past_eof_clamps() {
        let src = "ab";
        let lc = locate(src, 99);
        // 'a' 'b' then end -> col 3 on line 1
        assert_eq!(lc, LineCol { line: 1, col: 3 });
    }

    #[test]
    fn render_single_line_underlines_span() {
        let src = "let x = 1 + true";
        // Span of "true" is bytes 12..16.
        let out = render(src, Some(Span::new(12, 16)), "type error", "Int vs Bool");
        // Expected shape:
        //   type error: Int vs Bool
        //     --> input:1:13
        //      |
        //    1 | let x = 1 + true
        //      |             ^^^^
        assert!(out.contains("type error: Int vs Bool"), "header: {out}");
        assert!(out.contains("--> input:1:13"), "locator: {out}");
        assert!(out.contains("1 | let x = 1 + true"), "source line: {out}");
        assert!(out.contains("^^^^"), "caret length: {out}");
        // Caret should be exactly under "true" -- 12 spaces of padding,
        // then ^^^^. We check the last line directly.
        let last = out.lines().last().expect("nonempty");
        assert_eq!(last, "    |             ^^^^", "last line: {last:?}");
    }

    #[test]
    fn render_multi_line_span_clips_to_first_line() {
        let src = "let x =\n  1 + true\nin x";
        // Pretend a span runs from byte 4 (the 'x') all the way to the
        // 'e' in "true" on line 2.
        let span = Span::new(4, 18);
        let out = render(src, Some(span), "error", "spans two lines");
        // The caret should appear on line 1's excerpt, clipped at end of
        // line 1 (byte 7).
        assert!(out.contains("1 | let x ="), "first-line source: {out}");
        // No second source line is emitted.
        assert!(!out.contains("2 |"), "should not render line 2: {out}");
    }

    #[test]
    fn render_no_span_omits_caret_block() {
        let out = render("ignored", None, "error", "no location");
        assert_eq!(out, "error: no location");
    }

    #[test]
    fn render_empty_source_with_no_span_is_terse() {
        let out = render("", None, "parse error", "unexpected end of input");
        assert_eq!(out, "parse error: unexpected end of input");
    }
}
DIAG_EOF

  echo "wrote $DIAG"

  # ---------- patch src/lib.rs: declare diag, add MiniError::render ----------
  local PY='/tmp/b17_patch_lib.py'
  cat > "$PY" <<'PY_EOF'
import sys, pathlib

path = pathlib.Path(sys.argv[1])
src = path.read_text()

# 1) Add `pub mod diag;` after `pub mod ast;` (alphabetic order).
needle = "pub mod ast;\n"
if needle not in src:
    sys.stderr.write("could not find 'pub mod ast;'\n")
    sys.exit(2)
src = src.replace(needle, needle + "pub mod diag;\n", 1)

# 2) After the closing brace of `pub enum MiniError { ... }`, append the
#    `impl MiniError` block with the `render` method.
marker = "    Eval(#[from] EvalError),\n}\n"
if marker not in src:
    sys.stderr.write("could not find MiniError closing brace\n")
    sys.exit(3)

impl_block = '''
impl MiniError {
    /// Render this error with a caret-underlined source excerpt.
    ///
    /// `Display` produces a single terse line; this method produces the
    /// multi-line rustc-style diagnostic. Callers that have the source
    /// string handy should prefer this for human-facing output.
    pub fn render(&self, source: &str) -> String {
        use crate::diag::render;
        match self {
            MiniError::Lex(e) => match e {
                LexError::Unrecognised { span, .. } => {
                    render(source, Some(*span), "lex error", &e.to_string())
                }
            },
            MiniError::Parse(e) => {
                let span = match e {
                    ParseError::UnexpectedEof { .. } => None,
                    ParseError::Unexpected { span, .. } => Some(*span),
                    ParseError::Trailing { span, .. } => Some(*span),
                    ParseError::LetRecNotLambda { span } => Some(*span),
                };
                render(source, span, "parse error", &e.to_string())
            }
            MiniError::Type(e) => {
                let span = match e {
                    TypeError::Mismatch { span, .. } => Some(*span),
                    TypeError::OccursCheck { span, .. } => Some(*span),
                    TypeError::Unbound { span, .. } => Some(*span),
                };
                render(source, span, "type error", &e.to_string())
            }
            MiniError::Eval(e) => {
                // Eval errors carry no spans in B1; they print terse.
                render(source, None, "eval error", &e.to_string())
            }
        }
    }
}
'''

src = src.replace(marker, marker + impl_block, 1)

path.write_text(src)
print('patched', path)
PY_EOF

  python3 "$PY" "$LIB"
  local PATCH_RC=$?
  if [ "$PATCH_RC" != "0" ]; then
    echo "lib.rs patch failed rc=$PATCH_RC"; return "$PATCH_RC"
  fi

  # ---------- add an end-to-end integration test ----------
  local INT_TEST="$CRATE/tests/integration.rs"
  local PY2='/tmp/b17_patch_int.py'
  cat > "$PY2" <<'PY_EOF'
import sys, pathlib

path = pathlib.Path(sys.argv[1])
src = path.read_text()

addition = r'''

#[test]
fn pipeline_type_error_renders_with_caret() {
    use sentinel_effects_proto::run;
    let source = "1 + true";
    let err = run(source).expect_err("Int + Bool should fail type-check");
    let rendered = err.render(source);
    // Header carries the severity tag.
    assert!(rendered.starts_with("type error:"), "header: {rendered}");
    // Source line excerpt is present.
    assert!(rendered.contains("1 | 1 + true"), "excerpt: {rendered}");
    // Some caret is present.
    assert!(rendered.contains("^"), "caret: {rendered}");
}
'''

if 'pipeline_type_error_renders_with_caret' in src:
    print('integration test already present; skipping')
else:
    src = src.rstrip() + addition
    path.write_text(src)
    print('patched', path)
PY_EOF

  python3 "$PY2" "$INT_TEST"
  local INT_RC=$?
  if [ "$INT_RC" != "0" ]; then
    echo "integration.rs patch failed rc=$INT_RC"; return "$INT_RC"
  fi

  # ---------- build / clippy / test ----------
  echo
  echo '====== BUILD ======'
  cargo build -p sentinel-effects-proto
  local BUILD_RC=$?

  echo
  echo '====== CLIPPY ======'
  cargo clippy -p sentinel-effects-proto --all-targets -- -D warnings
  local CLIPPY_RC=$?

  echo
  echo '====== LIB TESTS ======'
  cargo test -p sentinel-effects-proto --lib
  local LIB_RC=$?

  echo
  echo '====== INTEGRATION TESTS ======'
  cargo test -p sentinel-effects-proto --test integration
  local INTTEST_RC=$?

  echo
  echo '====== DOC TESTS ======'
  cargo test -p sentinel-effects-proto --doc
  local DOC_RC=$?

  echo
  echo '====== SUMMARY ======'
  echo "build:       $BUILD_RC"
  echo "clippy:      $CLIPPY_RC"
  echo "lib tests:   $LIB_RC"
  echo "integration: $INTTEST_RC"
  echo "doc tests:   $DOC_RC"

  echo
  echo '====== GIT STATUS ======'
  git status --short

  echo
  echo '====== DONE ======'
  echo 'Expected: lib tests 82 -> 88 (+6 in diag). Integration 5 -> 6 (+1 render test). Total 94.'
  echo 'If green, commit: feat mini B1.7 caret diagnostics'
  echo "then say 'go' for scripts/20-b1.8-state-refresh.sh."
}

run_b17
