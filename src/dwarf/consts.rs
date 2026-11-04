//! DWARF constants, named as the standard names them.

pub const DW_TAG_ARRAY_TYPE: u64 = 0x01;
pub const DW_TAG_ENUMERATION_TYPE: u64 = 0x04;
pub const DW_TAG_FORMAL_PARAMETER: u64 = 0x05;
pub const DW_TAG_POINTER_TYPE: u64 = 0x0f;
pub const DW_TAG_COMPILE_UNIT: u64 = 0x11;
pub const DW_TAG_STRUCTURE_TYPE: u64 = 0x13;
pub const DW_TAG_TYPEDEF: u64 = 0x16;
pub const DW_TAG_UNION_TYPE: u64 = 0x17;
pub const DW_TAG_UNSPECIFIED_PARAMETERS: u64 = 0x18;
pub const DW_TAG_BASE_TYPE: u64 = 0x24;
pub const DW_TAG_CONST_TYPE: u64 = 0x26;
pub const DW_TAG_SUBPROGRAM: u64 = 0x2e;
pub const DW_TAG_VARIABLE: u64 = 0x34;
pub const DW_TAG_VOLATILE_TYPE: u64 = 0x35;

pub const DW_AT_LOCATION: u64 = 0x02;
pub const DW_AT_NAME: u64 = 0x03;
pub const DW_AT_BYTE_SIZE: u64 = 0x0b;
pub const DW_AT_STMT_LIST: u64 = 0x10;
pub const DW_AT_LOW_PC: u64 = 0x11;
pub const DW_AT_HIGH_PC: u64 = 0x12;
pub const DW_AT_LANGUAGE: u64 = 0x13;
pub const DW_AT_COMP_DIR: u64 = 0x1b;
pub const DW_AT_PRODUCER: u64 = 0x25;
pub const DW_AT_PROTOTYPED: u64 = 0x27;
pub const DW_AT_ABSTRACT_ORIGIN: u64 = 0x31;
pub const DW_AT_DECL_FILE: u64 = 0x3a;
pub const DW_AT_DECL_LINE: u64 = 0x3b;
pub const DW_AT_DECLARATION: u64 = 0x3c;
pub const DW_AT_ENCODING: u64 = 0x3e;
pub const DW_AT_EXTERNAL: u64 = 0x3f;
pub const DW_AT_FRAME_BASE: u64 = 0x40;
pub const DW_AT_SPECIFICATION: u64 = 0x47;
pub const DW_AT_TYPE: u64 = 0x49;

pub const DW_ATE_ADDRESS: u64 = 0x01;
pub const DW_ATE_BOOLEAN: u64 = 0x02;
pub const DW_ATE_FLOAT: u64 = 0x04;
pub const DW_ATE_SIGNED: u64 = 0x05;
pub const DW_ATE_SIGNED_CHAR: u64 = 0x06;
pub const DW_ATE_UNSIGNED: u64 = 0x07;
pub const DW_ATE_UNSIGNED_CHAR: u64 = 0x08;

pub const DW_FORM_ADDR: u64 = 0x01;
pub const DW_FORM_BLOCK2: u64 = 0x03;
pub const DW_FORM_BLOCK4: u64 = 0x04;
pub const DW_FORM_DATA2: u64 = 0x05;
pub const DW_FORM_DATA4: u64 = 0x06;
pub const DW_FORM_DATA8: u64 = 0x07;
pub const DW_FORM_STRING: u64 = 0x08;
pub const DW_FORM_BLOCK: u64 = 0x09;
pub const DW_FORM_BLOCK1: u64 = 0x0a;
pub const DW_FORM_DATA1: u64 = 0x0b;
pub const DW_FORM_FLAG: u64 = 0x0c;
pub const DW_FORM_SDATA: u64 = 0x0d;
pub const DW_FORM_STRP: u64 = 0x0e;
pub const DW_FORM_UDATA: u64 = 0x0f;
pub const DW_FORM_REF_ADDR: u64 = 0x10;
pub const DW_FORM_REF1: u64 = 0x11;
pub const DW_FORM_REF2: u64 = 0x12;
pub const DW_FORM_REF4: u64 = 0x13;
pub const DW_FORM_REF8: u64 = 0x14;
pub const DW_FORM_REF_UDATA: u64 = 0x15;
pub const DW_FORM_INDIRECT: u64 = 0x16;
pub const DW_FORM_SEC_OFFSET: u64 = 0x17;
pub const DW_FORM_EXPRLOC: u64 = 0x18;
pub const DW_FORM_FLAG_PRESENT: u64 = 0x19;
pub const DW_FORM_IMPLICIT_CONST: u64 = 0x21;

pub fn tag_name(tag: u64) -> &'static str {
    match tag {
        DW_TAG_ARRAY_TYPE => "DW_TAG_array_type",
        DW_TAG_ENUMERATION_TYPE => "DW_TAG_enumeration_type",
        DW_TAG_FORMAL_PARAMETER => "DW_TAG_formal_parameter",
        DW_TAG_POINTER_TYPE => "DW_TAG_pointer_type",
        DW_TAG_COMPILE_UNIT => "DW_TAG_compile_unit",
        DW_TAG_STRUCTURE_TYPE => "DW_TAG_structure_type",
        DW_TAG_TYPEDEF => "DW_TAG_typedef",
        DW_TAG_UNION_TYPE => "DW_TAG_union_type",
        DW_TAG_UNSPECIFIED_PARAMETERS => "DW_TAG_unspecified_parameters",
        DW_TAG_BASE_TYPE => "DW_TAG_base_type",
        DW_TAG_CONST_TYPE => "DW_TAG_const_type",
        DW_TAG_SUBPROGRAM => "DW_TAG_subprogram",
        DW_TAG_VARIABLE => "DW_TAG_variable",
        DW_TAG_VOLATILE_TYPE => "DW_TAG_volatile_type",
        _ => "DW_TAG_<unknown>",
    }
}

/// The DWARF 5 indexed forms (section 7.5.6). ldb does not read them yet: they
/// need `.debug_str_offsets`/`.debug_addr` and the CU's base attributes.
pub fn form_name(form: u64) -> &'static str {
    match form {
        DW_FORM_ADDR => "DW_FORM_addr",
        DW_FORM_BLOCK2 => "DW_FORM_block2",
        DW_FORM_BLOCK4 => "DW_FORM_block4",
        DW_FORM_DATA2 => "DW_FORM_data2",
        DW_FORM_DATA4 => "DW_FORM_data4",
        DW_FORM_DATA8 => "DW_FORM_data8",
        DW_FORM_STRING => "DW_FORM_string",
        DW_FORM_BLOCK => "DW_FORM_block",
        DW_FORM_BLOCK1 => "DW_FORM_block1",
        DW_FORM_DATA1 => "DW_FORM_data1",
        DW_FORM_FLAG => "DW_FORM_flag",
        DW_FORM_SDATA => "DW_FORM_sdata",
        DW_FORM_STRP => "DW_FORM_strp",
        DW_FORM_UDATA => "DW_FORM_udata",
        DW_FORM_REF_ADDR => "DW_FORM_ref_addr",
        DW_FORM_REF1 => "DW_FORM_ref1",
        DW_FORM_REF2 => "DW_FORM_ref2",
        DW_FORM_REF4 => "DW_FORM_ref4",
        DW_FORM_REF8 => "DW_FORM_ref8",
        DW_FORM_REF_UDATA => "DW_FORM_ref_udata",
        DW_FORM_INDIRECT => "DW_FORM_indirect",
        DW_FORM_SEC_OFFSET => "DW_FORM_sec_offset",
        DW_FORM_EXPRLOC => "DW_FORM_exprloc",
        DW_FORM_FLAG_PRESENT => "DW_FORM_flag_present",
        0x1a => "DW_FORM_strx",
        0x1b => "DW_FORM_addrx",
        0x1c => "DW_FORM_ref_sup4",
        0x1d => "DW_FORM_strp_sup",
        0x1e => "DW_FORM_data16",
        0x1f => "DW_FORM_line_strp",
        0x20 => "DW_FORM_ref_sig8",
        DW_FORM_IMPLICIT_CONST => "DW_FORM_implicit_const",
        0x22 => "DW_FORM_loclistx",
        0x23 => "DW_FORM_rnglistx",
        0x25 => "DW_FORM_strx1",
        0x26 => "DW_FORM_strx2",
        0x27 => "DW_FORM_strx3",
        0x28 => "DW_FORM_strx4",
        0x29 => "DW_FORM_addrx1",
        0x2a => "DW_FORM_addrx2",
        0x2b => "DW_FORM_addrx3",
        0x2c => "DW_FORM_addrx4",
        _ => "DW_FORM_<unknown>",
    }
}
