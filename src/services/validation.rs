use crate::{db::models::extract_track_from_filename, services::disc_service};

pub fn validate_non_negative_int(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("value is empty".into());
    }
    let v: i64 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid integer", s))?;
    if v < 0 {
        return Err(format!("'{}' must be non-negative", s));
    }
    Ok(v)
}

pub fn validate_signed_int(s: &str) -> Result<i32, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("value is empty".into());
    }
    s.parse::<i32>()
        .map_err(|_| format!("'{}' is not a valid integer", s))
}

pub fn validate_sector_ranges(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    for (line_num, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row = line_num + 1;
        if parse_range_line(line).is_none() {
            return Err(format!(
                "line {}: '{}' is not a valid range (expected <integer>-<integer>)",
                row, line
            ));
        }
    }
    Ok(())
}

fn parse_range_line(line: &str) -> Option<(i32, i32)> {
    let bytes = line.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'-' && bytes[i - 1].is_ascii_digit() {
            let start = line[..i].trim().parse::<i32>().ok()?;
            let end = line[i + 1..].trim().parse::<i32>().ok()?;
            return Some((start, end));
        }
    }
    None
}

pub fn parse_sector_range_pairs(text: &str) -> Vec<(i32, i32)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            parse_range_line(line)
        })
        .collect()
}

pub fn validate_sbi(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    for (line_num, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row = line_num + 1;
        validate_sbi_line(line).map_err(|e| format!("line {}: {}", row, e))?;
    }
    Ok(())
}

fn validate_sbi_line(line: &str) -> Result<(), String> {
    if !line.starts_with("MSF: ") {
        return Err("must start with 'MSF: '".into());
    }
    let rest = &line[5..];
    if rest.len() < 8 {
        return Err("MSF time too short".into());
    }
    let msf_bytes = rest.as_bytes();
    if msf_bytes[2] != b':' || msf_bytes[5] != b':' {
        return Err("MSF format must be DD:DD:DD".into());
    }
    for &i in &[0, 1, 3, 4, 6, 7] {
        if !msf_bytes[i].is_ascii_digit() {
            return Err("MSF values must be decimal digits".into());
        }
    }
    let rest = &rest[8..];
    if !rest.starts_with(" Q-Data: ") {
        return Err("expected ' Q-Data: ' after MSF".into());
    }
    let qdata = &rest[9..];
    // Expected layout: HHHHHH HH:HH:HH HH HH:HH:HH HHHH = 32 chars
    if qdata.len() != 32 {
        return Err(format!(
            "Q-Data must be exactly 32 characters, got {}",
            qdata.len()
        ));
    }
    let b = qdata.as_bytes();
    for &i in &[0, 1, 2, 3, 4, 5] {
        if !b[i].is_ascii_hexdigit() {
            return Err("invalid hex in Q-Data control/adr/tno/index".into());
        }
    }
    if b[6] != b' ' {
        return Err("expected space after track info".into());
    }
    for &i in &[7, 8, 10, 11, 13, 14] {
        if !b[i].is_ascii_hexdigit() {
            return Err("invalid hex in Q-Data relative MSF".into());
        }
    }
    if b[9] != b':' || b[12] != b':' {
        return Err("expected colons in Q-Data relative MSF".into());
    }
    if b[15] != b' ' {
        return Err("expected space".into());
    }
    if !b[16].is_ascii_hexdigit() || !b[17].is_ascii_hexdigit() {
        return Err("invalid hex in Q-Data zero byte".into());
    }
    if b[18] != b' ' {
        return Err("expected space".into());
    }
    for &i in &[19, 20, 22, 23, 25, 26] {
        if !b[i].is_ascii_hexdigit() {
            return Err("invalid hex in Q-Data absolute MSF".into());
        }
    }
    if b[21] != b':' || b[24] != b':' {
        return Err("expected colons in Q-Data absolute MSF".into());
    }
    if b[27] != b' ' {
        return Err("expected space before CRC".into());
    }
    for &i in &[28, 29, 30, 31] {
        if !b[i].is_ascii_hexdigit() {
            return Err("invalid hex in Q-Data CRC".into());
        }
    }
    Ok(())
}

pub fn validate_hex_dump(text: &str) -> Result<(), String> {
    disc_service::parse_binary_hex_input(text).map(|_| ())
}

pub fn validate_cuesheet(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut file_opened = false;
    let mut track_opened = false;
    let mut gap_opened = false;
    let mut index01_seen = false;
    let mut last_index: u32 = 0;
    let mut track_count: u32 = 0;

    for (line_num, line) in text.lines().enumerate() {
        let row = line_num + 1;
        let line = line.trim();
        let upper = line.to_uppercase();

        if upper.is_empty() {
            continue;
        }

        if is_cue_file_line(&upper) {
            if file_opened && !index01_seen {
                return Err(format!("row {}: previous FILE not closed", row));
            }
            file_opened = true;
            track_opened = false;
            gap_opened = false;
            index01_seen = false;
            last_index = 0;
            continue;
        }

        if let Some((num, _mode)) = parse_cue_track_line(&upper) {
            if track_count + 1 != num {
                return Err(format!(
                    "row {}: track number not sequential (expected {}, got {})",
                    row,
                    track_count + 1,
                    num
                ));
            }
            if !file_opened {
                return Err(format!("row {}: TRACK without FILE", row));
            }
            if track_opened && !index01_seen {
                return Err(format!("row {}: previous TRACK not closed", row));
            }
            track_opened = true;
            track_count += 1;
            gap_opened = false;
            index01_seen = false;
            last_index = 0;
            continue;
        }

        if upper.starts_with("CATALOG ") {
            if file_opened {
                return Err(format!("row {}: CATALOG must appear before any FILE", row));
            }
            let value = &upper[8..];
            if value.len() != 13 || !value.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("row {}: CATALOG must be exactly 13 digits", row));
            }
            continue;
        }

        if upper.starts_with("FLAGS ") {
            if !file_opened {
                return Err(format!("row {}: FLAGS without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: FLAGS without TRACK", row));
            }
            if gap_opened {
                return Err(format!("row {}: FLAGS after INDEX 00", row));
            }
            if index01_seen {
                return Err(format!("row {}: FLAGS after INDEX 01", row));
            }
            let flags_part = &upper[6..];
            for flag in flags_part.split_whitespace() {
                if flag != "PRE" && flag != "DCP" {
                    return Err(format!("row {}: invalid flag '{}'", row, flag));
                }
            }
            continue;
        }

        if upper.starts_with("ISRC ") {
            if !file_opened {
                return Err(format!("row {}: ISRC without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: ISRC without TRACK", row));
            }
            if gap_opened || index01_seen {
                return Err(format!("row {}: ISRC must appear before INDEX", row));
            }
            let value = &upper[5..];
            if value.len() != 12 || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(format!(
                    "row {}: ISRC must be exactly 12 alphanumeric characters",
                    row
                ));
            }
            continue;
        }

        if let Some((idx_num, mm, ss, ff)) = parse_cue_index_line(&upper) {
            if !file_opened {
                return Err(format!("row {}: INDEX without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: INDEX without TRACK", row));
            }

            if idx_num == 0 {
                if mm != 0 || ss != 0 || ff != 0 {
                    return Err(format!("row {}: INDEX 00 must be 00:00:00", row));
                }
                if gap_opened {
                    return Err(format!("row {}: gap already opened", row));
                }
                if index01_seen {
                    return Err(format!("row {}: INDEX 00 after INDEX 01", row));
                }
                gap_opened = true;
            } else if idx_num == 1 {
                if mm == 0 && ss == 0 && ff == 0 {
                    if gap_opened {
                        return Err(format!("row {}: gap not closed", row));
                    }
                } else {
                    if !gap_opened {
                        return Err(format!(
                            "row {}: INDEX 01 with non-zero pregap requires INDEX 00 first",
                            row
                        ));
                    }
                    if ss >= 60 {
                        return Err(format!("row {}: seconds value {} >= 60", row, ss));
                    }
                    if ff >= 75 {
                        return Err(format!("row {}: frames value {} >= 75", row, ff));
                    }
                    gap_opened = false;
                }
                index01_seen = true;
                last_index = 1;
            } else {
                if !index01_seen {
                    return Err(format!(
                        "row {}: INDEX {:02} without INDEX 01",
                        row, idx_num
                    ));
                }
                if idx_num != last_index + 1 {
                    return Err(format!(
                        "row {}: INDEX not sequential (expected {:02}, got {:02})",
                        row,
                        last_index + 1,
                        idx_num
                    ));
                }
                if ss >= 60 {
                    return Err(format!("row {}: seconds value {} >= 60", row, ss));
                }
                if ff >= 75 {
                    return Err(format!("row {}: frames value {} >= 75", row, ff));
                }
                last_index = idx_num;
            }
            continue;
        }

        if upper.starts_with("REM")
            || upper.starts_with("PERFORMER")
            || upper.starts_with("TITLE")
            || upper.starts_with("SONGWRITER")
        {
            continue;
        }

        return Err(format!("row {}: syntax error", row));
    }
    Ok(())
}

fn is_cue_file_line(line: &str) -> bool {
    if !line.starts_with("FILE \"") {
        return false;
    }
    let rest = &line[6..];
    if let Some(pos) = rest.find('"') {
        let after_quote = &rest[pos + 1..];
        after_quote == " BINARY" || after_quote == " WAVE"
    } else {
        false
    }
}

fn parse_cue_track_line(line: &str) -> Option<(u32, &str)> {
    if !line.starts_with("TRACK ") {
        return None;
    }
    let rest = &line[6..];
    if rest.len() < 3 {
        return None;
    }
    if !rest.as_bytes()[0].is_ascii_digit()
        || !rest.as_bytes()[1].is_ascii_digit()
        || rest.as_bytes()[2] != b' '
    {
        return None;
    }
    let num: u32 = rest[..2].parse().ok()?;
    let mode = &rest[3..];
    match mode {
        "MODE1/2352" | "MODE2/2352" | "AUDIO" => Some((num, mode)),
        _ => None,
    }
}

fn parse_cue_index_line(line: &str) -> Option<(u32, u32, u32, u32)> {
    if !line.starts_with("INDEX ") {
        return None;
    }
    let rest = &line[6..];
    if rest.len() < 3 {
        return None;
    }
    if !rest.as_bytes()[0].is_ascii_digit()
        || !rest.as_bytes()[1].is_ascii_digit()
        || rest.as_bytes()[2] != b' '
    {
        return None;
    }
    let idx_num: u32 = rest[..2].parse().ok()?;
    let time = &rest[3..];
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    for p in &parts {
        if p.len() != 2 || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let mm = parts[0].parse::<u32>().ok()?;
    let ss = parts[1].parse::<u32>().ok()?;
    let ff = parts[2].parse::<u32>().ok()?;
    Some((idx_num, mm, ss, ff))
}

pub fn validate_dat(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut names: Vec<(usize, String)> = Vec::new();

    for (line_num, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row = line_num + 1;
        if !line.starts_with("<rom ") || !line.ends_with("/>") {
            return Err(format!("row {}: expected <rom ... /> format", row));
        }
        let name = dat_extract_attr(line, "name")
            .ok_or_else(|| format!("row {}: missing 'name' attribute", row))?;
        if name.is_empty() {
            return Err(format!("row {}: 'name' cannot be empty", row));
        }
        let size_str = dat_extract_attr(line, "size")
            .ok_or_else(|| format!("row {}: missing 'size' attribute", row))?;
        size_str
            .parse::<u64>()
            .map_err(|_| format!("row {}: 'size' must be a non-negative integer", row))?;
        let crc = dat_extract_attr(line, "crc")
            .ok_or_else(|| format!("row {}: missing 'crc' attribute", row))?;
        if crc.len() != 8 || !crc.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("row {}: 'crc' must be 8 hex characters", row));
        }
        let md5 = dat_extract_attr(line, "md5")
            .ok_or_else(|| format!("row {}: missing 'md5' attribute", row))?;
        if md5.len() != 32 || !md5.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("row {}: 'md5' must be 32 hex characters", row));
        }
        let sha1 = dat_extract_attr(line, "sha1")
            .ok_or_else(|| format!("row {}: missing 'sha1' attribute", row))?;
        if sha1.len() != 40 || !sha1.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("row {}: 'sha1' must be 40 hex characters", row));
        }
        names.push((row, name));
    }

    if names.len() > 1 {
        for (row, name) in &names {
            if dat_extract_track_number(name).is_none() {
                return Err(format!(
                    "row {}: 'name' must contain a recognizable track number when multiple tracks are present",
                    row
                ));
            }
        }
    }
    Ok(())
}

fn dat_extract_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn dat_extract_track_number(filename: &str) -> Option<String> {
    extract_track_from_filename(filename)
}

pub fn validate_ring_code_offsets(json_str: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let entries: Vec<serde_json::Value> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return errors,
    };
    for (i, entry) in entries.iter().enumerate() {
        let entry_num = i + 1;
        check_optional_signed_int(
            entry,
            "offset_value",
            &format!("Ring Code #{}: Offset", entry_num),
            &mut errors,
        );
        check_optional_signed_int(
            entry,
            "offset_extra_value",
            &format!("Ring Code #{}: Extra Offset", entry_num),
            &mut errors,
        );
        check_optional_signed_int(
            entry,
            "sample_start",
            &format!("Ring Code #{}: Sample Start", entry_num),
            &mut errors,
        );
    }
    errors
}

fn check_optional_signed_int(
    obj: &serde_json::Value,
    key: &str,
    label: &str,
    errors: &mut Vec<String>,
) {
    if let Some(s) = obj[key].as_str() {
        let s = s.trim();
        if !s.is_empty() {
            if validate_signed_int(s).is_err() {
                errors.push(format!("{}: must be a valid integer", label));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_negative_int() {
        assert!(validate_non_negative_int("0").is_ok());
        assert!(validate_non_negative_int("42").is_ok());
        assert!(validate_non_negative_int("999999").is_ok());
        assert!(validate_non_negative_int("-1").is_err());
        assert!(validate_non_negative_int("abc").is_err());
        assert!(validate_non_negative_int("").is_err());
        assert!(validate_non_negative_int("  5  ").is_ok());
    }

    #[test]
    fn test_signed_int() {
        assert!(validate_signed_int("0").is_ok());
        assert!(validate_signed_int("-100").is_ok());
        assert!(validate_signed_int("100").is_ok());
        assert!(validate_signed_int("abc").is_err());
        assert!(validate_signed_int("").is_err());
    }

    #[test]
    fn test_sector_ranges_valid() {
        assert!(validate_sector_ranges("").is_ok());
        assert!(validate_sector_ranges("0-100").is_ok());
        assert!(validate_sector_ranges("0-100\n200-300").is_ok());
        assert!(validate_sector_ranges("-5-10").is_ok());
        assert!(validate_sector_ranges("-10--5").is_ok());
        assert!(validate_sector_ranges("10--5").is_ok());
    }

    #[test]
    fn test_sector_ranges_invalid() {
        assert!(validate_sector_ranges("abc").is_err());
        assert!(validate_sector_ranges("1-2-3").is_err());
        assert!(validate_sector_ranges("hello").is_err());
    }

    #[test]
    fn test_sbi_valid() {
        assert!(validate_sbi("").is_ok());
        assert!(validate_sbi("MSF: 02:03:04 Q-Data: 410102 03:04:05 00 06:07:08 ABCD").is_ok());
        let multi = "MSF: 02:03:04 Q-Data: 410102 03:04:05 00 06:07:08 ABCD\n\
                      MSF: 10:20:30 Q-Data: FF0A0B 0C:0D:0E 00 0F:10:11 1234";
        assert!(validate_sbi(multi).is_ok());
    }

    #[test]
    fn test_sbi_invalid() {
        assert!(validate_sbi("not sbi data").is_err());
        assert!(validate_sbi("MSF: 0:0:0 Q-Data: 000000 00:00:00 00 00:00:00 0000").is_err());
        assert!(validate_sbi("MSF: 02:03:04 Q-Data: ZZZZZZ 00:00:00 00 00:00:00 0000").is_err());
        assert!(validate_sbi("MSF: 02:03:04 Q-Data: 41010203:04:05 00 06:07:08 ABCD").is_err());
    }

    #[test]
    fn test_hex_dump_valid() {
        assert!(validate_hex_dump("").is_ok());
        assert!(validate_hex_dump("0000 : 01 02 03 04   ....").is_ok());
        assert!(validate_hex_dump("01 02 03 04\n05 06").is_ok());
        assert!(validate_hex_dump("01020304\n0506").is_ok());
        let multi = "0320 : 01 02 03 04 05 06 07 08  09 0A 0B 0C 0D 0E 0F 10   ................\n\
                      0330 : 11 12 13 14 15 16 17 18  19 1A 1B 1C 1D 1E 1F 20   ............... ";
        assert!(validate_hex_dump(multi).is_ok());
    }

    #[test]
    fn test_hex_dump_invalid() {
        assert!(validate_hex_dump("no colon here").is_err());
        assert!(validate_hex_dump("GGGG : ZZ ZZ").is_err());
        assert!(validate_hex_dump("ABC").is_err());
        assert!(validate_hex_dump("01 02 XX").is_err());
        assert!(validate_hex_dump("0000 : 01 02\n03 04").is_err());
    }

    #[test]
    fn test_cuesheet_valid() {
        assert!(validate_cuesheet("").is_ok());

        let simple = r#"FILE "Track.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(simple).is_ok());

        let multi = r#"FILE "Track 1.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
FILE "Track 2.bin" BINARY
  TRACK 02 MODE1/2352
    INDEX 00 00:00:00
    INDEX 01 00:02:00"#;
        assert!(validate_cuesheet(multi).is_ok());

        let with_flags = r#"FILE "Track 1.bin" BINARY
  TRACK 01 AUDIO
    FLAGS DCP
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(with_flags).is_ok());
    }

    #[test]
    fn test_cuesheet_catalog() {
        let with_catalog = r#"CATALOG 0000000000000
FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(with_catalog).is_ok());

        let catalog_after_file = r#"FILE "Track 01.bin" BINARY
CATALOG 0000000000000
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(catalog_after_file).is_err());

        let catalog_bad_len = r#"CATALOG 123456789
FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(catalog_bad_len).is_err());

        let catalog_non_digit = r#"CATALOG 000000000000A
FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(catalog_non_digit).is_err());
    }

    #[test]
    fn test_cuesheet_isrc() {
        let with_isrc = r#"FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    ISRC ZWUFD3135734
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(with_isrc).is_ok());

        let isrc_without_track = r#"FILE "Track 01.bin" BINARY
  ISRC ZWUFD3135734
  TRACK 01 AUDIO
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(isrc_without_track).is_err());

        let isrc_after_index = r#"FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    INDEX 00 00:00:00
    ISRC ZWUFD3135734
    INDEX 01 00:02:00"#;
        assert!(validate_cuesheet(isrc_after_index).is_err());

        let isrc_bad_len = r#"FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    ISRC ZWUFD31357
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(isrc_bad_len).is_err());

        let isrc_non_alnum = r#"FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    ISRC ZWUFD31357-4
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(isrc_non_alnum).is_err());
    }

    #[test]
    fn test_cuesheet_songwriter() {
        let with_songwriter = r#"FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    TITLE "Song Title"
    PERFORMER "Artist"
    SONGWRITER "Writer"
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(with_songwriter).is_ok());
    }

    #[test]
    fn test_cuesheet_index_subindexes() {
        let with_index02 = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 02 05:27:51"#;
        assert!(validate_cuesheet(with_index02).is_ok());

        let with_many_indexes = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 02 05:27:51
    INDEX 03 10:30:00
    INDEX 04 15:00:00"#;
        assert!(validate_cuesheet(with_many_indexes).is_ok());

        let indexes_then_next_file = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 02 05:27:51
FILE "Track 02.bin" BINARY
  TRACK 02 AUDIO
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(indexes_then_next_file).is_ok());

        let index_skip = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 03 05:27:51"#;
        assert!(validate_cuesheet(index_skip).is_err());

        let index02_without_01 = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 02 05:27:51"#;
        assert!(validate_cuesheet(index02_without_01).is_err());

        let index02_bad_frames = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 02 05:27:75"#;
        assert!(validate_cuesheet(index02_bad_frames).is_err());

        let index02_bad_secs = r#"FILE "Track 01.bin" BINARY
  TRACK 01 MODE1/2352
    INDEX 01 00:00:00
    INDEX 02 05:60:00"#;
        assert!(validate_cuesheet(index02_bad_secs).is_err());
    }

    #[test]
    fn test_cuesheet_full_redump() {
        let full = r#"CATALOG 5099750122020
REM SINGLE-DENSITY AREA
FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
FILE "Track 02.bin" BINARY
  TRACK 02 AUDIO
    ISRC GAJPN9100001
    FLAGS DCP
    INDEX 00 00:00:00
    INDEX 01 00:02:00
REM HIGH-DENSITY AREA
FILE "Track 03.bin" BINARY
  TRACK 03 MODE2/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(full).is_ok());

        let gdrom_session = r#"REM SESSION 01
FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
REM LEAD-OUT 01:30:00
REM SESSION 02
REM LEAD-IN 01:00:00
REM PREGAP 00:02:00
FILE "Track 02.bin" BINARY
  TRACK 02 MODE2/2352
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(gdrom_session).is_ok());

        let cd_audio = r#"TITLE "ACR Soundtrack"
PERFORMER "Artist"
FILE "Track 01.bin" BINARY
  TRACK 01 AUDIO
    TITLE "Song 1"
    PERFORMER "Artist"
    SONGWRITER "Writer"
    INDEX 01 00:00:00
FILE "Track 02.bin" BINARY
  TRACK 02 AUDIO
    TITLE "Song 2"
    PERFORMER "Artist"
    SONGWRITER "Writer"
    INDEX 00 00:00:00
    INDEX 01 00:01:74"#;
        assert!(validate_cuesheet(cd_audio).is_ok());
    }

    #[test]
    fn test_cuesheet_invalid() {
        assert!(validate_cuesheet("TRACK 01 AUDIO\n    INDEX 01 00:00:00").is_err());

        let bad_seq = r#"FILE "T1.bin" BINARY
  TRACK 02 AUDIO
    INDEX 01 00:00:00"#;
        assert!(validate_cuesheet(bad_seq).is_err());

        let bad_frames = r#"FILE "T.bin" BINARY
  TRACK 01 AUDIO
    INDEX 00 00:00:00
    INDEX 01 00:00:75"#;
        assert!(validate_cuesheet(bad_frames).is_err());

        let bad_secs = r#"FILE "T.bin" BINARY
  TRACK 01 AUDIO
    INDEX 00 00:00:00
    INDEX 01 00:60:00"#;
        assert!(validate_cuesheet(bad_secs).is_err());
    }

    #[test]
    fn test_dat_valid() {
        assert!(validate_dat("").is_ok());
        let single = r#"<rom name="Track.bin" size="1185760800" crc="deadbeef" md5="dddddddddddddddddddddddddddddddd" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />"#;
        assert!(validate_dat(single).is_ok());

        let multi = r#"<rom name="Track 01.bin" size="100" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000" />
<rom name="Track 02.bin" size="200" crc="11111111" md5="11111111111111111111111111111111" sha1="1111111111111111111111111111111111111111" />"#;
        assert!(validate_dat(multi).is_ok());
    }

    #[test]
    fn test_dat_invalid() {
        let bad_crc = r#"<rom name="T.bin" size="100" crc="ZZZZ" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000" />"#;
        assert!(validate_dat(bad_crc).is_err());

        let missing_name = r#"<rom size="100" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000" />"#;
        assert!(validate_dat(missing_name).is_err());

        let multi_no_track = r#"<rom name="foo.bin" size="100" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000" />
<rom name="bar.bin" size="200" crc="11111111" md5="11111111111111111111111111111111" sha1="1111111111111111111111111111111111111111" />"#;
        assert!(validate_dat(multi_no_track).is_err());
    }

    #[test]
    fn test_ring_code_offsets() {
        let good =
            r#"[{"offset_value":"+123","offset_extra_value":"1","sample_start":"-5","layers":[]}]"#;
        assert!(validate_ring_code_offsets(good).is_empty());

        let bad =
            r#"[{"offset_value":"abc","offset_extra_value":"","sample_start":"","layers":[]}]"#;
        assert!(!validate_ring_code_offsets(bad).is_empty());
    }

    #[test]
    fn test_parse_sector_range_pairs() {
        assert_eq!(
            parse_sector_range_pairs("0-100\n200-300"),
            vec![(0, 100), (200, 300)]
        );
        assert_eq!(parse_sector_range_pairs("-5-10"), vec![(-5, 10)]);
        assert_eq!(parse_sector_range_pairs(""), Vec::<(i32, i32)>::new());
    }

    #[test]
    fn test_sector_ranges_1_2_3_is_invalid() {
        let result = validate_sector_ranges("1-2-3");
        assert!(
            result.is_err(),
            "1-2-3 should fail: only one separator allowed"
        );
    }
}
