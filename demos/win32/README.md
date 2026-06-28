# Win32 GUI demos (ADR 0057 + 0060)

Windows-only demonstrations that **Sentinel calls native GUI functions** over the
C-ABI FFI (ADR 0057), linked against `user32.dll` via the extra-library `--link`
flag (ADR 0057 pillar 4) on the Windows host-toolchain backend (ADR 0060).

These live under `demos/` (not `examples/`) on purpose: `examples/` is the
cross-platform, CI-enforced corpus built on every host, whereas these are
platform-specific and link a native GUI library, so they are built and run
manually on Windows.

## Building

From a **Developer Command Prompt for VS** (so the MSVC `link.exe` + libraries are
on `PATH`), with `snc` built (`cargo build -p sentinel-driver -p sentinel-runtime`):

```
snc build demos/win32/screen_metrics.sentinel -o screen_metrics.exe
snc build demos/win32/messagebox.sentinel     -o messagebox.exe
```

(No `--link` flag: each `extern` block declares `link("user32")`, so `snc`
self-links it — ADR 0057 A9. You can still add `--link <lib>` manually for libs
a program needs beyond what its modules declare.)

- **`screen_metrics`** — calls `GetSystemMetrics` (non-blocking); prints the
  primary screen width + height; exits `42` when both are positive. Good for an
  automated check.
- **`messagebox`** — STANDALONE (one file, no `use`): inlines the same Win32
  bindings as `std::sys::win32` (`message_box` / `screen_width|height` / `beep`);
  prints the screen size, beeps, then pops a real message box (blocks until OK).
- **`messagebox_compact`** — the LIBRARY-USING version of the same program:
  `use std::sys::win32::message_box;` instead of inlining, so it is a few lines.
  The `std/` library lives under `sentinel_library/` (ADR 0064), not adjacent to
  `demos/`; point `snc` at it with `--lib-path` (ADR 0037 point 12) so it builds
  IN PLACE — no copy needed. From the repo root:

  ```
  snc build demos\win32\messagebox_compact.sentinel -o %TEMP%\messagebox_compact.exe --lib-path sentinel_library
  ```

These prove that Sentinel calls real Win32 GUI symbols from verified code — and,
with `std::sys::win32` self-linking `user32` (ADR 0057 A9), a real program is a
plain `use` + call with no `--link` flag.
