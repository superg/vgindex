use std::collections::HashSet;

use serde::Serialize;

use crate::services::validation;

pub const MINIMUM_AUTOFILL_BUILD: u64 = 737;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ParsedRedumperLog {
    pub system_code: Option<String>,
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
    let normalized = log.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let info = latest_section(&lines, "INFO");
    let split = latest_section(&lines, "SPLIT");
    let hash = latest_section(&lines, "HASH");
    let protection_section = latest_section(&lines, "PROTECTION");

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

    result.protection = protection(protection_section.as_deref(), info.as_deref());
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
    lines.iter().find_map(|line| {
        let code = block_header_code(line)?;
        if code == "SecuROM" && known.contains("PC") {
            return Some("PC".to_owned());
        }
        known.contains(code).then(|| code.to_owned())
    })
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

fn protection(protection_lines: Option<&[&str]>, info_lines: Option<&[&str]>) -> Option<String> {
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
            "AUDIO-CD",
            "ENHANCED-CD",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
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
    fn singular_protection_is_included() {
        let log = "*** PROTECTION (time check: 0s)\n\
protection: SafeDisc 00000001.TMP, C2: 589, gap range: 275-10274";
        assert_eq!(
            parse(log, &[]).protection.as_deref(),
            Some("SafeDisc 00000001.TMP, C2: 589, gap range: 275-10274")
        );
    }
}
