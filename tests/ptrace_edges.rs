//! Edge cases around the traced process, and the refusals that must stand in
//! for an answer ldb cannot give. These build with the system C compiler, so
//! they exercise producers other than lamina.

mod common;

use common::{TempDir, cc, ldb};

macro_rules! build {
    ($dir:ident, $name:expr, $src:expr, $flags:expr) => {
        match cc(&$dir, $name, $src, $flags) {
            Some(exe) => exe,
            None => {
                eprintln!("skipping: no working cc");
                return;
            }
        }
    };
}

const CRASHER: &str = "int boom(void) { volatile int *p = (int*)0; return *p; }\n\
                       int later(void) { return 7; }\n\
                       int main(void) { boom(); return later(); }\n";

const RECURSE: &str = "long rec(long n) { if (n == 0) return 0; return n + rec(n-1); }\n\
                       int main(void) { return (int)rec(400); }\n";

const LEAF: &str = "__attribute__((noinline)) long leaf(long a, long b) { return a*b+1; }\n\
                    int main(void) { return (int)leaf(3, 4); }\n";

const LEAF_TWICE: &str = "__attribute__((noinline)) long leaf(long a, long b) { return a*b+1; }\n\
                          int main(void) { return (int)(leaf(3, 4) + leaf(1, 1)); }\n";

#[test]
fn a_program_that_exits_immediately_leaves_every_later_command_refusing() {
    let dir = TempDir::new("exits");
    let exe = build!(dir, "exits", "int main(void) { return 3; }\n", &["-g"]);
    let out = ldb(
        &exe,
        &["run", "bt", "continue", "finish", "step", "info args"],
    );
    assert!(out.contains("exited with code 3"), "{out}");
    assert_eq!(
        out.matches("The program is not being run").count(),
        5,
        "{out}"
    );
}

/// A dropped stop signal would leave `continue` re-running the faulting
/// instruction for ever; the signal is delivered on resume instead.
#[test]
fn continue_after_a_fault_delivers_the_signal_instead_of_looping() {
    let dir = TempDir::new("fault");
    let exe = build!(dir, "fault", CRASHER, &["-g"]);
    let out = ldb(&exe, &["break later", "run", "continue", "continue"]);
    assert!(out.contains("Program received signal 11"), "{out}");
    assert!(out.contains("Program terminated with signal 11."), "{out}");
    assert_eq!(out.matches("Program received signal").count(), 1, "{out}");
    assert!(out.contains("The program is not being run"), "{out}");
}

/// Two user breakpoints share one `int3`. Deleting one used to lift the byte
/// and leave the other listed but dead.
#[test]
fn deleting_one_of_two_breakpoints_at_an_address_leaves_the_other_live() {
    let dir = TempDir::new("dup-bp");
    let exe = build!(dir, "dup", LEAF_TWICE, &["-g", "-O0"]);
    let out = ldb(
        &exe,
        &["break leaf", "break leaf", "run", "delete 1", "continue"],
    );
    assert!(out.contains("Breakpoint 1, 2, leaf"), "{out}");
    assert!(
        out.contains("Breakpoint 2, leaf"),
        "breakpoint 2 stopped firing once 1 was deleted: {out}"
    );
}

#[test]
fn a_backtrace_cut_short_by_the_frame_limit_says_so() {
    let dir = TempDir::new("deep");
    let exe = build!(dir, "deep", RECURSE, &["-g", "-O0"]);
    let mut cmds = vec!["break rec".to_string(), "run".to_string()];
    cmds.extend(std::iter::repeat_n("c".to_string(), 399));
    cmds.push("bt".to_string());
    let refs: Vec<&str> = cmds.iter().map(String::as_str).collect();
    let out = ldb(&exe, &refs);
    assert!(out.contains("More stack frames follow"), "{out}");
}

#[test]
fn finish_from_the_outermost_frame_is_refused_by_name() {
    let dir = TempDir::new("outermost");
    let exe = build!(dir, "outermost", LEAF, &["-g", "-O0"]);
    let out = ldb(&exe, &["break main", "run", "finish"]);
    assert!(out.contains("has no caller"), "{out}");
}

/// gcc at `-g1` emits a subprogram with no `DW_AT_type`, no `DW_AT_prototyped`
/// and no parameter DIE. Reading that as `void f(void)` is a wrong answer:
/// the function really takes two arguments and returns one.
#[test]
fn a_subprogram_with_no_recorded_signature_is_not_reported_as_void() {
    let dir = TempDir::new("g1");
    let exe = build!(dir, "g1", LEAF, &["-g1", "-O0"]);
    let out = ldb(&exe, &["functions leaf"]);
    assert!(!out.contains("void leaf(void)"), "{out}");
    assert!(out.contains("no DW_AT_prototyped"), "{out}");
}

#[test]
fn finish_on_an_unrecorded_signature_reports_rax_rather_than_claiming_void() {
    let dir = TempDir::new("g1-finish");
    let exe = build!(dir, "g1f", LEAF, &["-g1", "-O0"]);
    let out = ldb(&exe, &["break *0x1129", "run", "finish", "info args"]);
    assert!(!out.contains("returns void"), "{out}");
}

#[test]
fn a_stripped_binary_says_what_it_cannot_do_and_still_takes_raw_addresses() {
    let dir = TempDir::new("stripped");
    let exe = build!(dir, "stripped", LEAF, &["-g", "-O0", "-s"]);
    let out = ldb(&exe, &["break leaf", "info line", "bt"]);
    assert!(out.contains("no .debug_info"), "{out}");
    assert!(out.contains("No symbol \"leaf\" in debug info"), "{out}");
    assert_eq!(
        out.matches("The program is not being run").count(),
        2,
        "{out}"
    );
}

#[test]
fn a_non_pie_binary_is_debugged_without_a_bias() {
    let dir = TempDir::new("nopie");
    let exe = build!(dir, "nopie", LEAF, &["-g", "-O0", "-no-pie"]);
    let out = ldb(&exe, &["break leaf", "run", "bt", "finish"]);
    assert!(out.contains("Breakpoint 1, leaf"), "{out}");
    assert!(out.contains("Value returned is 13"), "{out}");
}

#[test]
fn a_pie_binary_is_debugged_with_the_bias_from_proc_maps() {
    let dir = TempDir::new("pie");
    let exe = build!(dir, "pie", LEAF, &["-g", "-O0", "-pie", "-fPIE"]);
    let out = ldb(&exe, &["break leaf", "run", "bt", "finish"]);
    assert!(out.contains("Breakpoint 1, leaf"), "{out}");
    assert!(out.contains("Value returned is 13"), "{out}");
}

#[test]
fn an_unknown_command_names_itself_and_points_at_help() {
    let dir = TempDir::new("unknown-cmd");
    let exe = build!(dir, "unk", LEAF, &["-g"]);
    let out = ldb(&exe, &["frobnicate", "info frobnicate", "break", "delete"]);
    assert!(out.contains("`frobnicate` is not an ldb command"), "{out}");
    assert!(
        out.contains("`info frobnicate` is not an ldb command"),
        "{out}"
    );
    assert!(out.contains("break needs a location"), "{out}");
    assert!(out.contains("delete needs a breakpoint number"), "{out}");
}

#[test]
fn a_breakpoint_address_that_is_not_mapped_is_refused_rather_than_planted() {
    let dir = TempDir::new("badaddr");
    let exe = build!(dir, "badaddr", LEAF, &["-g", "-O0"]);
    let out = ldb(&exe, &["break *0xdeadbeef0000", "run"]);
    assert!(out.contains("ptrace error"), "{out}");
}
