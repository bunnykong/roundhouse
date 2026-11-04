//! `tests/support/scratch.rs` never adopts a path it did not create (#1734).
//!
//! The PID-named roots it replaced were made with `create_dir_all`, which
//! accepts a directory — or a symlink to one — that is already there. These
//! tests plant both at the names the helper will try and check that fixture
//! writes cannot land in them.

#[path = "support/scratch.rs"]
mod scratch;

use std::collections::HashSet;
use std::path::Path;

#[test]
fn each_call_is_a_fresh_empty_private_directory() {
    let a = scratch::dir("rh-scratch-fresh");
    let b = scratch::dir("rh-scratch-fresh");
    assert_ne!(a, b);
    for dir in [&a, &b] {
        let meta = std::fs::symlink_metadata(dir).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink(), "{}", dir.display());
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0, "{}", dir.display());
        assert_eq!(dir.canonicalize().unwrap(), **dir, "returned path is canonical");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o700, "{}", dir.display());
        }
    }
    std::fs::remove_dir_all(a).unwrap();
    std::fs::remove_dir_all(b).unwrap();
}

#[cfg(unix)]
#[test]
fn planted_directories_and_symlinks_are_skipped() {
    use std::os::unix::fs::symlink;

    let base = scratch::dir("rh-scratch-planted");
    let outside = base.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let pid = std::process::id();
    // Every name the helper could try next in this binary: its counter is
    // shared by every call here, all of which happen well below this bound.
    let mut planted = HashSet::new();
    for n in 0..1000 {
        let path = base.join(format!("victim-{pid}-{n}"));
        if n % 2 == 0 {
            std::fs::create_dir(&path).unwrap();
        } else {
            symlink(&outside, &path).unwrap();
        }
        planted.insert(path);
    }

    let dir = scratch::dir_in(&base, "victim");
    assert!(!planted.contains(&dir), "adopted a planted path: {}", dir.display());
    assert!(!std::fs::symlink_metadata(&dir).unwrap().file_type().is_symlink());
    std::fs::write(dir.join("fixture.rb"), "1\n").unwrap();
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "a write escaped through a symlink");
    for path in &planted {
        if !std::fs::symlink_metadata(path).unwrap().file_type().is_symlink() {
            assert_eq!(std::fs::read_dir(path).unwrap().count(), 0, "{}", path.display());
        }
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn parallel_callers_never_share_a_directory() {
    let threads: Vec<_> = (0..16)
        .map(|_| std::thread::spawn(|| (0..20).map(|_| scratch::dir("rh-scratch-parallel")).collect::<Vec<_>>()))
        .collect();
    let dirs: Vec<_> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    let unique: HashSet<&Path> = dirs.iter().map(|d| d.as_path()).collect();
    assert_eq!(unique.len(), dirs.len());
    for dir in dirs {
        std::fs::remove_dir_all(dir).unwrap();
    }
}
