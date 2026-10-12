//! Application search rank. Name matches beat generic-name and keyword
//! matches; within each, an exact match beats a prefix, a prefix beats a word
//! prefix, and a word prefix beats a substring.

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

fn field(text: &str, query: &str, base: u32) -> Option<u32> {
    let text = text.to_lowercase();
    if text == query {
        Some(base + 300)
    } else if text.starts_with(query) {
        // Shorter names first: "Files" before "Files Backup" for "fil".
        let extra = u32::try_from(text.len() - query.len())
            .unwrap_or(u32::MAX)
            .min(99);
        Some(base + 200 - extra)
    } else if words(&text).any(|w| w.starts_with(query)) {
        Some(base + 100)
    } else if text.contains(query) {
        Some(base)
    } else {
        None
    }
}

pub fn score(
    name: &str,
    generic_name: Option<&str>,
    keywords: &[String],
    id: &str,
    query: &str,
) -> Option<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let id = id.trim_end_matches(".desktop");
    [
        field(name, &query, 1000),
        generic_name.and_then(|g| field(g, &query, 500)),
        keywords.iter().filter_map(|k| field(k, &query, 400)).max(),
        field(id, &query, 100),
    ]
    .into_iter()
    .flatten()
    .max()
}

#[cfg(test)]
mod tests {
    use super::score;

    fn s(name: &str, generic: Option<&str>, keywords: &[&str], query: &str) -> Option<u32> {
        let keywords: Vec<String> = keywords.iter().map(|k| k.to_string()).collect();
        score(name, generic, &keywords, "x.desktop", query)
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(s("Files", None, &[], "  "), Some(0));
    }

    #[test]
    fn exact_beats_prefix_beats_word_beats_substring() {
        let exact = s("Files", None, &[], "files").unwrap();
        let prefix = s("Firefox", None, &[], "fi").unwrap();
        let word = s("GNOME Files", None, &[], "fil").unwrap();
        let sub = s("Profiler", None, &[], "fil").unwrap();
        assert!(exact > prefix && prefix > word && word > sub);
    }

    #[test]
    fn shorter_prefix_match_first() {
        assert!(s("Files", None, &[], "fil") > s("Files Backup", None, &[], "fil"));
    }

    #[test]
    fn name_beats_keyword() {
        let by_name = s("Terminal", None, &[], "term").unwrap();
        let by_keyword = s("Foot", Some("Terminal"), &["shell", "terminal"], "term").unwrap();
        assert!(by_name > by_keyword);
        assert!(s("Foot", Some("Terminal"), &[], "term").is_some());
        assert!(s("Foot", None, &["shell"], "she").is_some());
    }

    #[test]
    fn no_match() {
        assert_eq!(s("Files", Some("File Manager"), &["folder"], "zzz"), None);
    }
}
