//! Whether a key becomes a period. The Wayland code only feeds this.

/// Two spaces at least this far apart stay two spaces.
pub const WINDOW_MS: u32 = 1100;

/// Evdev codes. The grab and the virtual keyboard both use these, not XKB codes.
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_SPACE: u32 = 57;

/// `zwp_text_input_v3.content_purpose`. These fields never get the substitution.
pub const PURPOSE_PASSWORD: u32 = 8;
pub const PURPOSE_PIN: u32 = 9;
pub const PURPOSE_TERMINAL: u32 = 13;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send the key on the virtual keyboard.
    Forward,
    /// Do not send the key. Delete `before` bytes before the cursor and commit `text`.
    Replace { before: u32, text: &'static str },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Timestamp of the last space this field forwarded.
    pub last_space_at: Option<u32>,
    /// The previous edit was a substitution, so Backspace undoes just that.
    pub last_was_substitution: bool,
    /// The previous edit was that undo, so the next space stays a space.
    pub suppress_space: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub action: Action,
    pub memory: Memory,
    /// The caller inserts a space into its copy of the field. The client may
    /// not echo that space before the next one arrives.
    pub note_space: bool,
}

pub struct Input<'a> {
    pub enabled: bool,
    pub active: bool,
    pub purpose: u32,
    /// Shift, Ctrl, Alt, or a latched modifier is down. Caps Lock is not.
    pub modifiers: bool,
    pub text: &'a str,
    pub cursor: u32,
    pub anchor: u32,
    /// The client has sent surrounding text for this field.
    pub known: bool,
    pub memory: &'a Memory,
    /// The key event's timestamp, same clock as `memory.last_space_at`.
    pub now: u32,
}

pub fn decide(input: &Input<'_>, key: u32, pressed: bool) -> Outcome {
    if !pressed {
        return Outcome {
            action: Action::Forward,
            memory: input.memory.clone(),
            note_space: false,
        };
    }

    if input.active && input.enabled && key == KEY_SPACE {
        if should_substitute(input) {
            let mut memory = input.memory.clone();
            memory.last_was_substitution = true;
            memory.suppress_space = false;
            return Outcome {
                action: Action::Replace {
                    before: 1,
                    text: ". ",
                },
                memory,
                note_space: false,
            };
        }
        let mut memory = input.memory.clone();
        memory.last_was_substitution = false;
        memory.suppress_space = false;
        memory.last_space_at = Some(input.now);
        return Outcome {
            action: Action::Forward,
            memory,
            note_space: true,
        };
    }

    if input.active && key == KEY_BACKSPACE && input.memory.last_was_substitution && input.known {
        let mut memory = input.memory.clone();
        memory.last_was_substitution = false;
        memory.suppress_space = true;
        return Outcome {
            action: Action::Replace {
                before: 2,
                text: " ",
            },
            memory,
            note_space: false,
        };
    }

    let mut memory = input.memory.clone();
    memory.last_was_substitution = false;
    memory.suppress_space = false;
    Outcome {
        action: Action::Forward,
        memory,
        note_space: false,
    }
}

fn should_substitute(input: &Input<'_>) -> bool {
    if !input.known
        || input.modifiers
        || input.memory.suppress_space
        || input.cursor != input.anchor
    {
        return false;
    }
    if blocked_purpose(input.purpose) {
        return false;
    }
    let Some(then) = input.memory.last_space_at else {
        return false;
    };
    if input.now.wrapping_sub(then) >= WINDOW_MS {
        return false;
    }
    ends_with_alnum_space(input.text, input.cursor)
}

fn blocked_purpose(purpose: u32) -> bool {
    matches!(purpose, PURPOSE_PASSWORD | PURPOSE_PIN | PURPOSE_TERMINAL)
}

fn ends_with_alnum_space(text: &str, cursor: u32) -> bool {
    let Some(before) = before_cursor(text, cursor) else {
        return false;
    };
    let mut chars = before.chars();
    let Some(' ') = chars.next_back() else {
        return false;
    };
    chars.next_back().is_some_and(char::is_alphanumeric)
}

/// The substitution is still what sits at the cursor, so Backspace may undo it.
pub fn substitution_still_there(text: &str, cursor: u32, anchor: u32) -> bool {
    cursor == anchor && before_cursor(text, cursor).is_some_and(|s| s.ends_with(". "))
}

fn before_cursor(text: &str, cursor: u32) -> Option<&str> {
    let cursor = usize::try_from(cursor).ok()?;
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    Some(&text[..cursor])
}

/// The field's surrounding text, plus the edit this program just made and has
/// not heard back yet. A stale echo of the earlier text is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
    pub current: Snapshot,
    pub known: bool,
    pending: Option<(Snapshot, Snapshot)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub cursor: u32,
    pub anchor: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    /// The client has not caught up with the edit we just sent.
    Stale,
    /// The client confirmed that edit.
    CaughtUp,
    /// This is the client's text, and it was not one of ours.
    Applied,
}

impl Field {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn apply_remote(&mut self, snap: Snapshot, known: bool) -> Echo {
        if let Some((before, after)) = &self.pending {
            if snap == *after {
                self.pending = None;
                self.known = true;
                return Echo::CaughtUp;
            }
            // The client can report the text from before the commit, or a
            // prefix of it, before it reports the result. Neither is the new
            // text.
            if snap == *before || after.text.starts_with(&snap.text) {
                return Echo::Stale;
            }
            self.pending = None;
        }
        self.current = snap;
        self.known = known;
        Echo::Applied
    }

    /// Record a space we forwarded, so a second space can see it immediately.
    pub fn note_space(&mut self) {
        self.splice(0, " ");
    }

    pub fn replace(&mut self, before: u32, insert: &str) {
        self.splice(before, insert);
    }

    fn splice(&mut self, before: u32, insert: &str) {
        if !self.known || self.current.cursor != self.current.anchor {
            return;
        }
        let cursor = self.current.cursor as usize;
        let before = before as usize;
        if cursor < before || cursor > self.current.text.len() {
            return;
        }
        let start = cursor - before;
        if !self.current.text.is_char_boundary(start) || !self.current.text.is_char_boundary(cursor)
        {
            return;
        }
        let prior = self.current.clone();
        self.current.text.replace_range(start..cursor, insert);
        self.current.cursor = (start + insert.len()) as u32;
        self.current.anchor = self.current.cursor;
        self.pending = Some((prior, self.current.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> Field {
        Field {
            current: Snapshot {
                text: text.to_owned(),
                cursor: text.len() as u32,
                anchor: text.len() as u32,
            },
            known: true,
            pending: None,
        }
    }

    fn press(field: &Field, memory: &Memory, key: u32, now: u32) -> Outcome {
        decide(
            &Input {
                enabled: true,
                active: true,
                purpose: 0,
                modifiers: false,
                text: &field.current.text,
                cursor: field.current.cursor,
                anchor: field.current.anchor,
                known: field.known,
                memory,
                now,
            },
            key,
            true,
        )
    }

    fn apply(field: &mut Field, outcome: &Outcome) {
        match outcome.action {
            Action::Forward if outcome.note_space => field.note_space(),
            Action::Replace { before, text } => field.replace(before, text),
            Action::Forward => {}
        }
    }

    #[test]
    fn two_spaces_inside_the_window_become_a_period() {
        let mut text = field("hi");
        let mut memory = Memory::default();
        let first = press(&text, &memory, KEY_SPACE, 5_000);
        assert_eq!(first.action, Action::Forward);
        assert!(first.note_space);
        apply(&mut text, &first);
        memory = first.memory;
        assert_eq!(text.current.text, "hi ");

        // The client has not echoed the first space yet.
        assert_eq!(
            text.apply_remote(
                Snapshot {
                    text: "hi".to_owned(),
                    cursor: 2,
                    anchor: 2,
                },
                true,
            ),
            Echo::Stale,
        );
        assert_eq!(text.current.text, "hi ");

        let second = press(&text, &memory, KEY_SPACE, 5_000 + WINDOW_MS - 1);
        assert_eq!(
            second.action,
            Action::Replace {
                before: 1,
                text: ". "
            }
        );
        assert!(second.memory.last_was_substitution);
        apply(&mut text, &second);
        assert_eq!(text.current.text, "hi. ");
    }

    #[test]
    fn a_gap_of_1100_ms_stays_two_spaces() {
        let mut text = field("hi");
        let first = press(&text, &Memory::default(), KEY_SPACE, 100);
        apply(&mut text, &first);
        let second = press(&text, &first.memory, KEY_SPACE, 100 + WINDOW_MS);
        assert_eq!(second.action, Action::Forward);
        assert!(second.note_space);
    }

    #[test]
    fn backspace_undoes_only_the_period_and_the_next_space_stays() {
        let mut text = field("hi ");
        let mut memory = Memory {
            last_space_at: Some(10),
            ..Memory::default()
        };
        let period = press(&text, &memory, KEY_SPACE, 20);
        apply(&mut text, &period);
        memory = period.memory;
        assert_eq!(text.current.text, "hi. ");

        let undo = press(&text, &memory, KEY_BACKSPACE, 30);
        assert_eq!(
            undo.action,
            Action::Replace {
                before: 2,
                text: " "
            }
        );
        assert!(undo.memory.suppress_space);
        assert!(!undo.memory.last_was_substitution);
        apply(&mut text, &undo);
        memory = undo.memory;
        assert_eq!(text.current.text, "hi ");

        let again = press(&text, &memory, KEY_SPACE, 40);
        assert_eq!(again.action, Action::Forward);
        apply(&mut text, &again);
        assert_eq!(text.current.text, "hi  ");
    }

    #[test]
    fn another_key_cancels_the_undo() {
        let text = field("hi. ");
        let memory = Memory {
            last_was_substitution: true,
            last_space_at: Some(1),
            ..Memory::default()
        };
        let letter = press(&text, &memory, 30, 50);
        assert_eq!(letter.action, Action::Forward);
        assert!(!letter.memory.last_was_substitution);
        let back = press(&text, &letter.memory, KEY_BACKSPACE, 60);
        assert_eq!(back.action, Action::Forward);
    }

    #[test]
    fn a_release_does_not_cancel_the_undo() {
        let text = field("hi. ");
        let memory = Memory {
            last_was_substitution: true,
            ..Memory::default()
        };
        let release = decide(
            &Input {
                enabled: true,
                active: true,
                purpose: 0,
                modifiers: false,
                text: &text.current.text,
                cursor: text.current.cursor,
                anchor: text.current.anchor,
                known: true,
                memory: &memory,
                now: 5,
            },
            KEY_SPACE,
            false,
        );
        assert!(release.memory.last_was_substitution);
        let undo = press(&text, &release.memory, KEY_BACKSPACE, 6);
        assert!(matches!(undo.action, Action::Replace { .. }));
    }

    #[test]
    fn password_pin_terminal_and_a_modifier_stay_spaces() {
        let text = field("hi ");
        let memory = Memory {
            last_space_at: Some(0),
            ..Memory::default()
        };
        for purpose in [PURPOSE_PASSWORD, PURPOSE_PIN, PURPOSE_TERMINAL] {
            let outcome = decide(
                &Input {
                    enabled: true,
                    active: true,
                    purpose,
                    modifiers: false,
                    text: &text.current.text,
                    cursor: text.current.cursor,
                    anchor: text.current.anchor,
                    known: true,
                    memory: &memory,
                    now: 10,
                },
                KEY_SPACE,
                true,
            );
            assert_eq!(outcome.action, Action::Forward, "purpose {purpose}");
        }
        let shifted = decide(
            &Input {
                enabled: true,
                active: true,
                purpose: 0,
                modifiers: true,
                text: &text.current.text,
                cursor: text.current.cursor,
                anchor: text.current.anchor,
                known: true,
                memory: &memory,
                now: 10,
            },
            KEY_SPACE,
            true,
        );
        assert_eq!(shifted.action, Action::Forward);
    }

    #[test]
    fn off_unknown_selection_and_a_non_word_do_not_substitute() {
        let memory = Memory {
            last_space_at: Some(0),
            ..Memory::default()
        };
        let cases = [
            ("hi ", false, true, 3, 3),
            ("hi ", true, false, 3, 3),
            ("hi ", true, true, 2, 3),
            ("hi", true, true, 2, 2),
            ("hi  ", true, true, 4, 4),
            ("hi, ", true, true, 4, 4),
            (" ", true, true, 1, 1),
        ];
        for (text, enabled, known, cursor, anchor) in cases {
            let outcome = decide(
                &Input {
                    enabled,
                    active: true,
                    purpose: 0,
                    modifiers: false,
                    text,
                    cursor,
                    anchor,
                    known,
                    memory: &memory,
                    now: 10,
                },
                KEY_SPACE,
                true,
            );
            assert_eq!(outcome.action, Action::Forward, "{text:?} known={known}");
        }
    }

    #[test]
    fn a_digit_and_a_non_ascii_letter_both_count() {
        for word in ["a1 ", "café "] {
            let text = field(word);
            let memory = Memory {
                last_space_at: Some(0),
                ..Memory::default()
            };
            let outcome = press(&text, &memory, KEY_SPACE, 10);
            assert_eq!(
                outcome.action,
                Action::Replace {
                    before: 1,
                    text: ". "
                },
                "{word:?}"
            );
        }
    }

    #[test]
    fn an_inactive_field_never_substitutes() {
        let text = field("hi ");
        let memory = Memory {
            last_space_at: Some(0),
            last_was_substitution: true,
            ..Memory::default()
        };
        let outcome = decide(
            &Input {
                enabled: true,
                active: false,
                purpose: 0,
                modifiers: false,
                text: &text.current.text,
                cursor: text.current.cursor,
                anchor: text.current.anchor,
                known: true,
                memory: &memory,
                now: 10,
            },
            KEY_SPACE,
            true,
        );
        assert_eq!(outcome.action, Action::Forward);
        assert!(!outcome.memory.last_was_substitution);
    }

    #[test]
    fn an_intermediate_echo_does_not_replace_the_period() {
        let mut text = field("hi ");
        text.replace(1, ". ");
        assert_eq!(text.current.text, "hi. ");
        assert_eq!(
            text.apply_remote(
                Snapshot {
                    text: "hi".to_owned(),
                    cursor: 2,
                    anchor: 2,
                },
                true,
            ),
            Echo::Stale,
        );
        assert_eq!(text.current.text, "hi. ");
        assert_eq!(
            text.apply_remote(
                Snapshot {
                    text: "hi. ".to_owned(),
                    cursor: 4,
                    anchor: 4,
                },
                true,
            ),
            Echo::CaughtUp,
        );
    }

    #[test]
    fn the_period_is_still_there_only_at_that_cursor() {
        assert!(substitution_still_there("hi. ", 4, 4));
        assert!(!substitution_still_there("hi. ", 2, 2));
        assert!(!substitution_still_there("hi. ", 4, 2));
        assert!(!substitution_still_there("hi ", 3, 3));
    }
}
