use std::env;
use std::ffi::CString;
use std::io::{self, Write};

enum BuiltinResult {
    NotBuiltin,
    Handled,
    Exit,
}

fn run_builtin(args: &[&str]) -> BuiltinResult {
    match args[0] {
        "exit" => BuiltinResult::Exit,
        "cd" => {
            if args.len() > 2 {
                eprintln!("cd: too many arguments");
                return BuiltinResult::Handled;
            }

            let target = if let Some(path) = args.get(1) {
                path.to_string()
            } else {
                match env::var("HOME") {
                    Ok(home) => home,
                    Err(_) => {
                        eprintln!("cd: HOME is not set");
                        return BuiltinResult::Handled;
                    }
                }
            };

            let target = match CString::new(target.as_bytes()) {
                Ok(path) => path,
                Err(_) => {
                    eprintln!("cd: path contains NUL byte");
                    return BuiltinResult::Handled;
                }
            };

            if unsafe { libc::chdir(target.as_ptr()) } != 0 {
                eprintln!("cd: {}", std::io::Error::last_os_error());
            }

            BuiltinResult::Handled
        }
        "export" => {
            if args.len() < 2 {
                eprintln!("export: usage: export NAME=VALUE [...]");
                return BuiltinResult::Handled;
            }

            for assignment in &args[1..] {
                let Some((name, value)) = assignment.split_once('=') else {
                    eprintln!("export: expected NAME=VALUE, got {assignment}");
                    continue;
                };

                if name.is_empty() {
                    eprintln!("export: variable name cannot be empty");
                    continue;
                }

                let name = match CString::new(name.as_bytes()) {
                    Ok(name) => name,
                    Err(_) => {
                        eprintln!("export: variable name contains NUL byte");
                        continue;
                    }
                };

                let value = match CString::new(value.as_bytes()) {
                    Ok(value) => value,
                    Err(_) => {
                        eprintln!("export: variable value contains NUL byte");
                        continue;
                    }
                };

                if unsafe { libc::setenv(name.as_ptr(), value.as_ptr(), 1) } != 0 {
                    eprintln!("export: {}", std::io::Error::last_os_error());
                }
            }

            BuiltinResult::Handled
        }
        "unset" => {
            if args.len() < 2 {
                eprintln!("unset: usage: unset NAME [...]");
                return BuiltinResult::Handled;
            }

            for name in &args[1..] {
                let name = match CString::new(name.as_bytes()) {
                    Ok(name) => name,
                    Err(_) => {
                        eprintln!("unset: variable name contains NUL byte");
                        continue;
                    }
                };

                if unsafe { libc::unsetenv(name.as_ptr()) } != 0 {
                    eprintln!("unset: {}", std::io::Error::last_os_error());
                }
            }

            BuiltinResult::Handled
        }
        _ => BuiltinResult::NotBuiltin,
    }
}

fn main() {
    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("cash> ");
        io::stdout().flush().expect("failed to flush prompt");

        input.clear();
        match stdin.read_line(&mut input) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("failed to read line: {error}");
                continue;
            }
        }

        let line = input.trim();
        if line.is_empty() {
            continue;
        }

        let args: Vec<&str> = line.split_whitespace().collect();
        if args.is_empty() {
            continue;
        }

        match run_builtin(&args) {
            BuiltinResult::Exit => break,
            BuiltinResult::Handled => continue,
            BuiltinResult::NotBuiltin => {}
        }

        let args: Vec<CString> = args
            .iter()
            .map(|arg| CString::new(arg.as_bytes()).expect("argument contains NUL byte"))
            .collect();

        let mut argv: Vec<*const libc::c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
        argv.push(std::ptr::null());

        let pid = unsafe { libc::fork() };

        if pid < 0 {
            eprintln!("fork failed");
            continue;
        }

        if pid == 0 {
            unsafe {
                libc::execvp(args[0].as_ptr(), argv.as_ptr());
                eprintln!("{}: command not found", line);
                libc::_exit(1);
            }
        }

        let mut status: libc::c_int = 0;
        if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
            eprintln!("waitpid failed");
            continue;
        }

        let _exit_status = status;
    }
}
