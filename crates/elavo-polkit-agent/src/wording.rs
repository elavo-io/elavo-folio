//! What the card says.

pub const BODY: &str = "Enter your password to continue.";

/// The title names the application when the request can be traced to one.
/// Otherwise polkit's message for the action is the title, as a sentence.
pub fn title(message: &str, app: Option<&str>) -> String {
    if let Some(app) = app.map(str::trim).filter(|a| !a.is_empty()) {
        return format!("{app} is trying to modify your system settings.");
    }
    let message = message.trim();
    if message.is_empty() {
        "An application is trying to modify your system settings.".to_owned()
    } else if message.ends_with(['.', '!', '?']) {
        message.to_owned()
    } else {
        format!("{message}.")
    }
}

/// The helper escapes what PAM says the way `g_strescape` does.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some(d @ '0'..='7') => {
                let mut value = d.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(next) => {
                            value = value * 8 + next;
                            chars.next();
                        }
                        None => break,
                    }
                }
                if let Some(c) = char::from_u32(value) {
                    out.push(c);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_the_title() {
        assert_eq!(
            title("Authentication is required", Some("System Settings")),
            "System Settings is trying to modify your system settings."
        );
    }

    #[test]
    fn message_becomes_a_sentence() {
        assert_eq!(
            title("Authentication is required to change the hostname", None),
            "Authentication is required to change the hostname."
        );
        assert_eq!(title("Run this?", None), "Run this?");
        assert_eq!(
            title("  ", None),
            "An application is trying to modify your system settings."
        );
    }

    #[test]
    fn unescapes_like_strcompress() {
        assert_eq!(unescape("Password: "), "Password: ");
        assert_eq!(unescape("a\\nb\\\\c\\\"d"), "a\nb\\c\"d");
        assert_eq!(unescape("\\303\\251"), "\u{c3}\u{a9}");
    }
}
