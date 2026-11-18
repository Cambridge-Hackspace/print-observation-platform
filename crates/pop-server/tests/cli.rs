//! The `pop` binary's configuration handling, run as a user would run it.

use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Runs `pop` with `config` as its file. A `pop` still running after ten
/// seconds has started a server it should not have; it is killed, and the
/// test fails, rather than hanging the suite.
/// Returns its exit code (2 is a configuration error), stdout and stderr.
fn pop(args: &[&str], config: &str) -> (Option<i32>, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pop.toml");
    std::fs::write(&path, config).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(args)
        .arg("--config")
        .arg(&path)
        .env_remove("POP_CONFIG")
        .env_remove("POP_SERVER_LISTEN")
        // If it does start, on a port of its own and away from the checkout.
        .env("POP_SERVER_LISTEN", "127.0.0.1:0")
        .env("POP_STORAGE_DATA_DIR", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("pop {args:?} was still running after ten seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (mut out, mut err) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    (status.code(), out, err)
}

#[test]
fn config_check_accepts_the_sample() {
    let sample = include_str!("../../../pop.example.toml");
    let (code, out, err) = pop(&["config", "check"], sample);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("valid"), "{out}");
}

#[test]
fn config_check_refuses_a_bad_file_naming_the_key() {
    let (code, _, err) = pop(&["config", "check"], "[server]\nlisten = \"nowhere\"\n");
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("server.listen"), "{err}");
}

#[test]
fn the_server_does_not_start_on_a_bad_file() {
    // `pop` alone is `pop serve`; with a bad file it must exit, not listen.
    // Exit code 2 is the configuration error itself, not some later failure
    // of a server that went ahead anyway.
    let (code, _, err) = pop(&[], "[log]\nlevel = \"loud\"\n");
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("log.level"), "{err}");
}

/// `pop user create-admin`, as a person setting POP up would run it, with the
/// password on standard input. Returns its exit code and stderr.
fn create_admin(dir: &std::path::Path, args: &[&str], password: &str) -> (Option<i32>, String) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["user", "create-admin"])
        .args(args)
        .env_remove("POP_CONFIG")
        .env("POP_STORAGE_DATA_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{password}\n").as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "create-admin did not finish");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    (status.code(), err)
}

#[tokio::test]
async fn create_admin_makes_an_admin_who_can_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let (code, err) = create_admin(
        dir.path(),
        &["ada", "--name", "Ada Lovelace"],
        "the first admin's pw",
    );
    assert_eq!(code, Some(0), "{err}");
    let db = pop_server::db::open(&dir.path().join("pop.db"))
        .await
        .unwrap();
    let user = pop_server::users::authenticate(&db, "ada", "the first admin's pw")
        .await
        .unwrap()
        .expect("the admin can sign in with the password given");
    assert_eq!(user.role, pop_server::api::Role::Admin);
    assert_eq!(user.name, "Ada Lovelace");
}

#[test]
fn create_admin_refuses_a_short_password_and_a_taken_name() {
    let dir = tempfile::tempdir().unwrap();
    let (code, err) = create_admin(dir.path(), &["ada"], "short");
    assert_eq!(code, Some(1));
    assert!(err.contains("password"), "{err}");
    let (code, _) = create_admin(dir.path(), &["ada"], "a long enough pw");
    assert_eq!(code, Some(0));
    let (code, err) = create_admin(dir.path(), &["ADA"], "a long enough pw");
    assert_eq!(code, Some(1));
    assert!(err.contains("already exists"), "{err}");
}
