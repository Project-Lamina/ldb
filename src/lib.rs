//! ldb - a native Linux x86_64 ELF debugger for Lamina.
//!
//! The DWARF reader under [`dwarf`] is written here rather than taken from an
//! existing crate on purpose: reading our own debug info with our own parser is
//! how the project finds out whether what Lamina emits is actually consumable.
//!
//! The command line is the interface: [`main`] runs one session from
//! `std::env::args` and exits with its status, exactly as the `ldb` binary does.

pub mod dwarf;
pub mod elf;
pub mod error;
pub mod inferior;
pub mod repl;
pub mod session;
pub mod symbols;
pub mod unwind;

pub use error::{LdbError, Result};
pub use session::Session;

use std::io::IsTerminal;
use std::path::PathBuf;

const USAGE: &str = "\
Usage: ldb <program> [program args...] [-ex <command>]...
Options:
  -ex <command>   run a command before reading from stdin
  -batch          run the -ex commands and exit without reading stdin
  -h, --help      print this message
";

struct Args {
    program: PathBuf,
    program_args: Vec<String>,
    commands: Vec<String>,
    batch: bool,
}

fn parse_args(argv: &[String]) -> std::result::Result<Args, String> {
    let mut program: Option<PathBuf> = None;
    let mut program_args = Vec::new();
    let mut commands = Vec::new();
    let mut batch = false;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => return Err(String::new()),
            "-batch" | "--batch" => batch = true,
            "-ex" | "--eval-command" => {
                i += 1;
                commands.push(
                    argv.get(i)
                        .ok_or_else(|| "-ex needs a command".to_string())?
                        .clone(),
                );
            }
            other if program.is_none() => program = Some(PathBuf::from(other)),
            other => program_args.push(other.to_string()),
        }
        i += 1;
    }
    Ok(Args {
        program: program.ok_or_else(|| "ldb needs a program to debug".to_string())?,
        program_args,
        commands,
        batch,
    })
}

pub fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("ldb: {}", msg);
            }
            eprint!("{}", USAGE);
            std::process::exit(if msg.is_empty() { 0 } else { 2 });
        }
    };

    let mut session = match Session::open(&args.program, args.program_args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ldb: {}", e);
            std::process::exit(1);
        }
    };
    println!("Reading symbols from {}...", session.program().display());
    report_coverage(&session);

    let script = args.commands.join("\n") + "\n";
    let mut cursor = std::io::Cursor::new(script.into_bytes());
    if let Err(e) = repl::run(&mut session, &mut cursor, false) {
        eprintln!("ldb: {}", e);
    }
    if args.batch {
        return;
    }
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let mut locked = stdin.lock();
    if let Err(e) = repl::run(&mut session, &mut locked, interactive) {
        eprintln!("ldb: {}", e);
        std::process::exit(1);
    }
}

fn report_coverage(session: &Session) {
    let db = &session.db;
    if db.units.is_empty() {
        println!("(no .debug_info: ldb can set breakpoints only on raw addresses)");
    }
    if db.lines.iter().all(|l| l.rows.is_empty()) {
        println!("(no .debug_line: ldb cannot map addresses to source lines)");
    }
    if db.eh_frame.is_none() {
        println!("(no .eh_frame: ldb cannot produce a backtrace)");
    }
}
