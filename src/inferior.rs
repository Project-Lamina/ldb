//! The traced process: fork, `PTRACE_TRACEME`, breakpoints, registers.
//!
//! This is the process-control seam, and it is Linux. macOS drives a debuggee
//! through mach exception ports and Windows through `WaitForDebugEvent`;
//! neither resembles a `waitpid` loop closely enough to share this code, so the
//! module is gated out there rather than abstracted. What it borrows from
//! [`crate::arch`] is the trap bytes and how far past them the program counter
//! reports. The register set it reads is `user_regs_struct`, which is the
//! operating system's shape for an architecture's registers and so stays here.

use crate::arch;
use crate::error::{LdbError, Result};
use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Stopped by a signal; `0` is the stop that follows `execve`.
    Stopped(i32),
    Exited(i32),
    Signalled(i32),
}

#[derive(Debug, Clone)]
struct Breakpoint {
    /// The instruction bytes the trap was written over, `arch::TRAP.len()` of
    /// them.
    original: Vec<u8>,
    enabled: bool,
}

pub struct Inferior {
    pid: libc::pid_t,
    breakpoints: HashMap<u64, Breakpoint>,
    running: bool,
    /// The signal the child last stopped on, other than the `SIGTRAP`s ptrace
    /// raises itself. `PTRACE_CONT` and `PTRACE_SINGLESTEP` take it in `data`
    /// to deliver it on resume; dropping it would make `continue` after a
    /// `SIGSEGV` re-run the faulting instruction for ever.
    pending_signal: i32,
    /// `ET_DYN` images are relocated; every address from the file needs this
    /// added before it names anything in the child.
    load_bias: u64,
}

impl Inferior {
    /// Fork, disable ASLR in the child, `PTRACE_TRACEME`, and exec. Returns once
    /// the child has stopped at its post-`execve` `SIGTRAP`.
    pub fn launch(program: &Path, args: &[String]) -> Result<Self> {
        let path = CString::new(program.as_os_str().as_encoded_bytes())
            .map_err(|_| LdbError::IoError("program path contains a NUL".into()))?;
        let mut argv_owned = vec![path.clone()];
        for a in args {
            argv_owned.push(
                CString::new(a.as_str())
                    .map_err(|_| LdbError::IoError("argument contains a NUL".into()))?,
            );
        }
        let mut argv: Vec<*const libc::c_char> = argv_owned.iter().map(|c| c.as_ptr()).collect();
        argv.push(std::ptr::null());

        // SAFETY: fork is async-signal-safe; the child path below calls only
        // async-signal-safe functions before execv.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(LdbError::PtraceError("fork failed".into()));
        }
        if pid == 0 {
            // SAFETY: child of fork, about to exec; nothing here allocates.
            unsafe {
                libc::personality(libc::ADDR_NO_RANDOMIZE as libc::c_ulong);
                if libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0) < 0 {
                    libc::_exit(127);
                }
                libc::execv(path.as_ptr(), argv.as_ptr());
                libc::_exit(127);
            }
        }

        let mut inferior = Inferior {
            pid,
            breakpoints: HashMap::new(),
            running: true,
            pending_signal: 0,
            load_bias: 0,
        };
        match inferior.wait()? {
            Status::Stopped(_) => {}
            Status::Exited(code) => {
                return Err(LdbError::PtraceError(format!(
                    "the program exited with {} before ldb could attach",
                    code
                )));
            }
            Status::Signalled(sig) => {
                return Err(LdbError::PtraceError(format!(
                    "the program died from signal {} before ldb could attach",
                    sig
                )));
            }
        }
        Ok(inferior)
    }

    pub fn pid(&self) -> libc::pid_t {
        self.pid
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn load_bias(&self) -> u64 {
        self.load_bias
    }

    pub fn set_load_bias(&mut self, bias: u64) {
        self.load_bias = bias;
    }

    /// The load bias of the main executable, read from `/proc/<pid>/maps`.
    pub fn detect_load_bias(&self, program: &Path) -> Result<u64> {
        let canonical = std::fs::canonicalize(program)?;
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", self.pid))?;
        let wanted = canonical.to_string_lossy();
        maps.lines()
            .filter(|l| l.ends_with(wanted.as_ref()))
            .filter_map(|l| l.split('-').next())
            .filter_map(|a| u64::from_str_radix(a, 16).ok())
            .min()
            .ok_or_else(|| {
                LdbError::PtraceError(format!(
                    "{} is not mapped in /proc/{}/maps",
                    wanted, self.pid
                ))
            })
    }

    pub fn wait(&mut self) -> Result<Status> {
        let mut status: libc::c_int = 0;
        // SAFETY: status is a live local; waitpid writes only into it.
        let r = unsafe { libc::waitpid(self.pid, &mut status, 0) };
        if r < 0 {
            return Err(LdbError::PtraceError("waitpid failed".into()));
        }
        if libc::WIFEXITED(status) {
            self.running = false;
            return Ok(Status::Exited(libc::WEXITSTATUS(status)));
        }
        if libc::WIFSIGNALED(status) {
            self.running = false;
            return Ok(Status::Signalled(libc::WTERMSIG(status)));
        }
        let sig = libc::WSTOPSIG(status);
        self.pending_signal = if sig == libc::SIGTRAP { 0 } else { sig };
        Ok(Status::Stopped(sig))
    }

    /// The signal that will be delivered on the next resume, if any.
    pub fn pending_signal(&self) -> i32 {
        self.pending_signal
    }

    fn ptrace(&self, req: libc::c_uint, addr: u64, data: u64) -> Result<libc::c_long> {
        // SAFETY: the request codes below take only integer arguments.
        unsafe {
            *libc::__errno_location() = 0;
            let r = libc::ptrace(
                req,
                self.pid,
                addr as *mut libc::c_void,
                data as *mut libc::c_void,
            );
            let errno = *libc::__errno_location();
            if r == -1 && errno != 0 {
                return Err(LdbError::PtraceError(format!(
                    "request {} on pid {} failed: {}",
                    req,
                    self.pid,
                    std::io::Error::from_raw_os_error(errno)
                )));
            }
            Ok(r)
        }
    }

    pub fn peek_word(&self, addr: u64) -> Result<u64> {
        Ok(self.ptrace(libc::PTRACE_PEEKDATA, addr, 0)? as u64)
    }

    pub fn poke_word(&self, addr: u64, word: u64) -> Result<()> {
        self.ptrace(libc::PTRACE_POKEDATA, addr, word)?;
        Ok(())
    }

    pub fn read_bytes(&self, addr: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut a = addr;
        while out.len() < len {
            let word = self.peek_word(a)?.to_le_bytes();
            let take = (len - out.len()).min(8);
            out.extend_from_slice(&word[..take]);
            a += 8;
        }
        Ok(out)
    }

    pub fn regs(&self) -> Result<libc::user_regs_struct> {
        let mut regs: libc::user_regs_struct = unsafe { std::mem::zeroed() };
        // SAFETY: PTRACE_GETREGS writes sizeof(user_regs_struct) bytes into data.
        unsafe {
            *libc::__errno_location() = 0;
            let r = libc::ptrace(
                libc::PTRACE_GETREGS,
                self.pid,
                std::ptr::null_mut::<libc::c_void>(),
                std::ptr::addr_of_mut!(regs) as *mut libc::c_void,
            );
            let errno = *libc::__errno_location();
            if r == -1 && errno != 0 {
                return Err(LdbError::PtraceError(format!(
                    "PTRACE_GETREGS failed: {}",
                    std::io::Error::from_raw_os_error(errno)
                )));
            }
        }
        Ok(regs)
    }

    /// The low 16 bytes of `xmm0`, where the SysV ABI returns a float or double.
    pub fn xmm0(&self) -> Result<[u8; 16]> {
        let mut fp: libc::user_fpregs_struct = unsafe { std::mem::zeroed() };
        // SAFETY: PTRACE_GETFPREGS writes sizeof(user_fpregs_struct) into data.
        unsafe {
            *libc::__errno_location() = 0;
            let r = libc::ptrace(
                libc::PTRACE_GETFPREGS,
                self.pid,
                std::ptr::null_mut::<libc::c_void>(),
                std::ptr::addr_of_mut!(fp) as *mut libc::c_void,
            );
            let errno = *libc::__errno_location();
            if r == -1 && errno != 0 {
                return Err(LdbError::PtraceError(format!(
                    "PTRACE_GETFPREGS failed: {}",
                    std::io::Error::from_raw_os_error(errno)
                )));
            }
        }
        let mut out = [0u8; 16];
        for (i, word) in fp.xmm_space[..4].iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(out)
    }

    pub fn set_regs(&self, regs: &libc::user_regs_struct) -> Result<()> {
        // SAFETY: PTRACE_SETREGS reads sizeof(user_regs_struct) bytes from data.
        unsafe {
            *libc::__errno_location() = 0;
            let r = libc::ptrace(
                libc::PTRACE_SETREGS,
                self.pid,
                std::ptr::null_mut::<libc::c_void>(),
                std::ptr::addr_of!(*regs) as *mut libc::c_void,
            );
            let errno = *libc::__errno_location();
            if r == -1 && errno != 0 {
                return Err(LdbError::PtraceError(format!(
                    "PTRACE_SETREGS failed: {}",
                    std::io::Error::from_raw_os_error(errno)
                )));
            }
        }
        Ok(())
    }

    pub fn has_breakpoint(&self, addr: u64) -> bool {
        self.breakpoints.get(&addr).is_some_and(|b| b.enabled)
    }

    pub fn breakpoint_addresses(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.breakpoints.keys().copied().collect();
        v.sort_unstable();
        v
    }

    pub fn insert_breakpoint(&mut self, addr: u64) -> Result<()> {
        if self.has_breakpoint(addr) {
            return Ok(());
        }
        let original = self.write_trap(addr)?;
        self.breakpoints.insert(
            addr,
            Breakpoint {
                original,
                enabled: true,
            },
        );
        Ok(())
    }

    pub fn remove_breakpoint(&mut self, addr: u64) -> Result<bool> {
        let Some(bp) = self.breakpoints.remove(&addr) else {
            return Ok(false);
        };
        if bp.enabled {
            self.write_bytes_at(addr, &bp.original)?;
        }
        Ok(true)
    }

    fn disable(&mut self, addr: u64) -> Result<()> {
        let Some(bp) = self.breakpoints.get_mut(&addr) else {
            return Ok(());
        };
        if !bp.enabled {
            return Ok(());
        }
        let original = bp.original.clone();
        bp.enabled = false;
        self.write_bytes_at(addr, &original)
    }

    fn enable(&mut self, addr: u64) -> Result<()> {
        let Some(bp) = self.breakpoints.get_mut(&addr) else {
            return Ok(());
        };
        if bp.enabled {
            return Ok(());
        }
        bp.enabled = true;
        self.write_trap(addr).map(|_| ())
    }

    /// Write `arch::TRAP` over the instruction at `addr`, returning the bytes
    /// it replaced. The trap is written through a whole `PTRACE_POKEDATA` word,
    /// so the surrounding bytes of that word are read and put back unchanged.
    fn write_trap(&self, addr: u64) -> Result<Vec<u8>> {
        let word = self.peek_word(addr)?;
        let mut bytes = word.to_le_bytes();
        let n = arch::TRAP.len();
        let original = bytes[..n].to_vec();
        bytes[..n].copy_from_slice(arch::TRAP);
        self.poke_word(addr, u64::from_le_bytes(bytes))?;
        Ok(original)
    }

    fn write_bytes_at(&self, addr: u64, replacement: &[u8]) -> Result<()> {
        let word = self.peek_word(addr)?;
        let mut bytes = word.to_le_bytes();
        bytes[..replacement.len()].copy_from_slice(replacement);
        self.poke_word(addr, u64::from_le_bytes(bytes))
    }

    /// After a trap the program counter sits `arch::TRAP_PC_ADVANCE` bytes past
    /// it. Rewind so it names the breakpoint address again.
    pub fn rewind_over_breakpoint(&mut self) -> Result<Option<u64>> {
        let mut regs = self.regs()?;
        let candidate = regs.rip.wrapping_sub(arch::TRAP_PC_ADVANCE);
        if !self.has_breakpoint(candidate) {
            return Ok(None);
        }
        regs.rip = candidate;
        self.set_regs(&regs)?;
        Ok(Some(candidate))
    }

    /// Single-step one instruction, stepping off a breakpoint byte first if the
    /// program counter is sitting on one.
    pub fn single_step(&mut self) -> Result<Status> {
        let rip = self.regs()?.rip;
        let replanted = if self.has_breakpoint(rip) {
            self.disable(rip)?;
            Some(rip)
        } else {
            None
        };
        let sig = self.take_pending_signal();
        self.ptrace(libc::PTRACE_SINGLESTEP, 0, sig)?;
        let status = self.wait()?;
        if let Some(addr) = replanted
            && self.running
        {
            self.enable(addr)?;
        }
        Ok(status)
    }

    pub fn resume(&mut self) -> Result<Status> {
        let rip = self.regs()?.rip;
        if self.has_breakpoint(rip) {
            match self.single_step()? {
                Status::Stopped(_) => {}
                other => return Ok(other),
            }
        }
        let sig = self.take_pending_signal();
        self.ptrace(libc::PTRACE_CONT, 0, sig)?;
        self.wait()
    }

    fn take_pending_signal(&mut self) -> u64 {
        let sig = self.pending_signal;
        self.pending_signal = 0;
        sig as u64
    }

    pub fn kill(&mut self) {
        if self.running {
            // SAFETY: the pid is one we forked and have not reaped.
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.pid, &mut status, 0);
            }
            self.running = false;
        }
    }
}

impl Drop for Inferior {
    fn drop(&mut self) {
        self.kill();
    }
}
