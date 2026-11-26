//! How big the text of the document is.
//!
//! The size used to be a constant in the code. A constant is a size nobody can
//! change: the person who cannot read the text at 16 pixels has no way of
//! making it bigger. So the size is a **state** now, and it is remembered in a
//! file of its own, `settings.json`, in the folder of the user:
//!
//! ```json
//! { "fontSize": 120 }
//! ```
//!
//! The number is a percentage of the usual size: 100 is the size the editor
//! has always had, 120 is a fifth bigger, 80 a fifth smaller.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The size the text is drawn at when nothing says otherwise.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;
/// The height of one line when the text is drawn at its usual size.
pub const DEFAULT_LINE_HEIGHT: f32 = 22.0;
/// The smallest the text may be, in percent.
pub const MIN: f32 = 80.0;
/// The largest the text may be, in percent.
pub const MAX: f32 = 300.0;
/// How much one notch of the wheel changes the size, in percent.
pub const STEP: f32 = 10.0;
/// The name of the file the size is remembered in.
pub const FILE_NAME: &str = "settings.json";

/// The sizes the setting of the panel offers, from the smallest to the largest.
pub const STEPS: [u32; 10] = [80, 90, 100, 110, 125, 150, 175, 200, 250, 300];

/// The size of the text of the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    /// How big the text is, in percent of the usual size.
    pub percent: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { percent: 100.0 }
    }
}

impl Zoom {
    /// A size that is given as a percentage.
    ///
    /// A size outside the range is brought into it: a file that says 4000 is
    /// not a size the editor can draw.
    pub fn of(percent: f32) -> Zoom {
        Zoom {
            percent: percent.clamp(MIN, MAX),
        }
    }

    /// The size of the text, in pixels.
    pub fn font_size(self) -> f32 {
        DEFAULT_FONT_SIZE * self.percent / 100.0
    }

    /// The height of one line of the text, in pixels.
    pub fn line_height(self) -> f32 {
        DEFAULT_LINE_HEIGHT * self.percent / 100.0
    }

    /// The size the line numbers are drawn at.
    ///
    /// They are smaller than the text of the document, so that the text stays
    /// the thing the eye goes to.
    pub fn gutter_font_size(self) -> f32 {
        self.font_size() * 13.0 / DEFAULT_FONT_SIZE
    }

    /// How big the text is, the way a person reads it.
    pub fn label(self) -> String {
        format!("{}%", self.percent.round() as i32)
    }

    /// The size one notch bigger.
    pub fn bigger(self) -> Zoom {
        Zoom::of(self.percent + STEP)
    }

    /// The size one notch smaller.
    pub fn smaller(self) -> Zoom {
        Zoom::of(self.percent - STEP)
    }

    /// The usual size.
    pub fn reset(self) -> Zoom {
        Zoom::default()
    }

    /// The step of the panel that is the nearest to this size.
    ///
    /// The panel offers steps, not every size there is: a setting whose value
    /// has to be typed cannot be changed without a keyboard layout.
    pub fn nearest_step(self) -> usize {
        let mut nearest = 0;
        let mut distance = f32::MAX;
        for (index, step) in STEPS.iter().enumerate() {
            let difference = (self.percent - *step as f32).abs();
            if difference < distance {
                distance = difference;
                nearest = index;
            }
        }
        nearest
    }
}

/// What the file says, and what was wrong with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The size the file asks for.
    pub zoom: Zoom,
    /// What could not be read, one line per mistake.
    pub errors: Vec<String>,
}

/// The place the file of the user lives in.
///
/// Windows keeps the settings of a user under `%APPDATA%`; the other systems
/// under the folder of the user or the one the environment names. A machine
/// that says none of them has no file of the user, which is not an error.
pub fn user_path() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("CodeWeaver").join(FILE_NAME))
}

/// Reads the size from the text of a file.
///
/// A file that cannot be read is not a reason to refuse to start: the editor
/// uses the usual size and says what was wrong with the file.
pub fn parse(text: &str) -> Loaded {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Loaded {
                zoom: Zoom::default(),
                errors: vec![format!("设置文件读不出来：{error}")],
            };
        }
    };
    let mut errors = Vec::new();
    let mut zoom = Zoom::default();
    match value.get("fontSize") {
        None => {}
        Some(size) => match size.as_u64() {
            Some(percent) => zoom = Zoom::of(percent as f32),
            None => errors.push("设置文件里的 fontSize 不是一个数字，没有用上".to_owned()),
        },
    }
    Loaded { zoom, errors }
}

/// Reads the size from a file.
///
/// A file that is not there is not a mistake: the editor starts with the usual
/// size.
pub fn read(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                zoom: Zoom::default(),
                errors: Vec::new(),
            };
        }
        Err(error) => {
            return Loaded {
                zoom: Zoom::default(),
                errors: vec![format!("{} 读不出来：{error}", path.display())],
            };
        }
    };
    let mut loaded = parse(&text);
    if !loaded.errors.is_empty() {
        let name = path.display().to_string();
        loaded.errors = loaded
            .errors
            .into_iter()
            .map(|error| format!("{name}：{error}"))
            .collect();
    }
    loaded
}

/// Writes the size into a file.
pub fn write(path: &Path, zoom: Zoom) -> Result<(), String> {
    let text = format!("{{\n  \"fontSize\": {}\n}}\n", zoom.percent.round() as i32);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{} 建不出来：{error}", folder.display()))?;
    }
    std::fs::write(path, text).map_err(|error| format!("{} 写不进去：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usual_size_is_the_size_the_editor_has_always_had() {
        let zoom = Zoom::default();
        assert_eq!(zoom.font_size(), 16.0);
        assert_eq!(zoom.line_height(), 22.0);
        assert_eq!(zoom.label(), "100%");
    }

    #[test]
    fn a_bigger_size_makes_the_text_and_the_line_bigger() {
        let zoom = Zoom::of(120.0);
        assert_eq!(zoom.font_size(), 19.2);
        assert_eq!(zoom.line_height(), 26.4);
    }

    #[test]
    fn the_size_stays_inside_the_range() {
        assert_eq!(Zoom::of(10.0).percent, MIN);
        assert_eq!(Zoom::of(9000.0).percent, MAX);
    }

    #[test]
    fn one_notch_is_a_tenth_of_the_size() {
        let zoom = Zoom::default();
        assert_eq!(zoom.bigger().percent, 110.0);
        assert_eq!(zoom.smaller().percent, 90.0);
        assert_eq!(zoom.bigger().bigger().smaller().percent, 110.0);
    }

    #[test]
    fn the_size_cannot_be_taken_outside_the_range() {
        let mut zoom = Zoom::default();
        for _ in 0..40 {
            zoom = zoom.bigger();
        }
        assert_eq!(zoom.percent, MAX);
        for _ in 0..40 {
            zoom = zoom.smaller();
        }
        assert_eq!(zoom.percent, MIN);
    }

    #[test]
    fn the_size_can_be_made_the_usual_size_again() {
        assert_eq!(Zoom::of(200.0).reset().percent, 100.0);
    }

    #[test]
    fn the_line_numbers_are_smaller_than_the_text() {
        let zoom = Zoom::of(200.0);
        assert!(zoom.gutter_font_size() < zoom.font_size());
    }

    #[test]
    fn the_size_is_read_from_the_text_of_the_file() {
        let loaded = parse("{\n  \"fontSize\": 150\n}\n");
        assert_eq!(loaded.zoom.percent, 150.0);
        assert!(loaded.errors.is_empty());
    }

    #[test]
    fn a_file_without_a_size_leaves_the_usual_size() {
        let loaded = parse("{}");
        assert_eq!(loaded.zoom.percent, 100.0);
        assert!(loaded.errors.is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_named_and_the_usual_size_is_used() {
        let loaded = parse("{ not a file");
        assert_eq!(loaded.zoom.percent, 100.0);
        assert_eq!(loaded.errors.len(), 1);
    }

    #[test]
    fn a_size_that_is_not_a_number_is_named() {
        let loaded = parse("{ \"fontSize\": \"big\" }");
        assert_eq!(loaded.zoom.percent, 100.0);
        assert_eq!(loaded.errors.len(), 1);
    }

    /// A folder the test may write in.
    ///
    /// The folder of the system is not one: the test binary of the workspace
    /// runs with a low label and the folder of the system belongs to the
    /// medium one, so writing there is refused. The folder the test binary
    /// sits in is inside the workspace, where writing works.
    fn a_folder_to_write_in() -> PathBuf {
        let binary = std::env::current_exe().expect("the test binary has a place");
        binary
            .parent()
            .expect("the test binary is in a folder")
            .join("zoom-write-test")
    }

    #[test]
    fn the_size_that_is_written_can_be_read_again() {
        let folder = a_folder_to_write_in();
        let path = folder.join(FILE_NAME);
        let _ = std::fs::remove_file(&path);
        write(&path, Zoom::of(125.0)).expect("the size is written");
        let loaded = read(&path);
        assert_eq!(loaded.zoom.percent, 125.0);
        assert!(loaded.errors.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&folder);
    }

    #[test]
    fn the_panel_offers_the_step_that_is_the_nearest_to_the_size() {
        assert_eq!(Zoom::default().nearest_step(), 2);
        // 125 is nearer to 120 than 110 is.
        assert_eq!(Zoom::of(120.0).nearest_step(), 4);
        assert_eq!(Zoom::of(260.0).nearest_step(), 8);
    }
}
