//! URL editing belongs to the trusted controller, independently of page loading.
use crate::TextInput;

/// Commit a destination when a load completes.
pub fn commit_address(address: &mut TextInput, editing: bool, effective_url: &str) {
    if !editing {
        address.set(effective_url);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_late_redirect_cannot_replace_the_selected_or_partially_typed_address() {
        let mut draft = TextInput::new("http://fixture/start");
        draft.select_all();
        commit_address(&mut draft, true, "http://fixture/redirected");
        assert!(draft.is_selected());
        draft.byte(b'n');
        assert_eq!(
            draft.as_str(),
            "n",
            "selection survives a background completion"
        );
        commit_address(&mut draft, true, "http://fixture/another");
        draft.byte(b'e');
        assert_eq!(
            draft.as_str(),
            "ne",
            "draft survives a background completion"
        );
        commit_address(&mut draft, false, "http://fixture/committed");
        assert_eq!(draft.as_str(), "http://fixture/committed");
    }
}

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[derive(Clone, Copy)]
pub enum Mode {
    Push,
    Back,
    Forward,
    Reload,
}
#[derive(Default)]
pub struct History {
    back: Vec<String>,
    forward: Vec<String>,
}
impl History {
    pub fn back(&self) -> Option<&str> {
        self.back.last().map(String::as_str)
    }
    pub fn forward(&self) -> Option<&str> {
        self.forward.last().map(String::as_str)
    }
    pub fn commit(&mut self, old: &str, new: &str, mode: Mode) {
        if old == new {
            return;
        }
        match mode {
            Mode::Push => {
                if !old.is_empty() {
                    self.back.push(old.to_string());
                }
                self.forward.clear();
            }
            Mode::Back => {
                self.back.pop();
                if !old.is_empty() {
                    self.forward.push(old.to_string());
                }
            }
            Mode::Forward => {
                self.forward.pop();
                if !old.is_empty() {
                    self.back.push(old.to_string());
                }
            }
            Mode::Reload => {}
        }
        if self.back.len() > 32 {
            self.back.remove(0);
        }
        if self.forward.len() > 32 {
            self.forward.remove(0);
        }
    }
}
#[cfg(test)]
mod history_tests {
    use super::*;
    #[test]
    fn success_commits_history_but_failures_and_reload_preserve_forward_navigation() {
        let mut history = History::default();
        history.commit("", "a", Mode::Push);
        history.commit("a", "b", Mode::Push);
        assert_eq!(history.back(), Some("a"));
        assert_eq!(history.forward(), None);
        history.commit("b", "a", Mode::Back);
        assert_eq!(history.forward(), Some("b"));
        history.commit("a", "a", Mode::Reload);
        assert_eq!(history.forward(), Some("b"));
        // A failed or cancelled request never calls commit.
        assert_eq!(history.forward(), Some("b"));
        history.commit("a", "b", Mode::Forward);
        assert_eq!(history.back(), Some("a"));
        history.commit("b", "c", Mode::Push);
        assert_eq!(history.forward(), None);
    }
}
