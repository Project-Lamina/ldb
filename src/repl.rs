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

#[cfg(test)]
mod tests {
    use crate::test_support::{FACTORIAL, TempDir, build, ldb};

    macro_rules! subject {
        ($dir:ident) => {{
            let Some(exe) = build(&$dir, "subject", FACTORIAL) else {
                eprintln!("skipping: no lamina release build next to this checkout");
                return;
            };
            exe
        }};
    }

    #[test]
    fn functions_lists_every_subprogram_with_its_decl_line() {
        let dir = TempDir::new("functions");
        let exe = subject!(dir);
        let out = ldb(&exe, &["functions"]);
        assert!(out.contains("File subject.lamina:"), "{out}");
        assert!(out.contains("static i64 factorial(i64);"), "{out}");
        assert!(out.contains("i64 main(void);"), "{out}");
        assert!(out.contains("static f64 half(f64);"), "{out}");
        assert!(out.contains("static void nothing(i32);"), "{out}");
        assert!(out.contains("1:\tstatic i64 factorial(i64);"), "{out}");
    }

    #[test]
    fn break_on_a_function_stops_and_names_the_source_line() {
        let dir = TempDir::new("break-fn");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break factorial", "run"]);
        assert!(
            out.contains("Breakpoint 1 at 0x") && out.contains("file subject.lamina, line 3."),
            "{out}"
        );
        assert!(
            out.contains("Breakpoint 1, factorial (n=<unavailable>) at subject.lamina:3"),
            "{out}"
        );
        assert!(out.contains("%is_zero = eq.i64 %n, 0"), "{out}");
    }

    #[test]
    fn break_on_a_file_and_line_stops_there() {
        let dir = TempDir::new("break-line");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break subject.lamina:27", "run"]);
        assert!(out.contains("main () at subject.lamina:27"), "{out}");
    }

    #[test]
    fn a_line_with_no_code_is_refused_with_the_lines_that_have_some() {
        let dir = TempDir::new("break-nocode");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break subject.lamina:13"]);
        assert!(out.contains("has no code"), "{out}");
        assert!(out.contains("The nearest lines that do:"), "{out}");
    }

    #[test]
    fn an_unknown_function_is_refused_by_name() {
        let dir = TempDir::new("break-unknown");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break no_such_function"]);
        assert!(
            out.contains("No symbol \"no_such_function\" in debug info"),
            "{out}"
        );
    }

    #[test]
    fn backtrace_names_every_recursive_frame() {
        let dir = TempDir::new("bt");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break factorial", "run", "c", "c", "c", "c", "bt"]);
        let frames: Vec<&str> = out.lines().filter(|l| l.starts_with('#')).collect();
        assert_eq!(frames.len(), 6, "{out}");
        assert!(frames[0].contains("#0  factorial"), "{out}");
        for f in &frames[1..5] {
            assert!(
                f.contains("in factorial") && f.contains("subject.lamina:9"),
                "{out}"
            );
        }
        assert!(
            frames[5].contains("in main () at subject.lamina:27"),
            "{out}"
        );
    }

    #[test]
    fn step_advances_one_source_line() {
        let dir = TempDir::new("step");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break subject.lamina:27", "run", "step", "bt"]);
        assert!(
            out.contains("factorial (n=<unavailable>) at subject.lamina:3"),
            "step did not enter factorial: {out}"
        );
    }

    #[test]
    fn finish_reports_an_integer_return_through_dw_at_type() {
        let dir = TempDir::new("finish-int");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break factorial", "run", "delete 1", "finish"]);
        assert!(out.contains("Run till exit from #0  factorial"), "{out}");
        assert!(out.contains("in main () at subject.lamina:27"), "{out}");
        assert!(out.contains("Value returned is 24"), "{out}");
    }

    #[test]
    fn finish_reports_a_double_return_from_xmm0() {
        let dir = TempDir::new("finish-f64");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break half", "run", "finish"]);
        assert!(out.contains("Value returned is 4.5"), "{out}");
    }

    #[test]
    fn finish_says_so_when_the_function_returns_void() {
        let dir = TempDir::new("finish-void");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break nothing", "run", "finish"]);
        assert!(
            out.contains("The function returns void; no value."),
            "{out}"
        );
    }

    #[test]
    fn info_args_names_the_parameter_and_refuses_to_invent_a_location() {
        let dir = TempDir::new("info-args");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break factorial", "run", "info args"]);
        assert!(out.contains("n = <unavailable>"), "{out}");
        assert!(out.contains("no DW_AT_location"), "{out}");
        assert!(out.contains("(i64;"), "{out}");
        assert!(!out.contains("optimized out"), "{out}");
    }

    #[test]
    fn finish_stops_at_a_breakpoint_hit_on_the_way_out() {
        let dir = TempDir::new("finish-bp");
        let exe = subject!(dir);
        let out = ldb(&exe, &["break factorial", "run", "finish"]);
        assert!(out.contains("Run till exit from #0  factorial"), "{out}");
        assert_eq!(out.matches("Breakpoint 1, factorial").count(), 2, "{out}");
        assert!(!out.contains("Value returned"), "{out}");
    }

    #[test]
    fn continue_runs_to_exit() {
        let dir = TempDir::new("exit");
        let exe = subject!(dir);
        let out = ldb(&exe, &["run"]);
        assert!(out.contains("exited normally"), "{out}");
    }

    #[test]
    fn commands_refuse_clearly_when_nothing_is_running() {
        let dir = TempDir::new("not-running");
        let exe = subject!(dir);
        let out = ldb(&exe, &["bt", "info args", "finish", "step", "continue"]);
        assert_eq!(
            out.matches("The program is not being run").count(),
            5,
            "{out}"
        );
    }
}
