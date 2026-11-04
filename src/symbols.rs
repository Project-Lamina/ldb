//! The resolved view of a binary's debug info that the commands query.

use crate::dwarf::consts::*;
use crate::dwarf::{cfi, info, line};
use crate::error::{LdbError, Result};
use crate::object::{DebugSection, Elf};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Signed,
    Unsigned,
    Float,
    Bool,
    Pointer,
    Other,
}

#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub byte_size: u64,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Option<String>,
    pub ty: Option<TypeInfo>,
    /// `false` when the DIE carries no `DW_AT_location`. Lamina emits none at
    /// all today, so the honest answer is that the location is unavailable.
    pub has_location: bool,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub low_pc: u64,
    pub high_pc: u64,
    pub decl_file: Option<String>,
    pub decl_line: Option<u64>,
    pub return_type: Option<TypeInfo>,
    pub params: Vec<Param>,
    pub external: bool,
    /// `DW_AT_prototyped`. Without it an empty parameter list and an absent
    /// `DW_AT_type` mean the producer recorded no signature, not that the
    /// function takes nothing and returns nothing: gcc at `-g1` emits exactly
    /// that shape for a function with two arguments and a return value.
    pub prototyped: bool,
    pub unit: usize,
}

impl Function {
    pub fn contains(&self, pc: u64) -> bool {
        pc >= self.low_pc && pc < self.high_pc
    }

    /// `true` when the DIE says nothing at all about the signature, so neither
    /// the return type nor the parameter list may be reported as `void`.
    pub fn signature_unrecorded(&self) -> bool {
        !self.prototyped && self.return_type.is_none() && self.params.is_empty()
    }

    pub fn signature(&self) -> String {
        if self.signature_unrecorded() {
            return format!(
                "{} (no DW_AT_prototyped, no DW_AT_type, no parameter DIE)",
                self.name
            );
        }
        let ret = self
            .return_type
            .as_ref()
            .map_or("void".to_string(), |t| t.name.clone());
        let args = if self.params.is_empty() {
            "void".to_string()
        } else {
            self.params
                .iter()
                .map(|p| p.ty.as_ref().map_or("?".to_string(), |t| t.name.clone()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!("{} {}({})", ret, self.name, args)
    }
}

pub struct DebugInfo {
    pub elf: Elf,
    pub path: std::path::PathBuf,
    pub units: Vec<info::Unit>,
    pub lines: Vec<line::LineProgram>,
    pub eh_frame: Option<cfi::EhFrame>,
    pub functions: Vec<Function>,
    pub comp_dir: Option<String>,
    /// One line per section ldb could not read. A section that will not parse
    /// costs the commands that need it, not the whole session: `.eh_frame` and
    /// `.symtab` still answer `bt` and `break *addr` without `.debug_info`.
    pub warnings: Vec<String>,
}

impl DebugInfo {
    pub fn load(path: &Path) -> Result<Self> {
        let elf = Elf::load(path)?;
        Self::from_elf(elf, path.to_path_buf())
    }

    pub fn from_elf(elf: Elf, path: std::path::PathBuf) -> Result<Self> {
        let info_sec = elf.debug_section_data(DebugSection::Info).to_vec();
        let abbrev_sec = elf.debug_section_data(DebugSection::Abbrev).to_vec();
        let str_sec = elf.debug_section_data(DebugSection::Str).to_vec();
        let line_str_sec = elf.debug_section_data(DebugSection::LineStr).to_vec();
        let line_sec = elf.debug_section_data(DebugSection::Line).to_vec();

        let mut warnings = Vec::new();
        let units = if info_sec.is_empty() {
            Vec::new()
        } else {
            match info::parse_units(&info::Sections {
                info: &info_sec,
                abbrev: &abbrev_sec,
                str_: &str_sec,
                line_str: &line_str_sec,
            }) {
                Ok(units) => units,
                Err(e) => {
                    warnings.push(format!(".debug_info was not read: {}", e));
                    Vec::new()
                }
            }
        };

        let line_sections = line::LineSections {
            line: &line_sec,
            line_str: &line_str_sec,
            str_: &str_sec,
        };
        let mut lines = Vec::with_capacity(units.len());
        for unit in &units {
            let stmt_list = unit
                .root_die()
                .attr(DW_AT_STMT_LIST)
                .and_then(info::AttrValue::as_u64);
            match stmt_list.map(|off| line::parse_program(&line_sections, off as usize)) {
                Some(Ok(p)) => lines.push(p),
                Some(Err(e)) => {
                    warnings.push(format!(".debug_line was not read: {}", e));
                    lines.push(line::LineProgram::default());
                }
                None => lines.push(line::LineProgram::default()),
            }
        }

        let eh_frame = match elf.debug_section(DebugSection::EhFrame) {
            Some(s) if s.size > 0 => {
                match cfi::parse(elf.debug_section_data(DebugSection::EhFrame), s.addr) {
                    Ok(eh) => Some(eh),
                    Err(e) => {
                        warnings.push(format!(".eh_frame was not read: {}", e));
                        None
                    }
                }
            }
            _ => None,
        };

        let comp_dir = units.first().and_then(|u| {
            u.root_die()
                .attr(DW_AT_COMP_DIR)
                .and_then(info::AttrValue::as_str)
                .map(str::to_string)
        });

        let mut db = DebugInfo {
            elf,
            path,
            units,
            lines,
            eh_frame,
            functions: Vec::new(),
            comp_dir,
            warnings,
        };
        db.functions = db.collect_functions();
        db.functions.sort_by_key(|f| f.low_pc);
        Ok(db)
    }

    fn collect_functions(&self) -> Vec<Function> {
        let mut out = Vec::new();
        for (ui, unit) in self.units.iter().enumerate() {
            for off in &unit.order {
                let die = &unit.dies[off];
                if die.tag != DW_TAG_SUBPROGRAM {
                    continue;
                }
                if die.attr(DW_AT_DECLARATION).is_some() {
                    continue;
                }
                let Some(low_pc) = die.attr(DW_AT_LOW_PC).and_then(info::AttrValue::as_u64) else {
                    continue;
                };
                let high = die
                    .attr(DW_AT_HIGH_PC)
                    .and_then(info::AttrValue::as_u64)
                    .unwrap_or(0);
                // DWARF 4 section 2.17.2: a DW_AT_high_pc of class address is an
                // address, of class constant it is an offset from DW_AT_low_pc.
                let high_pc = if matches!(die.attr(DW_AT_HIGH_PC), Some(info::AttrValue::Addr(_))) {
                    high
                } else {
                    low_pc + high
                };
                let decl_file = die
                    .attr(DW_AT_DECL_FILE)
                    .and_then(info::AttrValue::as_u64)
                    .and_then(|i| self.lines[ui].file_name(i))
                    .map(str::to_string);
                let params = die
                    .children
                    .iter()
                    .filter_map(|c| unit.die(*c))
                    .filter(|c| {
                        c.tag == DW_TAG_FORMAL_PARAMETER || c.tag == DW_TAG_UNSPECIFIED_PARAMETERS
                    })
                    .map(|c| Param {
                        name: if c.tag == DW_TAG_UNSPECIFIED_PARAMETERS {
                            Some("...".to_string())
                        } else {
                            c.name().map(str::to_string)
                        },
                        ty: c
                            .attr(DW_AT_TYPE)
                            .and_then(info::AttrValue::as_ref_offset)
                            .and_then(|o| self.resolve_type(ui, o)),
                        has_location: c.attr(DW_AT_LOCATION).is_some(),
                    })
                    .collect();
                out.push(Function {
                    name: die.name().unwrap_or("<anonymous>").to_string(),
                    low_pc,
                    high_pc,
                    decl_file,
                    decl_line: die.attr(DW_AT_DECL_LINE).and_then(info::AttrValue::as_u64),
                    return_type: die
                        .attr(DW_AT_TYPE)
                        .and_then(info::AttrValue::as_ref_offset)
                        .and_then(|o| self.resolve_type(ui, o)),
                    params,
                    external: die
                        .attr(DW_AT_EXTERNAL)
                        .and_then(info::AttrValue::as_u64)
                        .unwrap_or(0)
                        != 0,
                    prototyped: die.attr(DW_AT_PROTOTYPED).is_some(),
                    unit: ui,
                });
            }
        }
        out
    }

    pub fn resolve_type(&self, unit_index: usize, offset: usize) -> Option<TypeInfo> {
        let unit = self.units.get(unit_index)?;
        let mut off = offset;
        for _ in 0..16 {
            let die = unit.die(off)?;
            match die.tag {
                DW_TAG_BASE_TYPE => {
                    let encoding = die
                        .attr(DW_AT_ENCODING)
                        .and_then(info::AttrValue::as_u64)
                        .unwrap_or(0);
                    let kind = match encoding {
                        DW_ATE_SIGNED | DW_ATE_SIGNED_CHAR => TypeKind::Signed,
                        DW_ATE_UNSIGNED | DW_ATE_UNSIGNED_CHAR => TypeKind::Unsigned,
                        DW_ATE_FLOAT => TypeKind::Float,
                        DW_ATE_BOOLEAN => TypeKind::Bool,
                        DW_ATE_ADDRESS => TypeKind::Pointer,
                        _ => TypeKind::Other,
                    };
                    return Some(TypeInfo {
                        name: die.name().unwrap_or("<unnamed>").to_string(),
                        kind,
                        byte_size: die
                            .attr(DW_AT_BYTE_SIZE)
                            .and_then(info::AttrValue::as_u64)
                            .unwrap_or(0),
                    });
                }
                DW_TAG_POINTER_TYPE => {
                    return Some(TypeInfo {
                        name: die.name().unwrap_or("ptr").to_string(),
                        kind: TypeKind::Pointer,
                        byte_size: die
                            .attr(DW_AT_BYTE_SIZE)
                            .and_then(info::AttrValue::as_u64)
                            .unwrap_or(8),
                    });
                }
                DW_TAG_TYPEDEF | DW_TAG_CONST_TYPE | DW_TAG_VOLATILE_TYPE => {
                    off = die
                        .attr(DW_AT_TYPE)
                        .and_then(info::AttrValue::as_ref_offset)?;
                }
                other => {
                    return Some(TypeInfo {
                        name: die.name().unwrap_or(tag_name(other)).to_string(),
                        kind: TypeKind::Other,
                        byte_size: die
                            .attr(DW_AT_BYTE_SIZE)
                            .and_then(info::AttrValue::as_u64)
                            .unwrap_or(0),
                    });
                }
            }
        }
        None
    }

    pub fn function_named(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.name == name)
    }

    pub fn function_at(&self, pc: u64) -> Option<&Function> {
        self.functions.iter().find(|f| f.contains(pc))
    }

    /// The source position of `pc`, as `(file, line)`.
    pub fn source_at(&self, pc: u64) -> Option<(String, u64)> {
        for (ui, lp) in self.lines.iter().enumerate() {
            if let Some(row) = lp.row_for_pc(pc) {
                let file = lp
                    .file_name(row.file)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("<file {}>", row.file));
                let _ = ui;
                return Some((file, row.line));
            }
        }
        None
    }

    /// The address a `break <function>` lands on: the first line-table row
    /// inside the body, which is past the prologue. Falls back to `low_pc`
    /// when the producer gave the function no line rows at all.
    pub fn breakpoint_address(&self, f: &Function) -> u64 {
        self.lines
            .get(f.unit)
            .and_then(|lp| lp.first_row_in(f.low_pc, f.high_pc))
            .map_or(f.low_pc, |row| row.address)
    }

    pub fn address_for_file_line(&self, file: &str, line: u64) -> Option<u64> {
        let mut best = None;
        for lp in &self.lines {
            for (i, name) in lp.files.iter().enumerate() {
                if !file_matches(name, file) {
                    continue;
                }
                let index = i as u64 + lp.file_base;
                if let Some(a) = lp.address_for_line(index, line) {
                    best = Some(best.map_or(a, |b: u64| b.min(a)));
                }
            }
        }
        best
    }

    /// Every source line in the table that has code, for `break file:line`
    /// diagnostics.
    pub fn lines_in_file(&self, file: &str) -> Vec<u64> {
        let mut out = Vec::new();
        for lp in &self.lines {
            for (i, name) in lp.files.iter().enumerate() {
                if !file_matches(name, file) {
                    continue;
                }
                let index = i as u64 + lp.file_base;
                out.extend(
                    lp.rows
                        .iter()
                        .filter(|r| !r.end_sequence && r.file == index)
                        .map(|r| r.line),
                );
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn cfi(&self) -> Result<&cfi::EhFrame> {
        self.eh_frame.as_ref().ok_or_else(|| {
            LdbError::MissingDebugInfo(
                ".eh_frame is absent, so ldb cannot unwind this binary".into(),
            )
        })
    }

    pub fn source_file_path(&self, file: &str) -> std::path::PathBuf {
        let p = Path::new(file);
        if p.is_absolute() {
            return p.to_path_buf();
        }
        if let Some(dir) = &self.comp_dir {
            let joined = Path::new(dir).join(p);
            if joined.exists() {
                return joined;
            }
        }
        if let Some(parent) = self.path.parent() {
            let joined = parent.join(p);
            if joined.exists() {
                return joined;
            }
        }
        p.to_path_buf()
    }
}

fn file_matches(table_name: &str, wanted: &str) -> bool {
    table_name == wanted
        || Path::new(table_name).file_name() == Path::new(wanted).file_name()
            && Path::new(wanted).file_name().is_some()
}
