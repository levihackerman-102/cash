use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn shell_starts_and_exits() {
    let mut cmd = Command::cargo_bin("cash").expect("binary exists");
    cmd.write_stdin("exit\n");
    cmd.assert().success().stdout(predicate::str::contains("cash> "));
}
