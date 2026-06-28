//! ADR 0063 — the pre-built-library interface descriptor (`.sif`).
//!
//! A compiled Sentinel library ships its object alongside a `.sif` descriptor: the
//! serialized public interface a consumer needs to resolve `use lib::item` against
//! the binary (the ADR 0037 exports table — pub fn signatures + effect rows, with
//! `pub struct`/`pub enum` layouts + generic bodies to follow).
//!
//! **Format (a)** (ADR 0063 D2a, owner-chosen): a structured **header** read by the
//! dedicated reader here, then an **interface body** of ordinary Sentinel signature
//! declarations, reconstructed via the existing parser + [`extract_exports`]. The
//! body adds **no new lexer/parser syntax** (a non-generic `pub fn` carries a stub
//! `{ 0 }` body the extractor discards), so the descriptor is **not oracle-moving**.
//! The header carries the trust-relevant metadata the ADR 0061 gate binds — the
//! object's SHA-512, the `abi-v1` + compiler versions, the module path.
//!
//! v1 slice: non-generic `pub fn` signatures. Layout decls (`pub struct`/`pub
//! enum`) + generic-fn bodies are the next increment.
//!
//! The format + reader are exercised by this module's tests; the non-test wiring
//! (`snc build … --emit-interface` on the producer side, `.sif`-backed resolution
//! on the consumer side) lands in the following ADR 0063 increments — until then
//! these items are dead in a non-test build.
#![allow(dead_code)]

use crate::{extract_exports, ExportedFn, ExportedItem};

const MAGIC: &str = "sentinel-interface";
const VERSION: &str = "v1";
/// The header / interface-body separator.
const SEP: &str = "---";

/// Parsed `.sif` header metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceHeader {
    pub version: String,
    pub abi: String,
    pub compiler: String,
    pub module: String,
    pub object_sha512: String,
}

fn kind_name(item: &ExportedItem) -> &'static str {
    match item {
        ExportedItem::Fn(_) => "non-generic fn",
        ExportedItem::Struct(_) => "struct",
        ExportedItem::Enum(_) => "enum",
        ExportedItem::GenericFn(_) => "generic fn",
        ExportedItem::Trait(_) => "trait",
        ExportedItem::Effect(_) => "effect",
    }
}

/// Render one non-generic fn export as a parseable Sentinel signature decl. The
/// `{ 0 }` stub body parses for any return type (the parser does not type-check)
/// and is discarded by [`extract_exports`] for a non-generic fn — so only the
/// signature (param/return types + effect row) crosses, not the implementation.
fn render_fn(name: &str, ef: &ExportedFn) -> String {
    let params: Vec<String> = ef
        .param_type_exprs
        .iter()
        .enumerate()
        .map(|(i, t)| format!("_p{i}: {t}"))
        .collect();
    let effects = if ef.effect_row_names.is_empty() {
        String::new()
    } else {
        format!(" ! {{ {} }}", ef.effect_row_names.join(", "))
    };
    format!("pub fn {name}({}) -> {}{} {{ 0 }}\n", params.join(", "), ef.return_type_expr, effects)
}

/// Render the interface descriptor for a library module `module` whose compiled
/// object hashes to `object_sha512_hex`.
pub fn render_interface(
    module: &str,
    object_sha512_hex: &str,
    exports: &[(String, ExportedItem)],
) -> Result<String, String> {
    let mut body = String::new();
    for (name, item) in exports {
        match item {
            ExportedItem::Fn(ef) => body.push_str(&render_fn(name, ef)),
            other => {
                return Err(format!(
                    "`{name}`: the `.sif` descriptor v1 supports only non-generic `pub fn`s \
                     (found a {}); struct/enum/generic support is the next increment",
                    kind_name(other)
                ));
            }
        }
    }
    let mut out = String::new();
    out.push_str(&format!("{MAGIC} {VERSION}\n"));
    out.push_str("abi 1\n");
    out.push_str(&format!("compiler {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("module {module}\n"));
    out.push_str(&format!("object-sha512 {object_sha512_hex}\n"));
    out.push_str(SEP);
    out.push('\n');
    out.push_str(&body);
    Ok(out)
}

/// Read a `.sif`: parse the header (the dedicated reader), then reconstruct the
/// exports table from the body via the existing parser + [`extract_exports`].
pub fn read_interface(text: &str) -> Result<(InterfaceHeader, Vec<(String, ExportedItem)>), String> {
    let Some((header_text, body)) = text.split_once(&format!("\n{SEP}\n")) else {
        return Err("malformed `.sif`: missing the `---` header/body separator".to_string());
    };

    let (mut version, mut abi, mut compiler, mut module, mut object) = (None, None, None, None, None);
    for raw in header_text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(&format!("{MAGIC} ")) {
            version = Some(rest.trim().to_string());
            continue;
        }
        let Some((k, v)) = line.split_once(' ') else { continue };
        let v = v.trim().to_string();
        match k {
            "abi" => abi = Some(v),
            "compiler" => compiler = Some(v),
            "module" => module = Some(v),
            "object-sha512" => object = Some(v),
            _ => {} // forward-compatible: ignore unknown header keys.
        }
    }
    let bad = |m: &str| m.to_string();
    let header = InterfaceHeader {
        version: version.ok_or_else(|| bad("missing `sentinel-interface` header line"))?,
        abi: abi.ok_or_else(|| bad("missing `abi`"))?,
        compiler: compiler.ok_or_else(|| bad("missing `compiler`"))?,
        module: module.ok_or_else(|| bad("missing `module`"))?,
        object_sha512: object.ok_or_else(|| bad("missing `object-sha512`"))?,
    };

    let program = sentinel_syntax::parse(body)
        .map_err(|e| format!("malformed `.sif` interface body: {e:?}"))?;
    let exports = extract_exports(&program)?;
    Ok((header, exports))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fn_descriptor_round_trips() {
        // A library of pub fns (one effectful), plus a private helper that must
        // NOT cross the interface.
        let src = "\
pub fn add(a: i64, b: i64) -> i64 { a + b }\n\
pub fn ct_byte_eq(a: [u8], b: [u8]) -> i64 { 0 }\n\
pub fn widen(x: i64) -> secret i64 { 0 }\n\
fn private_helper() -> i64 { 0 }\n";
        let prog = sentinel_syntax::parse(src).unwrap();
        let exports = extract_exports(&prog).unwrap();

        let sif = render_interface("cryptolib", "deadbeef", &exports).unwrap();
        let (hdr, exports2) = read_interface(&sif).unwrap();

        assert_eq!(hdr.module, "cryptolib");
        assert_eq!(hdr.object_sha512, "deadbeef");
        assert_eq!(hdr.version, VERSION);

        // Re-rendering the read-back table is byte-identical (the format is a
        // faithful round-trip of the exports table).
        let sif2 = render_interface("cryptolib", "deadbeef", &exports2).unwrap();
        assert_eq!(sif, sif2);

        // The three pub fns crossed (incl. the `[u8]` and `secret i64` types);
        // the private helper did not.
        assert!(sif.contains("pub fn add(_p0: i64, _p1: i64) -> i64"), "sif:\n{sif}");
        assert!(sif.contains("pub fn ct_byte_eq(_p0: [u8], _p1: [u8]) -> i64"), "sif:\n{sif}");
        assert!(sif.contains("-> secret i64"), "sif:\n{sif}");
        assert!(!sif.contains("private_helper"), "a private fn must not cross:\n{sif}");
    }

    #[test]
    fn malformed_descriptor_is_reported() {
        assert!(read_interface("no separator here").is_err());
        assert!(read_interface("sentinel-interface v1\n---\npub fn (").is_err());
    }

    #[test]
    fn non_fn_exports_are_rejected_in_v1() {
        let prog = sentinel_syntax::parse("pub struct Point { x: i64, y: i64 }\n").unwrap();
        let exports = extract_exports(&prog).unwrap();
        assert!(render_interface("m", "h", &exports).is_err());
    }
}
