use std::collections::HashSet;

use serde::Serialize;

use crate::services::validation;

pub const MINIMUM_AUTOFILL_BUILD: u64 = 737;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ParsedRedumperLog {
    pub system_code: Option<String>,
    pub media_type: Option<String>,
    pub version: Option<String>,
    pub exe_date: Option<String>,
    pub edc: Option<bool>,
    pub error_count: Option<i64>,
    pub universal_hash: Option<String>,
    pub sample_start: Option<String>,
    pub offset_value: Option<String>,
    pub sector_ranges: Option<String>,
    pub sbi: Option<String>,
    pub pvd: Option<String>,
    pub header: Option<String>,
    pub cuesheet: Option<String>,
    pub dat: Option<String>,
    /// `None` means no protection information was found. `Some("")` means
    /// `protection: none` was explicitly reported and the field should clear.
    pub protection: Option<String>,
    pub layerbreaks: Option<Vec<String>>,
}

pub fn has_supported_autofill_builds(log: &str) -> bool {
    let mut found_header = false;

    for line in log.lines() {
        let Some(value) = line.trim().strip_prefix("redumper (build:") else {
            continue;
        };
        found_header = true;

        let Some(value) = value.strip_suffix(')').map(str::trim) else {
            return false;
        };
        let Some(number) = value.strip_prefix('b') else {
            return false;
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(number) = number.parse::<u64>() else {
            return false;
        };
        if number < MINIMUM_AUTOFILL_BUILD {
            return false;
        }
    }

    found_header
}

pub fn parse(log: &str, known_system_codes: &[String]) -> ParsedRedumperLog {
    parse_internal(log, known_system_codes, &[])
}

pub fn parse_with_system_media(
    log: &str,
    known_systems: &[(String, Vec<String>)],
) -> ParsedRedumperLog {
    let known_system_codes: Vec<String> =
        known_systems.iter().map(|(code, _)| code.clone()).collect();
    parse_internal(log, &known_system_codes, known_systems)
}

fn parse_internal(
    log: &str,
    known_system_codes: &[String],
    known_systems: &[(String, Vec<String>)],
) -> ParsedRedumperLog {
    let normalized = log.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let info = latest_section(&lines, "INFO");
    let split = latest_section(&lines, "SPLIT");
    let hash = latest_section(&lines, "HASH");
    let protection_section = latest_section(&lines, "PROTECTION");
    let dvdkey = latest_section(&lines, "DVDKEY");
    let dump = latest_section(&lines, "DUMP");

    let mut result = ParsedRedumperLog::default();

    if let Some(info) = info.as_deref() {
        result.version = scalar_value(info, "version:");
        result.exe_date =
            scalar_value(info, "EXE date:").or_else(|| scalar_value(info, "build date:"));
        result.edc = scalar_value(info, "mode2 (form 2) EDC:").and_then(|value| {
            if value.eq_ignore_ascii_case("yes") {
                Some(true)
            } else if value.eq_ignore_ascii_case("no") {
                Some(false)
            } else {
                None
            }
        });
        result.error_count = error_count(info);
        result.sector_ranges = indented_valid_lines_after(
            info,
            "security sector ranges:",
            validation::validate_sector_ranges,
        );
        result.sbi =
            nonempty_join(info.iter().map(|line| line.trim()).filter(|line| {
                line.starts_with("MSF: ") && validation::validate_sbi(line).is_ok()
            }));
        result.pvd = hex_dump_after(info, "PVD:");
        result.header = hex_dump_after(info, "header:");
        result.system_code = system_code(info, known_system_codes);
    }

    if let Some(code) = protection_section
        .as_deref()
        .and_then(|lines| protection_system_code(lines, known_system_codes))
    {
        result.system_code = Some(code);
    }

    if dvdkey.as_deref().is_some_and(dvdkey_has_css_cppm)
        && known_system_codes.iter().any(|code| code == "DVD-VIDEO")
    {
        result.system_code = Some("DVD-VIDEO".to_owned());
    }

    if let Some(split) = split.as_deref() {
        result.universal_hash = scalar_value(split, "Universal Hash (SHA-1):").filter(|value| {
            value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        result.sample_start = signed_value_after(split, "non-zero data sample range:");
        result.offset_value = signed_value_after(split, "disc write offset:");
        result.cuesheet = block_after_prefix(split, "CUE [");
        if result.system_code.is_none() {
            if result
                .cuesheet
                .as_deref()
                .is_some_and(validation::cuesheet_is_enhanced_cd)
                && known_system_codes.iter().any(|code| code == "ENHANCED-CD")
            {
                result.system_code = Some("ENHANCED-CD".to_owned());
            } else if result
                .cuesheet
                .as_deref()
                .is_some_and(validation::cuesheet_has_only_audio_tracks)
                && known_system_codes.iter().any(|code| code == "AUDIO-CD")
            {
                result.system_code = Some("AUDIO-CD".to_owned());
            }
        }
    }

    if let Some(hash) = hash.as_deref() {
        result.dat = consecutive_matching_after(hash, "dat:", |line| {
            let line = line.trim();
            line.starts_with("<rom ") && line.ends_with("/>")
        });
    }

    result.protection = protection(
        protection_section.as_deref(),
        dvdkey.as_deref(),
        info.as_deref(),
    );
    let bd_layer_count = dump.as_deref().and_then(bd_layer_count);
    result.layerbreaks = dump.as_deref().and_then(dump_layerbreaks);
    result.media_type = result.system_code.as_deref().and_then(|system_code| {
        let media_types = known_systems
            .iter()
            .find(|(code, _)| code == system_code)
            .map(|(_, media_types)| media_types.as_slice())?;
        if matches!(system_code, "XBOX" | "XBOX360")
            && media_types.iter().any(|m| m.eq_ignore_ascii_case("xgd"))
        {
            return Some("xgd".to_owned());
        }
        let profile = latest_profile(&lines)?;
        media_type_for_profile(profile, bd_layer_count, media_types)
    });
    if matches!(result.system_code.as_deref(), Some("GC" | "WII")) {
        result.version = result.version.map(|version| {
            if version == "0" {
                String::new()
            } else {
                format!("Rev {version}")
            }
        });
    }
    result
}

fn latest_profile<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().rev().find_map(|line| {
        line.trim()
            .strip_prefix("profile:")
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
    })
}

fn media_type_for_profile(
    profile: &str,
    bd_layer_count: Option<u32>,
    system_media_types: &[String],
) -> Option<String> {
    let candidates: &[&str] = match profile {
        "CD-ROM" | "CD-R" | "CD-RW" => &["cd", "gdrom"],
        "DVD-ROM" | "DVD-R" | "DVD-RAM" | "DVD-RW RO" | "DVD-RW" | "DVD+RW" | "DVD+R" => {
            &["dvd5", "dvd5gc", "dvd5wii"]
        }
        "DVD-R DL" | "DVD-R DL LJR" | "DVD+RW DL" | "DVD+R DL" => &["dvd9", "dvd9wii"],
        "BD-ROM" | "BD-R" | "BD-R RRM" | "BD-RW" => match bd_layer_count {
            Some(2) => &["bd50", "bd66", "bd50wiiu"][..],
            Some(3) => &["bd100"][..],
            Some(4) => &["bd128"][..],
            _ => &["bd25", "bd25wiiu"][..],
        },
        "HD DVD-ROM" | "HD DVD-R" | "HD DVD-RAM" | "HD DVD-RW" => &["hdvd15"],
        "HD DVD-R DL" | "HD DVD-RW DL" => &["hdvd30"],
        _ => return None,
    };

    system_media_types
        .iter()
        .find(|media_type| {
            candidates
                .iter()
                .any(|candidate| media_type.eq_ignore_ascii_case(candidate))
        })
        .cloned()
}

fn section_name(line: &str) -> Option<&str> {
    line.strip_prefix("*** ")?.split_whitespace().next()
}

fn latest_section<'a>(lines: &[&'a str], wanted: &str) -> Option<Vec<&'a str>> {
    let mut latest = None;
    let mut collecting = false;

    for line in lines {
        if let Some(name) = section_name(line) {
            collecting = name == wanted;
            if collecting {
                latest = Some(Vec::new());
            }
            continue;
        }
        if collecting {
            latest.as_mut().expect("section initialized").push(*line);
        }
    }

    latest
}

fn scalar_value(lines: &[&str], label: &str) -> Option<String> {
    lines.iter().find_map(|line| {
        line.trim()
            .strip_prefix(label)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn error_count(lines: &[&str]) -> Option<i64> {
    if lines
        .iter()
        .any(|line| line.trim().starts_with("REDUMP.ORG errors:"))
    {
        return None;
    }

    let mut found = false;
    let mut total = 0i64;
    for line in lines {
        let Some(value) = line.trim().strip_prefix("REDUMP.INFO errors:") else {
            continue;
        };
        let value = value.trim().parse::<i64>().ok()?;
        if value < 0 {
            return None;
        }
        total = total.checked_add(value)?;
        found = true;
    }
    found.then_some(total)
}

fn indented_valid_lines_after(
    lines: &[&str],
    label: &str,
    validate: fn(&str) -> Result<(), String>,
) -> Option<String> {
    let index = lines.iter().position(|line| line.trim() == label)?;
    let mut values = Vec::new();
    for line in &lines[index + 1..] {
        let value = line.trim();
        if value.is_empty() || validate(value).is_err() {
            break;
        }
        values.push(value);
    }
    nonempty_join(values)
}

fn looks_like_hex_dump(line: &str) -> bool {
    let line = line.trim();
    let Some((offset, _)) = line.split_once(" : ") else {
        return false;
    };
    offset.len() == 4 && offset.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_dump_after(lines: &[&str], label: &str) -> Option<String> {
    let index = lines.iter().position(|line| line.trim() == label)?;
    let values = lines[index + 1..]
        .iter()
        .take_while(|line| looks_like_hex_dump(line))
        .map(|line| line.trim_end());
    nonempty_join(values)
}

fn signed_value_after(lines: &[&str], label: &str) -> Option<String> {
    let value = scalar_value(lines, label)?;
    let candidate = value
        .split(|character: char| character.is_whitespace() || matches!(character, '[' | ']' | '.'))
        .find(|part| {
            let digits = part
                .strip_prefix('+')
                .or_else(|| part.strip_prefix('-'))
                .unwrap_or(part);
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })?;
    Some(candidate.to_owned())
}

fn block_after_prefix(lines: &[&str], prefix: &str) -> Option<String> {
    let index = lines
        .iter()
        .position(|line| line.trim_start().starts_with(prefix))?;
    let values = lines[index + 1..]
        .iter()
        .take_while(|line| !line.trim().is_empty() && !line.trim_start().starts_with(prefix))
        .map(|line| line.trim_end());
    nonempty_join(values)
}

fn consecutive_matching_after(
    lines: &[&str],
    label: &str,
    matches: impl Fn(&str) -> bool,
) -> Option<String> {
    let index = lines.iter().position(|line| line.trim() == label)?;
    let values = lines[index + 1..]
        .iter()
        .take_while(|line| matches(line))
        .map(|line| line.trim());
    nonempty_join(values)
}

fn block_header_code(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.ends_with("]:") {
        return None;
    }
    let (code, _) = line.split_once(" [")?;
    if code.is_empty()
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(code)
}

fn system_code(lines: &[&str], known_system_codes: &[String]) -> Option<String> {
    let known: HashSet<&str> = known_system_codes.iter().map(String::as_str).collect();
    let code = lines.iter().find_map(|line| {
        let code = block_header_code(line)?;
        if code == "SecuROM" && known.contains("PC") {
            return Some("PC".to_owned());
        }
        known.contains(code).then(|| code.to_owned())
    })?;
    if code == "XBOX" && known.contains("XBOX360") {
        let index = lines.iter().position(|line| block_header_code(line) == Some("XBOX"))?;
        let block_end = lines[index + 1..]
            .iter()
            .position(|line| block_header_code(line).is_some())
            .map(|offset| index + 1 + offset)
            .unwrap_or(lines.len());
        if scalar_value(&lines[index + 1..block_end], "system:")
            .is_some_and(|value| value.starts_with("Xbox 360"))
        {
            return Some("XBOX360".to_owned());
        }
    }
    Some(code)
}

fn protection_system_code(lines: &[&str], known_system_codes: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let trimmed = line.trim();
        let value = trimmed
            .strip_prefix("protection:")
            .map(str::trim)
            .unwrap_or(trimmed);
        let code = if value.starts_with("SafeDisc") {
            "PC"
        } else if value.starts_with("PS2/Datel") {
            "PS2"
        } else {
            return None;
        };
        known_system_codes
            .iter()
            .any(|known| known == code)
            .then(|| code.to_owned())
    })
}

fn dvdkey_has_css_cppm(lines: &[&str]) -> bool {
    scalar_value(lines, "protection system type:")
        .is_some_and(|value| value.eq_ignore_ascii_case("CSS/CPPM"))
}

fn normalized_dvd_key(value: &str) -> Option<String> {
    let bytes: Vec<&str> = value.trim().split(':').collect();
    if bytes.len() != 5
        || bytes
            .iter()
            .any(|byte| byte.len() != 2 || !byte.bytes().all(|value| value.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(
        bytes
            .into_iter()
            .map(str::to_ascii_uppercase)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn dvdkey_protection(lines: &[&str]) -> Option<Vec<String>> {
    if !dvdkey_has_css_cppm(lines) {
        return None;
    }

    let mut values = Vec::new();
    if let Some(region) = scalar_value(lines, "region management information:") {
        values.push(format!("Region: {region}"));
    }
    values.push("Copyright Protection System Type: CSS/CPPM".to_owned());

    if let Some(index) = lines.iter().position(|line| line.trim() == "title keys:") {
        let title_keys_indent = lines[index].len() - lines[index].trim_start().len();
        for line in &lines[index + 1..] {
            let trimmed = line.trim();
            let indent = line.len() - line.trim_start().len();
            if trimmed.is_empty() || indent <= title_keys_indent {
                break;
            }
            let Some((name, key)) = trimmed.split_once(':') else {
                continue;
            };
            let Some(key) = normalized_dvd_key(key) else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() {
                values.push(format!("{name} Title Key: {key}"));
            }
        }
    }

    if let Some(key) = scalar_value(lines, "disc key:").and_then(|key| normalized_dvd_key(&key)) {
        values.push(format!("Decrypted Disc Key: {key}"));
    }

    Some(values)
}

fn protection(
    protection_lines: Option<&[&str]>,
    dvdkey_lines: Option<&[&str]>,
    info_lines: Option<&[&str]>,
) -> Option<String> {
    let mut values = Vec::new();
    let mut explicit_none = false;

    if let Some(lines) = protection_lines {
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed == "protections:" {
                for entry in &lines[index + 1..] {
                    if entry.trim().is_empty() || entry.trim_start().len() == entry.len() {
                        break;
                    }
                    values.push(entry.trim().to_owned());
                }
            } else if let Some(value) = trimmed.strip_prefix("protection:") {
                let value = value.trim();
                if value.eq_ignore_ascii_case("none") {
                    explicit_none = true;
                } else if !value.is_empty() {
                    values.push(value.to_owned());
                }
            }
        }
    }

    if let Some(dvdkey_values) = dvdkey_lines.and_then(dvdkey_protection) {
        values.extend(dvdkey_values);
    }

    if let Some(lines) = info_lines {
        let mut index = 0;
        while index < lines.len() {
            let Some(code) = block_header_code(lines[index]) else {
                index += 1;
                continue;
            };
            let end = lines[index + 1..]
                .iter()
                .position(|line| block_header_code(line).is_some())
                .map(|offset| index + 1 + offset)
                .unwrap_or(lines.len());
            let block = &lines[index + 1..end];

            if code == "SecuROM" {
                if let Some(scheme) = scalar_value(block, "scheme:") {
                    values.push(format!("SecuROM scheme: {scheme}"));
                }
            } else if code == "PSX"
                && scalar_value(block, "libcrypt:")
                    .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
            {
                values.push("LibCrypt".to_owned());
            }
            index = end;
        }
    }

    if !values.is_empty() {
        Some(values.join("\n"))
    } else if explicit_none {
        Some(String::new())
    } else {
        None
    }
}

fn bd_layer_count(dump_lines: &[&str]) -> Option<u32> {
    dump_lines.iter().find_map(|line| {
        line.trim()
            .strip_prefix("layers count:")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&n| n > 0)
    })
}

fn dump_layerbreaks(dump_lines: &[&str]) -> Option<Vec<String>> {
    let mut breaks: Vec<(u32, String)> = dump_lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("layer break (layer:") {
                let (layer_part, value_part) = rest.split_once(')')?;
                let layer = layer_part.trim().parse::<u32>().ok()?;
                let value = value_part.trim().strip_prefix(':')?.trim();
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Some((layer, value.to_owned()))
            } else if let Some(value) = trimmed.strip_prefix("layer break:") {
                let value = value.trim();
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Some((0, value.to_owned()))
            } else {
                None
            }
        })
        .collect();
    if breaks.is_empty() {
        return None;
    }
    breaks.sort_by_key(|(layer, _)| *layer);
    Some(breaks.into_iter().map(|(_, v)| v).collect())
}

fn nonempty_join<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let values: Vec<&str> = values.into_iter().collect();
    (!values.is_empty()).then(|| values.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn systems() -> Vec<String> {
        [
            "PSX",
            "PS2",
            "SS",
            "PC",
            "GC",
            "WII",
            "DVD-VIDEO",
            "AUDIO-CD",
            "ENHANCED-CD",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn systems_with_media() -> Vec<(String, Vec<String>)> {
        vec![
            (
                "PS2".to_owned(),
                vec!["dvd5".to_owned(), "dvd9".to_owned(), "cd".to_owned()],
            ),
            ("GC".to_owned(), vec!["dvd5gc".to_owned(), "cd".to_owned()]),
            (
                "WII".to_owned(),
                vec!["dvd9wii".to_owned(), "dvd5wii".to_owned()],
            ),
            ("WIIU".to_owned(), vec!["bd25wiiu".to_owned()]),
            ("DC".to_owned(), vec!["gdrom".to_owned(), "cd".to_owned()]),
            (
                "XBOX".to_owned(),
                vec!["xgd".to_owned(), "dvd9".to_owned(), "dvd5".to_owned()],
            ),
            (
                "XBOX360".to_owned(),
                vec!["xgd".to_owned(), "dvd9".to_owned(), "dvd5".to_owned()],
            ),
            ("PS3".to_owned(), vec!["bd25".to_owned()]),
            (
                "HDDVD".to_owned(),
                vec!["hdvd30".to_owned(), "hdvd15".to_owned()],
            ),
            (
                "DVD-VIDEO".to_owned(),
                vec!["dvd5".to_owned(), "dvd9".to_owned()],
            ),
        ]
    }

    #[test]
    fn profile_selects_first_compatible_media_in_system_priority_order() {
        let log = r#"drive information
  profile: CD-ROM
*** INFO (time check: 0s)
PS2 [disc.bin]:"#;

        let parsed = parse_with_system_media(log, &systems_with_media());

        assert_eq!(parsed.system_code.as_deref(), Some("PS2"));
        assert_eq!(parsed.media_type.as_deref(), Some("cd"));
    }

    #[test]
    fn profile_media_mapping_covers_supported_optical_profiles() {
        let cases = [
            ("CD-R", "DC", "gdrom"),
            ("DVD-ROM", "GC", "dvd5gc"),
            ("DVD-R DL LJR", "WII", "dvd9wii"),
            ("BD-R RRM", "WIIU", "bd25wiiu"),
            ("HD DVD-RAM", "HDDVD", "hdvd15"),
            ("HD DVD-RW DL", "HDDVD", "hdvd30"),
        ];

        for (profile, system, expected) in cases {
            let log = format!(
                "drive information\n  profile: {profile}\n*** INFO (time check: 0s)\n{system} [disc.bin]:"
            );
            assert_eq!(
                parse_with_system_media(&log, &systems_with_media())
                    .media_type
                    .as_deref(),
                Some(expected),
                "profile {profile} for {system}"
            );
        }
    }

    #[test]
    fn profile_does_not_select_media_without_a_detected_compatible_system() {
        let missing_system = "drive information\n  profile: CD-ROM\n*** INFO (time check: 0s)";
        assert_eq!(
            parse_with_system_media(missing_system, &systems_with_media()).media_type,
            None
        );

        let incompatible =
            "drive information\n  profile: BD-ROM\n*** INFO (time check: 0s)\nPS2 [disc.bin]:";
        assert_eq!(
            parse_with_system_media(incompatible, &systems_with_media()).media_type,
            None
        );

        let unknown =
            "drive information\n  profile: UNKNOWN\n*** INFO (time check: 0s)\nPS2 [disc.bin]:";
        assert_eq!(
            parse_with_system_media(unknown, &systems_with_media()).media_type,
            None
        );
    }

    #[test]
    fn latest_profile_is_used_for_combined_logs() {
        let log = r#"profile: DVD-ROM
redumper (build: b738)
*** INFO (time check: 0s)
PS2 [old.bin]:
profile: CD-ROM
redumper (build: b739)
*** INFO (time check: 0s)
PS2 [new.bin]:"#;

        assert_eq!(
            parse_with_system_media(log, &systems_with_media())
                .media_type
                .as_deref(),
            Some("cd")
        );
    }

    #[test]
    fn latest_sections_supply_scalar_and_multiline_fields() {
        let log = r#"*** INFO (time check: 0s)
  version: 0.90
*** SPLIT (time check: 0s)
Universal Hash (SHA-1): aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
CUE [old.cue]:
FILE "old.bin" BINARY
*** HASH (time check: 0s)
dat:
<rom name="old.bin" size="1" crc="aaaaaaaa" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />
*** INFO (time check: 1s)
  version: 1.00
  EXE date: 2004-04-21
  mode2 (form 2) EDC: no
  security sector ranges:
    10-20
    30-40
  PVD:
0320 : 00 01                                             ..
  header:
0000 : 53 45                                             SE
ISO9660 [disc.bin]:
PSX [disc.bin]:
*** SPLIT (time check: 1s)
non-zero data sample range: [    +2354 .. +166271106]
Universal Hash (SHA-1): 8b7cd238b2537a235d4726e8c103a57c28a9a825
disc write offset: +2
CUE [first.cue]:
FILE "first.bin" BINARY
  TRACK 01 MODE1/2352
CUE [second.cue]:
FILE "second.bin" BINARY
*** HASH (time check: 1s)
dat:
<rom name="one.bin" size="1" crc="aaaaaaaa" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />
<rom name="two.bin" size="2" crc="bbbbbbbb" md5="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />
*** END (time check: 0s)"#;

        let parsed = parse(log, &systems());

        assert_eq!(parsed.version.as_deref(), Some("1.00"));
        assert_eq!(parsed.exe_date.as_deref(), Some("2004-04-21"));
        assert_eq!(parsed.edc, Some(false));
        assert_eq!(
            parsed.universal_hash.as_deref(),
            Some("8b7cd238b2537a235d4726e8c103a57c28a9a825")
        );
        assert_eq!(parsed.sample_start.as_deref(), Some("+2354"));
        assert_eq!(parsed.offset_value.as_deref(), Some("+2"));
        assert_eq!(parsed.sector_ranges.as_deref(), Some("10-20\n30-40"));
        assert_eq!(
            parsed.pvd.as_deref(),
            Some("0320 : 00 01                                             ..")
        );
        assert_eq!(
            parsed.header.as_deref(),
            Some("0000 : 53 45                                             SE")
        );
        assert_eq!(parsed.system_code.as_deref(), Some("PSX"));
        assert_eq!(
            parsed.cuesheet.as_deref(),
            Some("FILE \"first.bin\" BINARY\n  TRACK 01 MODE1/2352")
        );
        assert_eq!(parsed.dat.as_deref().unwrap().lines().count(), 2);
    }

    #[test]
    fn gc_and_wii_versions_are_revision_labels() {
        let parse_version = |system: &str, version: &str| {
            let log =
                format!("*** INFO (time check: 0s)\n{system} [disc.bin]:\nversion: {version}");
            parse(&log, &systems())
        };

        let gc_zero = parse_version("GC", "0");
        assert_eq!(gc_zero.system_code.as_deref(), Some("GC"));
        assert_eq!(gc_zero.version.as_deref(), Some(""));

        let gc_revision = parse_version("GC", "2");
        assert_eq!(gc_revision.version.as_deref(), Some("Rev 2"));

        let wii_string_revision = parse_version("WII", "B");
        assert_eq!(wii_string_revision.version.as_deref(), Some("Rev B"));

        let other_system = parse_version("PSX", "1.00");
        assert_eq!(other_system.version.as_deref(), Some("1.00"));
    }

    #[test]
    fn autofill_requires_every_redumper_build_to_be_supported() {
        let header = |build: &str| format!("redumper (build: {build})");

        assert!(!has_supported_autofill_builds(&header("b736")));
        assert!(has_supported_autofill_builds(&header("b737")));
        assert!(has_supported_autofill_builds(&header("b1000")));
        assert!(has_supported_autofill_builds(
            "  redumper (build: b737)  \r\nredumper (build: b900)"
        ));

        for unsupported in [
            "",
            "redumper (build: LOCAL)",
            "redumper (build:)",
            "redumper (build: b)",
            "redumper (build: b7x7)",
            "redumper (build: b737",
            "redumper (build: b18446744073709551616)",
            "redumper (build: b900)\nredumper (build: b736)",
            "redumper (build: b900)\nredumper (build: LOCAL)",
        ] {
            assert!(
                !has_supported_autofill_builds(unsupported),
                "unexpectedly supported: {unsupported:?}"
            );
        }
    }

    #[test]
    fn dat_extra_entries_are_excluded_without_a_blank_separator() {
        let log = r#"*** HASH (time check: 0s)
dat:
<rom name="xbox47.iso" size="7825162240" crc="ffaf1dcc" md5="e057504a77b4e0ad6271ea76f352a68f" sha1="4c4377a74a07ec2af8c7ef11ae30490f48d0ef13" />
dat (extra):
<rom name="xbox47.dmi" size="2048" crc="043bf884" md5="efe2e7d26f6a375ea6a5e17074bcc908" sha1="ab7246ac85974768f7f3fb79bd6f7bad33e818c8" />
<rom name="xbox47.pfi" size="2048" crc="8fc52135" md5="51badb1da2cc5fb0b272061dab9ef75b" sha1="1b1c6e61835799dd182dea5b3f3f35447216a8ac" />"#;

        assert_eq!(
            parse(log, &[]).dat.as_deref(),
            Some(
                r#"<rom name="xbox47.iso" size="7825162240" crc="ffaf1dcc" md5="e057504a77b4e0ad6271ea76f352a68f" sha1="4c4377a74a07ec2af8c7ef11ae30490f48d0ef13" />"#
            )
        );
    }

    #[test]
    fn error_count_sums_info_lines_and_org_invalidates_the_field() {
        let valid = "*** INFO (time check: 0s)\n  REDUMP.INFO errors: 2\n  REDUMP.INFO errors: 3";
        assert_eq!(parse(valid, &[]).error_count, Some(5));

        let old = "*** INFO (time check: 0s)\n  REDUMP.INFO errors: 2\n  REDUMP.ORG errors: 3";
        assert_eq!(parse(old, &[]).error_count, None);

        let absent = "*** INFO (time check: 0s)\n  version: 1.00";
        assert_eq!(parse(absent, &[]).error_count, None);
    }

    #[test]
    fn build_date_fills_exe_date_when_explicit_exe_date_is_absent() {
        let saturn = "*** INFO (time check: 0s)\n\
SS [disc.bin]:\n\
  build date: 1995-03-17";
        assert_eq!(
            parse(saturn, &systems()).exe_date.as_deref(),
            Some("1995-03-17")
        );

        let both = "*** INFO (time check: 0s)\n\
  build date: 1995-03-17\n\
  EXE date: 2004-04-21";
        assert_eq!(
            parse(both, &systems()).exe_date.as_deref(),
            Some("2004-04-21")
        );
    }

    #[test]
    fn detect_xbox360_system_code() {
        let systems = vec!["XBOX".to_owned(), "XBOX360".to_owned()];

        let xbox360 = "*** INFO (time check: 0s)\n\
XBOX [disc.iso]:\n\
  system: Xbox 360 (XGD2)";
        assert_eq!(
            parse(xbox360, &systems).system_code.as_deref(),
            Some("XBOX360")
        );
    }

    #[test]
    fn securom_info_block_maps_to_pc_system() {
        let log = "*** INFO (time check: 0s)\n\
CD-ROM [disc.bin]:\n\
SecuROM [disc.bin]:\n\
  scheme: 3\n\
PSX [disc.bin]:";
        assert_eq!(parse(log, &systems()).system_code.as_deref(), Some("PC"));

        let without_pc = ["PSX".to_owned()];
        assert_eq!(parse(log, &without_pc).system_code.as_deref(), Some("PSX"));
    }

    #[test]
    fn safedisc_protection_maps_to_pc_system() {
        let plural = "*** INFO (time check: 0s)\n\
PSX [disc.bin]:\n\
*** PROTECTION (time check: 0s)\n\
protections:\n\
  SafeDisc 2.51.020\n\
*** END (time check: 0s)";
        assert_eq!(parse(plural, &systems()).system_code.as_deref(), Some("PC"));

        let singular = "*** PROTECTION (time check: 0s)\n\
protection: SafeDisc 1.50.020";
        assert_eq!(
            parse(singular, &systems()).system_code.as_deref(),
            Some("PC")
        );

        let without_pc = ["PSX".to_owned()];
        assert_eq!(
            parse(plural, &without_pc).system_code.as_deref(),
            Some("PSX")
        );
    }

    #[test]
    fn ps2_datel_protection_maps_to_ps2_system() {
        let plural = "*** INFO (time check: 0s)\n\
PSX [disc.bin]:\n\
*** PROTECTION (time check: 0s)\n\
protections:\n\
  PS2/Datel BIG.DAT, C2: 4351, range: 35-4385\n\
*** END (time check: 0s)";
        assert_eq!(
            parse(plural, &systems()).system_code.as_deref(),
            Some("PS2")
        );

        let singular = "*** PROTECTION (time check: 0s)\n\
protection: PS2/Datel FakeTOC, lead-out: 305571";
        assert_eq!(
            parse(singular, &systems()).system_code.as_deref(),
            Some("PS2")
        );

        let without_ps2 = ["PSX".to_owned()];
        assert_eq!(
            parse(plural, &without_ps2).system_code.as_deref(),
            Some("PSX")
        );
    }

    #[test]
    fn all_audio_cue_maps_to_audio_cd_system() {
        let audio = "*** SPLIT (time check: 0s)\n\
CUE [disc.cue]:\n\
FILE \"Track 01.bin\" BINARY\n\
  TRACK 01 AUDIO\n\
    INDEX 01 00:00:00\n\
FILE \"Track 02.bin\" BINARY\n\
  TRACK 02 AUDIO\n\
    INDEX 01 00:00:00\n\
*** END (time check: 0s)";
        assert_eq!(
            parse(audio, &systems()).system_code.as_deref(),
            Some("AUDIO-CD")
        );

        let mixed = audio.replacen("TRACK 01 AUDIO", "TRACK 01 MODE1/2352", 1);
        assert_eq!(parse(&mixed, &systems()).system_code, None);

        let without_audio_cd = ["PSX".to_owned()];
        assert_eq!(parse(audio, &without_audio_cd).system_code, None);

        let detected = format!("*** INFO (time check: 0s)\nPSX [disc.bin]:\n{audio}");
        assert_eq!(
            parse(&detected, &systems()).system_code.as_deref(),
            Some("PSX")
        );
    }

    #[test]
    fn multisession_audio_then_data_cue_maps_to_enhanced_cd_system() {
        let enhanced = "*** SPLIT (time check: 0s)\n\
CUE [disc.cue]:\n\
REM SESSION 01\n\
FILE \"Track 01.bin\" BINARY\n\
  TRACK 01 AUDIO\n\
    INDEX 01 00:00:00\n\
FILE \"Track 02.bin\" BINARY\n\
  TRACK 02 AUDIO\n\
    INDEX 01 00:00:00\n\
REM SESSION 02\n\
REM LEAD-IN 01:00:00\n\
REM PREGAP 00:02:00\n\
FILE \"Track 03.bin\" BINARY\n\
  TRACK 03 MODE2/2352\n\
    INDEX 01 00:00:00\n\
*** END (time check: 0s)";
        assert_eq!(
            parse(enhanced, &systems()).system_code.as_deref(),
            Some("ENHANCED-CD")
        );

        let detected = format!("*** INFO (time check: 0s)\nPSX [disc.bin]:\n{enhanced}");
        assert_eq!(
            parse(&detected, &systems()).system_code.as_deref(),
            Some("PSX")
        );

        let without_enhanced_cd = ["AUDIO-CD".to_owned()];
        assert_eq!(parse(enhanced, &without_enhanced_cd).system_code, None);
    }

    #[test]
    fn sbi_includes_only_lines_accepted_by_the_shared_validator() {
        let log = "*** INFO (time check: 0s)\n\
MSF: 02:03:04 Q-Data: 410102 03:04:05 00 06:07:08 ABCD\n\
MSF: 0:0:0 Q-Data: 000000 00:00:00 00 00:00:00 0000";
        assert_eq!(
            parse(log, &[]).sbi.as_deref(),
            Some("MSF: 02:03:04 Q-Data: 410102 03:04:05 00 06:07:08 ABCD")
        );
    }

    #[test]
    fn protection_is_cumulative_and_none_only_clears_when_alone() {
        let log = r#"*** PROTECTION (time check: 0s)
protections:
  PS2/Datel BIG.DAT, C2: 4351, range: 35-4385
  PS2/Datel FakeTOC, lead-out: 305571, ISO9660 size: 328992
protection: none
*** INFO (time check: 0s)
SecuROM [disc.bin]:
  scheme: 3
PSX [disc.bin]:
  libcrypt: yes
*** END (time check: 0s)"#;
        assert_eq!(
            parse(log, &systems()).protection.as_deref(),
            Some(
                "PS2/Datel BIG.DAT, C2: 4351, range: 35-4385\n\
PS2/Datel FakeTOC, lead-out: 305571, ISO9660 size: 328992\n\
SecuROM scheme: 3\n\
LibCrypt"
            )
        );

        let none = "*** PROTECTION (time check: 0s)\nprotection: none";
        assert_eq!(parse(none, &[]).protection, Some(String::new()));

        let absent = "*** PROTECTION (time check: 0s)\n";
        assert_eq!(parse(absent, &[]).protection, None);
    }

    #[test]
    fn css_cppm_dvdkey_maps_to_dvd_video_and_formats_protection() {
        let log = r#"drive information
  profile: DVD-ROM
*** DVDKEY (time check: 0s)

copyright:
  protection system type: CSS/CPPM
  region management information: 1
  disc key: 0A:15:22:1E:B0
  title keys:
    VTS_01_0.VOB: C7:DC:7C:E9:99
    VTS_01_1.VOB: C7:DC:7C:E9:99
    VTS_01_2.VOB: C7:DC:7C:E9:99
    VTS_01_3.VOB: C7:DC:7C:E9:99

*** SPLIT (time check: 0s)"#;

        let parsed = parse_with_system_media(log, &systems_with_media());

        assert_eq!(parsed.system_code.as_deref(), Some("DVD-VIDEO"));
        assert_eq!(parsed.media_type.as_deref(), Some("dvd5"));
        assert_eq!(
            parsed.protection.as_deref(),
            Some(
                "Region: 1\n\
Copyright Protection System Type: CSS/CPPM\n\
VTS_01_0.VOB Title Key: C7 DC 7C E9 99\n\
VTS_01_1.VOB Title Key: C7 DC 7C E9 99\n\
VTS_01_2.VOB Title Key: C7 DC 7C E9 99\n\
VTS_01_3.VOB Title Key: C7 DC 7C E9 99\n\
Decrypted Disc Key: 0A 15 22 1E B0"
            )
        );
    }

    #[test]
    fn latest_dvdkey_is_cumulative_between_protection_and_info_sources() {
        let log = r#"*** DVDKEY (time check: 0s)
copyright:
  protection system type: CSS/CPPM
  region management information: 8
*** PROTECTION (time check: 0s)
protection: SafeDisc 1
*** DVDKEY (time check: 1s)
copyright:
  protection system type: CSS/CPPM
  region management information: 2
  disc key: aa:bb:cc:dd:ee
  title keys:
    VTS_02_0.VOB: 01:23:45:67:89
*** INFO (time check: 0s)
SecuROM [disc.bin]:
  scheme: 3"#;

        let parsed = parse(log, &systems());

        assert_eq!(parsed.system_code.as_deref(), Some("DVD-VIDEO"));
        assert_eq!(
            parsed.protection.as_deref(),
            Some(
                "SafeDisc 1\n\
Region: 2\n\
Copyright Protection System Type: CSS/CPPM\n\
VTS_02_0.VOB Title Key: 01 23 45 67 89\n\
Decrypted Disc Key: AA BB CC DD EE\n\
SecuROM scheme: 3"
            )
        );
    }

    #[test]
    fn non_css_dvdkey_and_malformed_keys_are_not_filled() {
        let unsupported = r#"*** DVDKEY (time check: 0s)
copyright:
  protection system type: CPRM
  region management information: 1
  disc key: 0A:15:22:1E:B0"#;
        let parsed = parse(unsupported, &systems());
        assert_eq!(parsed.system_code, None);
        assert_eq!(parsed.protection, None);

        let malformed = r#"*** DVDKEY (time check: 0s)
copyright:
  protection system type: CSS/CPPM
  disc key: 0A:15:22:1E
  title keys:
    bad.vob: GG:DC:7C:E9:99"#;
        assert_eq!(
            parse(malformed, &systems()).protection.as_deref(),
            Some("Copyright Protection System Type: CSS/CPPM")
        );
    }

    #[test]
    fn singular_protection_is_included() {
        let log = "*** PROTECTION (time check: 0s)\n\
protection: SafeDisc 00000001.TMP, C2: 589, gap range: 275-10274";
        assert_eq!(
            parse(log, &[]).protection.as_deref(),
            Some("SafeDisc 00000001.TMP, C2: 589, gap range: 275-10274")
        );
    }
}
