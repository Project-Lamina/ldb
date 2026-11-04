//! The command loop.

use crate::error::{LdbError, Result};
use crate::session::Session;
use std::io::{BufRead, Write};

const HELP: &str = "\
Commands:
  functions [substring]   list functions with their source file and decl line
  break <function>        set a breakpoint past the function's prologue
  break <file>:<line>     set a breakpoint on a source line
  break *0x<addr>         set a breakpoint on a raw address
  info breakpoints        list breakpoints
  delete <n>              remove breakpoint n
  run                     start the program
  continue                resume
  step                    run one source line
  finish                  run to return and report the value
  bt                      backtrace
  info args               name the arguments of the current frame
  info line               the source line of the program counter
  kill                    terminate the program
  quit                    leave ldb
";

pub fn run(session: &mut Session, input: &mut dyn BufRead, interactive: bool) -> Result<()> {
    let mut last = String::new();
    loop {
        if interactive {
            print!("(ldb) ");
            let _ = std::io::stdout().flush();
        }
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            if interactive {
                println!();
            }
            return Ok(());
        }
        let trimmed = line.trim();
        let line = if trimmed.is_empty() {
            last.clone()
        } else {
            trimmed.to_string()
        };
        if line.is_empty() {
            continue;
        }
        last = line.clone();
        if matches!(line.as_str(), "quit" | "q" | "exit") {
            return Ok(());
        }
        if let Err(e) = dispatch(session, &line) {
            println!("{}", e);
        }
        let _ = std::io::stdout().flush();
    }
}

fn dispatch(session: &mut Session, line: &str) -> Result<()> {
    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    match cmd {
        "help" | "h" => {
            print!("{}", HELP);
            Ok(())
        }
        "functions" => {
            session.cmd_functions(rest.first().copied());
            Ok(())
        }
        "break" | "b" => match rest.first() {
            Some(spec) => session.cmd_break(spec),
            None => Err(LdbError::Unsupported("break needs a location".into())),
        },
        "delete" | "d" => match rest.first().and_then(|n| n.parse().ok()) {
            Some(n) => session.cmd_delete(n),
            None => Err(LdbError::Unsupported(
                "delete needs a breakpoint number".into(),
            )),
        },
        "info" => dispatch_info(session, &rest),
        "run" | "r" => session.cmd_run(),
        "continue" | "c" => session.cmd_continue(),
        "step" | "s" => session.cmd_step(),
        "finish" | "fin" => session.cmd_finish(),
        "bt" | "backtrace" | "where" => session.cmd_backtrace(),
        "kill" => {
            session.cmd_kill();
            Ok(())
        }
        other => Err(LdbError::Unsupported(format!(
            "`{}` is not an ldb command; try `help`",
            other
        ))),
    }
}

fn dispatch_info(session: &mut Session, rest: &[&str]) -> Result<()> {
    match rest.first().copied() {
        Some("functions") => {
            session.cmd_functions(rest.get(1).copied());
            Ok(())
        }
        Some("args") => session.cmd_info_args(),
        Some("line") => session.cmd_info_line(),
        Some("breakpoints") | Some("break") | Some("b") => {
            session.cmd_info_breakpoints();
            Ok(())
        }
        Some(other) => Err(LdbError::Unsupported(format!(
            "`info {}` is not an ldb command; try `help`",
            other
        ))),
        None => Err(LdbError::Unsupported(
            "info needs a subcommand: functions, args, line, breakpoints".into(),
        )),
    }
}
