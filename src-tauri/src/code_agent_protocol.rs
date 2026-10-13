//! Bounded, string-aware extraction of one typed JSON tool call from model output.
use serde::de::DeserializeOwned;

pub fn parse_tool<T: DeserializeOwned>(content: &str) -> Result<T, String> {
    if content.len() > 2 * 1024 * 1024 {
        return Err("tool response exceeds the 2 MB limit".into());
    }
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut candidates = 0usize;
    let mut parsed = None;
    for (index, byte) in content.bytes().enumerate() {
        if start.is_none() {
            if byte == b'{' {
                start = Some(index);
                depth = 1;
                in_string = false;
                escaped = false;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let lo = start.take().expect("object start exists");
                    candidates += 1;
                    if candidates > 128 {
                        return Err("too many JSON objects in tool response".into());
                    }
                    // Braces are ASCII, so these byte indices are UTF-8 boundaries.
                    if let Ok(tool) = serde_json::from_str::<T>(&content[lo..=index]) {
                        if parsed.is_some() {
                            return Err("ambiguous response: emit exactly one tool call".into());
                        }
                        parsed = Some(tool);
                    }
                }
            }
            _ => {}
        }
    }
    if start.is_some() {
        return Err("unterminated JSON object in tool response".into());
    }
    parsed.ok_or_else(|| "could not parse a tool-call JSON object".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(tag = "intent", rename_all = "snake_case")]
    enum Tool {
        Finalize { summary: String },
        WriteFiles { files: Vec<File> },
    }
    #[derive(Debug, Deserialize)]
    struct File {
        rel_path: String,
        content: String,
    }

    #[test]
    fn skips_malformed_prose_object_without_discarding_valid_call() {
        let tool = parse_tool::<Tool>("noise {not-json} more noise {\"intent\":\"write_files\",\"files\":[{\"rel_path\":\"a.cs\",\"content\":\"//\"}]}").unwrap();
        match tool {
            Tool::WriteFiles { files } => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].rel_path, "a.cs");
                assert_eq!(files[0].content, "//");
            }
            _ => panic!("expected write call"),
        }
    }

    #[test]
    fn ignores_braces_and_escaped_quotes_in_strings_and_preserves_unicode() {
        let tool = parse_tool::<Tool>(
            r#"```json
{"intent":"finalize","summary":"سلام { \"quoted\" }"}
```"#,
        )
        .unwrap();
        match tool {
            Tool::Finalize { summary } => assert_eq!(summary, "سلام { \"quoted\" }"),
            _ => panic!("expected finalize"),
        }
    }

    #[test]
    fn rejects_multiple_valid_calls_and_partial_trailing_objects() {
        let call = r#"{"intent":"finalize","summary":"done"}"#;
        assert!(parse_tool::<Tool>(&format!("{call} {call}")).is_err());
        assert!(parse_tool::<Tool>(&format!("{call} {{unfinished")).is_err());
        assert!(parse_tool::<Tool>("just words").is_err());
    }

    #[test]
    fn enforces_response_size_and_candidate_count() {
        assert!(parse_tool::<Tool>(&"x".repeat(2 * 1024 * 1024 + 1)).is_err());
        assert!(parse_tool::<Tool>(&"{invalid}".repeat(129)).is_err());
    }
}
