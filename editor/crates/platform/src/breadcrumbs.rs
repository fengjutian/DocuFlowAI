//! The row above the text that says where the file is.
//!
//! The row is the path of the file that is on screen, piece by piece: the
//! project it belongs to, the folders below it and the file itself. Every
//! piece stands for a place, so a click on one says "take me there".
//!
//! The path is worked out from the file and the root of the project, not from
//! the tree of files: the tree is rooted at the folder of the file that is
//! open, so it cannot say where the file is inside the project.

use std::path::{Component, Path, PathBuf};

use codeweaver_render::{Color, FrameScene, Point, Rect, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;

/// The height of the row with the path.
pub const HEIGHT: f32 = 24.0;

/// The size the path is drawn at.
///
/// It is smaller than the text of the document: the path says where the
/// person is, the text is what they are working on.
const FONT_SIZE: f32 = 12.0;
/// The space on the left of the path.
const PADDING: f32 = 12.0;
/// The space between two pieces of the path.
const GAP: f32 = 10.0;
/// The text between two pieces.
const SEPARATOR: &str = "›";
/// The text that stands for the pieces that did not fit.
const DOTS: &str = "…";
/// The space around a piece of text, so that a click near it still counts.
const HIT_PADDING: f32 = 3.0;

/// The color of the row.
///
/// It is the color of the strip of tabs: the path is part of the window
/// around the text, not part of the text itself.
const BAR_COLOR: Color = Color::rgb(0.10, 0.12, 0.16);
/// The color of the line under the row.
const BORDER_COLOR: Color = Color::rgb(0.20, 0.23, 0.29);
/// The color of the name of a folder.
const FOLDER_COLOR: Color = Color::rgb(0.52, 0.56, 0.64);
/// The color of the name of the file.
const FILE_COLOR: Color = Color::rgb(0.82, 0.86, 0.93);
/// The color of the separator between two pieces.
const SEPARATOR_COLOR: Color = Color::rgb(0.38, 0.41, 0.48);

/// One piece of the path.
#[derive(Clone, Debug, PartialEq)]
pub struct Crumb {
    /// The name that is shown.
    pub label: String,
    /// The place it stands for.
    pub path: PathBuf,
    /// Whether it is the file that is open.
    pub file: bool,
    /// Where it is drawn.
    pub rect: Rect,
}

impl Crumb {
    /// A piece that is not drawn yet.
    fn new(label: impl Into<String>, path: PathBuf, file: bool) -> Crumb {
        Crumb {
            label: label.into(),
            path,
            file,
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }
}

/// The pieces of the path, and where they are drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    /// The pieces that are shown.
    pub crumbs: Vec<Crumb>,
    /// The place of the separator after each piece.
    pub separators: Vec<Rect>,
    /// The place of the dots that stand for the pieces that did not fit.
    pub dots: Option<Rect>,
}

/// The pieces of the path of a file.
///
/// The path is given as it is written down on the disk, and the root of the
/// project is the folder the path is shortened with: the path of a file in a
/// project reads `project › folder › file`, and the path of a file that is in
/// no project reads the whole way from the root of the disk.
pub fn of(path: &Path, root: Option<&Path>) -> Vec<Crumb> {
    let mut crumbs = Vec::new();
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new(""));

    // The path of the file inside the project, when the file is in one.
    let inside = root.and_then(|root| parent.strip_prefix(root).ok());
    match (inside, root) {
        (Some(rest), Some(root)) => {
            // The project itself is the first piece: it says which project
            // the file belongs to.
            if let Some(name) = root.file_name() {
                crumbs.push(Crumb::new(name.to_string_lossy(), root.to_path_buf(), false));
            }
            for (label, place) in folders(rest, root) {
                crumbs.push(Crumb::new(label, place, false));
            }
        }
        _ => {
            // The file is in no project: the whole way down is shown.
            for (label, place) in folders(parent, Path::new("")) {
                crumbs.push(Crumb::new(label, place, false));
            }
        }
    }
    crumbs.push(Crumb::new(file, path.to_path_buf(), true));
    crumbs
}

/// The folders of a path, piece by piece.
///
/// `base` is the folder the path is relative to: the pieces are the places
/// below it. The places are built up as the path is read, so that every piece
/// knows the folder it is in.
fn folders(path: &Path, base: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut place = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                place.push(prefix.as_os_str());
                out.push((
                    prefix.as_os_str().to_string_lossy().into_owned(),
                    place.clone(),
                ));
            }
            Component::RootDir => {
                place.push(Component::RootDir.as_os_str());
            }
            Component::Normal(name) => {
                place.push(name);
                out.push((name.to_string_lossy().into_owned(), place.clone()));
            }
            Component::CurDir | Component::ParentDir => {}
        }
    }
    out
}

/// Lays the pieces of the path out in the row.
///
/// The pieces are laid out from the left. When the row is too narrow for all
/// of them, the ones at the front are dropped and the dots stand for them:
/// the name of the file is the piece that matters, and it is the one that is
/// kept.
pub fn strip(rect: Rect, crumbs: &[Crumb], measurer: &mut dyn TextMeasurer) -> Strip {
    let widths: Vec<f32> = crumbs
        .iter()
        .map(|crumb| measurer.measure(&crumb.label, FONT_SIZE).width)
        .collect();
    let available = (rect.size.width - PADDING * 2.0).max(1.0);

    // The last piece is always there: it is the name of the file.
    let mut start = crumbs.len();
    let mut used = 0.0f32;
    for index in (0..crumbs.len()).rev() {
        let need = widths[index] + GAP;
        if used + need > available {
            break;
        }
        used += need;
        start = index;
    }
    if start == crumbs.len() && !crumbs.is_empty() {
        // Even the name of the file does not fit: it is shown anyway, and it
        // is cut off at the edge of the row.
        start = crumbs.len() - 1;
    }

    let separator_width = measurer.measure(SEPARATOR, FONT_SIZE).width;
    let dots_width = measurer.measure(DOTS, FONT_SIZE).width;
    let mut strip = Strip::default();
    let mut x = rect.origin.x + PADDING;
    if start > 0 {
        strip.dots = Some(Rect::new(x, rect.origin.y, dots_width, rect.size.height));
        x += dots_width + GAP;
    }
    for index in start..crumbs.len() {
        let place = Rect::new(x, rect.origin.y, widths[index], rect.size.height);
        strip.crumbs.push(Crumb {
            label: crumbs[index].label.clone(),
            path: crumbs[index].path.clone(),
            file: crumbs[index].file,
            rect: place,
        });
        x = place.max_x();
        if index + 1 < crumbs.len() {
            // The separator stands in the space between two pieces.
            strip.separators.push(Rect::new(
                x + (GAP - separator_width) / 2.0,
                rect.origin.y,
                separator_width,
                rect.size.height,
            ));
            x += GAP;
        }
    }
    strip
}

/// The piece of the path that was clicked, if one was.
pub fn hit(strip: &Strip, point: Point) -> Option<&Crumb> {
    strip.crumbs.iter().find(|crumb| {
        Rect::new(
            crumb.rect.origin.x - HIT_PADDING,
            crumb.rect.origin.y,
            crumb.rect.size.width + HIT_PADDING * 2.0,
            crumb.rect.size.height,
        )
        .contains(point)
    })
}

/// Draws the row and the path on it.
pub fn draw(scene: &mut FrameScene, viewport: Rect, rect: Rect, strip: &Strip) {
    scene.push_quad(rect, BAR_COLOR, viewport);
    scene.push_quad(
        Rect::new(
            rect.origin.x,
            rect.max_y() - 1.0,
            rect.size.width,
            1.0,
        ),
        BORDER_COLOR,
        viewport,
    );

    if let Some(dots) = strip.dots {
        scene.push_text(
            DOTS.to_owned(),
            Rect::new(dots.origin.x, dots.origin.y + 5.0, dots.size.width, 14.0),
            TextStyle {
                clip: viewport,
                color: SEPARATOR_COLOR,
                font_size: FONT_SIZE,
                line_height: 14.0,
                family: TextFamily::SansSerif,
            },
        );
    }
    for (index, crumb) in strip.crumbs.iter().enumerate() {
        scene.push_text(
            crumb.label.clone(),
            Rect::new(
                crumb.rect.origin.x,
                crumb.rect.origin.y + 5.0,
                crumb.rect.size.width,
                14.0,
            ),
            TextStyle {
                clip: viewport,
                color: if crumb.file { FILE_COLOR } else { FOLDER_COLOR },
                font_size: FONT_SIZE,
                line_height: 14.0,
                family: TextFamily::SansSerif,
            },
        );
        if let Some(separator) = strip.separators.get(index) {
            scene.push_text(
                SEPARATOR.to_owned(),
                Rect::new(
                    separator.origin.x,
                    separator.origin.y + 5.0,
                    separator.size.width,
                    14.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: SEPARATOR_COLOR,
                    font_size: FONT_SIZE,
                    line_height: 14.0,
                    family: TextFamily::SansSerif,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_ui::MonospaceMeasurer;

    /// The row of the path in a window that is 800 pixels wide.
    fn row() -> Rect {
        Rect::new(280.0, 34.0, 700.0, HEIGHT)
    }

    /// A measurer that says every character is eight pixels wide.
    fn measurer() -> MonospaceMeasurer {
        MonospaceMeasurer::new(8.0)
    }

    #[test]
    fn the_path_of_a_file_in_a_project_starts_at_the_project() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        assert_eq!(
            crumbs.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["project", "src", "main.rs"]
        );
    }

    #[test]
    fn the_file_itself_is_the_last_piece_and_the_others_are_folders() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        assert!(crumbs.last().expect("there is a last piece").file);
        assert!(crumbs[..crumbs.len() - 1].iter().all(|crumb| !crumb.file));
    }

    #[test]
    fn every_folder_knows_the_place_it_stands_for() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        assert_eq!(crumbs[1].path, Path::new("D:/work/project/src"));
        assert_eq!(crumbs[2].path, path);
    }

    #[test]
    fn a_file_that_is_in_no_project_shows_the_whole_way_down() {
        let path = Path::new("D:/notes/today.md");
        let crumbs = of(path, None);
        let labels: Vec<&str> = crumbs.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"notes"));
        assert_eq!(labels.last(), Some(&"today.md"));
    }

    #[test]
    fn a_file_that_is_not_in_the_project_shows_the_whole_way_down() {
        let path = Path::new("D:/other/today.md");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        let labels: Vec<&str> = crumbs.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"other"));
        assert_eq!(labels.last(), Some(&"today.md"));
    }

    #[test]
    fn the_pieces_are_laid_out_from_the_left() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        let strip = strip(row(), &crumbs, &mut measurer());
        assert_eq!(strip.crumbs.len(), 3);
        assert!(strip.crumbs[0].rect.origin.x < strip.crumbs[1].rect.origin.x);
        assert!(strip.crumbs[1].rect.origin.x < strip.crumbs[2].rect.origin.x);
        assert_eq!(strip.separators.len(), 2);
    }

    #[test]
    fn the_pieces_that_do_not_fit_are_dropped_and_the_dots_stand_for_them() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        let narrow = Rect::new(280.0, 34.0, 60.0, HEIGHT);
        let strip = strip(narrow, &crumbs, &mut measurer());
        assert!(strip.dots.is_some());
        assert_eq!(strip.crumbs.len(), 1);
        assert!(strip.crumbs[0].file);
    }

    #[test]
    fn the_piece_that_was_clicked_is_the_one_under_the_point() {
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        let strip = strip(row(), &crumbs, &mut measurer());
        let place = strip.crumbs[1].rect;
        let found = hit(&strip, Point {
            x: place.origin.x + 1.0,
            y: place.origin.y + 4.0,
        });
        assert_eq!(found.map(|crumb| crumb.label.as_str()), Some("src"));
        assert!(hit(&strip, Point { x: 5.0, y: 5.0 }).is_none());
    }

    #[test]
    fn the_row_is_drawn_with_a_piece_for_every_part_of_the_path() {
        let mut scene = FrameScene::default();
        let path = Path::new("D:/work/project/src/main.rs");
        let root = Path::new("D:/work/project");
        let crumbs = of(path, Some(root));
        let strip = strip(row(), &crumbs, &mut measurer());
        draw(
            &mut scene,
            Rect::new(0.0, 0.0, 1280.0, 800.0),
            row(),
            &strip,
        );
        // The ground of the row, the line under it, the three pieces of the
        // path and the two separators between them.
        assert_eq!(scene.quads.len(), 2);
        assert_eq!(scene.glyph_runs.len(), 5);
    }
}
