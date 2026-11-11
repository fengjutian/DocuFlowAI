use std::ops::Range;

/// Platform-independent text input state. A future editor view will apply
/// committed text through a Buffer transaction and display preedit separately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInputState {
    committed: String,
    preedit: String,
    preedit_selection: Option<Range<usize>>,
    ime_enabled: bool,
}

impl TextInputState {
    pub fn enable_ime(&mut self, enabled: bool) {
        self.ime_enabled = enabled;
        if !enabled {
            self.clear_preedit();
        }
    }

    pub fn set_preedit(&mut self, text: String, selection: Option<(usize, usize)>) {
        self.preedit_selection = selection.map(|(start, end)| start..end);
        self.preedit = text;
    }

    pub fn commit(&mut self, text: &str) {
        self.committed.push_str(text);
        self.clear_preedit();
    }

    pub fn delete_previous_scalar(&mut self) {
        self.committed.pop();
    }

    pub fn preedit(&self) -> &str {
        &self.preedit
    }
    pub fn ime_enabled(&self) -> bool {
        self.ime_enabled
    }

    pub fn display_text(&self) -> String {
        format!("{}{}", self.committed, self.preedit)
    }

    fn clear_preedit(&mut self) {
        self.preedit.clear();
        self.preedit_selection = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_is_replaced_by_commit() {
        let mut input = TextInputState::default();
        input.enable_ime(true);
        input.set_preedit("zhong wen".into(), Some((0, 9)));
        assert_eq!(input.display_text(), "zhong wen");
        input.commit("中文");
        assert_eq!(input.display_text(), "中文");
        assert!(input.preedit().is_empty());
    }

    #[test]
    fn backspace_deletes_a_unicode_scalar() {
        let mut input = TextInputState::default();
        input.commit("A中");
        input.delete_previous_scalar();
        assert_eq!(input.display_text(), "A");
    }
}
