//! Error types for the ldb debugger.

#[derive(Debug)]
pub enum LdbError {
    IoError(String),
    ElfError(String),
    DwarfError(String),
    Unsupported(String),
    PtraceError(String),
    NoSuchSymbol(String),
    NotRunning,
    MissingDebugInfo(String),
}

impl std::fmt::Display for LdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LdbError::IoError(msg) => write!(f, "I/O error: {}", msg),
            LdbError::ElfError(msg) => write!(f, "ELF error: {}", msg),
            LdbError::DwarfError(msg) => write!(f, "DWARF error: {}", msg),
            LdbError::Unsupported(msg) => write!(f, "Unsupported: {}", msg),
            LdbError::PtraceError(msg) => write!(f, "ptrace error: {}", msg),
            LdbError::NoSuchSymbol(msg) => write!(f, "No symbol \"{}\" in debug info", msg),
            LdbError::NotRunning => write!(f, "The program is not being run"),
            LdbError::MissingDebugInfo(msg) => write!(f, "Not in the debug info: {}", msg),
        }
    }
}

impl std::error::Error for LdbError {}

impl From<std::io::Error> for LdbError {
    fn from(e: std::io::Error) -> Self {
        LdbError::IoError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LdbError>;
