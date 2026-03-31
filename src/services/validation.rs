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
        validate_sbi_line(line)
            .map_err(|e| format!("line {}: {}", row, e))?;
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
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut total_bytes = 0usize;
    for (line_num, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row = line_num + 1;
        let colon_pos = line
            .find(':')
            .ok_or_else(|| format!("line {}: missing offset:colon prefix", row))?;
        let offset_part = line[..colon_pos].trim();
        if offset_part.is_empty() || !offset_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("line {}: invalid hex offset '{}'", row, offset_part));
        }
        let after_colon = &line[colon_pos + 1..];
        let trimmed = after_colon.trim_start();
        let hex_part = match trimmed.find("   ") {
            Some(pos) => &trimmed[..pos],
            None => trimmed,
        };
        let mut line_bytes = 0usize;
        for token in hex_part.split_whitespace() {
            if token.len() != 2 {
                return Err(format!("line {}: invalid hex token '{}'", row, token));
            }
            u8::from_str_radix(token, 16)
                .map_err(|_| format!("line {}: invalid hex byte '{}'", row, token))?;
            line_bytes += 1;
        }
        if line_bytes == 0 {
            return Err(format!("line {}: no hex bytes found", row));
        }
        total_bytes += line_bytes;
    }
    if total_bytes == 0 {
        return Err("no hex data found".into());
    }
    Ok(())
}

pub fn validate_cuesheet(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut file_opened = false;
    let mut track_opened = false;
    let mut gap_opened = false;
    let mut track_count: u32 = 0;

    for (line_num, line) in text.lines().enumerate() {
        let row = line_num + 1;
        let line = line.trim();
        let upper = line.to_uppercase();

        if upper.is_empty() {
            continue;
        }

        if is_cue_file_line(&upper) {
            if file_opened {
                return Err(format!("row {}: previous FILE not closed", row));
            }
            file_opened = true;
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
            if track_opened {
                return Err(format!("row {}: previous TRACK not closed", row));
            }
            track_opened = true;
            track_count += 1;
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
            let flags_part = &upper[6..];
            for flag in flags_part.split_whitespace() {
                if flag != "PRE" && flag != "DCP" {
                    return Err(format!("row {}: invalid flag '{}'", row, flag));
                }
            }
            continue;
        }

        if upper == "INDEX 00 00:00:00" {
            if !file_opened {
                return Err(format!("row {}: INDEX without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: INDEX without TRACK", row));
            }
            if gap_opened {
                return Err(format!("row {}: gap already opened", row));
            }
            gap_opened = true;
            continue;
        }

        if upper == "INDEX 01 00:00:00" {
            if !file_opened {
                return Err(format!("row {}: INDEX without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: INDEX without TRACK", row));
            }
            if gap_opened {
                return Err(format!("row {}: gap not closed", row));
            }
            file_opened = false;
            track_opened = false;
            continue;
        }

        if let Some((_mm, ss, ff)) = parse_cue_index01(&upper) {
            if !file_opened {
                return Err(format!("row {}: INDEX without FILE", row));
            }
            if !track_opened {
                return Err(format!("row {}: INDEX without TRACK", row));
            }
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
            file_opened = false;
            track_opened = false;
            gap_opened = false;
            continue;
        }

        if upper.starts_with("REM")
            || upper.starts_with("PERFORMER")
            || upper.starts_with("TITLE")
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

fn parse_cue_index01(line: &str) -> Option<(u32, u32, u32)> {
    if !line.starts_with("INDEX 01 ") {
        return None;
    }
    let time = &line[9..];
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
    Some((mm, ss, ff))
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
    if filename.ends_with(".iso") {
        return Some("1".to_string());
    }
    let lower = filename.to_lowercase();
    if lower.starts_with("track.") {
        return Some("1".to_string());
    }
    if let Some(pos) = lower.find("track ") {
        let rest = &filename[pos + 6..];
        let num: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !num.is_empty() {
            return Some(num);
        }
    }
    None
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
        assert!(validate_sbi(
            "MSF: 02:03:04 Q-Data: 410102 03:04:05 00 06:07:08 ABCD"
        )
        .is_ok());
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
        let multi = "0320 : 01 02 03 04 05 06 07 08  09 0A 0B 0C 0D 0E 0F 10   ................\n\
                      0330 : 11 12 13 14 15 16 17 18  19 1A 1B 1C 1D 1E 1F 20   ............... ";
        assert!(validate_hex_dump(multi).is_ok());
    }

    #[test]
    fn test_hex_dump_invalid() {
        assert!(validate_hex_dump("no colon here").is_err());
        assert!(validate_hex_dump("GGGG : ZZ ZZ").is_err());
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
        let good = r#"[{"offset_value":"123","offset_extra_value":"","sample_start":"-5","layers":[]}]"#;
        assert!(validate_ring_code_offsets(good).is_empty());

        let bad = r#"[{"offset_value":"abc","offset_extra_value":"","sample_start":"","layers":[]}]"#;
        assert!(!validate_ring_code_offsets(bad).is_empty());
    }

    #[test]
    fn test_parse_sector_range_pairs() {
        assert_eq!(parse_sector_range_pairs("0-100\n200-300"), vec![(0, 100), (200, 300)]);
        assert_eq!(parse_sector_range_pairs("-5-10"), vec![(-5, 10)]);
        assert_eq!(parse_sector_range_pairs(""), Vec::<(i32, i32)>::new());
    }

    #[test]
    fn test_sector_ranges_1_2_3_is_invalid() {
        let result = validate_sector_ranges("1-2-3");
        assert!(result.is_err(), "1-2-3 should fail: only one separator allowed");
    }
}
