use serde_json::Value;

/// Returns Some(new_body) only if `body` was rewritten (§6):
/// - malformed JSON            → None (forward as-is, let the provider error)
/// - top-level non-object      → None
/// - "reasoning_effort" absent → None
/// - value != "none"           → None
/// - value == "none"           → Some(body with the value replaced)
///
/// `preserve_order` keeps the original field order; no other bytes are touched
/// beyond reserialization of the whole document.
pub fn rewrite(body: &[u8], replacement: &str) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let slot = value.get_mut("reasoning_effort")?;
    if slot.as_str() == Some("none") {
        *slot = Value::String(replacement.to_string());
        serde_json::to_vec(&value).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIGH: &str = "high";

    #[test]
    fn replaces_none_with_replacement() {
        let body = br#"{"reasoning_effort":"none","messages":[{"role":"user","content":"hi"}]}"#;
        let out = rewrite(body, HIGH).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "high");
        // other fields untouched
        assert_eq!(v["messages"][0]["content"], "hi");
    }

    #[test]
    fn field_order_is_preserved() {
        let body = br#"{"aaa":1,"reasoning_effort":"none","zzz":2}"#;
        let out = rewrite(body, HIGH).unwrap();
        let s = String::from_utf8(out).unwrap();
        let aaa = s.find("\"aaa\"").unwrap();
        let re = s.find("\"reasoning_effort\"").unwrap();
        let zzz = s.find("\"zzz\"").unwrap();
        assert!(aaa < re && re < zzz);
    }

    #[test]
    fn non_none_value_is_not_touched() {
        for v in ["\"low\"", "\"high\"", "null", "42"] {
            let body = format!(r#"{{"reasoning_effort":{v}}}"#);
            assert!(rewrite(body.as_bytes(), HIGH).is_none());
        }
    }

    #[test]
    fn absent_field_is_not_touched() {
        let body = br#"{"messages":[]}"#;
        assert!(rewrite(body, HIGH).is_none());
    }

    #[test]
    fn nested_none_is_not_touched() {
        let body = br#"{"options":{"reasoning_effort":"none"}}"#;
        assert!(rewrite(body, HIGH).is_none());
    }

    #[test]
    fn malformed_json_passes_through() {
        let body = b"{not json";
        assert!(rewrite(body, HIGH).is_none());
    }

    #[test]
    fn empty_body_passes_through() {
        assert!(rewrite(b"", HIGH).is_none());
    }

    #[test]
    fn top_level_non_object_passes_through() {
        let body = br#"[1,2,3]"#;
        assert!(rewrite(body, HIGH).is_none());
    }
}
