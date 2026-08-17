//! Byte-order-aware parser for the tag-based .dta formats (117-121). Ported from the Java
//! `DtaReader`/`ByteReader`.

use crate::model::{
    Characteristic, Dataset, DtaVersion, Value, ValueLabel, VarType, Variable,
};
use std::collections::HashMap;

struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
    big_endian: bool,
}

impl<'a> ByteReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        ByteReader {
            data,
            pos: 0,
            big_endian: false,
        }
    }

    fn need(&self, n: usize) -> Result<(), String> {
        if self.pos + n > self.data.len() {
            Err(format!(
                "Unexpected end of file: needed {} bytes at offset {} but only {} remain",
                n,
                self.pos,
                self.data.len().saturating_sub(self.pos)
            ))
        } else {
            Ok(())
        }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn i8(&mut self) -> Result<i8, String> {
        Ok(self.u8()? as i8)
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.bytes(2)?;
        Ok(if self.big_endian {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    }

    fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(if self.big_endian {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        })
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.bytes(8)?;
        let arr = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Ok(if self.big_endian {
            u64::from_be_bytes(arr)
        } else {
            u64::from_le_bytes(arr)
        })
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// Unsigned integer of arbitrary byte width (the 2/3/4/5/6-byte K/sortlist/vo fields).
    fn uint(&mut self, width: usize) -> Result<u64, String> {
        let b = self.bytes(width)?;
        let mut v: u64 = 0;
        if self.big_endian {
            for &byte in b {
                v = (v << 8) | byte as u64;
            }
        } else {
            for (i, &byte) in b.iter().enumerate() {
                v |= (byte as u64) << (8 * i);
            }
        }
        Ok(v)
    }

    /// Fixed-width, NUL-terminated field trimmed at the first NUL.
    fn fixed_string(&mut self, width: usize, utf8: bool) -> Result<String, String> {
        let raw = self.bytes(width)?;
        let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        decode(&raw[..len], utf8)
    }

    /// Exactly `len` bytes decoded as text (no NUL trimming).
    fn text(&mut self, len: usize, utf8: bool) -> Result<String, String> {
        let raw = self.bytes(len)?;
        decode(raw, utf8)
    }

    fn expect_tag(&mut self, tag: &str) -> Result<(), String> {
        let start = self.pos;
        let actual = self.text(tag.len(), false)?;
        if actual != tag {
            return Err(format!(
                "Expected tag {} at offset {} but found {:?}",
                tag, start, actual
            ));
        }
        Ok(())
    }

    /// True if the upcoming bytes match `tag` without consuming them.
    fn peek_tag(&self, tag: &str) -> bool {
        let end = self.pos + tag.len();
        end <= self.data.len() && &self.data[self.pos..end] == tag.as_bytes()
    }

    fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }
}

fn decode(raw: &[u8], utf8: bool) -> Result<String, String> {
    if utf8 {
        // Stata writes valid UTF-8 in 118+, but be lenient rather than erroring on stray bytes.
        Ok(String::from_utf8_lossy(raw).into_owned())
    } else {
        // 117 is documented ASCII; treat each byte as Latin-1 so no byte is ever lost.
        Ok(raw.iter().map(|&b| b as char).collect())
    }
}

fn trim_trailing_nul(s: &str) -> &str {
    s.strip_suffix('\0').unwrap_or(s)
}

/// Parse a whole .dta file image into a `Dataset`.
pub fn read_dta(data: &[u8], source_path: &str) -> Result<Dataset, String> {
    let mut r = ByteReader::new(data);
    r.expect_tag("<stata_dta>")?;
    r.expect_tag("<header>")?;

    r.expect_tag("<release>")?;
    let release: u16 = r
        .text(3, false)?
        .trim()
        .parse()
        .map_err(|_| "Invalid <release> number".to_string())?;
    r.expect_tag("</release>")?;
    let version = DtaVersion::from_release(release)?;

    r.expect_tag("<byteorder>")?;
    let byteorder = r.text(3, false)?;
    r.big_endian = byteorder == "MSF";
    r.expect_tag("</byteorder>")?;

    r.expect_tag("<K>")?;
    let k = r.uint(version.k_width)? as usize;
    r.expect_tag("</K>")?;

    r.expect_tag("<N>")?;
    let n = r.uint(version.n_width)? as usize;
    r.expect_tag("</N>")?;

    r.expect_tag("<label>")?;
    let label_len = r.uint(version.dataset_label_len_width)? as usize;
    let dataset_label = r.text(label_len, version.utf8)?;
    r.expect_tag("</label>")?;

    r.expect_tag("<timestamp>")?;
    let ts_len = r.u8()? as usize;
    let timestamp = if ts_len == 0 {
        String::new()
    } else {
        r.text(ts_len, false)?
    };
    r.expect_tag("</timestamp>")?;
    r.expect_tag("</header>")?;

    r.expect_tag("<map>")?;
    for _ in 0..14 {
        r.u64()?;
    }
    r.expect_tag("</map>")?;

    r.expect_tag("<variable_types>")?;
    let mut types: Vec<VarType> = Vec::with_capacity(k);
    for _ in 0..k {
        types.push(VarType::from_type_code(r.u16()?)?);
    }
    r.expect_tag("</variable_types>")?;

    r.expect_tag("<varnames>")?;
    let mut names: Vec<String> = Vec::with_capacity(k);
    for _ in 0..k {
        names.push(r.fixed_string(version.varname_width, version.utf8)?);
    }
    r.expect_tag("</varnames>")?;

    r.expect_tag("<sortlist>")?;
    let mut sort_order: Vec<usize> = Vec::new();
    let mut saw_terminator = false;
    for _ in 0..(k + 1) {
        let v = r.uint(version.k_width)?;
        if saw_terminator {
            continue; // post-terminator bytes are documented junk
        }
        if v == 0 {
            saw_terminator = true;
        } else {
            sort_order.push((v - 1) as usize); // 1-based on disk -> 0-based index
        }
    }
    r.expect_tag("</sortlist>")?;

    r.expect_tag("<formats>")?;
    let mut formats: Vec<String> = Vec::with_capacity(k);
    for _ in 0..k {
        formats.push(r.fixed_string(version.format_width, version.utf8)?);
    }
    r.expect_tag("</formats>")?;

    r.expect_tag("<value_label_names>")?;
    let mut vl_names: Vec<String> = Vec::with_capacity(k);
    for _ in 0..k {
        vl_names.push(r.fixed_string(version.value_label_name_width, version.utf8)?);
    }
    r.expect_tag("</value_label_names>")?;

    r.expect_tag("<variable_labels>")?;
    let mut var_labels: Vec<String> = Vec::with_capacity(k);
    for _ in 0..k {
        var_labels.push(r.fixed_string(version.variable_label_width, version.utf8)?);
    }
    r.expect_tag("</variable_labels>")?;

    r.expect_tag("<characteristics>")?;
    let mut characteristics: Vec<Characteristic> = Vec::new();
    while r.peek_tag("<ch>") {
        r.expect_tag("<ch>")?;
        let llll = r.u32()? as usize;
        let char_width = version.characteristic_name_width;
        let varname = r.fixed_string(char_width, version.utf8)?;
        let charname = r.fixed_string(char_width, version.utf8)?;
        let contents_len = llll - 2 * char_width;
        let contents = trim_trailing_nul(&r.text(contents_len, version.utf8)?).to_string();
        characteristics.push(Characteristic {
            varname,
            charname,
            contents,
        });
        r.expect_tag("</ch>")?;
    }
    r.expect_tag("</characteristics>")?;

    //  data 
    let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(n); k];
    // Deferred strL references: (col, row, v, o).
    let mut strl_refs: Vec<(usize, usize, u64, u64)> = Vec::new();

    r.expect_tag("<data>")?;
    for row in 0..n {
        for col in 0..k {
            match &types[col] {
                VarType::Str(w) => {
                    columns[col].push(Value::Text(r.fixed_string(*w as usize, version.utf8)?));
                }
                VarType::StrL => {
                    let v = r.uint(version.data_vo_v_width)?;
                    let o = r.uint(version.data_vo_o_width)?;
                    strl_refs.push((col, row, v, o));
                    columns[col].push(Value::Text(String::new())); // placeholder, resolved below
                }
                VarType::Alias => {
                    columns[col].push(Value::Alias); // zero bytes consumed
                }
                numeric => {
                    columns[col].push(Value::Num(read_numeric(&mut r, numeric)?));
                }
            }
        }
    }
    r.expect_tag("</data>")?;

    //  strls (GSO table) 
    r.expect_tag("<strls>")?;
    let mut gso: HashMap<u64, Value> = HashMap::new();
    while r.peek_tag("GSO") {
        r.expect_tag("GSO")?;
        let v = r.uint(version.gso_v_width)?;
        let o = r.uint(version.gso_o_width)?;
        let t = r.u8()?;
        let len = r.u32()? as usize;
        let raw = r.bytes(len)?.to_vec();
        let value = match t {
            129 => Value::Binary(raw.len()),
            130 => {
                let str_len = if raw.last() == Some(&0) {
                    raw.len() - 1
                } else {
                    raw.len()
                };
                Value::Text(decode(&raw[..str_len], version.utf8)?)
            }
            other => return Err(format!("Unknown GSO type byte {}", other)),
        };
        gso.insert(strl_key(v, o), value);
    }
    r.expect_tag("</strls>")?;

    for (col, row, v, o) in strl_refs {
        let value = if v == 0 && o == 0 {
            Value::Text(String::new())
        } else {
            gso.get(&strl_key(v, o)).cloned().ok_or_else(|| {
                format!(
                    "strL (v,o)=({},{}) referenced at [{},{}] has no GSO definition",
                    v, o, col, row
                )
            })?
        };
        columns[col][row] = value;
    }

    //  value labels 
    r.expect_tag("<value_labels>")?;
    let mut value_labels: Vec<ValueLabel> = Vec::new();
    while r.peek_tag("<lbl>") {
        r.expect_tag("<lbl>")?;
        let table_len = r.u32()? as usize;
        let label_name = r.fixed_string(version.value_label_name_width, version.utf8)?;
        r.bytes(3)?; // padding
        let table_end = r.pos + table_len;
        let entry_count = r.u32()? as usize;
        let txt_len = r.u32()? as usize;
        let mut off = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            off.push(r.u32()? as usize);
        }
        let mut val = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            val.push(r.i32()?);
        }
        let txt = r.bytes(txt_len)?.to_vec();
        let mut label = ValueLabel::new(label_name.clone());
        for i in 0..entry_count {
            let start = off[i];
            let mut end = start;
            while end < txt.len() && txt[end] != 0 {
                end += 1;
            }
            let text = decode(&txt[start..end], version.utf8)?;
            label.entries.insert(val[i], text);
        }
        value_labels.push(label);
        r.seek(table_end);
        r.expect_tag("</lbl>")?;
    }
    r.expect_tag("</value_labels>")?;
    r.expect_tag("</stata_dta>")?;

    let variables: Vec<Variable> = (0..k)
        .map(|i| Variable {
            name: std::mem::take(&mut names[i]),
            vtype: types[i].clone(),
            format: std::mem::take(&mut formats[i]),
            value_label_name: std::mem::take(&mut vl_names[i]),
            label: std::mem::take(&mut var_labels[i]),
        })
        .collect();

    Ok(Dataset {
        variables,
        columns,
        nobs: n,
        dataset_label,
        timestamp,
        sort_order,
        source_release: release,
        value_labels,
        characteristics,
        source_path: source_path.to_string(),
    })
}

fn read_numeric(r: &mut ByteReader, t: &VarType) -> Result<f64, String> {
    Ok(match t {
        VarType::Byte => r.i8()? as f64,
        VarType::Int => r.i16()? as f64,
        VarType::Long => r.i32()? as f64,
        VarType::Float => r.f32()? as f64,
        VarType::Double => r.f64()?,
        _ => unreachable!("read_numeric called with non-numeric type"),
    })
}

fn strl_key(v: u64, o: u64) -> u64 {
    (v << 32) | (o & 0xFFFF_FFFF)
}
