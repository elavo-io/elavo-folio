//! Match a foreign-toplevel `app_id` to a desktop-file ID.
//!
//! Order: exact desktop ID, the same ignoring case, `StartupWMClass`, then the
//! last dot-separated part of a reverse-DNS ID (`firefox` finds
//! `org.mozilla.firefox.desktop`, `nautilus` finds `org.gnome.Nautilus.desktop`).

pub struct Candidate<'a> {
    pub id: &'a str,
    pub wm_class: Option<&'a str>,
}

fn stem(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

pub fn match_app_id<'a>(app_id: &str, candidates: &[Candidate<'a>]) -> Option<&'a str> {
    if app_id.is_empty() {
        return None;
    }
    let app_id = app_id.strip_suffix(".desktop").unwrap_or(app_id);

    let passes: [&dyn Fn(&Candidate) -> bool; 4] = [
        &|c| stem(c.id) == app_id,
        &|c| stem(c.id).eq_ignore_ascii_case(app_id),
        &|c| c.wm_class.is_some_and(|w| w.eq_ignore_ascii_case(app_id)),
        &|c| {
            stem(c.id)
                .rsplit('.')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case(app_id))
        },
    ];
    passes
        .iter()
        .find_map(|pass| candidates.iter().find(|c| pass(c)).map(|c| c.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<Candidate<'static>> {
        vec![
            Candidate {
                id: "foot.desktop",
                wm_class: None,
            },
            Candidate {
                id: "footclient.desktop",
                wm_class: None,
            },
            Candidate {
                id: "org.gnome.Nautilus.desktop",
                wm_class: None,
            },
            Candidate {
                id: "org.mozilla.firefox.desktop",
                wm_class: Some("firefox"),
            },
            Candidate {
                id: "com.example.Wrapped.desktop",
                wm_class: Some("wrapped-appimage"),
            },
        ]
    }

    #[test]
    fn exact_id() {
        assert_eq!(match_app_id("foot", &candidates()), Some("foot.desktop"));
        assert_eq!(
            match_app_id("org.gnome.Nautilus", &candidates()),
            Some("org.gnome.Nautilus.desktop")
        );
    }

    #[test]
    fn flatpak_firefox() {
        assert_eq!(
            match_app_id("org.mozilla.firefox", &candidates()),
            Some("org.mozilla.firefox.desktop")
        );
    }

    #[test]
    fn wm_class_before_last_component() {
        assert_eq!(
            match_app_id("firefox", &candidates()),
            Some("org.mozilla.firefox.desktop")
        );
        assert_eq!(
            match_app_id("wrapped-appimage", &candidates()),
            Some("com.example.Wrapped.desktop")
        );
    }

    #[test]
    fn last_component_ignores_case() {
        assert_eq!(
            match_app_id("nautilus", &candidates()),
            Some("org.gnome.Nautilus.desktop")
        );
    }

    #[test]
    fn no_match() {
        assert_eq!(match_app_id("", &candidates()), None);
        assert_eq!(match_app_id("unknown-thing", &candidates()), None);
    }
}
