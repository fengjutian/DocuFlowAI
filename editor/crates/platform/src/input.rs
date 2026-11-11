use std::ops::Range;

/// Transient input-method state. Committed text belongs to editor-core's
/// Buffer; only an active IME composition remains here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInputState {
    preedit: String,
    preedit_selection: Option<Range<usize>>,
    ime_enabled: bool,
}

impl TextInputState {
    pub fn enable_ime(&mut self, enabled: bool) {
        self.ime_enabled = enabled;
        if !enabled {
            self.finish_commit();
        }
    }

    pub fn set_preedit(&mut self, text: String, selection: Option<(usize, usize)>) {
        self.preedit_selection = selection.map(|(start, end)| start..end);
        self.preedit = text;
    }

    pub fn finish_commit(&mut self) {
        self.preedit.clear();
        self.preedit_selection = None;
    }

    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    pub fn ime_enabled(&self) -> bool {
        self.ime_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_clears_transient_composition() {
        let mut input = TextInputState::default();
        input.enable_ime(true);
        input.set_preedit("zhong wen".into(), Some((0, 9)));
        input.finish_commit();
        assert!(input.preedit().is_empty());
    }

    #[test]
    fn disabling_ime_cancels_preedit() {
        let mut input = TextInputState::default();
        input.set_preedit("中文".into(), None);
        input.enable_ime(false);
        assert!(input.preedit().is_empty());
    }
}
