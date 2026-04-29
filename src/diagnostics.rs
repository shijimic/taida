/// Split an inline diagnostic message into its stable code and hint.
///
/// The canonical code spelling is `[E####]`, but older parser/verify
/// diagnostics may still use `E####:`. JSON/JSONL/SARIF emitters should
/// accept both so legacy messages do not lose their structured code.
pub fn split_diag_code_and_hint(message: &str) -> (Option<String>, Option<String>) {
    let code = split_diag_code(message);
    let suggestion = message
        .split_once("Hint:")
        .map(|(_, hint)| hint.trim().to_string())
        .filter(|hint| !hint.is_empty());

    (code, suggestion)
}

fn split_diag_code(message: &str) -> Option<String> {
    if let Some(rest) = message.strip_prefix('[') {
        let bytes = rest.as_bytes();
        if bytes.len() >= 6 {
            let code_candidate = &bytes[..5];
            if bytes[5] == b']' && is_diag_code_bytes(code_candidate) {
                return Some(String::from_utf8_lossy(code_candidate).into_owned());
            }
        }
        return None;
    }

    let bytes = message.as_bytes();
    if bytes.len() >= 6 {
        let code_candidate = &bytes[..5];
        if bytes[5] == b':' && is_diag_code_bytes(code_candidate) {
            return Some(String::from_utf8_lossy(code_candidate).into_owned());
        }
    }

    None
}

fn is_diag_code_bytes(candidate: &[u8]) -> bool {
    candidate.len() == 5
        && candidate[0] == b'E'
        && candidate[1..].iter().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::split_diag_code_and_hint;

    #[test]
    fn extracts_bracket_code_and_hint() {
        let (code, hint) = split_diag_code_and_hint("[E1400] missing field Hint: add `x`");
        assert_eq!(code.as_deref(), Some("E1400"));
        assert_eq!(hint.as_deref(), Some("add `x`"));
    }

    #[test]
    fn extracts_colon_code() {
        let (code, hint) = split_diag_code_and_hint("E0301: 単一方向制約違反");
        assert_eq!(code.as_deref(), Some("E0301"));
        assert!(hint.is_none());
    }

    #[test]
    fn rejects_non_diagnostic_prefix() {
        let (code, hint) = split_diag_code_and_hint("error E0301: message");
        assert!(code.is_none());
        assert!(hint.is_none());
    }

    #[test]
    fn rejects_multibyte_prefix_without_panicking() {
        let (code, hint) = split_diag_code_and_hint("診断 E0301: message");
        assert!(code.is_none());
        assert!(hint.is_none());

        let (code, hint) = split_diag_code_and_hint("[診断] E0301: message");
        assert!(code.is_none());
        assert!(hint.is_none());
    }
}
