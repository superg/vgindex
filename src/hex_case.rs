use serde_json::Value;

pub fn normalize_sbi_hex_case(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let Some(marker) = line.find("Q-Data: ") else {
                return line.to_string();
            };
            let payload_start = marker + "Q-Data: ".len();
            let mut normalized = String::with_capacity(line.len());
            normalized.push_str(&line[..payload_start]);
            normalized.extend(line[payload_start..].chars().map(|ch| match ch {
                'a'..='f' => ch.to_ascii_uppercase(),
                _ => ch,
            }));
            normalized
        })
        .collect()
}

pub fn normalize_dat_hash_case(text: &str) -> String {
    let mut normalized = text.to_string();
    for attribute in ["crc", "md5", "sha1"] {
        let needle = format!(r#"{attribute}=""#);
        let mut search_from = 0;
        while let Some(relative_start) = normalized[search_from..].find(&needle) {
            let value_start = search_from + relative_start + needle.len();
            let Some(relative_end) = normalized[value_start..].find('"') else {
                break;
            };
            let value_end = value_start + relative_end;
            let lowercase = normalized[value_start..value_end].to_ascii_lowercase();
            normalized.replace_range(value_start..value_end, &lowercase);
            search_from = value_end + 1;
        }
    }
    normalized
}

pub fn normalize_hex_dump_case(text: &str) -> String {
    if !text.lines().any(|line| line.contains(':')) {
        let hex_digits = text
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect::<String>();
        if !hex_digits.is_empty()
            && hex_digits.len() % 2 == 0
            && hex_digits.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return text.to_ascii_uppercase();
        }
        return text.to_string();
    }

    text.split_inclusive('\n')
        .map(|line| {
            let (content, newline) = line
                .strip_suffix('\n')
                .map_or((line, ""), |content| (content, "\n"));
            let Some(colon) = content.find(':') else {
                return line.to_string();
            };
            let after_colon = &content[colon + 1..];
            let hex_end = after_colon.find("   ").unwrap_or(after_colon.len());
            let offset = content[..colon].trim();
            let tokens = after_colon[..hex_end].split_whitespace();
            if offset.is_empty()
                || !offset.chars().all(|ch| ch.is_ascii_hexdigit())
                || tokens.clone().any(|token| {
                    token.len() != 2 || !token.chars().all(|ch| ch.is_ascii_hexdigit())
                })
                || tokens.count() == 0
            {
                return line.to_string();
            }
            format!(
                "{}:{}{}{newline}",
                content[..colon].to_ascii_uppercase(),
                after_colon[..hex_end].to_ascii_uppercase(),
                &after_colon[hex_end..]
            )
        })
        .collect()
}

pub fn canonicalize_disc_snapshot_hex_fields(snapshot: &mut Value) {
    let Some(fields) = snapshot.as_object_mut() else {
        return;
    };

    for field in ["disc_id", "disc_key", "universal_hash"] {
        if let Some(Value::String(value)) = fields.get_mut(field) {
            *value = value.to_ascii_lowercase();
        }
    }
    if let Some(Value::String(value)) = fields.get_mut("sbi") {
        *value = normalize_sbi_hex_case(value);
    }
    if let Some(Value::String(value)) = fields.get_mut("dat") {
        *value = normalize_dat_hash_case(value);
    }
    for field in ["pvd", "header", "bca", "pic"] {
        if let Some(Value::String(value)) = fields.get_mut(field) {
            *value = normalize_hex_dump_case(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbi_normalization_uppercases_only_q_data_hex_payload() {
        let input = "MSF: 02:03:04 Q-Data: a1b2c3 0a:0b:0c 00 0d:0e:0f abcd\n";

        assert_eq!(
            normalize_sbi_hex_case(input),
            "MSF: 02:03:04 Q-Data: A1B2C3 0A:0B:0C 00 0D:0E:0F ABCD\n"
        );
    }

    #[test]
    fn dat_normalization_preserves_non_hash_content() {
        let input = r#"<rom name="MixedCase.iso" size="123" crc="AbCd1234" md5="AaBbCcDdEeFf00112233445566778899" sha1="ABCDEFabcdef00112233445566778899aabbccdd" status="KeepMe" />"#;

        assert_eq!(
            normalize_dat_hash_case(input),
            r#"<rom name="MixedCase.iso" size="123" crc="abcd1234" md5="aabbccddeeff00112233445566778899" sha1="abcdefabcdef00112233445566778899aabbccdd" status="KeepMe" />"#
        );
    }

    #[test]
    fn hex_dump_normalization_preserves_ascii_column_case() {
        let input = "000a : aa bb cc                                         abc\n";

        assert_eq!(
            normalize_hex_dump_case(input),
            "000A : AA BB CC                                         abc\n"
        );
    }

    #[test]
    fn snapshot_normalization_canonicalizes_every_hex_representation() {
        let mut snapshot = serde_json::json!({
            "disc_id": "AABB",
            "disc_key": "CCDDEE",
            "universal_hash": "ABCDEF",
            "sbi": "MSF: 02:03:04 Q-Data: aabbcc 00:00:00 00 00:00:00 ddee",
            "pvd": "032a : aa bb                                           ab",
            "header": "000a : cc dd                                           cd",
            "bca": "eeff",
            "pic": "a1b2",
            "dat": "<rom crc=\"ABCDEF12\" md5=\"AABB\" sha1=\"CCDD\" />"
        });

        canonicalize_disc_snapshot_hex_fields(&mut snapshot);

        assert_eq!(snapshot["disc_id"], "aabb");
        assert_eq!(snapshot["disc_key"], "ccddee");
        assert_eq!(snapshot["universal_hash"], "abcdef");
        assert_eq!(
            snapshot["sbi"],
            "MSF: 02:03:04 Q-Data: AABBCC 00:00:00 00 00:00:00 DDEE"
        );
        assert_eq!(
            snapshot["pvd"],
            "032A : AA BB                                           ab"
        );
        assert_eq!(
            snapshot["header"],
            "000A : CC DD                                           cd"
        );
        assert_eq!(snapshot["bca"], "EEFF");
        assert_eq!(snapshot["pic"], "A1B2");
        assert_eq!(
            snapshot["dat"],
            "<rom crc=\"abcdef12\" md5=\"aabb\" sha1=\"ccdd\" />"
        );
    }
}
