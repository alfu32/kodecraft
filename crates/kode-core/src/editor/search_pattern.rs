//! Explicit Java Pattern translation, rather than treating Rust's different
//! default classes/flags/line terminators as interchangeable with Java's.

#[derive(Clone, Copy, Default)]
struct Flags {
    insensitive: bool,
    unicode_case: bool,
    unicode_class: bool,
    multiline: bool,
    dotall: bool,
    unix: bool,
}
fn literal(out: &mut String, ch: char, flags: Flags) {
    if flags.insensitive && !flags.unicode_case && ch.is_ascii_alphabetic() {
        out.push_str(&format!(
            "[{}{}]",
            ch.to_ascii_lowercase(),
            ch.to_ascii_uppercase()
        ));
    } else {
        out.push_str(&format!("\\x{{{:X}}}", ch as u32));
    }
}
fn end_assertion(flags: Flags) -> &'static str {
    match (flags.multiline, flags.unix) {
        (true, true) => r"(?=\n|\z)",
        (false, true) => r"(?=\n\z|\z)",
        (true, false) => r"(?=\r\n|(?<!\r)\n|[\r\x{85}\x{2028}\x{2029}]|\z)",
        (false, false) => r"(?=\r\n\z|(?<!\r)\n\z|[\r\x{85}\x{2028}\x{2029}]\z|\z)",
    }
}
pub(super) fn translate(query: &str) -> std::result::Result<String, String> {
    let chars: Vec<char> = query.chars().collect();
    let mut index = 0;
    let mut out = String::new();
    let mut flags = Flags::default();
    let mut stack = vec![];
    let mut class_depth = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        index += 1;
        if ch == '\\' {
            let Some(&escaped) = chars.get(index) else {
                return Err("Incomplete regex escape".into());
            };
            index += 1;
            match escaped {
                'Q' => {
                    while index < chars.len() {
                        if chars[index] == '\\' && chars.get(index+1) == Some(&'E') { index += 2; break; }
                        literal(&mut out, chars[index], flags);
                        index += 1;
                    }
                }
                'd' | 'D' | 's' | 'S' | 'w' | 'W' if !flags.unicode_class => {
                    let body = match escaped.to_ascii_lowercase() { 'd' => "0-9", 's' => r" \t\n\x0B\x0C\r", _ => "a-zA-Z_0-9" };
                    out.push_str(&format!("[{}{body}]", if escaped.is_ascii_uppercase() { "^" } else { "" }));
                }
                'v' | 'V' | 'h' | 'H' => {
                    let body = if escaped.eq_ignore_ascii_case(&'v') { r"\n\x0B\x0C\r\x{85}\x{2028}\x{2029}" }
                        else { r" \t\x{A0}\x{1680}\x{180E}\x{2000}-\x{200A}\x{202F}\x{205F}\x{3000}" };
                    out.push_str(&format!("[{}{body}]", if escaped.is_ascii_uppercase() { "^" } else { "" }));
                }
                'R' if class_depth == 0 => out.push_str(r"(?:\r\n|[\n\x0B\x0C\r\x{85}\x{2028}\x{2029}])"),
                'b' if class_depth > 0 => out.push_str(r"\x08"),
                'b' | 'B' if !flags.unicode_class => out.push_str(&format!("(?-u:\\{escaped})")),
                'u' | 'x' => {
                    let count = if escaped == 'u' { 4 } else { 2 };
                    let hex = if chars.get(index) == Some(&'{') && escaped == 'x' {
                        index += 1;
                        let start = index;
                        while chars.get(index).is_some_and(|ch| *ch != '}') { index += 1; }
                        if chars.get(index) != Some(&'}') { return Err("Unclosed hexadecimal escape".into()); }
                        let hex: String = chars[start..index].iter().collect(); index += 1; hex
                    } else {
                        if index + count > chars.len() { return Err("Incomplete hexadecimal escape".into()); }
                        let hex: String = chars[index..index+count].iter().collect(); index += count; hex
                    };
                    let scalar = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                        .ok_or_else(|| "Invalid Unicode scalar escape; UTF-16 surrogate escapes are unsupported".to_string())?;
                    if class_depth > 0 && flags.insensitive && !flags.unicode_case
                        && chars.get(index) == Some(&'-') {
                        return Err("Escaped ranges in ASCII (?i) classes are unsupported; use an ordinary range such as [a-z]".into());
                    }
                    literal(&mut out, scalar, flags);
                }
                'p' | 'P' => {
                    if chars.get(index) != Some(&'{') { return Err("Unicode property requires braces".into()); }
                    index += 1;
                    let start = index;
                    while chars.get(index).is_some_and(|ch| *ch != '}') { index += 1; }
                    if chars.get(index) != Some(&'}') { return Err("Unclosed Unicode property".into()); }
                    let name: String = chars[start..index].iter().collect(); index += 1;
                    if name.starts_with("java") { return Err(format!("Java Character property {name} is unsupported; use a Unicode category such as \\p{{L}}")); }
                    let ascii = match name.as_str() {
                        "Lower" => Some(if flags.insensitive { "a-zA-Z" } else { "a-z" }),
                        "Upper" => Some(if flags.insensitive { "a-zA-Z" } else { "A-Z" }),
                        "ASCII" => Some(r"\x00-\x7F"), "Alpha" => Some("a-zA-Z"), "Digit" => Some("0-9"),
                        "Alnum" => Some("a-zA-Z0-9"), "Blank" => Some(r" \t"), "Cntrl" => Some(r"\x00-\x1F\x7F"),
                        "XDigit" => Some("0-9a-fA-F"), "Space" => Some(r" \t\n\x0B\x0C\r"),
                        "Punct" => Some(r"\x21-\x2F\x3A-\x40\x5B-\x60\x7B-\x7E"),
                        "Graph" => Some(r"\x21-\x7E"), "Print" => Some(r"\x20-\x7E"), _ => None,
                    };
                    if let Some(body) = ascii.filter(|_| !flags.unicode_class || name == "ASCII") {
                        out.push_str(&format!("[{}{body}]", if escaped == 'P' { "^" } else { "" }));
                    } else { out.push_str(&format!("\\{escaped}{{{name}}}")); }
                }
                'Z' if class_depth == 0 => out.push_str(end_assertion(Flags { multiline: false, ..flags })),
                'G' | 'X' | 'c' | '0' => return Err(format!("Java escape \\{escaped} is unsupported; use explicit scalar/classes or ordinary anchors")),
                '1'..='9' | 'k' if flags.insensitive && !flags.unicode_case => return Err("ASCII case-insensitive backreferences are unsupported; enable Unicode case with (?iu)".into()),
                _ => { out.push('\\'); out.push(escaped); }
            }
            continue;
        }
        match ch {
            '[' => {
                class_depth += 1;
                out.push(ch);
            }
            ']' => {
                class_depth = class_depth.saturating_sub(1);
                out.push(ch);
            }
            '(' if class_depth == 0 => {
                let mut end = index + 1;
                if chars.get(index) == Some(&'?') {
                    while chars
                        .get(end)
                        .is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '-')
                    {
                        end += 1;
                    }
                    if end > index + 1 && matches!(chars.get(end), Some(':' | ')')) {
                        let scoped = chars[end] == ':';
                        let previous = flags;
                        let mut enable = true;
                        for flag in &chars[index + 1..end] {
                            match flag {
                                '-' => enable = false,
                                'i' => flags.insensitive = enable,
                                'u' => flags.unicode_case = enable,
                                'U' => {
                                    flags.unicode_class = enable;
                                    if enable {
                                        flags.unicode_case = true;
                                    }
                                }
                                'm' => flags.multiline = enable,
                                's' => flags.dotall = enable,
                                'd' => flags.unix = enable,
                                _ => {
                                    return Err(format!(
                                        "Java flag {flag} is unsupported; use explicit whitespace/classes instead"
                                    ));
                                }
                            }
                        }
                        if scoped {
                            stack.push(previous);
                        }
                        out.push_str(if flags.insensitive && flags.unicode_case {
                            "(?i"
                        } else {
                            "(?-i"
                        });
                        out.push(if scoped { ':' } else { ')' });
                        index = end + 1;
                        continue;
                    }
                }
                stack.push(flags);
                out.push(ch);
            }
            ')' if class_depth == 0 => {
                flags = stack.pop().unwrap_or_default();
                out.push(ch);
            }
            '.' if class_depth == 0 && !flags.dotall => out.push_str(if flags.unix {
                r"[^\n]"
            } else {
                r"[^\n\r\x{85}\x{2028}\x{2029}]"
            }),
            '.' if class_depth == 0 => out.push_str(r"[\s\S]"),
            '^' if class_depth == 0 && flags.multiline => out.push_str(if flags.unix {
                r"(?:\A|(?<=\n))"
            } else {
                r"(?:\A|(?<=[\n\x{85}\x{2028}\x{2029}])|(?<=\r)(?!\n))"
            }),
            '$' if class_depth == 0 => out.push_str(end_assertion(flags)),
            ch if flags.insensitive && !flags.unicode_case && ch.is_ascii_alphabetic() => {
                // Do not rewrite names/lookaround syntax immediately after (? or
                // inside a named group. Those are grammar, not matched literals.
                if class_depth == 0 && (out.ends_with("(?") || out.ends_with("(?<")) {
                    return Err("Use named groups/backreferences outside ASCII (?i) scopes, or enable Unicode case (?iu)".into());
                }
                if class_depth > 0
                    && chars.get(index) == Some(&'-')
                    && chars.get(index + 1).is_some_and(char::is_ascii)
                {
                    let last = chars[index + 1];
                    if ch > last {
                        return Err("Reversed character range".into());
                    }
                    out.push('[');
                    for value in ch as u8..=last as u8 {
                        literal(&mut out, value as char, flags);
                    }
                    out.push(']');
                    index += 2;
                } else {
                    literal(&mut out, ch, flags);
                }
            }
            _ => out.push(ch),
        }
    }
    Ok(out)
}

/// fancy-regex cannot disable Unicode mode inline. Expand only the generated
/// ASCII boundary assertions when a pattern requires its look-around engine.
pub(super) fn backtracking(pattern: &str) -> String {
    pattern
        .replace(
            r"(?-u:\b)",
            r"(?:(?<=[A-Za-z0-9_])(?![A-Za-z0-9_])|(?<![A-Za-z0-9_])(?=[A-Za-z0-9_]))",
        )
        .replace(
            r"(?-u:\B)",
            r"(?:(?<=[A-Za-z0-9_])(?=[A-Za-z0-9_])|(?<![A-Za-z0-9_])(?![A-Za-z0-9_]))",
        )
}
