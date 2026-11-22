use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum SfoValue {
    Text(String),
    Int(u32),
}

/// Parses a PS4 param.sfo (PSF): header "\0PSF", key table, data table, 16-byte index entries.
pub fn parse(data: &[u8]) -> Result<HashMap<String, SfoValue>, String> {
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if data.len() < 20 || &data[0..4] != b"\0PSF" {
        return Err("not a param.sfo file".into());
    }
    let key_table = u32_at(8).unwrap() as usize;
    let data_table = u32_at(12).unwrap() as usize;
    let count = u32_at(16).unwrap() as usize;
    let mut out = HashMap::new();
    for i in 0..count {
        let e = 20 + 16 * i;
        let (Some(key_off), Some(fmt), Some(len), Some(data_off)) = (u16_at(e), u16_at(e + 2), u32_at(e + 4), u32_at(e + 12))
        else {
            return Err("truncated param.sfo".into());
        };
        let ks = key_table + key_off as usize;
        let key_bytes = data.get(ks..).ok_or("bad key offset")?;
        let end = key_bytes.iter().position(|&b| b == 0).ok_or("unterminated key")?;
        let key = String::from_utf8_lossy(&key_bytes[..end]).into_owned();
        let ds = data_table + data_off as usize;
        let raw = data.get(ds..ds + len as usize).ok_or("bad data offset")?;
        let value = match fmt {
            0x0404 if raw.len() >= 4 => SfoValue::Int(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
            _ => {
                let text_end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                SfoValue::Text(String::from_utf8_lossy(&raw[..text_end]).into_owned())
            }
        };
        out.insert(key, value);
    }
    Ok(out)
}

pub fn text<'a>(map: &'a HashMap<String, SfoValue>, key: &str) -> &'a str {
    match map.get(key) {
        Some(SfoValue::Text(s)) => s,
        _ => "",
    }
}

#[cfg(test)]
pub fn build(entries: &[(&str, SfoValue)]) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut index = Vec::new();
    for (k, v) in entries {
        let key_off = keys.len() as u16;
        keys.extend_from_slice(k.as_bytes());
        keys.push(0);
        let data_off = values.len() as u32;
        let (fmt, bytes) = match v {
            SfoValue::Int(n) => (0x0404u16, n.to_le_bytes().to_vec()),
            SfoValue::Text(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                (0x0204u16, b)
            }
        };
        let max = ((bytes.len() + 3) / 4 * 4) as u32;
        index.extend_from_slice(&key_off.to_le_bytes());
        index.extend_from_slice(&fmt.to_le_bytes());
        index.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        index.extend_from_slice(&max.to_le_bytes());
        index.extend_from_slice(&data_off.to_le_bytes());
        values.extend_from_slice(&bytes);
        values.resize(data_off as usize + max as usize, 0);
    }
    while keys.len() % 4 != 0 {
        keys.push(0);
    }
    let key_table = 20 + index.len() as u32;
    let data_table = key_table + keys.len() as u32;
    let mut out = b"\0PSF".to_vec();
    out.extend_from_slice(&0x101u32.to_le_bytes());
    out.extend_from_slice(&key_table.to_le_bytes());
    out.extend_from_slice(&data_table.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&index);
    out.extend_from_slice(&keys);
    out.extend_from_slice(&values);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data = build(&[
            ("APP_VER", SfoValue::Text("01.43".into())),
            ("ATTRIBUTE", SfoValue::Int(75513858)),
            ("CATEGORY", SfoValue::Text("gp".into())),
            ("TITLE_ID", SfoValue::Text("CUSA11182".into())),
        ]);
        let m = parse(&data).unwrap();
        assert_eq!(text(&m, "APP_VER"), "01.43");
        assert_eq!(text(&m, "CATEGORY"), "gp");
        assert_eq!(text(&m, "TITLE_ID"), "CUSA11182");
        assert_eq!(m["ATTRIBUTE"], SfoValue::Int(75513858));
        assert_eq!(text(&m, "MISSING"), "");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"not an sfo at all").is_err());
        let mut data = build(&[("TITLE_ID", SfoValue::Text("CUSA11182".into()))]);
        data.truncate(30);
        assert!(parse(&data).is_err());
    }
}
