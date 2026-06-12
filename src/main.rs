use std::env;
use std::ffi::CString;
use std::io::{self, Write};

enum BuiltinResult {
    NotBuiltin,
    Handled,
    Exit,
}

#[derive(Default)]
struct Redirections {
    stdin: Option<String>,
    stdout: Option<(String, bool)>,
    stderr: Option<String>,
}

struct ParsedCommand {
    args: Vec<String>,
    redirections: Redirections,
}

struct SavedFds {
    stdin: Option<libc::c_int>,
    stdout: Option<libc::c_int>,
    stderr: Option<libc::c_int>,
}

fn parse_command(line: &str) -> Result<ParsedCommand, String> {
    let mut args = Vec::new();
    let mut redirections = Redirections::default();
    let mut tokens = line.split_whitespace().peekable();

    while let Some(token) = tokens.next() {
        match token {
            "<" => {
                let file = tokens.next().ok_or_else(|| "missing file after <".to_string())?;
                redirections.stdin = Some(file.to_string());
            }
            ">" => {
                let file = tokens.next().ok_or_else(|| "missing file after >".to_string())?;
                redirections.stdout = Some((file.to_string(), false));
            }
            ">>" => {
                let file = tokens.next().ok_or_else(|| "missing file after >>".to_string())?;
                redirections.stdout = Some((file.to_string(), true));
            }
            "2>" => {
                let file = tokens.next().ok_or_else(|| "missing file after 2>".to_string())?;
                redirections.stderr = Some(file.to_string());
            }
            _ => args.push(token.to_string()),
        }
    }

    Ok(ParsedCommand { args, redirections })
}

fn cstring_from_str(value: &str, context: &str) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| format!("{context} contains NUL byte"))
}

fn open_for_redirection(path: &str, flags: libc::c_int) -> Result<libc::c_int, String> {
    let c_path = cstring_from_str(path, "path")?;
    let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o644) };
    if fd < 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(fd)
    }
}

fn apply_redirections(redirections: &Redirections) -> Result<SavedFds, String> {
    let mut saved = SavedFds {
        stdin: None,
        stdout: None,
        stderr: None,
    };

    if let Some(path) = &redirections.stdin {
        let fd = open_for_redirection(path, libc::O_RDONLY)?;
        let saved_fd = unsafe { libc::dup(libc::STDIN_FILENO) };
        if saved_fd < 0 {
            unsafe {
                libc::close(fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        if unsafe { libc::dup2(fd, libc::STDIN_FILENO) } < 0 {
            unsafe {
                libc::close(fd);
                libc::close(saved_fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        unsafe {
            libc::close(fd);
        }
        saved.stdin = Some(saved_fd);
    }

    if let Some((path, append)) = &redirections.stdout {
        let mut flags = libc::O_WRONLY | libc::O_CREAT;
        flags |= if *append { libc::O_APPEND } else { libc::O_TRUNC };
        let fd = open_for_redirection(path, flags)?;
        let saved_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if saved_fd < 0 {
            unsafe {
                libc::close(fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        if unsafe { libc::dup2(fd, libc::STDOUT_FILENO) } < 0 {
            unsafe {
                libc::close(fd);
                libc::close(saved_fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        unsafe {
            libc::close(fd);
        }
        saved.stdout = Some(saved_fd);
    }

    if let Some(path) = &redirections.stderr {
        let fd = open_for_redirection(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC)?;
        let saved_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if saved_fd < 0 {
            unsafe {
                libc::close(fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        if unsafe { libc::dup2(fd, libc::STDERR_FILENO) } < 0 {
            unsafe {
                libc::close(fd);
                libc::close(saved_fd);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        unsafe {
            libc::close(fd);
        }
        saved.stderr = Some(saved_fd);
    }

    Ok(saved)
}

fn restore_redirections(saved: SavedFds) {
    if let Some(fd) = saved.stdin {
        unsafe {
            libc::dup2(fd, libc::STDIN_FILENO);
            libc::close(fd);
        }
    }
    if let Some(fd) = saved.stdout {
        unsafe {
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::close(fd);
        }
    }
    if let Some(fd) = saved.stderr {
        unsafe {
            libc::dup2(fd, libc::STDERR_FILENO);
            libc::close(fd);
        }
    }
}

fn run_with_redirections<F>(redirections: &Redirections, action: F) -> Result<BuiltinResult, String>
where
    F: FnOnce() -> BuiltinResult,
{
    let saved = apply_redirections(redirections)?;
    let result = action();
    restore_redirections(saved);
    Ok(result)
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

        let parsed = match parse_command(line) {
            Ok(parsed) => parsed,
            Err(error) => {
                eprintln!("{error}");
                continue;
            }
        };

        if parsed.args.is_empty() {
            continue;
        }

        match parsed.args[0].as_str() {
            "exit" => break,
            "cd" | "export" | "unset" => {
                let builtin_args: Vec<&str> = parsed.args.iter().map(|arg| arg.as_str()).collect();
                match run_with_redirections(&parsed.redirections, || run_builtin(&builtin_args)) {
                    Ok(BuiltinResult::Exit) => break,
                    Ok(BuiltinResult::Handled) => continue,
                    Ok(BuiltinResult::NotBuiltin) => continue,
                    Err(error) => {
                        eprintln!("{error}");
                        continue;
                    }
                }
            }
            _ => {}
        }

        let args: Vec<CString> = parsed
            .args
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
                if let Err(error) = apply_redirections(&parsed.redirections) {
                    eprintln!("{error}");
                    libc::_exit(1);
                }
                libc::execvp(args[0].as_ptr(), argv.as_ptr());
                eprintln!("{}: command not found", parsed.args[0]);
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
