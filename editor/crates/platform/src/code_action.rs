//! The things the server offers to do about a place in the document.
//!
//! The list floats under the caret, the way the list of completions does.
//! Nothing is changed until the user has picked one of them: a list of
//! things that the editor does on its own is a list of surprises.

use codeweaver_lsp::protocol::CodeAction;
use codeweaver_render::{Color, FrameScene, Rect, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;

/// The width of the list.
const WIDTH: f32 = 440.0;
/// The height of one item of the list.
const ITEM_HEIGHT: f32 = 26.0;
/// How many items are shown at once.
const VISIBLE: usize = 8;
/// The padding around the items.
const PADDING: f32 = 6.0;

/// The things the server offers, and the one that is picked.
pub struct ActionPopup {
    actions: Vec<CodeAction>,
    /// Which one is picked, counted from the first one.
    selected: usize,
}

impl ActionPopup {
    /// Makes a list, or nothing when the server had nothing to offer.
    pub fn new(actions: Vec<CodeAction>) -> Option<Self> {
        if actions.is_empty() {
            return None;
        }
        Some(ActionPopup {
            actions,
            selected: 0,
        })
    }

    /// The things the server offers.
    pub fn actions(&self) -> &[CodeAction] {
        &self.actions
    }

    /// How many things the server offers.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Which one is picked, counted from the first one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The one that is picked.
    pub fn selected_action(&self) -> Option<&CodeAction> {
        self.actions.get(self.selected)
    }

    /// Picks the one after this one, going round to the first one.
    pub fn select_next(&mut self) {
        if self.actions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.actions.len();
    }

    /// Picks the one before this one, going round to the last one.
    pub fn select_previous(&mut self) {
        if self.actions.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.actions.len() - 1
        } else {
            self.selected - 1
        };
    }
}

/// Draws the list under the caret.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    body: Rect,
    popup: &ActionPopup,
    caret: (f32, f32),
    measurer: &mut dyn TextMeasurer,
) {
    let actions = popup.actions();
    let shown = actions.len().min(VISIBLE);
    let height = shown as f32 * ITEM_HEIGHT + PADDING * 2.0;
    let width = WIDTH.min(body.size.width.max(1.0));

    // The list goes under the caret, unless there is no room for it there.
    let below = body.origin.y + caret.1 + ITEM_HEIGHT;
    let y = if below + height <= body.max_y() {
        below
    } else {
        (body.origin.y + caret.1 - height).max(body.origin.y)
    };
    let x = (body.origin.x + caret.0).min(body.max_x() - width);
    let list = Rect::new(x.max(body.origin.x), y, width, height);

    scene.push_quad(list, Color::rgb(0.11, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(list.origin.x, list.origin.y, list.size.width, 1.0),
        Color::rgb(0.24, 0.27, 0.34),
        viewport,
    );

    let first = popup
        .selected()
        .saturating_sub(shown - 1)
        .min(actions.len() - 1);
    for (index, action) in actions.iter().enumerate().skip(first).take(shown) {
        let row = index - first;
        let y = list.origin.y + PADDING + row as f32 * ITEM_HEIGHT;
        if index == popup.selected() {
            scene.push_quad(
                Rect::new(list.origin.x, y, list.size.width, ITEM_HEIGHT),
                Color::rgb(0.18, 0.30, 0.52),
                viewport,
            );
        }
        // The kind of the action is in a quieter colour, so that the title
        // is what the eye goes to.
        scene.push_text(
            action.title.clone(),
            Rect::new(
                list.origin.x + 8.0,
                y + 3.0,
                (list.size.width - 16.0).max(1.0),
                ITEM_HEIGHT,
            ),
            TextStyle {
                clip: list,
                color: Color::rgb(0.88, 0.90, 0.95),
                font_size: 14.0,
                line_height: 20.0,
                family: TextFamily::SansSerif,
            },
        );
        if let Some(kind) = action.kind.as_ref() {
            let width = measurer.measure(kind, 12.0).width;
            scene.push_text(
                kind.clone(),
                Rect::new(
                    list.origin.x + list.size.width - width - 8.0,
                    y + 5.0,
                    width + 2.0,
                    18.0,
                ),
                TextStyle {
                    clip: list,
                    color: Color::rgb(0.52, 0.57, 0.66),
                    font_size: 12.0,
                    line_height: 16.0,
                    family: TextFamily::SansSerif,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An action with a title and nothing else.
    fn action(title: &str) -> CodeAction {
        CodeAction {
            title: title.to_owned(),
            kind: None,
            edit: None,
            command: None,
        }
    }

    #[test]
    fn a_list_of_actions_can_be_made() {
        let popup = ActionPopup::new(vec![action("one"), action("two")]).expect("there is a list");
        assert_eq!(popup.len(), 2);
        assert_eq!(popup.selected(), 0);
        assert_eq!(popup.selected_action().map(|action| action.title.as_str()), Some("one"));
    }

    #[test]
    fn a_list_of_nothing_is_no_list() {
        assert!(ActionPopup::new(Vec::new()).is_none());
    }

    #[test]
    fn the_picked_action_goes_round_the_list() {
        let mut popup = ActionPopup::new(vec![action("one"), action("two")]).expect("there is a list");
        popup.select_next();
        assert_eq!(popup.selected(), 1);
        popup.select_next();
        assert_eq!(popup.selected(), 0);
        popup.select_previous();
        assert_eq!(popup.selected(), 1);
    }
}
