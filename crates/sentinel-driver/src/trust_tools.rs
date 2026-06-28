//! ADR 0061 — the `snc sign` / `snc keygen` authoring subcommands.
//!
//! Signing crypto stays in Sentinel (the dogfooded `tools/trust/{sign,keygen}_core`,
//! which use the verified-constant-time `std::security::ed25519`); these commands
//! are the **Rust orchestration** around them: argument parsing, file I/O, the
//! canonical payload + carrier (so the signature format has a single Rust-only
//! implementation, ADR 0061 D2/D3), and shelling out to the Sentinel core for the
//! Ed25519 math over opaque bytes.
//!
//! The cores are located next to the `snc` executable (the toolchain ships them
//! there), or via an explicit `--signer` / `--keygen-tool` path. Because Sentinel
//! has no argv, data crosses through fixed filenames in a private temp dir (the
//! `input.sentinel` convention the self-host compiler uses).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sentinel_trust::{
    canonical_payload, evaluate, serialize_carrier, sha512, GateOutcome, SigPolicy, SignedObject,
    TrustManifest,
};

/// The default consumer trust manifest filename, looked for in the working
/// directory when `--trust` is not given.
const DEFAULT_MANIFEST: &str = "sentinel-trust.toml";

/// The ADR 0061 trust-gate configuration parsed from `snc build` flags.
pub struct TrustOpts {
    pub policy: SigPolicy,
    pub manifest_path: Option<String>,
}

impl Default for TrustOpts {
    fn default() -> Self {
        Self { policy: SigPolicy::Off, manifest_path: None }
    }
}

/// One module presented to the gate: a display label, its source file (whose
/// `<file>.sig` is the detached carrier), its raw bytes, and the **capabilities**
/// its code exercises (ADR 0061 D6 — checked against the signing key's grants).
pub struct GateUnit {
    pub label: String,
    pub file: PathBuf,
    pub source: Vec<u8>,
    pub capabilities: Vec<String>,
}

fn short_fp(pk: &[u8; 32]) -> String {
    pk[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn sig_path_for(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_os_string();
    s.push(".sig");
    PathBuf::from(s)
}

fn load_manifest(explicit: Option<&str>) -> Result<TrustManifest, String> {
    let path = match explicit {
        Some(p) => PathBuf::from(p),
        None => {
            let def = PathBuf::from(DEFAULT_MANIFEST);
            if !def.is_file() {
                // No manifest → trust nobody (the gate reports per policy). Strict
                // then refuses everything, which is the safe default.
                return Ok(TrustManifest::default());
            }
            def
        }
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read trust manifest `{}`: {e}", path.display()))?;
    TrustManifest::parse(&text)
}

/// Run the ADR 0061 build-time trust gate (D7) over `units` under `opts`. On a
/// strict violation, prints diagnostics and returns the build's exit code. On
/// success returns each trusted module's **effective grants** (label → grants)
/// for the capability check (D6).
pub fn run_trust_gate(
    units: &[GateUnit],
    opts: &TrustOpts,
) -> Result<HashMap<String, Vec<String>>, ExitCode> {
    let mut grants_by_label = HashMap::new();
    if opts.policy == SigPolicy::Off {
        return Ok(grants_by_label);
    }
    let manifest = match load_manifest(opts.manifest_path.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("snc: trust: {e}");
            return Err(ExitCode::from(1));
        }
    };

    let mut violations = 0u32;
    for u in units {
        let sig_path = sig_path_for(&u.file);
        let carrier = std::fs::read_to_string(&sig_path).ok();
        match evaluate(&u.source, carrier.as_deref(), &manifest) {
            GateOutcome::Trusted { pubkey, grants } => {
                // D6 capability bounding: every capability the module exercises
                // must be granted to its signing key. A trusted-but-over-reaching
                // module (e.g. a crypto key's code opening FFI) is refused — a
                // compromised key cannot pivot beyond its declared envelope.
                let ungranted: Vec<&String> =
                    u.capabilities.iter().filter(|c| !grants.contains(c)).collect();
                if ungranted.is_empty() {
                    eprintln!("snc: trust: `{}` verified — key {}…", u.label, short_fp(&pubkey));
                    grants_by_label.insert(u.label.clone(), grants);
                } else {
                    report(
                        opts.policy,
                        &mut violations,
                        &format!(
                            "`{}` exercises capabilit{} {:?} not granted to its key {}… (grants: {:?})",
                            u.label,
                            if ungranted.len() == 1 { "y" } else { "ies" },
                            ungranted,
                            short_fp(&pubkey),
                            grants
                        ),
                    );
                }
            }
            GateOutcome::Unsigned => report(
                opts.policy,
                &mut violations,
                &format!("`{}` is unsigned (no `{}`)", u.label, sig_path.display()),
            ),
            GateOutcome::BadSignature(e) => {
                report(opts.policy, &mut violations, &format!("`{}`: {e}", u.label));
            }
            GateOutcome::Untrusted { pubkey } => report(
                opts.policy,
                &mut violations,
                &format!(
                    "`{}` is signed by an UNTRUSTED key {}… (not in the trust manifest)",
                    u.label,
                    short_fp(&pubkey)
                ),
            ),
        }
    }

    if opts.policy == SigPolicy::Strict && violations > 0 {
        eprintln!(
            "snc: trust: {violations} signature violation(s) — build refused (--require-signatures strict)"
        );
        return Err(ExitCode::from(1));
    }
    Ok(grants_by_label)
}

fn report(policy: SigPolicy, violations: &mut u32, msg: &str) {
    match policy {
        SigPolicy::Strict => {
            eprintln!("snc: trust: ERROR: {msg}");
            *violations += 1;
        }
        SigPolicy::Warn => eprintln!("snc: trust: warning: {msg}"),
        SigPolicy::Off => {}
    }
}

/// Locate a signing core by name (`sign_core` / `keygen_core`): an explicit
/// `--…` path if given, else next to the running `snc` executable.
fn locate_core(explicit: Option<&str>, name: &str) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        let pb = PathBuf::from(p);
        return if pb.is_file() {
            Ok(pb)
        } else {
            Err(format!("signing tool `{p}` not found"))
        };
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the snc executable: {e}"))?;
    let dir = exe.parent().ok_or_else(|| "snc executable has no parent directory".to_string())?;
    let cand = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if cand.is_file() {
        Ok(cand)
    } else {
        Err(format!(
            "signing tool `{name}` not found at `{}` — build it once:\n    \
             snc build tools/trust/{name}.sentinel --lib-path <repo-root> -o {}\n  \
             or pass its path explicitly.",
            cand.display(),
            cand.display()
        ))
    }
}

/// Run a Sentinel core in a private temp dir: write `inputs` (fixed filenames),
/// run the core there, read back `output`. The core reads/writes its CWD.
fn run_core(core: &Path, inputs: &[(&str, &[u8])], output: &str) -> Result<Vec<u8>, String> {
    let work = std::env::temp_dir().join(format!("snc_trust_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create temp dir: {e}"))?;
    for (name, bytes) in inputs {
        std::fs::write(work.join(name), bytes).map_err(|e| format!("cannot stage `{name}`: {e}"))?;
    }
    let status = Command::new(core)
        .current_dir(&work)
        .status()
        .map_err(|e| format!("cannot run `{}`: {e}", core.display()))?;
    if !status.success() {
        return Err(format!("`{}` exited with {status}", core.display()));
    }
    let out = std::fs::read(work.join(output)).map_err(|e| format!("`{output}` not produced: {e}"))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(out)
}

/// `snc keygen [-o <keyfile>] [--keygen-tool <exe>]` — generate an Ed25519
/// keypair via the Sentinel `keygen_core`. Writes a 64-byte key file
/// (seed‖pubkey); prints the public-key fingerprint. The seed is the SECRET key —
/// protect the file (v1 stores it unencrypted; passphrase protection is a
/// follow-up).
pub fn run_keygen(args: &[String]) -> ExitCode {
    let mut out: Option<&str> = None;
    let mut tool: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                match args.get(i) {
                    Some(o) => out = Some(o),
                    None => return err("`-o` requires a path"),
                }
            }
            "--keygen-tool" => {
                i += 1;
                match args.get(i) {
                    Some(t) => tool = Some(t),
                    None => return err("`--keygen-tool` requires a path"),
                }
            }
            other => return err(&format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    let out_path = out.unwrap_or("sentinel.key");

    let core = match locate_core(tool, "keygen_core") {
        Ok(c) => c,
        Err(e) => return err(&e),
    };
    let key = match run_core(&core, &[], "keyout.bin") {
        Ok(k) => k,
        Err(e) => return err(&e),
    };
    if key.len() != 64 {
        return err(&format!("keygen_core produced {} bytes, expected 64 (seed||pubkey)", key.len()));
    }
    if let Err(e) = std::fs::write(out_path, &key) {
        return err(&format!("cannot write key file `{out_path}`: {e}"));
    }
    let fp: String = key[32..40].iter().map(|b| format!("{b:02x}")).collect();
    println!("snc: wrote keypair to `{out_path}` — public key {fp}… (KEEP THIS FILE SECRET)");
    ExitCode::SUCCESS
}

/// `snc sign <file> [-o <sig>] --key <keyfile> [--grant <cap>]... [--signer <exe>]`
/// — sign `<file>` with the Sentinel `sign_core`, producing a detached carrier
/// (default `<file>.sig`). The whole file is the signed body.
pub fn run_sign(args: &[String]) -> ExitCode {
    let mut file: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut keyfile: Option<&str> = None;
    let mut signer: Option<&str> = None;
    let mut grants: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                match args.get(i) {
                    Some(o) => out = Some(o),
                    None => return err("`-o` requires a path"),
                }
            }
            "--key" => {
                i += 1;
                match args.get(i) {
                    Some(k) => keyfile = Some(k),
                    None => return err("`--key` requires a key file"),
                }
            }
            "--grant" => {
                i += 1;
                match args.get(i) {
                    Some(g) => grants.push((*g).clone()),
                    None => return err("`--grant` requires a capability name"),
                }
            }
            "--signer" => {
                i += 1;
                match args.get(i) {
                    Some(s) => signer = Some(s),
                    None => return err("`--signer` requires a path"),
                }
            }
            other => {
                if file.is_some() {
                    return err(&format!("unexpected argument `{other}`"));
                }
                file = Some(other);
            }
        }
        i += 1;
    }
    let Some(file) = file else { return err("`sign` requires a source file") };
    let Some(keyfile) = keyfile else { return err("`sign` requires `--key <keyfile>`") };

    let body = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => return err(&format!("cannot read `{file}`: {e}")),
    };
    let key = match std::fs::read(keyfile) {
        Ok(k) => k,
        Err(e) => return err(&format!("cannot read key `{keyfile}`: {e}")),
    };
    if key.len() != 64 {
        return err(&format!("key file `{keyfile}` is {} bytes, expected 64 (seed||pubkey)", key.len()));
    }
    let seed = &key[..32];
    let pubkey: [u8; 32] = key[32..].try_into().expect("32 bytes");

    // payload = domain || algo || pubkey || grants || SHA512(body) (Rust-built).
    let payload = canonical_payload(&pubkey, &grants, &sha512(&body));

    let core = match locate_core(signer, "sign_core") {
        Ok(c) => c,
        Err(e) => return err(&e),
    };
    let sigout = match run_core(&core, &[("seed.bin", seed), ("payload.bin", &payload)], "sigout.bin") {
        Ok(s) => s,
        Err(e) => return err(&e),
    };
    if sigout.len() != 96 {
        return err(&format!("sign_core produced {} bytes, expected 96 (pubkey||sig)", sigout.len()));
    }
    // sign_core emits pubkey||sig; cross-check its pubkey against the key file.
    if sigout[..32] != pubkey {
        return err("sign_core's derived public key does not match the key file (corrupt key?)");
    }
    let signature: [u8; 64] = sigout[32..].try_into().expect("64 bytes");

    let carrier = serialize_carrier(&SignedObject { pubkey, grants, signature });
    let sig_path = out.map_or_else(|| format!("{file}.sig"), String::from);
    if let Err(e) = std::fs::write(&sig_path, &carrier) {
        return err(&format!("cannot write signature `{sig_path}`: {e}"));
    }
    println!("snc: signed `{file}` → `{sig_path}`");
    ExitCode::SUCCESS
}

fn err(msg: &str) -> ExitCode {
    eprintln!("snc: {msg}");
    ExitCode::from(2)
}
