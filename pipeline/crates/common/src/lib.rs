// ─── barquitos-common ────────────────────────────────────────────────────────
//
// Shared library used by all Barquitos pipeline binaries:
//   • config      — load config.toml at runtime (no recompile needed)
//   • mount_check — disk health verification with timeout protection
//   • safe_io     — file reads with timeout + quarantine (anti-crash)
// ─────────────────────────────────────────────────────────────────────────────

pub mod config;
pub mod mount_check;
pub mod safe_io;
