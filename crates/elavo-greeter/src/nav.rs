//! Keyboard moves on the login user list: which row is lit, and what
//! Enter and Escape do. The screen drives the widgets from these answers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKey {
    Up,
    Down,
    Enter,
    Escape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListAction {
    /// Light this row. The focus index already moved there.
    Focus(usize),
    /// Choose this account and show the password.
    Select(usize),
    /// Leave the password and show the list again.
    Back,
    /// The key is ours but changes nothing (Escape with one account).
    Stay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListNav {
    pub count: usize,
    pub focus: usize,
    /// Password is on screen for the chosen person.
    pub chosen: bool,
    /// More than one account: Escape may return to the list.
    pub can_leave: bool,
}

/// What the screen should do with this key. `None` means leave the key for
/// the focused widget (the password field's Enter, for example).
pub fn handle(nav: ListNav, key: ListKey) -> Option<(usize, ListAction)> {
    if nav.count == 0 {
        return Some((0, ListAction::Stay));
    }
    let focus = nav.focus.min(nav.count - 1);

    if nav.chosen {
        return match key {
            ListKey::Escape if nav.can_leave => Some((focus, ListAction::Back)),
            ListKey::Escape => Some((focus, ListAction::Stay)),
            // Enter submits the password; arrows stay in the field.
            ListKey::Up | ListKey::Down | ListKey::Enter => None,
        };
    }

    Some(match key {
        ListKey::Up => {
            let next = focus.saturating_sub(1);
            (next, ListAction::Focus(next))
        }
        ListKey::Down => {
            let next = (focus + 1).min(nav.count - 1);
            (next, ListAction::Focus(next))
        }
        ListKey::Enter => (focus, ListAction::Select(focus)),
        ListKey::Escape => (focus, ListAction::Stay),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(count: usize, focus: usize) -> ListNav {
        ListNav {
            count,
            focus,
            chosen: false,
            can_leave: count > 1,
        }
    }

    fn password(count: usize, focus: usize) -> ListNav {
        ListNav {
            count,
            focus,
            chosen: true,
            can_leave: count > 1,
        }
    }

    #[test]
    fn enter_selects_the_focused_row() {
        assert_eq!(
            handle(list(3, 0), ListKey::Enter),
            Some((0, ListAction::Select(0)))
        );
        assert_eq!(
            handle(list(3, 2), ListKey::Enter),
            Some((2, ListAction::Select(2)))
        );
    }

    #[test]
    fn arrows_move_focus_and_stop_at_the_ends() {
        assert_eq!(
            handle(list(3, 0), ListKey::Up),
            Some((0, ListAction::Focus(0)))
        );
        assert_eq!(
            handle(list(3, 0), ListKey::Down),
            Some((1, ListAction::Focus(1)))
        );
        assert_eq!(
            handle(list(3, 2), ListKey::Down),
            Some((2, ListAction::Focus(2)))
        );
        assert_eq!(
            handle(list(3, 2), ListKey::Up),
            Some((1, ListAction::Focus(1)))
        );
    }

    #[test]
    fn escape_returns_to_the_list_only_with_more_than_one_account() {
        assert_eq!(
            handle(password(2, 1), ListKey::Escape),
            Some((1, ListAction::Back))
        );
        assert_eq!(
            handle(password(1, 0), ListKey::Escape),
            Some((0, ListAction::Stay))
        );
    }

    #[test]
    fn password_keeps_enter_and_arrows() {
        assert_eq!(handle(password(3, 1), ListKey::Down), None);
        assert_eq!(handle(password(3, 1), ListKey::Enter), None);
    }

    #[test]
    fn empty_list_stays_put() {
        assert_eq!(
            handle(
                ListNav {
                    count: 0,
                    focus: 0,
                    chosen: false,
                    can_leave: false,
                },
                ListKey::Enter
            ),
            Some((0, ListAction::Stay))
        );
    }
}
