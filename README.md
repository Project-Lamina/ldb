# ldb

gdb/lldb alternative debugger by Lamina Chain

Native Linux x86_64 ELF only. No remote protocol, no core files, no attaching to a
running process. `ldb` forks the program under `PTRACE_TRACEME` and drives it with
`PTRACE_PEEKDATA`/`POKEDATA`, `PTRACE_GETREGS` and `waitpid`.

The DWARF reader is ldb's own. `gimli` is not a dependency. The only dependencies are
`libc` and `log`.

## Usage

```
ldb <program> [program args...] [-ex <command>]... [-batch]
```

`-ex` runs a command before ldb reads stdin. `-batch` exits after the `-ex` commands.

```
functions [substring]      list functions with their source file and decl line
break <function>           breakpoint past the prologue
break <file>:<line>        breakpoint on a source line
break *0x<addr>           breakpoint on a raw address
info breakpoints           list breakpoints
delete <n>                 remove breakpoint n
run                        start the program
continue                   resume
step                       run one source line, entering functions that have one
finish                     run to return and report the value
bt                         backtrace
info args                  name each parameter of the current frame
info line                  source line of the program counter
kill                       terminate the program
help                       print this list
quit                       leave ldb
```

Aliases exist for some of these (`b`, `d`, `r`, `c`, `s`, `fin`, `h`, `q`).

A session against a binary built with `lamina recursive_factorial.lamina -g -o factorial`:

```
$ ldb ./factorial -batch -ex 'break factorial' -ex run -ex bt -ex finish
Reading symbols from ./factorial...
Breakpoint 1 at 0x401037: file recursive_factorial.lamina, line 4.
Starting program: ./factorial

Breakpoint 1, factorial (n=<unavailable>) at recursive_factorial.lamina:4
4	    %is_zero = eq.i64 %n, 0
#0  factorial (n=<unavailable>) at recursive_factorial.lamina:4
#1  0x00000000004010ac in main () at recursive_factorial.lamina:19
Run till exit from #0  factorial (n=<unavailable>) at recursive_factorial.lamina:4
0x00000000004010ac in main () at recursive_factorial.lamina:19
19	    %result1 = call @factorial(0)  # Should be 1
Value returned is 1
```

## What works

- ELF64 files with `e_machine` 62 (x86_64). Anything else is refused.
- `.debug_info`: DWARF 2 through 4, and DWARF 5 `DW_UT_compile` units. 32- and 64-bit
  unit formats. DWARF 5 units of any other type are refused.
- `.debug_line`: DWARF 2 through 5.
- `.debug_abbrev` and `.debug_str`.
- `.eh_frame`: CIE versions 1 and 3, `DW_EH_PE` pointer encodings, and the call-frame
  opcodes that do not need an expression evaluator, including `remember_state` and
  `restore_state`. `DW_CFA_def_cfa_expression` and `DW_CFA_expression` are refused.
- Backtraces are driven by CFI.
- `finish` reads the return value through the subprogram's `DW_AT_type`: integers
  sign- or zero-extended by their byte size, `bool`, pointers, and `f32`/`f64` from
  `xmm0`.
- PIE and non-PIE executables. The load bias comes from `/proc/<pid>/maps`.

## What it refuses

`info args` names each parameter and reports it as unavailable. Lamina emits no
`DW_AT_location` today, so the value of a parameter cannot be read, and ldb does not
guess one. A wrong value shown with confidence is worse than a refusal.

Other refusals name what is missing. `break file:line` on a line with no code lists the
nearest lines that have code. An unknown function is named. An unreadable `DW_FORM` is
reported by its code. `DW_CFA_def_cfa_expression` says that ldb has no DWARF expression
evaluator.

## Not yet

- No `next`, `up`, `down`, `frame`, `list`, watchpoints, conditional breakpoints or
  temporary breakpoints.
- No `print` of an expression, and no reading of locals or globals. Parameter values
  need `DW_AT_location` and a DWARF expression evaluator first.
- No disassembler.
- No `attach`, no core files, no multi-threaded or multi-process inferiors.
- No `.debug_ranges` or `.debug_rnglists`. A function split into non-contiguous ranges
  is not found.
- No `.debug_lamina` section. Lamina does not write one and ldb does not read one.

## Building

```
cargo build --release
cargo clippy --all-targets
cargo fmt --check
cargo test
```

The integration tests build real programs with the lamina compiler. They look for
`../lamina/target/release/lamina`, or `$LAMINA`, and skip themselves if neither exists.
