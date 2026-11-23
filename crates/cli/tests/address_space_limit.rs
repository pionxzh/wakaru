//! Thread stacks count against an address-space limit (`RLIMIT_AS`), so the
//! CLI shrinks its work stacks to fit one. These spawn the binary under a
//! limit set in the child: the thread setup lives in `main`, which the
//! in-crate tests never reach.

#![cfg(target_os = "linux")]
#![allow(clippy::disallowed_methods, reason = "tests build their own fixtures")]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIB: u64 = 1024 * 1024;

/// Limits integrators run the CLI under. Whether the threads fit depends on
/// what else claims address space first, which does not grow evenly with the
/// limit, so the tests try several.
const LIMITS: [u64; 4] = [256 * MIB, 384 * MIB, 512 * MIB, 768 * MIB];

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wakaru-address-space-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir should be created");
    dir
}

/// Unpacks a small webpack 4 bundle, so the module pipeline starts the worker
/// pool, with the address space limited to `limit` bytes, and asserts that it
/// succeeds.
fn assert_unpacks_under_limit(dir: &Path, limit: u64, rayon_threads: Option<&str>) {
    let input = dir.join("bundle.js");
    let out = dir.join(format!("out-{limit}"));
    std::fs::write(
        &input,
        "!function(e){var n={};function r(t){if(n[t])return n[t].exports;var o=n[t]={i:t,l:!1,exports:{}};return e[t].call(o.exports,o,o.exports,r),o.l=!0,o.exports}r.m=e,r.c=n,r.d=function(e,n,t){r.o(e,n)||Object.defineProperty(e,n,{enumerable:!0,get:t})},r.o=function(e,n){return Object.prototype.hasOwnProperty.call(e,n)},r.p=\"\",r(r.s=0)}([function(e,n,r){var v=r(1);globalThis.greeting=v.name+\"!\"},function(e,n){n.name=\"wakaru\"}]);",
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_wakaru"));
    command.arg(&input).arg("--unpack").arg("-o").arg(&out);
    match rayon_threads {
        Some(count) => command.env("RAYON_NUM_THREADS", count),
        None => command.env_remove("RAYON_NUM_THREADS"),
    };
    // SAFETY: the hook only calls `setrlimit`, which is async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: limit,
                rlim_max: limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().expect("wakaru should start");

    assert!(
        output.status.success(),
        "wakaru failed under a {} MiB limit with {}:\n{}",
        limit / MIB,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let modules = std::fs::read_dir(&out).unwrap().count();
    assert!(
        modules >= 2,
        "expected the two modules to be split out, got {modules}"
    );
}

#[test]
fn unpacks_under_an_address_space_limit_on_all_cores() {
    let dir = scratch_dir("all-cores");
    for limit in LIMITS {
        assert_unpacks_under_limit(&dir, limit, None);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unpacks_under_an_address_space_limit_on_one_worker() {
    let dir = scratch_dir("one-worker");
    for limit in LIMITS {
        assert_unpacks_under_limit(&dir, limit, Some("1"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
