//! The completions the language server offered.
//!
//! The list floats under the caret. It is drawn like the find bar: a block
//! of colour with the text on top of it.

use codeweaver_editor_core::Editor;
use codeweaver_lsp::protocol::CompletionItem;
use codeweaver_render::{Color, FrameScene, Rect, TextFamily, TextStyle};
use codeweaver_text::{ApplyError, Buffer, TextOffset};
use codeweaver_ui::TextMeasurer;

/// The width of the list.
const WIDTH: f32 = 360.0;
/// The height of one item of the list.
const ITEM_HEIGHT: f32 = 24.0;
/// How many items are shown at once.
const VISIBLE: usize = 10;
/// The padding around the items.
const PADDING: f32 = 6.0;

/// The completions that are on offer, and the one that is picked.
pub struct CompletionPopup {
    items: Vec<CompletionItem>,
    /// Which item is picked, counted from the first one.
    selected: usize,
    /// The byte offset where the word that is being completed starts.
    start: usize,
}

impl CompletionPopup {
    /// Makes a list, or nothing when the server had nothing to offer.
    pub fn new(items: Vec<CompletionItem>, start: usize) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        Some(CompletionPopup {
            items,
            selected: 0,
            start,
        })
    }

    /// The items of the list.
    pub fn items(&self) -> &[CompletionItem] {
        &self.items
    }

    /// Which item is picked, counted from the first one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The item that is picked.
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    /// Picks the item after this one, going round to the first one.
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
    }

    /// Picks the item before this one, going round to the last one.
    pub fn select_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
    }

    /// Puts the picked item into the document.
    ///
    /// The word that is being completed is replaced by the text of the item,
    /// and the caret goes after it.
    pub fn apply(&self, editor: &mut Editor) -> Result<(), ApplyError> {
        let Some(item) = self.selected_item() else {
            return Ok(());
        };
        let text = item.insert_text().to_owned();
        let caret = editor.caret();
        editor.replace_ranges(vec![TextOffset(self.start)..caret], &text)
    }
}

/// The offset where the word before the caret starts.
///
/// The word is what a completion replaces: the letters and the digits that
/// are right in front of the caret.
pub fn word_start(buffer: &Buffer, caret: usize) -> usize {
    let line = buffer.line_of_offset(caret);
    let text = buffer.line_text(line);
    let start = buffer.line_range(line).start.0;
    let column = caret.saturating_sub(start).min(text.len());
    let prefix = &text[..column];
    // The word is the run of letters and digits that ends at the caret.
    let word =
        prefix.trim_end_matches(|character: char| character.is_alphanumeric() || character == '_');
    start + word.len()
}

/// The word before the caret.
pub fn word_before(buffer: &Buffer, caret: usize) -> String {
    let start = word_start(buffer, caret);
    buffer.slice(TextOffset(start)..TextOffset(caret))
}

/// Keeps the items that start with what has been typed.
///
/// The list of a server can hold thousands of items, and the ones that do
/// not match the word in front of the caret are of no use.
pub fn matching(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let prefix = prefix.to_lowercase();
    items
        .into_iter()
        .filter(|item| item.filter_text().to_lowercase().starts_with(&prefix))
        .collect()
}

/// Draws the list under the caret.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    body: Rect,
    popup: &CompletionPopup,
    caret: (f32, f32),
    measurer: &mut dyn TextMeasurer,
) {
    let items = popup.items();
    let shown = items.len().min(VISIBLE);
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

    // The item that is picked is the one that stands out.
    let first = popup
        .selected()
        .saturating_sub(shown - 1)
        .min(items.len() - 1);
    for (index, item) in items.iter().enumerate().skip(first).take(shown) {
        let row = index - first;
        let y = list.origin.y + PADDING + row as f32 * ITEM_HEIGHT;
        if index == popup.selected() {
            scene.push_quad(
                Rect::new(list.origin.x, y, list.size.width, ITEM_HEIGHT),
                Color::rgb(0.18, 0.30, 0.52),
                viewport,
            );
        }
        scene.push_text(
            item.label.clone(),
            Rect::new(
                list.origin.x + 8.0,
                y + 2.0,
                list.size.width * 0.55,
                ITEM_HEIGHT,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.88, 0.90, 0.95),
                font_size: 14.0,
                line_height: 20.0,
                family: TextFamily::SansSerif,
            },
        );
        if let Some(detail) = item.detail.as_ref() {
            let width = measurer.measure(detail, 12.0).width;
            scene.push_text(
                detail.clone(),
                Rect::new(
                    list.origin.x + list.size.width - width - 8.0,
                    y + 4.0,
                    width + 2.0,
                    18.0,
                ),
                TextStyle {
                    clip: viewport,
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

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_owned(),
            detail: None,
            kind: None,
            insert_text: None,
            sort_text: None,
            filter_text: None,
        }
    }

    #[test]
    fn the_word_in_front_of_the_caret_starts_where_it_should() {
        let buffer = Buffer::new("let value = 1;\n");
        // The caret is after "val".
        assert_eq!(word_start(&buffer, 7), 4);
        assert_eq!(word_before(&buffer, 7), "val");
        // After a space there is no word.
        assert_eq!(word_start(&buffer, 12), 12);
    }

    #[test]
    fn only_the_items_that_match_are_kept() {
        let items = vec![item("alpha"), item("beta"), item("alphabet")];
        let kept = matching(items, "alph");
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].label, "alpha");
        assert_eq!(kept[1].label, "alphabet");
    }

    #[test]
    fn an_empty_prefix_keeps_everything() {
        let items = vec![item("alpha"), item("beta")];
        assert_eq!(matching(items, "").len(), 2);
    }

    #[test]
    fn the_picked_item_goes_into_the_document() {
        let mut editor = Editor::new("let val = 1;\n");
        editor.set_carets(&[TextOffset(7)]);
        let mut popup = CompletionPopup::new(vec![item("value"), item("other")], 4).unwrap();
        popup.select_next();
        assert_eq!(popup.selected(), 1);
        popup.apply(&mut editor).unwrap();
        assert_eq!(editor.text(), "let other = 1;\n");
    }
}
