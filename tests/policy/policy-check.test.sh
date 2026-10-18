#!/usr/bin/env bash
# Tests of scripts/policy-check.sh (the static half of the testing policy, docs/TESTING.md) on small throw-away trees: what it accepts, and,
# above all, that it FAILS for each forbidden thing. Pure text analysis, a few seconds. Needs bash 3.2+ and python3.
# shellcheck disable=SC2015  # "test && ok || bad" is the reporting idiom here
# policy-allow-file: gui, network, heavy, sleep, loop -- the fixtures are text for the checker to judge; nothing in this file runs them
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/policy-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
fails=0
ok()  { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fails=$((fails + 1)); }

n=0
# newroot [called-script-body]: a fresh tree; scripts/check.sh runs tests/a.sh, whose content is the argument (default: nothing to see)
newroot() {
  n=$((n + 1)); R="$tmp/t$n"; mkdir -p "$R/crates/c/tests" "$R/crates/c/src" "$R/scripts" "$R/tests"
  printf '#!/usr/bin/env bash\ntests/a.sh\n' >"$R/scripts/check.sh"
  printf '#!/usr/bin/env bash\n%s\n' "${1:-true}" >"$R/tests/a.sh"
}
put() { mkdir -p "$(dirname "$R/$1")"; cat >"$R/$1"; }                          # put <path> < content
run() { OUT=$("$repo/scripts/policy-check.sh" --root "$R" "$@" 2>&1); RC=$?; }  # sets OUT and RC
# accepts <description>: the tree in $R passes;  rejects <description> <rule>: it fails and the report names the rule
accepts() { run; [ $RC = 0 ] && ok "accepts $1" || bad "accepts $1 (rc=$RC): $OUT"; }
rejects() { run; if [ $RC = 1 ] && printf '%s' "$OUT" | grep -q "\[$2\]"; then ok "rejects $1 [$2]"; else bad "rejects $1 [$2] (rc=$RC): $OUT"; fi; }

rust_test() { # rust_test <attributes> <body>: a file with one test
  put crates/c/tests/t.rs <<END
use std::time::{Duration, Instant};
$1
#[test]
fn the_test() {
$2
}
END
}

echo "== the repository itself"
"$repo/scripts/policy-check.sh" >"$tmp/self.out" 2>&1 && ok "the repository passes its own policy check" || bad "the repository fails: $(head -5 "$tmp/self.out")"

echo "== Rust: what is accepted"
newroot; rust_test "" '    assert_eq!(1 + 1, 2);'; accepts "a plain test"
newroot; rust_test "" '    std::thread::sleep(Duration::from_millis(150));'; accepts "a sleep of 150 ms"
newroot; rust_test "" '    for i in 0..999 { let _ = i; }'; accepts "a loop of 999"
newroot; rust_test "" '    let v = vec![0u32; 4999]; assert_eq!(v.len(), 4999);'; accepts "a data set of 4999 items"
newroot; rust_test "" '    let t = Instant::now(); assert!(t.elapsed() < Duration::from_secs(30));'; accepts "a hang guard of 30 s"
newroot; rust_test "" '    // Command::new("osascript") in a comment
    let s = "osascript, screencapture and open are only words here"; assert!(s.len() > 3);'; accepts "forbidden words in a comment and in a string"
newroot; rust_test '#[ignore = "ci: drives the real osascript"]' '    std::process::Command::new("osascript").status().unwrap();'; accepts "an ignored test that does everything forbidden"
newroot; rust_test '#[ignore = "known: SEC-01 not fixed yet"]' '    assert!(false);'; accepts "an ignored test with a known: reason"
newroot; put crates/c/tests/t.rs <<'END'
fn helper() { std::process::Command::new("osascript").status().unwrap(); }
#[test]
#[ignore = "ci: real osascript"]
fn only_the_ignored_test_calls_it() { helper(); }
#[test]
fn a_fast_one() { assert!(true); }
END
accepts "a helper that only ignored tests call"
newroot; put crates/c/src/lib.rs <<'END'
pub fn production() { std::process::Command::new("osascript").status().unwrap(); for _ in 0..100000 {} }
#[cfg(test)]
mod tests {
    #[test]
    fn fine() { assert!(true); }
}
END
accepts "forbidden calls in production code (only tests are judged)"
newroot; rust_test "" '    // policy-allow: sleep -- the debounce is 250 ms and there is no event to wait for
    std::thread::sleep(Duration::from_millis(250));'; accepts "an opt-out with a reason"
run --list-allowed; printf '%s' "$OUT" | grep -q "allowed .*\[sleep\] the debounce is 250 ms" && ok "--list-allowed shows the opt-out and its reason" || bad "--list-allowed: $OUT"
newroot; put crates/c/tests/t.rs <<'END'
// policy-allow-fn: loop -- 2,000 rows is the smallest size that shows the bug
#[test]
fn the_test() { for i in 0..2000 { let _ = i; } for i in 0..3000 { let _ = i; } }
END
accepts "a function-wide opt-out"

echo "== Rust: what is rejected"
newroot; rust_test "" '    std::process::Command::new("osascript").arg("-e").status().unwrap();'; rejects "osascript" gui
newroot; rust_test "" '    std::process::Command::new("/usr/sbin/screencapture").status().unwrap();'; rejects "screencapture by path" gui
newroot; rust_test "" '    std::process::Command::new("open").arg("-a").arg("Safari").status().unwrap();'; rejects "open of an app" gui
newroot; rust_test "" '    std::process::Command::new("tart").arg("list").status().unwrap();'; rejects "tart" gui
newroot; rust_test "" '    let b = NSPasteboard::generalPasteboard(); b.clearContents();'; rejects "a pasteboard write" gui
newroot; rust_test "" '    let _ = ashdash_core::engine::notify("t", "x");'; rejects "a real notification" gui
newroot; rust_test "" '    let _ = RealOs.run(&Cmd::osascript(&["x"], [""; 0]));'; rejects "osascript through RealOs" gui
newroot; rust_test "" '    std::thread::sleep(Duration::from_millis(250));'; rejects "a sleep of 250 ms" sleep
newroot; rust_test "" '    std::thread::sleep(Duration::from_millis(120)); std::thread::sleep(Duration::from_millis(120));'; rejects "two sleeps of 120 ms" sleep
newroot; rust_test "" '    std::thread::sleep(Duration::from_secs(2));'; rejects "a sleep of 2 s" sleep
newroot; rust_test "" '    for i in 0..1000 { let _ = i; }'; rejects "a loop of 1000" loop
newroot; rust_test "" '    for i in 0..=5_000 { let _ = i; }'; rejects "an inclusive loop of 5001" loop
newroot; rust_test "" '    let v: Vec<u32> = (0..2500).map(|i| i * 2).collect(); assert_eq!(v.len(), 2500);'; rejects "an iterator over 2500 items" loop
newroot; rust_test "" '    let v = vec![String::new(); 5000]; assert_eq!(v.len(), 5000);'; rejects "a data set of 5000 items" loop
newroot; rust_test "" '    let v = vec![0u8; 100 * 1024 * 1024]; assert_eq!(v.len(), 1);'; rejects "a buffer of 100 MiB" loop
newroot; rust_test "" '    let s = "x".repeat(4 * 1024 * 1024); assert_eq!(s.len(), 1);'; rejects "a string of 4 MiB" loop
newroot; rust_test "" '    let s = std::net::TcpStream::connect("example.org:80"); drop(s);'; rejects "a TCP connection" network
newroot; rust_test "" '    std::process::Command::new("curl").arg("https://example.org").status().unwrap();'; rejects "curl" network
newroot; rust_test "" '    let t = Instant::now(); assert!(t.elapsed() < Duration::from_millis(50));'; rejects "a latency ceiling of 50 ms" clock
newroot; rust_test "" '    let t = Instant::now(); assert!(t.elapsed() < Duration::from_secs(2), "slow");'; rejects "a ceiling of 2 s" clock
newroot; rust_test '#[ignore]' '    assert!(true);'; rejects "an ignore without a reason" ignore
newroot; rust_test '#[ignore = "slow"]' '    assert!(true);'; rejects "an ignore reason without ci: or known:" ignore
newroot; rust_test "" '    // policy-allow: sleep
    std::thread::sleep(Duration::from_millis(250));'; rejects "an opt-out without a reason" opt-out
newroot; rust_test "" '    // policy-allow: sleep -- ok
    std::thread::sleep(Duration::from_millis(250));'; rejects "an opt-out with a reason that says nothing" opt-out
newroot; rust_test "" '    // policy-allow: speed -- because I want to
    std::thread::sleep(Duration::from_millis(250));'; rejects "an opt-out naming an unknown rule" opt-out
newroot; rust_test "" '    // policy-allow: loop -- the loop below is allowed, the sleep is not
    for i in 0..5000 { let _ = i; }
    std::thread::sleep(Duration::from_millis(250));'; rejects "an opt-out for another rule" sleep
newroot; put crates/c/src/lib.rs <<'END'
#[cfg(test)]
mod tests {
    #[test]
    fn in_a_unit_test_module() { std::process::Command::new("osascript").status().unwrap(); }
}
END
rejects "a forbidden call in a cfg(test) module" gui
newroot; put crates/c/src/tests.rs <<'END'
#[test]
fn in_a_test_file() { std::process::Command::new("screencapture").status().unwrap(); }
END
rejects "a forbidden call in a tests.rs file" gui
newroot; put crates/c/tests/t.rs <<'END'
fn helper() { std::process::Command::new("osascript").status().unwrap(); }
#[test]
fn calls_the_helper() { helper(); }
END
rejects "a helper called by a non-ignored test" gui

echo "== shell scripts of the local gate"
newroot 'echo hello; sleep 1'; accepts "a script that sleeps 1 s"
newroot 'sleep 3'; rejects "a script that sleeps 3 s" sleep
newroot 'sleep 1; sleep 1; sleep 1'; rejects "sleeps that add up to 3 s" sleep
newroot "osascript -e 'tell application \"Finder\" to activate'"; rejects "osascript in a script" gui
newroot 'screencapture -x /tmp/x.png'; rejects "screencapture in a script" gui
newroot 'tart run vm &'; rejects "tart in a script" gui
newroot 'open -a Safari'; rejects "open of an app in a script" gui
newroot 'curl -s https://example.org | head'; rejects "curl in a script" network
newroot 'git fetch origin'; rejects "git fetch in a script" network
newroot 'for i in $(seq 1 5000); do :; done'; rejects "a shell loop of 5000" loop
newroot 'cargo bench'; rejects "cargo bench in the gate" heavy
newroot 'cargo test --workspace -- --ignored'; rejects "ignored tests in the gate" heavy
newroot 'tests/e2e/smoke.sh'; rejects "an e2e script called from the gate" heavy
newroot 'tests/b.sh'; put tests/b.sh <<'END'
#!/usr/bin/env bash
osascript -e 'x'
END
rejects "a forbidden call in a script called by a script of the gate" gui
newroot 'echo "install with: brew install shellcheck, then open the docs"
# osascript is not called here
true'
accepts "forbidden words in messages and comments"
newroot 'sleep 5'; put tests/not-called.sh <<'END'
#!/usr/bin/env bash
osascript -e 'x'
END
rejects "the sleep in the called script (the uncalled one is not judged)" sleep
newroot 'true'; put tests/not-called.sh <<'END'
#!/usr/bin/env bash
osascript -e 'x'
END
accepts "a forbidden call in a script the gate does not run"
newroot '# policy-allow: network -- talks to a server on 127.0.0.1 that the test starts itself
curl -s http://127.0.0.1:8080/ping'
accepts "a shell opt-out with a reason"
newroot '# policy-allow-file: gui -- this test only checks the text of the scripts it greps, it runs none of them
osascript -e x'
accepts "a file-wide shell opt-out with a reason"
M=policy-allow   # spelled out only here, so that this file does not contain a malformed opt-out itself
newroot "# $M: network
curl -s http://127.0.0.1:8080/ping"
rejects "a shell opt-out without a reason" opt-out

echo
if [ "$fails" = 0 ]; then echo "policy-check tests: OK"; else echo "policy-check tests: $fails FAILED"; exit 1; fi
