# `examples/` — programs that use the core libraries

Real, idiomatic Sentinel programs that `use` the [`std/`](../std/) core
libraries. Each one doubles as a **feature test**: it is compiled and run by
`cargo nextest`, and its exit code is asserted — the "exit-code-is-the-answer"
convention.

## Layout mirrors `std/`

`examples/` is subdivided by the same functional categories as `std/`, so an
example lives next to the kind of library it exercises:

```
examples/
  security/secure_compare.sentinel   →  uses std::security::ct
  math/...                            →  uses std::math::num
```

## How each example is tested

The harness, [`crates/sentinel-driver/tests/examples.rs`](../crates/sentinel-driver/tests/examples.rs),
builds every example **twice from the same source** and asserts both back ends
agree with each other and with the expected exit code:

1. `snc build <entry> --separate` — the per-unit separate-compilation back end
   (each module → its own object, linked by module-qualified `abi-v1` symbols).
   This **dogfoods the module system + `--separate`** on real multi-module
   programs, including the incremental `.o.fp` cache.
2. `snc build <entry>` — the default merge path.

A successful build also means the **constant-time check** passed
(`sentinel::mir::secret_leak` runs on every build), so an example that carries
`secret` values compiling at all is a proof that the constant-time discipline
held across the call graph.

Every `.sentinel` file here must be registered in the harness's `EXAMPLES`
table (a coverage-guard test enforces it), so no example is silently untested.
