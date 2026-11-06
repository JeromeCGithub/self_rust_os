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
