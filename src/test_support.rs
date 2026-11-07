//! Helpers the test modules across this crate share: a scratch directory, the two
//! compilers a subject program can come from, and a batch run of ldb over one.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        let base = option_env!("CARGO_TARGET_TMPDIR")
            .unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"));
        std::fs::create_dir_all(base).unwrap();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = Path::new(base).join(format!("{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The lamina compiler, if the sibling checkout has a release build. Tests that
/// need a real lamina binary skip themselves when it is absent rather than fail,
/// because ldb does not own that tree.
pub fn lamina() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LAMINA") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("lamina/target/release/lamina");
    p.is_file().then_some(p)
}

/// The `ldb` executable beside this test binary. `cargo test` builds it as part of the
/// bin target; `cargo test --lib` does not, so say which command is missing rather than
/// failing later on an absent program.
pub fn ldb_binary() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    let ldb = p.join("ldb");
    assert!(
        ldb.is_file(),
        "{} is not built: run `cargo build --bin ldb`",
        ldb.display()
    );
    ldb
}

/// Compile `source` with `-g` and return the executable's path.
pub fn build(dir: &TempDir, name: &str, source: &str) -> Option<PathBuf> {
    let compiler = lamina()?;
    let src = dir.path().join(format!("{name}.lamina"));
    std::fs::write(&src, source).unwrap();
    let exe = dir.path().join(name);
    let out = Command::new(compiler)
        .arg(src.file_name().unwrap())
        .arg("-g")
        .arg("-o")
        .arg(name)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success() && exe.is_file(),
        "lamina failed to build {name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(exe)
}

/// Compile `source` with the system C compiler, or `None` when there is none.
pub fn cc(dir: &TempDir, name: &str, source: &str, flags: &[&str]) -> Option<PathBuf> {
    let src = dir.path().join(format!("{name}.c"));
    std::fs::write(&src, source).unwrap();
    let exe = dir.path().join(name);
    let out = Command::new("cc")
        .arg(src.file_name().unwrap())
        .args(flags)
        .arg("-o")
        .arg(name)
        .current_dir(dir.path())
        .output()
        .ok()?;
    (out.status.success() && exe.is_file()).then_some(exe)
}

/// Run ldb in batch mode over `commands` and return everything it printed.
pub fn ldb(exe: &Path, commands: &[&str]) -> String {
    let mut cmd = Command::new(ldb_binary());
    cmd.arg(exe).arg("-batch");
    for c in commands {
        cmd.arg("-ex").arg(c);
    }
    let out = cmd
        .current_dir(exe.parent().unwrap())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text
}

pub const FACTORIAL: &str = r#"fn @factorial(i64 %n) -> i64 {
  entry:
    %is_zero = eq.i64 %n, 0
    br %is_zero, base_case, recursive_case
  base_case:
    ret.i64 1
  recursive_case:
    %n_minus_1 = sub.i64 %n, 1
    %rec = call @factorial(%n_minus_1)
    %result = mul.i64 %n, %rec
    ret.i64 %result
}

fn @half(f64 %x) -> f64 {
  entry:
    %r = div.f64 %x, 2.0
    ret.f64 %r
}

fn @nothing(i32 %k) -> void {
  entry:
    ret.void
}

fn @main() -> i64 {
  entry:
    %a = call @factorial(4)
    %b = call @half(9.0)
    call @nothing(3)
    ret.i64 0
}
"#;

/// xorshift64*, so a failing seed can be reproduced from the number alone.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

pub fn units(info_sec: &[u8], abbrev: &[u8]) -> crate::Result<Vec<crate::dwarf::info::Unit>> {
    crate::dwarf::info::parse_units(&crate::dwarf::info::Sections {
        info: info_sec,
        abbrev,
        str_: &[],
        line_str: &[],
    })
}

pub fn program(line_sec: &[u8]) -> crate::Result<crate::dwarf::line::LineProgram> {
    crate::dwarf::line::parse_program(
        &crate::dwarf::line::LineSections {
            line: line_sec,
            line_str: &[],
            str_: &[],
        },
        0,
    )
}

pub struct Sections {
    pub info: Vec<u8>,
    pub abbrev: Vec<u8>,
    pub line: Vec<u8>,
    pub eh_frame: Vec<u8>,
    pub eh_frame_addr: u64,
}

/// The same C program gcc gives richer DWARF for than lamina does: several
/// compilation units, `.debug_str`, more forms. Worth fuzzing beside our own.
pub const C_SUBJECT: &str = "#include <stdio.h>\n\
     struct pair { long a; double b; };\n\
     static long rec(long n) { return n == 0 ? 0 : n + rec(n - 1); }\n\
     double mix(struct pair p, const char *s) { return p.b + (double)p.a + (s ? 1 : 0); }\n\
     int main(void) { struct pair p = {rec(5), 0.5}; printf(\"%f\\n\", mix(p, \"x\")); return 0; }\n";

pub fn sections_of(exe: &std::path::Path) -> Sections {
    let elf = crate::object::Elf::load(exe).unwrap();
    Sections {
        info: elf.section_data(".debug_info").to_vec(),
        abbrev: elf.section_data(".debug_abbrev").to_vec(),
        line: elf.section_data(".debug_line").to_vec(),
        eh_frame: elf.section_data(".eh_frame").to_vec(),
        eh_frame_addr: elf.section(".eh_frame").map_or(0, |s| s.addr),
    }
}

pub fn gcc_sections(dir: &TempDir, name: &str, flags: &[&str]) -> Option<Sections> {
    Some(sections_of(&cc(dir, name, C_SUBJECT, flags)?))
}

pub fn real_sections(dir: &TempDir) -> Option<Sections> {
    let exe = build(dir, "fuzzed", FACTORIAL)?;
    let elf = crate::object::Elf::load(&exe).unwrap();
    Some(Sections {
        info: elf.section_data(".debug_info").to_vec(),
        abbrev: elf.section_data(".debug_abbrev").to_vec(),
        line: elf.section_data(".debug_line").to_vec(),
        eh_frame: elf.section_data(".eh_frame").to_vec(),
        eh_frame_addr: elf.section(".eh_frame").map_or(0, |s| s.addr),
    })
}
