# ldb

gdb/lldb alternative Debugger by Lamina Chain

Native Linux x86_64 ELF only. No remote protocol, no core files, no attaching
to a running process: `ldb` forks the program under `PTRACE_TRACEME` and drives
it with `PTRACE_PEEKDATA`/`POKEDATA`, `PTRACE_GETREGS` and `waitpid`.

The DWARF reader is ours. `gimli` is not a dependency and will not become one.
Reading our own debug info with our own parser is the point: it is how the
project finds out whether what Lamina emits is actually consumable, and the
answers go back to the producer rather than into a workaround here. The only
dependencies are `libc` and `log`.

## Using it

```
ldb <program> [program args...] [-ex <command>]... [-batch]
```

```
(ldb) functions            list functions with their source file and decl line
(ldb) break <function>     breakpoint past the prologue
(ldb) break <file>:<line>  breakpoint on a source line
(ldb) break *0x401037      breakpoint on a raw address
(ldb) run
(ldb) continue
(ldb) step                 one source line, entering functions that have one
(ldb) finish               run to return and report the value
(ldb) bt
(ldb) info args
(ldb) info line
(ldb) info breakpoints
(ldb) delete <n>
(ldb) kill
(ldb) quit
```

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

`.debug_info`, `.debug_abbrev`, `.debug_line` and `.debug_str` are parsed from
scratch, DWARF 2 through 5 for `.debug_info` and `.debug_line`, 32- and 64-bit
unit formats. `.eh_frame` CFI is interpreted far enough to unwind x86_64:
`DW_EH_PE` pointer encodings, the full non-expression call-frame opcode set,
and `remember_state`/`restore_state`.

Backtraces are CFI-driven and are correct through recursion. `finish` reads the
return value through the subprogram's `DW_AT_type`: integers sign- or
zero-extended by the declared byte size, `bool`, pointers, and `f32`/`f64` out
of `xmm0`. Both PIE and non-PIE images work; the load bias comes from
`/proc/<pid>/maps`.

Checked against gdb on lamina-built binaries: 220 breakpoint backtraces across
186 programs agree frame for frame, and 2381 `break`/`run`/`bt`/`info
args`/`finish` cycles complete without a parser or ptrace error.

## What it refuses, and why

`info args` names each parameter and then says the value is unavailable. Lamina
emits no `DW_AT_location` anywhere today, so no debugger can read a parameter
out of one of its binaries. gdb says `<optimized out>`, which is the same
answer in misleading words. Inventing a location would be a silent wrong
answer, which this project ranks below a loud refusal.

Every other refusal names what is missing rather than guessing: a `break
file:line` on a line with no code lists the nearest lines that have some; an
unreadable `DW_FORM` is reported by its code; a `DW_CFA_def_cfa_expression`
says that ldb has no DWARF expression evaluator.

## What is not here yet

No `next` (step over), no `up`/`down`/`frame`, no `list`, no watchpoints, no
conditional or temporary breakpoints, no `print` of an expression, no reading
of locals or globals, no disassembler, no signal handling policy, no
multi-threaded or multi-process inferiors, no core files, no `attach`.

No DWARF expression evaluator, so `DW_AT_location` will need one before `info
args` can print a value. No `.debug_ranges`/`.debug_rnglists`, so a function
split into non-contiguous ranges is not found. No `.debug_lamina`: that section
does not exist yet, and the rule is that nothing is read before it is written
and nothing is written before something reads it.

## Building

```
cargo build --release
cargo clippy --all-targets
cargo fmt --check
cargo test
```

The integration tests build real programs with the lamina compiler. They look
for `../lamina/target/release/lamina`, or `$LAMINA`, and skip themselves if
neither is there.
