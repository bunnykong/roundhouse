//! Test-owned scratch directories (#1734).
//!
//! `scratch::dir(prefix)` creates a fresh directory under the system temp
//! directory with an exclusive `mkdir` (mode 0700 on Unix) and returns it.
//! A path that already exists — a stale tree from an earlier run, another
//! test binary's live tree, or something another local user planted (a
//! directory, or a symlink pointing elsewhere) — is never adopted: the name
//! is skipped and the next counter value tried. Inside the returned root,
//! `create_dir_all` for nested fixture folders is fine; the root itself is
//! ours.
//!
//! The returned path is canonical, so paths a child Ruby reports
//! (`__dir__` is a realpath) match on macOS, where `$TMPDIR` sits under
//! `/var -> /private/var`.
//!
//! Lifetime is unchanged from the PID-named directories this replaces:
//! nothing is removed on drop; tests that clean up call `remove_dir_all`
//! themselves.
//!
//! Intentional exceptions: directories under `CARGO_TARGET_TMPDIR` live in
//! the build's own `target/` tree, which other users cannot write, and keep
//! their deterministic names.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh, exclusively created directory named `{prefix}-{pid}-{n}`.
pub fn dir(prefix: &str) -> PathBuf {
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonicalize the system temp directory");
    dir_in(&base, prefix)
}

/// As [`dir`], under `base` instead of the system temp directory.
pub fn dir_in(base: &std::path::Path, prefix: &str) -> PathBuf {
    // Several test binaries, and several copies of this module inside one
    // binary, share a PID: the counter only spreads names, exclusivity
    // comes from `create`.
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    for _ in 0..10_000 {
        let dir = base.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create scratch directory {}: {error}", dir.display()),
        }
    }
    panic!("no free scratch directory name for {prefix:?} under {}", base.display())
}
