//! In-memory data model for a Stata dataset, plus the storage-type, format-version, and
//! missing-value tables. Ported from the Java `core` module (com.searchlight.dta / .model).

use std::collections::BTreeMap;

//  storage types (dta_121 section 5.3 type codes) 

#[derive(Debug, Clone, PartialEq)]
pub enum VarType {
    /// Fixed-width string, str1..str2045. `width` bytes stored per observation.
    Str(u16),
    /// Long string (strL): an 8-byte (v,o) reference into the <strls> section.
    StrL,
    /// Cross-frame alias variable (Stata 16+): zero bytes in <data>.
    Alias,
    Byte,
    Int,
    Long,
    Float,
    Double,
}

impl VarType {
    pub fn from_type_code(code: u16) -> Result<VarType, String> {
        if (1..=2045).contains(&code) {
            return Ok(VarType::Str(code));
        }
        Ok(match code {
            32768 => VarType::StrL,
            65525 => VarType::Alias,
            65526 => VarType::Double,
            65527 => VarType::Float,
            65528 => VarType::Long,
            65529 => VarType::Int,
            65530 => VarType::Byte,
            other => return Err(format!("Unknown Stata variable type code: {}", other)),
        })
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            VarType::Byte | VarType::Int | VarType::Long | VarType::Float | VarType::Double
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self, VarType::Str(_) | VarType::StrL)
    }

    /// Stata's short storage-type name as shown by `describe` (e.g. "int", "str18", "double").
    pub fn name(&self) -> String {
        match self {
            VarType::Str(w) => format!("str{}", w),
            VarType::StrL => "strL".to_string(),
            VarType::Alias => "alias".to_string(),
            VarType::Byte => "byte".to_string(),
            VarType::Int => "int".to_string(),
            VarType::Long => "long".to_string(),
            VarType::Float => "float".to_string(),
            VarType::Double => "double".to_string(),
        }
    }
}

//  format version field-width table (dta 117-121) 

/// Byte widths of the length-prefixed / fixed-width header fields, which differ across the
/// tag-based formats. Cross-checked against StataCorp's dta/dta_117/dta_119/dta_120/dta_121 help.
#[derive(Debug, Clone, Copy)]
pub struct DtaVersion {
    pub release: u16,
    pub k_width: usize,
    pub n_width: usize,
    pub dataset_label_len_width: usize,
    pub varname_width: usize,
    pub format_width: usize,
    pub value_label_name_width: usize,
    pub variable_label_width: usize,
    pub characteristic_name_width: usize,
    pub data_vo_v_width: usize,
    pub data_vo_o_width: usize,
    pub gso_v_width: usize,
    pub gso_o_width: usize,
    pub utf8: bool,
}

impl DtaVersion {
    pub fn from_release(release: u16) -> Result<DtaVersion, String> {
        // release, kW, nW, dsLabelLenW, varnameW, formatW, vlNameW, varLabelW, charNameW,
        // dataVoVW, dataVoOW, gsoVW, gsoOW, utf8
        let v = match release {
            117 => DtaVersion::new(117, 2, 4, 1, 33, 49, 33, 81, 33, 4, 4, 4, 4, false),
            118 => DtaVersion::new(118, 2, 8, 2, 129, 57, 129, 321, 129, 2, 6, 4, 8, true),
            119 => DtaVersion::new(119, 4, 8, 2, 129, 57, 129, 321, 129, 3, 5, 4, 8, true),
            120 => DtaVersion::new(120, 2, 8, 2, 129, 57, 129, 321, 129, 2, 6, 4, 8, true),
            121 => DtaVersion::new(121, 4, 8, 2, 129, 57, 129, 321, 129, 3, 5, 4, 8, true),
            other => return Err(format!("Unsupported .dta format release: {}", other)),
        };
        Ok(v)
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        release: u16,
        k_width: usize,
        n_width: usize,
        dataset_label_len_width: usize,
        varname_width: usize,
        format_width: usize,
        value_label_name_width: usize,
        variable_label_width: usize,
        characteristic_name_width: usize,
        data_vo_v_width: usize,
        data_vo_o_width: usize,
        gso_v_width: usize,
        gso_o_width: usize,
        utf8: bool,
    ) -> DtaVersion {
        DtaVersion {
            release,
            k_width,
            n_width,
            dataset_label_len_width,
            varname_width,
            format_width,
            value_label_name_width,
            variable_label_width,
            characteristic_name_width,
            data_vo_v_width,
            data_vo_o_width,
            gso_v_width,
            gso_o_width,
            utf8,
        }
    }
}

//  missing values (dta_121 section 4.6) 

/// Stata's 27 missing-value codes (., .a..z) per numeric type. Cell values are stored uniformly
/// as f64; for byte/int/long these are exact widenings, for float/double the sentinels are large
/// finite IEEE values just past the nonmissing range.
pub mod missing {
    use super::VarType;

    const BYTE_MAX_NONMISSING: f64 = 100.0;
    const INT_MAX_NONMISSING: f64 = 32740.0;
    const LONG_MAX_NONMISSING: f64 = 2147483620.0;

    const FLOAT_MISSING_BASE: u32 = 0x7f00_0000;
    const FLOAT_POS_INFINITY: u32 = 0x7f80_0000;
    const DOUBLE_MISSING_BASE: u64 = 0x7fe0_0000_0000_0000;
    const DOUBLE_POS_INFINITY: u64 = 0x7ff0_0000_0000_0000;
    const FLOAT_CODE_SHIFT: u32 = 11;
    const DOUBLE_CODE_SHIFT: u64 = 40;

    pub fn is_missing(t: &VarType, value: f64) -> bool {
        match t {
            VarType::Byte => value > BYTE_MAX_NONMISSING,
            VarType::Int => value > INT_MAX_NONMISSING,
            VarType::Long => value > LONG_MAX_NONMISSING,
            VarType::Float => {
                let raw = (value as f32).to_bits();
                raw >= FLOAT_MISSING_BASE && raw < FLOAT_POS_INFINITY
            }
            VarType::Double => {
                let raw = value.to_bits();
                raw >= DOUBLE_MISSING_BASE && raw < DOUBLE_POS_INFINITY
            }
            _ => false,
        }
    }

    /// 0 for '.', 1 for '.a', ..., 26 for '.z'. Requires `is_missing(t, value)`.
    pub fn missing_code(t: &VarType, value: f64) -> i32 {
        let clamp = |c: i32| c.clamp(0, 26);
        match t {
            VarType::Byte => (value - 101.0) as i32,
            VarType::Int => (value - 32741.0) as i32,
            VarType::Long => (value - 2147483621.0) as i32,
            VarType::Float => {
                clamp((((value as f32).to_bits() - FLOAT_MISSING_BASE) >> FLOAT_CODE_SHIFT) as i32)
            }
            VarType::Double => {
                clamp(((value.to_bits() - DOUBLE_MISSING_BASE) >> DOUBLE_CODE_SHIFT) as i32)
            }
            _ => 0,
        }
    }

    /// "." for code 0, ".a".."z" for 1..26.
    pub fn code_to_display(code: i32) -> String {
        if code == 0 {
            ".".to_string()
        } else {
            format!(".{}", (b'a' + (code - 1) as u8) as char)
        }
    }
}

//  value labels 

/// A named integer->text mapping (dta_121 5.12), kept sorted by value (ascending), matching the
/// on-disk val[] array.
#[derive(Debug, Clone, Default)]
pub struct ValueLabel {
    pub name: String,
    pub entries: BTreeMap<i32, String>,
}

impl ValueLabel {
    pub fn new(name: String) -> ValueLabel {
        ValueLabel {
            name,
            entries: BTreeMap::new(),
        }
    }

    pub fn get(&self, value: i32) -> Option<&String> {
        self.entries.get(&value)
    }
}

//  characteristics (notes are stored here) 

#[derive(Debug, Clone)]
pub struct Characteristic {
    pub varname: String,
    pub charname: String,
    pub contents: String,
}

/// The dta convention for a dataset-wide (as opposed to per-variable) characteristic.
pub const DATASET_SCOPE: &str = "_dta";

//  variable metadata 

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub vtype: VarType,
    pub format: String,
    pub value_label_name: String,
    pub label: String,
}

impl Variable {
    pub fn is_numeric(&self) -> bool {
        self.vtype.is_numeric()
    }
    pub fn is_string(&self) -> bool {
        self.vtype.is_string()
    }
    pub fn has_value_label(&self) -> bool {
        !self.value_label_name.is_empty()
    }
}

//  cell value 

/// One stored cell. Numeric cells (including dates and missing) are `Num(f64)`; string cells are
/// `Text`; alias cells carry no data.
#[derive(Debug, Clone)]
pub enum Value {
    Num(f64),
    Text(String),
    /// Binary strL blob (embedded NUL); carries its byte length for reporting.
    Binary(usize),
    Alias,
}

//  dataset 

pub struct Dataset {
    pub variables: Vec<Variable>,
    /// Column-major storage: `columns[col][row]`.
    pub columns: Vec<Vec<Value>>,
    pub nobs: usize,
    pub dataset_label: String,
    pub timestamp: String,
    pub sort_order: Vec<usize>, // variable indices, in sort priority order
    pub source_release: u16,
    pub value_labels: Vec<ValueLabel>,
    pub characteristics: Vec<Characteristic>,
    /// Absolute path the dataset was read from (for describe's "Contains data from" line).
    pub source_path: String,
}

impl Dataset {
    pub fn nvars(&self) -> usize {
        self.variables.len()
    }

    pub fn value_label(&self, name: &str) -> Option<&ValueLabel> {
        self.value_labels.iter().find(|v| v.name == name)
    }

    /// Index of a variable by exact name.
    pub fn var_index(&self, name: &str) -> Option<usize> {
        self.variables.iter().position(|v| v.name == name)
    }
}
