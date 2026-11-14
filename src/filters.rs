use std::fmt::Display;

pub fn script_json<T: Display>(json: T) -> askama::Result<String> {
    Ok(escape_script_json(&json.to_string()))
}

pub fn script_json_value<T: serde::Serialize>(value: T) -> askama::Result<String> {
    serde_json::to_string(&value)
        .map(|json| escape_script_json(&json))
        .map_err(|error| askama::Error::Custom(Box::new(error)))
}

pub fn escape_script_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closing_script_tag_cannot_survive() {
        let json = serde_json::json!({"code": "</script><script>alert(1)</script>"}).to_string();
        let safe = script_json(&json).unwrap();
        assert!(!safe.contains('<') && !safe.contains('>'));
        let decoded: serde_json::Value = serde_json::from_str(&safe).unwrap();
        assert_eq!(decoded["code"], "</script><script>alert(1)</script>");
    }

    #[test]
    fn line_separators_and_ampersands_are_escaped() {
        assert_eq!(
            escape_script_json("[\"a\u{2028}b\u{2029}&\"]"),
            "[\"a\\u2028b\\u2029\\u0026\"]"
        );
    }

    #[test]
    fn values_are_serialized_and_escaped() {
        let breaks = vec!["\\".to_string(), "];alert(1)//".to_string()];
        assert_eq!(
            script_json_value(&breaks).unwrap(),
            r#"["\\","];alert(1)//"]"#
        );
        assert_eq!(script_json_value(["<b>"]).unwrap(), r#"["\u003cb\u003e"]"#);
    }
}
