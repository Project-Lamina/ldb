//! Debugger state and the implementation of each command.

use crate::error::{LdbError, Result};
use crate::inferior::{Inferior, Status};
use crate::symbols::{DebugInfo, Function, Param, TypeInfo, TypeKind};
use crate::unwind::{self, Frame, StopReason};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const STEP_LIMIT: usize = 500_000;

/// What `info args` prints when a parameter DIE carries no `DW_AT_location`.
/// Lamina emits none today; inventing one would be a wrong answer, which the
/// project ranks below a loud refusal.
const NO_LOCATION: &str = "<unavailable>";

#[derive(Debug, Clone)]
pub struct UserBreakpoint {
    pub number: u32,
    /// Address as the ELF file records it, before any load bias.
    pub file_address: u64,
    pub what: String,
}

pub struct Session {
    pub db: DebugInfo,
    program: PathBuf,
    args: Vec<String>,
    inferior: Option<Inferior>,
    breakpoints: Vec<UserBreakpoint>,
    next_number: u32,
    bias: u64,
    source_cache: HashMap<String, Vec<String>>,
}

impl Session {
    pub fn open(program: &Path, args: Vec<String>) -> Result<Self> {
        let db = DebugInfo::load(program)?;
        Ok(Session {
            db,
            program: program.to_path_buf(),
            args,
            inferior: None,
            breakpoints: Vec::new(),
            next_number: 1,
            bias: 0,
            source_cache: HashMap::new(),
        })
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    fn inferior(&self) -> Result<&Inferior> {
        self.inferior
            .as_ref()
            .filter(|i| i.is_running())
            .ok_or(LdbError::NotRunning)
    }

    fn inferior_mut(&mut self) -> Result<&mut Inferior> {
        self.inferior
            .as_mut()
            .filter(|i| i.is_running())
            .ok_or(LdbError::NotRunning)
    }

    fn runtime(&self, file_address: u64) -> u64 {
        file_address.wrapping_add(self.bias)
    }

    fn file_address(&self, runtime: u64) -> u64 {
        runtime.wrapping_sub(self.bias)
    }

    // ---- functions -------------------------------------------------------

    pub fn cmd_functions(&self, filter: Option<&str>) {
        if self.db.functions.is_empty() {
            println!(
                "No functions in the debug info of {}.",
                self.program.display()
            );
            if self.db.units.is_empty() {
                println!("(.debug_info holds no compilation unit at all)");
            }
            return;
        }
        let matched: Vec<&Function> = self
            .db
            .functions
            .iter()
            .filter(|f| filter.is_none_or(|pat| f.name.contains(pat)))
            .collect();
        if matched.is_empty() {
            println!(
                "No function in the debug info has `{}` in its name; {} are known.",
                filter.unwrap_or(""),
                self.db.functions.len()
            );
            return;
        }
        let mut by_file: HashMap<String, Vec<&Function>> = HashMap::new();
        for f in matched {
            by_file
                .entry(
                    f.decl_file
                        .clone()
                        .unwrap_or_else(|| "<no DW_AT_decl_file>".to_string()),
                )
                .or_default()
                .push(f);
        }
        let mut files: Vec<&String> = by_file.keys().collect();
        files.sort();
        println!("All defined functions:");
        for file in files {
            println!("\nFile {}:", file);
            let mut fns = by_file[file].clone();
            fns.sort_by(|a, b| a.name.cmp(&b.name));
            for f in fns {
                let line = f.decl_line.map_or("?".to_string(), |l| l.to_string());
                let scope = if f.external { "" } else { "static " };
                println!("{}:\t{}{};", line, scope, f.signature());
            }
        }
    }

    // ---- breakpoints -----------------------------------------------------

    pub fn cmd_break(&mut self, spec: &str) -> Result<()> {
        let (file_address, what) = self.resolve_location(spec)?;
        let number = self.next_number;
        self.next_number += 1;
        self.breakpoints.push(UserBreakpoint {
            number,
            file_address,
            what: what.clone(),
        });
        if self.inferior.as_ref().is_some_and(Inferior::is_running) {
            let addr = self.runtime(file_address);
            self.inferior_mut()?.insert_breakpoint(addr)?;
        }
        println!("Breakpoint {} at {:#x}: {}", number, file_address, what);
        Ok(())
    }

    fn resolve_location(&self, spec: &str) -> Result<(u64, String)> {
        if let Some(rest) = spec.strip_prefix('*') {
            let rest = rest.trim();
            // `break *N` must say its own base. Reading a bare `4198400` as hex
            // would quietly set the breakpoint somewhere else entirely.
            let addr = match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16).map_err(|_| {
                    LdbError::Unsupported(format!("`{}` is not a hexadecimal address", rest))
                })?,
                None => {
                    return Err(LdbError::Unsupported(format!(
                        "`{}` has no base; write `*0x{}` for hexadecimal",
                        rest, rest
                    )));
                }
            };
            let what = if self.db.elf.is_pie() {
                format!(
                    "file address {:#x} (this image is ET_DYN, so the load bias is added at run)",
                    addr
                )
            } else {
                format!("address {:#x}", addr)
            };
            return Ok((addr, what));
        }
        if let Some((file, line)) = spec.rsplit_once(':')
            && let Ok(line) = line.parse::<u64>()
        {
            return self.resolve_file_line(file, line);
        }
        let f = self
            .db
            .function_named(spec)
            .ok_or_else(|| LdbError::NoSuchSymbol(spec.to_string()))?;
        let addr = self.db.breakpoint_address(f);
        let what = match self.db.source_at(addr) {
            Some((file, line)) => format!("file {}, line {}.", file, line),
            None => format!(
                "function {}, which has no .debug_line row to name a source line.",
                f.name
            ),
        };
        Ok((addr, what))
    }

    fn resolve_file_line(&self, file: &str, line: u64) -> Result<(u64, String)> {
        if let Some(addr) = self.db.address_for_file_line(file, line) {
            return Ok((addr, format!("file {}, line {}.", file, line)));
        }
        let known = self.db.lines_in_file(file);
        if known.is_empty() {
            return Err(LdbError::MissingDebugInfo(format!(
                "no file named {} in .debug_line",
                file
            )));
        }
        Err(LdbError::MissingDebugInfo(format!(
            "{}:{} has no code. The nearest lines that do: {}",
            file,
            line,
            nearest(&known, line)
        )))
    }

    pub fn cmd_info_breakpoints(&self) {
        if self.breakpoints.is_empty() {
            println!("No breakpoints.");
            return;
        }
        println!("Num     Type           Address            What");
        for b in &self.breakpoints {
            println!(
                "{:<8}breakpoint     0x{:016x} {}",
                b.number,
                self.runtime(b.file_address),
                b.what
            );
        }
    }

    pub fn cmd_delete(&mut self, number: u32) -> Result<()> {
        let Some(i) = self.breakpoints.iter().position(|b| b.number == number) else {
            println!("No breakpoint number {}.", number);
            return Ok(());
        };
        let file_address = self.breakpoints[i].file_address;
        self.breakpoints.remove(i);
        // Several user breakpoints may share one address and one int3 byte;
        // the byte goes back only once the last of them is gone.
        if self
            .breakpoints
            .iter()
            .any(|b| b.file_address == file_address)
        {
            return Ok(());
        }
        if self.inferior.as_ref().is_some_and(Inferior::is_running) {
            let addr = self.runtime(file_address);
            self.inferior_mut()?.remove_breakpoint(addr)?;
        }
        Ok(())
    }

    // ---- running ---------------------------------------------------------

    pub fn cmd_run(&mut self) -> Result<()> {
        if self.inferior.as_ref().is_some_and(Inferior::is_running) {
            println!("The program is already running; ldb restarts it.");
            self.inferior = None;
        }
        let mut inf = Inferior::launch(&self.program, &self.args.clone())?;
        self.bias = if self.db.elf.is_pie() {
            inf.detect_load_bias(&self.program)?
        } else {
            0
        };
        inf.set_load_bias(self.bias);
        for b in &self.breakpoints {
            inf.insert_breakpoint(b.file_address.wrapping_add(self.bias))?;
        }
        self.inferior = Some(inf);
        println!("Starting program: {}", self.program.display());
        let status = self.inferior_mut()?.resume()?;
        self.report_stop(status)
    }

    pub fn cmd_continue(&mut self) -> Result<()> {
        self.inferior()?;
        println!("Continuing.");
        let status = self.inferior_mut()?.resume()?;
        self.report_stop(status)
    }

    pub fn cmd_kill(&mut self) {
        if let Some(inf) = self.inferior.as_mut() {
            inf.kill();
        }
        self.inferior = None;
    }

    fn report_stop(&mut self, status: Status) -> Result<()> {
        match status {
            Status::Exited(code) => {
                println!(
                    "[Inferior 1 (process {}) exited {}]",
                    self.inferior.as_ref().map_or(0, Inferior::pid),
                    if code == 0 {
                        "normally".to_string()
                    } else {
                        format!("with code {}", code)
                    }
                );
                self.inferior = None;
                Ok(())
            }
            Status::Signalled(sig) => {
                println!("Program terminated with signal {}.", sig);
                self.inferior = None;
                Ok(())
            }
            Status::Stopped(sig) => self.report_signal_stop(sig),
        }
    }

    fn report_signal_stop(&mut self, sig: i32) -> Result<()> {
        match self.inferior_mut()?.rewind_over_breakpoint()? {
            Some(addr) => self.announce_stop_at(addr),
            None if sig == libc::SIGTRAP => println!(),
            None => print!("\nProgram received signal {}, ", sig),
        }
        let pc = self.inferior()?.regs()?.rip;
        self.print_frame_line(None, pc, false);
        self.print_source_line(pc);
        Ok(())
    }

    fn announce_stop_at(&self, addr: u64) {
        let numbers = self.user_breakpoints_at(addr);
        if numbers.is_empty() {
            print!("\nStopped at {:#x}, ", addr);
            return;
        }
        let list = numbers
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        print!("\nBreakpoint {}, ", list);
    }

    fn announce_breakpoint(&mut self, addr: u64) {
        self.announce_stop_at(addr);
        self.print_frame_line(None, addr, false);
        self.print_source_line(addr);
    }

    // ---- frames ----------------------------------------------------------

    /// The stack, truncated after `main`. Everything above it is startup code
    /// that carries no CFI, so the walk could not continue anyway.
    fn frames(&self) -> Result<(Vec<Frame>, Option<StopReason>)> {
        let inf = self.inferior()?;
        let eh = self.db.cfi()?;
        let (mut frames, stop) = unwind::backtrace(inf, eh, self.bias)?;
        if let Some(i) = frames.iter().position(|f| {
            self.db
                .function_at(self.file_address(f.lookup_pc()))
                .is_some_and(|fun| fun.name == "main")
        }) {
            frames.truncate(i + 1);
            return Ok((frames, None));
        }
        Ok((frames, Some(stop)))
    }

    pub fn cmd_backtrace(&self) -> Result<()> {
        let (frames, stop) = self.frames()?;
        for (i, f) in frames.iter().enumerate() {
            self.print_frame_line(Some(i), f.pc, f.is_return_address);
        }
        if let Some(reason) = stop {
            println!("{}", reason);
        }
        Ok(())
    }

    fn print_frame_line(&self, index: Option<usize>, pc: u64, is_return_address: bool) {
        let lookup = if is_return_address {
            pc.wrapping_sub(1)
        } else {
            pc
        };
        let file_pc = self.file_address(lookup);
        let func = self.db.function_at(file_pc);
        if let Some(i) = index {
            print!("#{}  ", i);
        }
        let name = match func {
            Some(f) => format!("{} ({})", f.name, self.format_args(f)),
            None => match self
                .db
                .elf
                .functions
                .iter()
                .rev()
                .find(|s| s.value <= file_pc && file_pc < s.value + s.size.max(1))
            {
                Some(sym) => format!("{} ()", sym.name),
                None => "?? ()".to_string(),
            },
        };
        let at = match self.db.source_at(file_pc) {
            Some((file, line)) => format!(" at {}:{}", file, line),
            None => String::new(),
        };
        let show_addr = is_return_address || at.is_empty() || !self.at_row_start(file_pc);
        if show_addr {
            println!("0x{:016x} in {}{}", pc, name, at);
        } else {
            println!("{}{}", name, at);
        }
    }

    fn format_args(&self, f: &Function) -> String {
        if f.signature_unrecorded() {
            return "<no parameter DIE, no DW_AT_prototyped>".to_string();
        }
        f.params
            .iter()
            .map(|p| {
                format!(
                    "{}={}",
                    p.name.as_deref().unwrap_or("<anonymous>"),
                    if p.has_location {
                        "<not evaluated>"
                    } else {
                        NO_LOCATION
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn cmd_info_args(&self) -> Result<()> {
        let pc = self.file_address(self.inferior()?.regs()?.rip);
        let Some(f) = self.db.function_at(pc) else {
            return Err(LdbError::MissingDebugInfo(format!(
                "{:#x} is in no DW_TAG_subprogram",
                pc
            )));
        };
        if f.signature_unrecorded() {
            return Err(LdbError::MissingDebugInfo(format!(
                "{} has no DW_TAG_formal_parameter child and no DW_AT_prototyped, so this \
                 binary does not record whether it takes arguments",
                f.name
            )));
        }
        if f.params.is_empty() {
            println!("No arguments.");
            return Ok(());
        }
        for p in &f.params {
            println!("{}", describe_param(p));
        }
        Ok(())
    }

    // ---- stepping --------------------------------------------------------

    pub fn cmd_step(&mut self) -> Result<()> {
        let start_pc = self.inferior()?.regs()?.rip;
        let start = self.db.source_at(self.file_address(start_pc));
        let mut steps = 0usize;
        loop {
            if steps >= STEP_LIMIT {
                return Err(LdbError::Unsupported(format!(
                    "step gave up after {} instructions without reaching a new source line",
                    STEP_LIMIT
                )));
            }
            steps += 1;
            match self.inferior_mut()?.single_step()? {
                Status::Stopped(_) => {}
                other => return self.report_stop(other),
            }
            let pc = self.inferior()?.regs()?.rip;
            if self.inferior()?.has_breakpoint(pc) {
                // Landed exactly on a user breakpoint address.
                return self.report_landing(pc);
            }
            let file_pc = self.file_address(pc);
            match self.db.source_at(file_pc) {
                Some(here) => {
                    if Some(&here) != start.as_ref() && self.at_row_start(file_pc) {
                        return self.report_landing(pc);
                    }
                }
                None if self.db.function_at(file_pc).is_some() => {
                    // Inside a known subprogram with no line row: Lamina leaves
                    // the prologue uncovered, so keep stepping to reach its body.
                }
                None => {
                    if self.leave_unknown_code()? {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn at_row_start(&self, file_pc: u64) -> bool {
        self.db.lines.iter().any(|lp| {
            lp.rows
                .iter()
                .any(|r| !r.end_sequence && r.address == file_pc)
        })
    }

    /// Run out of a region with no line table rather than single-stepping
    /// through it. Returns `true` when the program stopped for another reason.
    fn leave_unknown_code(&mut self) -> Result<bool> {
        let (frames, _) = self.frames()?;
        let Some(caller) = frames.get(1) else {
            return Ok(false);
        };
        let target = caller.pc;
        let guard = frames[0].cfa;
        let planted = !self.inferior()?.has_breakpoint(target);
        if planted {
            self.inferior_mut()?.insert_breakpoint(target)?;
        }
        let outcome = self.run_to(target, guard);
        if planted && self.inferior.as_ref().is_some_and(Inferior::is_running) {
            self.inferior_mut()?.remove_breakpoint(target)?;
        }
        outcome
    }

    /// Resume until `target` is reached with the stack already unwound past
    /// `guard`, so recursion does not stop us at the wrong depth.
    fn run_to(&mut self, target: u64, guard: u64) -> Result<bool> {
        loop {
            let status = self.inferior_mut()?.resume()?;
            match status {
                Status::Stopped(sig) if sig == libc::SIGTRAP => {}
                other => {
                    self.report_stop(other)?;
                    return Ok(true);
                }
            }
            let hit = self.inferior_mut()?.rewind_over_breakpoint()?;
            let regs = self.inferior()?.regs()?;
            if hit == Some(target) && regs.rsp >= guard {
                return Ok(false);
            }
            if let Some(addr) = hit
                && self.user_breakpoint_at(addr).is_some()
            {
                self.announce_breakpoint(addr);
                return Ok(true);
            }
        }
    }

    fn user_breakpoint_at(&self, runtime: u64) -> Option<u32> {
        self.breakpoints
            .iter()
            .find(|b| self.runtime(b.file_address) == runtime)
            .map(|b| b.number)
    }

    /// Every user breakpoint sharing `runtime`, so a stop names all of them
    /// rather than only the first.
    fn user_breakpoints_at(&self, runtime: u64) -> Vec<u32> {
        self.breakpoints
            .iter()
            .filter(|b| self.runtime(b.file_address) == runtime)
            .map(|b| b.number)
            .collect()
    }

    fn report_landing(&mut self, pc: u64) -> Result<()> {
        match self.user_breakpoint_at(pc) {
            Some(_) => {
                self.announce_stop_at(pc);
                self.print_frame_line(None, pc, false);
            }
            None => {
                let file_pc = self.file_address(pc);
                let entering = self
                    .db
                    .function_at(file_pc)
                    .is_some_and(|f| self.db.breakpoint_address(f) == file_pc);
                if entering {
                    self.print_frame_line(None, pc, false);
                }
            }
        }
        self.print_source_line(pc);
        Ok(())
    }

    // ---- finish ----------------------------------------------------------

    pub fn cmd_finish(&mut self) -> Result<()> {
        let (frames, stop) = self.frames()?;
        if frames.len() < 2 {
            if let Some(reason) = stop {
                return Err(LdbError::MissingDebugInfo(format!(
                    "the selected frame has no caller ldb can name {}",
                    reason
                )));
            }
            return Err(LdbError::MissingDebugInfo(
                "the selected frame has no caller that .eh_frame can name".into(),
            ));
        }
        let callee_pc = self.file_address(frames[0].pc);
        let callee = self.db.function_at(callee_pc);
        let return_type = callee.and_then(|f| f.return_type.clone());
        let no_die = callee.is_none();
        let return_unrecorded = callee.is_some_and(Function::signature_unrecorded);
        print!("Run till exit from #0  ");
        self.print_frame_line(None, frames[0].pc, false);

        let target = frames[1].pc;
        let guard = frames[0].cfa;
        let planted = !self.inferior()?.has_breakpoint(target);
        if planted {
            self.inferior_mut()?.insert_breakpoint(target)?;
        }
        let interrupted = self.run_to(target, guard);
        if planted && self.inferior.as_ref().is_some_and(Inferior::is_running) {
            self.inferior_mut()?.remove_breakpoint(target)?;
        }
        if interrupted? {
            return Ok(());
        }

        let regs = self.inferior()?.regs()?;
        self.print_frame_line(None, regs.rip, true);
        self.print_source_line(regs.rip.wrapping_sub(1));
        let xmm0 = self.inferior()?.xmm0()?;
        if no_die {
            println!(
                "No DW_TAG_subprogram covers {:#x}, so ldb has no return type to \
                 format; rax = {:#x}",
                callee_pc, regs.rax
            );
            return Ok(());
        }
        if return_unrecorded {
            println!(
                "The DIE for this function carries no DW_AT_type and no DW_AT_prototyped, \
                 so ldb cannot say whether it returns a value; rax = {:#x}",
                regs.rax
            );
            return Ok(());
        }
        self.print_return_value(return_type.as_ref(), regs.rax, &xmm0);
        Ok(())
    }

    fn print_return_value(&self, ty: Option<&TypeInfo>, rax: u64, xmm0: &[u8; 16]) {
        let Some(ty) = ty else {
            println!("The function returns void; no value.");
            return;
        };
        match ty.kind {
            TypeKind::Float => match ty.byte_size {
                4 => println!(
                    "Value returned is {}",
                    f32::from_le_bytes([xmm0[0], xmm0[1], xmm0[2], xmm0[3]])
                ),
                8 => println!(
                    "Value returned is {}",
                    f64::from_le_bytes([
                        xmm0[0], xmm0[1], xmm0[2], xmm0[3], xmm0[4], xmm0[5], xmm0[6], xmm0[7],
                    ])
                ),
                n => println!(
                    "Value returned has type {} of {} bytes, which the ABI does not \
                     return whole in xmm0; ldb will not guess.",
                    ty.name, n
                ),
            },
            TypeKind::Signed => {
                println!("Value returned is {}", sign_extend(rax, ty.byte_size))
            }
            TypeKind::Unsigned => {
                println!("Value returned is {}", truncate(rax, ty.byte_size))
            }
            TypeKind::Bool => println!(
                "Value returned is {}",
                if truncate(rax, ty.byte_size.max(1)) != 0 {
                    "true"
                } else {
                    "false"
                }
            ),
            TypeKind::Pointer => println!("Value returned is ({}) {:#x}", ty.name, rax),
            TypeKind::Other => println!(
                "Value returned has type {}, which ldb cannot format; rax = {:#x}",
                ty.name, rax
            ),
        }
    }

    // ---- source ----------------------------------------------------------

    fn print_source_line(&mut self, pc: u64) {
        let Some((file, line)) = self.db.source_at(self.file_address(pc)) else {
            return;
        };
        let Some(text) = self.source_line(&file, line) else {
            return;
        };
        println!("{}\t{}", line, text);
    }

    fn source_line(&mut self, file: &str, line: u64) -> Option<String> {
        if !self.source_cache.contains_key(file) {
            let path = self.db.source_file_path(file);
            let text = std::fs::read_to_string(path).ok()?;
            self.source_cache
                .insert(file.to_string(), text.lines().map(str::to_string).collect());
        }
        self.source_cache
            .get(file)?
            .get(line.checked_sub(1)? as usize)
            .cloned()
    }

    pub fn cmd_info_line(&self) -> Result<()> {
        let pc = self.file_address(self.inferior()?.regs()?.rip);
        match self.db.source_at(pc) {
            Some((file, line)) => println!("{:#x} is {}:{}", pc, file, line),
            None => println!(
                "{:#x} has no .debug_line row; the line table does not cover it",
                pc
            ),
        }
        Ok(())
    }
}

fn describe_param(p: &Param) -> String {
    let name = p.name.as_deref().unwrap_or("<anonymous>");
    let ty =
        p.ty.as_ref()
            .map_or("<no DW_AT_type>".to_string(), |t| t.name.clone());
    if p.has_location {
        format!("{} = <{}, location not evaluated by ldb>", name, ty)
    } else {
        format!(
            "{} = {}  ({}; the DW_TAG_formal_parameter DIE carries no DW_AT_location, \
             so its value is not recorded in this binary)",
            name, NO_LOCATION, ty
        )
    }
}

fn sign_extend(value: u64, byte_size: u64) -> i64 {
    match byte_size {
        1 => i64::from(value as u8 as i8),
        2 => i64::from(value as u16 as i16),
        4 => i64::from(value as u32 as i32),
        _ => value as i64,
    }
}

fn truncate(value: u64, byte_size: u64) -> u64 {
    match byte_size {
        1 => value & 0xff,
        2 => value & 0xffff,
        4 => value & 0xffff_ffff,
        _ => value,
    }
}

fn nearest(lines: &[u64], wanted: u64) -> String {
    let mut sorted: Vec<u64> = lines.to_vec();
    sorted.sort_by_key(|l| l.abs_diff(wanted));
    let mut pick: Vec<u64> = sorted.into_iter().take(5).collect();
    pick.sort_unstable();
    pick.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_extension_follows_the_declared_byte_size() {
        assert_eq!(sign_extend(0xff, 1), -1);
        assert_eq!(sign_extend(0xff, 2), 255);
        assert_eq!(sign_extend(0xffff_ffff, 4), -1);
        assert_eq!(sign_extend(0xffff_ffff, 8), 0xffff_ffff);
    }

    #[test]
    fn truncation_follows_the_declared_byte_size() {
        assert_eq!(truncate(0x1234, 1), 0x34);
        assert_eq!(truncate(0x1_0000_0001, 4), 1);
        assert_eq!(truncate(0x1_0000_0001, 8), 0x1_0000_0001);
    }

    #[test]
    fn a_parameter_without_a_location_says_so() {
        let p = Param {
            name: Some("n".into()),
            ty: Some(TypeInfo {
                name: "i64".into(),
                kind: TypeKind::Signed,
                byte_size: 8,
            }),
            has_location: false,
        };
        let text = describe_param(&p);
        assert!(text.starts_with("n = <unavailable>"));
        assert!(text.contains("no DW_AT_location"));
        assert!(text.contains("i64"));
    }
}
