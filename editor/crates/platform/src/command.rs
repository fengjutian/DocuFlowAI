//! The box that asks for a command.
//!
//! The task runner knows how to build the project on its own; this box is
//! for the commands the editor does not know: the ones a person types.
//!
//! The commands that were run are kept, so that the last one can be run
//! again without typing it a second time.

use crate::rename::RenameBox;

/// The box that asks for a command.
pub struct CommandBox {
    /// The command that is being typed.
    text: RenameBox,
    /// Which of the commands in the history the box is on.
    ///
    /// Nothing means that what is in the box is not from the history.
    at: Option<usize>,
}

impl CommandBox {
    /// Opens the box, ready to take a command.
    pub fn new() -> Self {
        Self::titled("运行命令")
    }

    /// Opens the box with a title of its own.
    ///
    /// The terminal asks for a line of its own, and says so.
    pub fn titled(title: &str) -> Self {
        CommandBox {
            text: RenameBox::message(title),
            at: None,
        }
    }

    /// The command as it is now.
    pub fn text(&self) -> &str {
        self.text.text()
    }

    /// The text of the box, for the window to draw.
    pub fn field(&self) -> &RenameBox {
        &self.text
    }

    /// Puts a command into the box, the way the history does.
    pub fn set_text(&mut self, text: &str) {
        self.text.set_text(text);
    }

    /// Adds text at the caret of the command.
    ///
    /// The text that an input method composes goes into the box, not into
    /// the document behind it.
    pub fn type_text(&mut self, text: &str) {
        self.text.type_text(text);
    }

    /// Gives a key to the text of the command.
    ///
    /// Returns whether the key was one of the box's.
    pub fn handle_key<T: AsRef<str>>(
        &mut self,
        key: &winit::keyboard::Key<T>,
        text: Option<&str>,
        control: bool,
    ) -> bool {
        self.text.handle_key(key, text, control)
    }

    /// The command that was before this one in the history.
    ///
    /// The oldest one is the first that is asked for, and asking for it
    /// again stays there.
    pub fn previous(&mut self, history: &[String]) -> Option<String> {
        if history.is_empty() {
            return None;
        }
        let at = match self.at {
            Some(at) => at.saturating_sub(1),
            None => history.len() - 1,
        };
        self.at = Some(at);
        Some(history[at].clone())
    }

    /// The command that was after this one in the history.
    ///
    /// Past the newest one the box is empty again: what is being typed is
    /// a new command.
    pub fn next(&mut self, history: &[String]) -> Option<String> {
        let at = self.at?;
        let next = at + 1;
        if next >= history.len() {
            self.at = None;
            return Some(String::new());
        }
        self.at = Some(next);
        Some(history[next].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_starts_empty_and_says_what_it_asks_for() {
        let box_ = CommandBox::new();
        assert_eq!(box_.text(), "");
        assert_eq!(box_.field().title(), "运行命令");
    }

    #[test]
    fn the_history_is_walked_back_and_forth() {
        let history = vec!["cargo build".to_owned(), "npm test".to_owned()];
        let mut box_ = CommandBox::new();

        // The newest one comes first.
        assert_eq!(box_.previous(&history).as_deref(), Some("npm test"));
        assert_eq!(box_.previous(&history).as_deref(), Some("cargo build"));
        // The oldest one stays there.
        assert_eq!(box_.previous(&history).as_deref(), Some("cargo build"));

        assert_eq!(box_.next(&history).as_deref(), Some("npm test"));
        // Past the newest one the box is empty again.
        assert_eq!(box_.next(&history).as_deref(), Some(""));
    }

    #[test]
    fn there_is_no_history_to_walk() {
        let mut box_ = CommandBox::new();
        assert_eq!(box_.previous(&[]), None);
        assert_eq!(box_.next(&[]), None);
    }
}
