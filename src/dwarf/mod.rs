//! A DWARF reader written for Lamina's own output.
//!
//! Deliberately not `gimli`: reading our DWARF with our own parser is how we
//! find out whether what Lamina emits is actually consumable.

pub mod abbrev;
pub mod cfi;
pub mod consts;
pub mod info;
pub mod line;
pub mod reader;

#[cfg(test)]
mod tests {
    use crate::test_support::*;

    #[test]
    fn an_empty_section_is_not_a_unit() {
        assert!(units(&[], &[]).unwrap().is_empty());
        assert!(crate::dwarf::cfi::parse(&[], 0).unwrap().fdes().is_empty());
        let err = program(&[]).unwrap_err();
        assert!(format!("{err}").contains("past the"), "{err}");
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
            let _ = crate::dwarf::cfi::parse(&e[..n], addr);
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
            let _ = crate::dwarf::cfi::parse(&e2, addr);
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
                let _ = crate::dwarf::cfi::parse(
                    &s.eh_frame[..len.min(s.eh_frame.len())],
                    s.eh_frame_addr,
                );
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
                let _ = crate::dwarf::cfi::parse(&e2, s.eh_frame_addr);
            }
        }
    }
}
