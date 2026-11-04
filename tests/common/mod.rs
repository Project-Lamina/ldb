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

pub fn ldb_binary() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("ldb")
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
