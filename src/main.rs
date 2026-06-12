use std::ffi::CString;
use std::io::{self, Write};

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

        if line == "exit" {
            break;
        }

        let args: Vec<CString> = line
            .split_whitespace()
            .map(|arg| CString::new(arg.as_bytes()).expect("argument contains NUL byte"))
            .collect();

        if args.is_empty() {
            continue;
        }

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
