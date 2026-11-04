//! Hostile input: truncated, corrupted and self-contradicting debug sections.
//! Every case here must come back as an error, never a panic and never a hang.

mod common;

use common::{FACTORIAL, TempDir, build, cc};
use ldb::dwarf::{cfi, info, line};
use ldb::object::Elf;

/// xorshift64*, so a failing seed can be reproduced from the number alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn units(info_sec: &[u8], abbrev: &[u8]) -> ldb::Result<Vec<info::Unit>> {
    info::parse_units(&info::Sections {
        info: info_sec,
        abbrev,
        str_: &[],
        line_str: &[],
    })
}

fn program(line_sec: &[u8]) -> ldb::Result<line::LineProgram> {
    line::parse_program(
        &line::LineSections {
            line: line_sec,
            line_str: &[],
            str_: &[],
        },
        0,
    )
}

// ---- synthetic shapes ----------------------------------------------------

#[test]
fn an_empty_section_is_not_a_unit() {
    assert!(units(&[], &[]).unwrap().is_empty());
    assert!(cfi::parse(&[], 0).unwrap().fdes().is_empty());
    let err = program(&[]).unwrap_err();
    assert!(format!("{err}").contains("past the"), "{err}");
}

#[test]
fn a_compilation_unit_of_an_unknown_version_is_refused_by_number() {
    let mut info_sec = Vec::new();
    info_sec.extend_from_slice(&7u32.to_le_bytes());
    info_sec.extend_from_slice(&99u16.to_le_bytes());
    info_sec.extend_from_slice(&0u32.to_le_bytes());
    info_sec.push(8);
    let err = units(&info_sec, &[]).unwrap_err();
    assert!(format!("{err}").contains("DWARF version 99"), "{err}");
}

#[test]
fn a_unit_longer_than_the_section_is_refused() {
    let mut info_sec = Vec::new();
    info_sec.extend_from_slice(&u32::MAX.wrapping_sub(0x20).to_le_bytes());
    info_sec.extend_from_slice(&4u16.to_le_bytes());
    let err = units(&info_sec, &[]).unwrap_err();
    assert!(format!("{err}").contains("section has"), "{err}");
}

#[test]
fn a_unit_with_no_die_at_all_is_refused_rather_than_indexed_into() {
    let mut body = Vec::new();
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.push(8);
    body.extend_from_slice(&[0, 0, 0, 0]); // nothing but padding where DIEs go
    let mut info_sec = (body.len() as u32).to_le_bytes().to_vec();
    info_sec.extend_from_slice(&body);
    let err = units(&info_sec, &[]).unwrap_err();
    assert!(format!("{err}").contains("no DIE"), "{err}");
}

#[test]
fn a_line_program_whose_line_range_is_zero_is_refused() {
    let mut header = vec![1, 1, 1, 0xfb, 0, 13];
    header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.push(0);
    header.push(0);
    let mut body = 4u16.to_le_bytes().to_vec();
    body.extend_from_slice(&(header.len() as u32).to_le_bytes());
    body.extend_from_slice(&header);
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    let err = program(&out).unwrap_err();
    assert!(format!("{err}").contains("line_range is 0"), "{err}");
}

#[test]
fn a_line_file_table_claiming_more_entries_than_the_section_holds_is_refused() {
    let mut header: Vec<u8> = vec![1, 1, 1, 0xfb, 14, 13];
    header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.extend_from_slice(&[0x00, 0x00]); // empty v5 directory table
    header.push(0x01); // one format description
    header.extend_from_slice(&[0x01, 0x08]); // DW_LNCT_path, DW_FORM_string
    header.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f]); // uleb: a billion files
    let mut body = 5u16.to_le_bytes().to_vec();
    body.push(8);
    body.push(0);
    body.extend_from_slice(&(header.len() as u32).to_le_bytes());
    body.extend_from_slice(&header);
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    let err = program(&out).unwrap_err();
    assert!(format!("{err}").contains("more than the"), "{err}");
}

#[test]
fn an_eh_frame_entry_running_past_the_section_is_refused() {
    let mut sec = u32::MAX.wrapping_sub(0x10).to_le_bytes().to_vec();
    sec.extend_from_slice(&[0u8; 16]);
    let err = cfi::parse(&sec, 0x1000).unwrap_err();
    assert!(format!("{err}").contains("runs past"), "{err}");
}

#[test]
fn an_fde_whose_fields_overrun_its_own_entry_is_refused() {
    // A "zR"/absptr CIE, then an FDE whose declared length cannot hold the
    // 8-byte initial_location the encoding demands.
    let cie_body: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, // CIE_id
        0x01, // version
        b'z', b'R', 0x00, // augmentation
        0x01, // code alignment
        0x78, // data alignment
        0x10, // return address register
        0x01, // augmentation data length
        0x00, // DW_EH_PE_absptr
    ];
    let mut sec = (cie_body.len() as u32).to_le_bytes().to_vec();
    sec.extend_from_slice(&cie_body);
    let cie_ptr = (sec.len() + 4) as u32;
    let fde_body = cie_ptr.to_le_bytes().to_vec();
    sec.extend_from_slice(&(fde_body.len() as u32).to_le_bytes());
    sec.extend_from_slice(&fde_body);
    sec.extend_from_slice(&[0u8; 32]); // room for the overrun to land in
    let err = cfi::parse(&sec, 0x1000).unwrap_err();
    assert!(format!("{err}").contains("run past its own"), "{err}");
}

#[test]
fn an_fde_at_the_top_of_the_address_space_does_not_overflow_a_lookup() {
    let cie_body: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x01, b'z', b'R', 0x00, 0x01, 0x78, 0x10, 0x01, 0x00,
    ];
    let mut sec = (cie_body.len() as u32).to_le_bytes().to_vec();
    sec.extend_from_slice(&cie_body);
    let cie_ptr = (sec.len() + 4) as u32;
    let mut fde_body = cie_ptr.to_le_bytes().to_vec();
    fde_body.extend_from_slice(&u64::MAX.to_le_bytes()); // initial_location
    fde_body.extend_from_slice(&u64::MAX.to_le_bytes()); // address_range
    fde_body.push(0x00);
    sec.extend_from_slice(&(fde_body.len() as u32).to_le_bytes());
    sec.extend_from_slice(&fde_body);
    let eh = cfi::parse(&sec, 0x1000).unwrap();
    assert!(eh.fde_for_pc(0).is_none());
    assert!(eh.fde_for_pc(u64::MAX).is_some());
}

#[test]
fn a_section_header_table_pointing_outside_the_file_is_refused() {
    let mut bytes = vec![0u8; 128];
    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    bytes[40..48].copy_from_slice(&u64::MAX.to_le_bytes()); // e_shoff
    bytes[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    bytes[60..62].copy_from_slice(&4u16.to_le_bytes()); // e_shnum
    let err = Elf::parse(bytes).err().expect("should be refused");
    assert!(format!("{err}").contains("past EOF"), "{err}");
}

#[test]
fn a_section_name_index_outside_the_table_is_refused() {
    let mut bytes = vec![0u8; 64 + 64 * 2];
    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[40..48].copy_from_slice(&64u64.to_le_bytes());
    bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
    bytes[60..62].copy_from_slice(&2u16.to_le_bytes());
    bytes[62..64].copy_from_slice(&9u16.to_le_bytes()); // e_shstrndx, out of range
    let err = Elf::parse(bytes).err().expect("should be refused");
    assert!(format!("{err}").contains("e_shstrndx"), "{err}");
}

// ---- random truncation and corruption of a real binary -------------------

struct Sections {
    info: Vec<u8>,
    abbrev: Vec<u8>,
    line: Vec<u8>,
    eh_frame: Vec<u8>,
    eh_frame_addr: u64,
}

/// The same C program gcc gives richer DWARF for than lamina does: several
/// compilation units, `.debug_str`, more forms. Worth fuzzing beside our own.
const C_SUBJECT: &str = "#include <stdio.h>\n\
     struct pair { long a; double b; };\n\
     static long rec(long n) { return n == 0 ? 0 : n + rec(n - 1); }\n\
     double mix(struct pair p, const char *s) { return p.b + (double)p.a + (s ? 1 : 0); }\n\
     int main(void) { struct pair p = {rec(5), 0.5}; printf(\"%f\\n\", mix(p, \"x\")); return 0; }\n";

fn sections_of(exe: &std::path::Path) -> Sections {
    let elf = Elf::load(exe).unwrap();
    Sections {
        info: elf.section_data(".debug_info").to_vec(),
        abbrev: elf.section_data(".debug_abbrev").to_vec(),
        line: elf.section_data(".debug_line").to_vec(),
        eh_frame: elf.section_data(".eh_frame").to_vec(),
        eh_frame_addr: elf.section(".eh_frame").map_or(0, |s| s.addr),
    }
}

fn gcc_sections(dir: &TempDir, name: &str, flags: &[&str]) -> Option<Sections> {
    Some(sections_of(&cc(dir, name, C_SUBJECT, flags)?))
}

fn real_sections(dir: &TempDir) -> Option<Sections> {
    let exe = build(dir, "fuzzed", FACTORIAL)?;
    let elf = Elf::load(&exe).unwrap();
    Some(Sections {
        info: elf.section_data(".debug_info").to_vec(),
        abbrev: elf.section_data(".debug_abbrev").to_vec(),
        line: elf.section_data(".debug_line").to_vec(),
        eh_frame: elf.section_data(".eh_frame").to_vec(),
        eh_frame_addr: elf.section(".eh_frame").map_or(0, |s| s.addr),
    })
}

#[test]
fn every_truncation_of_a_real_binarys_sections_errors_instead_of_panicking() {
    let dir = TempDir::new("fuzz-truncate");
    let Some(s) = real_sections(&dir) else {
        eprintln!("skipping: no lamina release build next to this checkout");
        return;
    };
    let (i, a, l, e, addr) = (s.info, s.abbrev, s.line, s.eh_frame, s.eh_frame_addr);
    for n in 0..i.len() {
        let _ = units(&i[..n], &a);
        let _ = units(&i, &a[..n.min(a.len())]);
    }
    for n in 0..l.len() {
        let _ = program(&l[..n]);
    }
    for n in 0..e.len() {
        let _ = cfi::parse(&e[..n], addr);
    }
}

#[test]
fn random_single_byte_corruptions_error_instead_of_panicking() {
    let dir = TempDir::new("fuzz-corrupt");
    let Some(s) = real_sections(&dir) else {
        eprintln!("skipping: no lamina release build next to this checkout");
        return;
    };
    let (i, a, l, e, addr) = (s.info, s.abbrev, s.line, s.eh_frame, s.eh_frame_addr);
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for _ in 0..4000 {
        let mut i2 = i.clone();
        let mut a2 = a.clone();
        let mut l2 = l.clone();
        let mut e2 = e.clone();
        for buf in [&mut i2, &mut a2, &mut l2, &mut e2] {
            if buf.is_empty() {
                continue;
            }
            for _ in 0..3 {
                let at = rng.below(buf.len());
                buf[at] = (rng.next() & 0xff) as u8;
            }
        }
        let _ = units(&i2, &a2);
        let _ = program(&l2);
        let _ = cfi::parse(&e2, addr);
    }
}

#[test]
fn random_truncations_of_the_whole_file_error_instead_of_panicking() {
    let dir = TempDir::new("fuzz-file");
    let Some(exe) = build(&dir, "whole", FACTORIAL) else {
        eprintln!("skipping: no lamina release build next to this checkout");
        return;
    };
    let bytes = std::fs::read(&exe).unwrap();
    let mut rng = Rng(0x5eed_1234_abcd_0002);
    for _ in 0..600 {
        let n = rng.below(bytes.len());
        let _ = Elf::parse(bytes[..n].to_vec())
            .map(|e| ldb::symbols::DebugInfo::from_elf(e, exe.clone()));
    }
    for _ in 0..600 {
        let mut b = bytes.clone();
        for _ in 0..6 {
            let at = rng.below(b.len());
            b[at] = (rng.next() & 0xff) as u8;
        }
        let _ = Elf::parse(b).map(|e| ldb::symbols::DebugInfo::from_elf(e, exe.clone()));
    }
}

/// gcc emits forms, a file table and a `.eh_frame` lamina never produces, so
/// its sections reach reader paths our own output does not.
#[test]
fn corrupting_gcc_debug_sections_errors_instead_of_panicking() {
    let dir = TempDir::new("fuzz-gcc");
    let mut rng = Rng(0x5eed_1234_abcd_0003);
    for (n, flags) in [
        ("g2", &["-g", "-O0"][..]),
        ("g3", &["-g3", "-O2"][..]),
        ("dw5", &["-gdwarf-5", "-O0"][..]),
        ("dw2", &["-gdwarf-2", "-O0"][..]),
        ("dw64", &["-g", "-gdwarf64", "-O0"][..]),
    ] {
        let Some(s) = gcc_sections(&dir, n, flags) else {
            eprintln!("skipping: no working cc");
            return;
        };
        for len in [0, 1, 7, 16, 64, 255, s.info.len() / 3, s.info.len() / 2] {
            let _ = units(&s.info[..len.min(s.info.len())], &s.abbrev);
            let _ = program(&s.line[..len.min(s.line.len())]);
            let _ = cfi::parse(&s.eh_frame[..len.min(s.eh_frame.len())], s.eh_frame_addr);
        }
        for _ in 0..3000 {
            let mut i2 = s.info.clone();
            let mut a2 = s.abbrev.clone();
            let mut l2 = s.line.clone();
            let mut e2 = s.eh_frame.clone();
            for buf in [&mut i2, &mut a2, &mut l2, &mut e2] {
                if buf.is_empty() {
                    continue;
                }
                for _ in 0..4 {
                    let at = rng.below(buf.len());
                    buf[at] = (rng.next() & 0xff) as u8;
                }
            }
            let _ = units(&i2, &a2);
            let _ = program(&l2);
            let _ = cfi::parse(&e2, s.eh_frame_addr);
        }
    }
}
