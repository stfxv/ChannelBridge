use std::fs;
use std::process::Command;

#[test]
fn command_rejects_bad_input_and_migrates_idempotently() {
    let path = std::env::temp_dir().join(format!("rust-style-fixture-{}.rs", std::process::id()));
    let binary = env!("CARGO_BIN_EXE_channelbridge-rust-style");
    let run = |mode| {
        Command::new(binary)
            .arg(mode)
            .arg(&path)
            .output()
            .expect("checker starts")
    };
    fs::write(
        &path,
        "/// Preserve x.unwrap()\nfn f() {}\npub use a::{B, c::D};\n",
    )
    .expect("fixture written");
    let bad = run("check");
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stdout).contains("import-group"));
    assert!(!String::from_utf8_lossy(&bad.stdout).contains(": unwrap"));
    assert!(run("fix-imports").status.success());
    let changed = fs::read_to_string(&path).expect("migration readable");
    assert!(changed.starts_with("/// Preserve x.unwrap()\n"));
    assert!(run("check").status.success());
    assert!(run("fix-imports").status.success());
    assert_eq!(
        fs::read_to_string(&path).expect("second migration readable"),
        changed
    );
    fs::write(&path, "fn f() { assert!(Some(true).unwrap()); }").expect("unwrap fixture written");
    assert_eq!(run("check").status.code(), Some(1));
    fs::write(&path, "fn invalid(").expect("invalid fixture written");
    assert_eq!(run("check").status.code(), Some(2));
    fs::remove_file(path).expect("fixture removed");
}
