use std::env;
use std::ffi::CString;
use std::io::{self, Write};
use std::sync::atomic::{AtomicI32, Ordering};

static FOREGROUND_PGID: AtomicI32 = AtomicI32::new(0);

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

extern "C" fn sigchld_handler(_: libc::c_int) {
    let saved_errno = unsafe { *libc::__errno_location() };

    loop {
        let mut status: libc::c_int = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };

        if pid > 0 {
            // Successfully reaped a child; continue draining any others.
            continue;
        }

        if pid == 0 {
            // No more exited children at the moment.
            break;
        }

        // pid < 0 => error; inspect errno to decide what to do.
        let err = unsafe { *libc::__errno_location() };
        if err == libc::EINTR {
            // Interrupted by a signal, retry the wait.
            continue;
        }

        if err == libc::ECHILD {
            // No child processes exist.
            break;
        }

        // Unexpected error; stop to avoid unsafe behavior in handler.
        break;
    }

    unsafe {
        *libc::__errno_location() = saved_errno;
    }
}

extern "C" fn sigint_handler(_: libc::c_int) {
    let pgid = FOREGROUND_PGID.load(Ordering::SeqCst);

    if pgid > 0 {
        unsafe {
            libc::kill(-pgid, libc::SIGINT);
        }
        return;
    }

    let newline = b"\n";
    unsafe {
        libc::write(
            libc::STDOUT_FILENO,
            newline.as_ptr() as *const libc::c_void,
            newline.len(),
        );
    }
}

fn install_signal_handlers() -> Result<(), String> {
    unsafe {
        // Ignore terminal background I/O signals so the shell isn't stopped
        // when it changes terminal foreground process groups.
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);

        let mut sigchld_action: libc::sigaction = std::mem::zeroed();
        sigchld_action.sa_sigaction = sigchld_handler as usize;
        sigchld_action.sa_flags = libc::SA_RESTART | libc::SA_NOCLDSTOP;
        libc::sigemptyset(&mut sigchld_action.sa_mask);
        if libc::sigaction(libc::SIGCHLD, &sigchld_action, std::ptr::null_mut()) < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }

        let mut sigint_action: libc::sigaction = std::mem::zeroed();
        sigint_action.sa_sigaction = sigint_handler as usize;
        sigint_action.sa_flags = 0;
        libc::sigemptyset(&mut sigint_action.sa_mask);
        if libc::sigaction(libc::SIGINT, &sigint_action, std::ptr::null_mut()) < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }

    Ok(())
}

fn parse_pipeline(line: &str) -> Result<Vec<ParsedCommand>, String> {
    let mut commands = Vec::new();
    let mut current = ParsedCommand {
        args: Vec::new(),
        redirections: Redirections::default(),
    };

    let mut tokens = line.split_whitespace().peekable();

    while let Some(token) = tokens.next() {
        match token {
            "|" => {
                if current.args.is_empty() {
                    return Err("missing command before |".to_string());
                }

                commands.push(current);
                current = ParsedCommand {
                    args: Vec::new(),
                    redirections: Redirections::default(),
                };
            }
            "<" => {
                let file = tokens.next().ok_or_else(|| "missing file after <".to_string())?;
                current.redirections.stdin = Some(file.to_string());
            }
            ">" => {
                let file = tokens.next().ok_or_else(|| "missing file after >".to_string())?;
                current.redirections.stdout = Some((file.to_string(), false));
            }
            ">>" => {
                let file = tokens.next().ok_or_else(|| "missing file after >>".to_string())?;
                current.redirections.stdout = Some((file.to_string(), true));
            }
            "2>" => {
                let file = tokens.next().ok_or_else(|| "missing file after 2>".to_string())?;
                current.redirections.stderr = Some(file.to_string());
            }
            _ => current.args.push(token.to_string()),
        }
    }

    if current.args.is_empty() {
        if commands.is_empty() {
            return Err("missing command".to_string());
        }

        return Err("missing command after |".to_string());
    }

    commands.push(current);
    Ok(commands)
}

fn cstring_from_str(value: &str, context: &str) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| format!("{context} contains NUL byte"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_command() {
        let pipeline = parse_pipeline("echo hello").expect("parse");
        assert_eq!(pipeline.len(), 1);
        let cmd = &pipeline[0];
        assert_eq!(cmd.args, vec!["echo".to_string(), "hello".to_string()]);
    }

    #[test]
    fn parse_pipeline_and_redirections() {
        let pipeline = parse_pipeline("ls | grep foo > out.txt 2> err.txt").expect("parse");
        assert_eq!(pipeline.len(), 2);
        assert_eq!(pipeline[0].args[0], "ls");
        assert_eq!(pipeline[1].args[0], "grep");
        assert_eq!(pipeline[1].redirections.stdout.as_ref().map(|(p,_)| p.clone()), Some("out.txt".to_string()));
        assert_eq!(pipeline[1].redirections.stderr.as_ref().map(|p| p.clone()), Some("err.txt".to_string()));
    }

    #[test]
    fn cstring_from_str_rejects_nul() {
        let s = "foo\0bar";
        let res = cstring_from_str(s, "test");
        assert!(res.is_err());
    }
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

fn close_pipe_fds(pipes: &[(libc::c_int, libc::c_int)]) {
    for (read_end, write_end) in pipes {
        unsafe {
            libc::close(*read_end);
            libc::close(*write_end);
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

fn is_builtin(command: &str) -> bool {
    matches!(command, "exit" | "cd" | "export" | "unset")
}

fn run_pipeline_command(command: &ParsedCommand, pipes: &[(libc::c_int, libc::c_int)], index: usize) -> ! {
    unsafe {
        if index > 0 {
            let (read_end, _) = pipes[index - 1];
            if libc::dup2(read_end, libc::STDIN_FILENO) < 0 {
                eprintln!("dup2 failed: {}", std::io::Error::last_os_error());
                libc::_exit(1);
            }
        }

        if index < pipes.len() {
            let (_, write_end) = pipes[index];
            if libc::dup2(write_end, libc::STDOUT_FILENO) < 0 {
                eprintln!("dup2 failed: {}", std::io::Error::last_os_error());
                libc::_exit(1);
            }
        }

        close_pipe_fds(pipes);

        if let Err(error) = apply_redirections(&command.redirections) {
            eprintln!("{error}");
            libc::_exit(1);
        }
    }

    if is_builtin(command.args[0].as_str()) {
        let builtin_args: Vec<&str> = command.args.iter().map(|arg| arg.as_str()).collect();
        let exit_code = match run_builtin(&builtin_args) {
            BuiltinResult::Exit | BuiltinResult::Handled => 0,
            BuiltinResult::NotBuiltin => 127,
        };
        unsafe {
            libc::_exit(exit_code);
        }
    }

    let args: Vec<CString> = command
        .args
        .iter()
        .map(|arg| CString::new(arg.as_bytes()).expect("argument contains NUL byte"))
        .collect();

    let mut argv: Vec<*const libc::c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
    argv.push(std::ptr::null());

    unsafe {
        libc::execvp(args[0].as_ptr(), argv.as_ptr());
        eprintln!("{}: command not found", command.args[0]);
        libc::_exit(1);
    }
}

fn wait_for_foreground_job(children: &[libc::pid_t], pgid: libc::pid_t) -> Result<(), String> {
    FOREGROUND_PGID.store(pgid, Ordering::SeqCst);

    for child in children {
        loop {
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(*child, &mut status, 0) };

            if result == *child {
                break;
            }

            if result < 0 {
                let error = std::io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(code) if code == libc::EINTR => continue,
                    Some(code) if code == libc::ECHILD => break,
                    _ => {
                        FOREGROUND_PGID.store(0, Ordering::SeqCst);
                        return Err(error.to_string());
                    }
                }
            }
        }
    }

    FOREGROUND_PGID.store(0, Ordering::SeqCst);
    Ok(())
}

fn run_single_external(parsed: &ParsedCommand, background: bool) -> Result<(), String> {
    let args: Vec<CString> = parsed
        .args
        .iter()
        .map(|arg| CString::new(arg.as_bytes()).expect("argument contains NUL byte"))
        .collect();

    let mut argv: Vec<*const libc::c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
    argv.push(std::ptr::null());

    let pid = unsafe { libc::fork() };

    if pid < 0 {
        return Err("fork failed".to_string());
    }

    if pid == 0 {
        unsafe {
            let child_pid = libc::getpid();
            libc::setpgid(0, child_pid);

            // Reset signal handlers to defaults in the child so programs like
            // `vim` receive SIGINT/SIGQUIT normally.
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);

            if let Err(error) = apply_redirections(&parsed.redirections) {
                eprintln!("{error}");
                libc::_exit(1);
            }
            libc::execvp(args[0].as_ptr(), argv.as_ptr());
            eprintln!("{}: command not found", parsed.args[0]);
            libc::_exit(1);
        }
    }

    unsafe {
        libc::setpgid(pid, pid);
    }

    if background {
        println!("[{pid}] running in background");
        return Ok(());
    }

    // Give the child process group control of the terminal so interactive
    // programs (vim, nano, etc.) can read from / write to the tty.
    let shell_pgid = unsafe { libc::getpgrp() };
    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, pid);
    }

    let res = wait_for_foreground_job(&[pid], pid);

    // Restore terminal control to the shell.
    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, shell_pgid);
    }

    res
}

fn run_pipeline(commands: &[ParsedCommand], background: bool) -> Result<(), String> {
    let mut pipes = Vec::new();
    for _ in 0..commands.len().saturating_sub(1) {
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
            close_pipe_fds(&pipes);
            return Err(std::io::Error::last_os_error().to_string());
        }
        pipes.push((fds[0], fds[1]));
    }

    let mut children = Vec::new();
    let mut pgid: libc::pid_t = 0;

    for (index, command) in commands.iter().enumerate() {
        let pid = unsafe { libc::fork() };

        if pid < 0 {
            close_pipe_fds(&pipes);
            for child in &children {
                unsafe {
                    libc::waitpid(*child, std::ptr::null_mut(), 0);
                }
            }
            return Err("fork failed".to_string());
        }

        if pid == 0 {
            unsafe {
                if pgid == 0 {
                    pgid = libc::getpid();
                }
                libc::setpgid(0, pgid);

                // In child, reset signal dispositions to defaults so interactive
                // programs behave normally.
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            }
            run_pipeline_command(command, &pipes, index);
        }

        if pgid == 0 {
            pgid = pid;
        }
        unsafe {
            libc::setpgid(pid, pgid);
        }

        children.push(pid);
    }

    close_pipe_fds(&pipes);

    if background {
        println!("[{pgid}] running in background");
        return Ok(());
    }

    // Give the pipeline control of the terminal so interactive stages work.
    let shell_pgid = unsafe { libc::getpgrp() };
    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
    }

    let res = wait_for_foreground_job(&children, pgid);

    // Restore control to the shell.
    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, shell_pgid);
    }

    res
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
    if let Err(error) = install_signal_handlers() {
        eprintln!("failed to install signal handlers: {error}");
        return;
    }

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
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                eprintln!("failed to read line: {error}");
                continue;
            }
        }

        let line = input.trim();
        if line.is_empty() {
            continue;
        }

        let mut tokens: Vec<&str> = line.split_whitespace().collect();
        let background = matches!(tokens.last(), Some(&"&"));
        if background {
            tokens.pop();
        }

        let command_line = tokens.join(" ");
        if command_line.is_empty() {
            continue;
        }

        let pipeline = match parse_pipeline(command_line.as_str()) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                eprintln!("{error}");
                continue;
            }
        };

        if pipeline.is_empty() {
            continue;
        }

        if pipeline.len() == 1 {
            let parsed = &pipeline[0];

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
        }

        if pipeline.len() == 1 {
            let parsed = &pipeline[0];
            if let Err(error) = run_single_external(parsed, background) {
                eprintln!("{error}");
            }
            continue;
        }

        if let Err(error) = run_pipeline(&pipeline, background) {
            eprintln!("{error}");
        }
    }
}
