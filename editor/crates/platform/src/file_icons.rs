//! The little mark in front of the name of a file.
//!
//! A column of names says nothing about what the files are: a name does not
//! say whether it is code, a setting, a piece of text or a picture. The mark
//! says it, in the colour and the shape of the kind of the file.
//!
//! The kind is worked out from the name of the file, and the mark is drawn
//! with shapes: the editor has no pictures of its own, and a picture of a mark
//! that does not fit the theme is worse than a shape that does.

use std::path::Path;

use codeweaver_render::{Color, FrameScene, Rect};

use crate::icons;

/// The size of the mark.
pub const SIZE: f32 = 10.0;
/// The space between the mark and the name of the file.
pub const GAP: f32 = 5.0;

/// The kind of thing a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Something that is written by a person and read by a machine.
    Code,
    /// Something that says how a program is to be run.
    Setting,
    /// Something that is read by a person.
    Text,
    /// A picture.
    Picture,
    /// The file that names the folders of a project.
    Workspace,
    /// Something the editor does not know.
    Other,
}

impl Kind {
    /// The kind of a file, worked out from its name.
    ///
    /// The name of the file says what it is: a file that ends in `.rs` is
    /// code, one that ends in `.toml` is a setting. The letters do not have
    /// to be written in small ones: `.RS` is the same kind of file as `.rs`.
    pub fn of(path: &Path) -> Kind {
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return Kind::Other;
        };
        match extension.to_lowercase().as_str() {
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "c" | "h" | "cc" | "cpp" | "hpp"
            | "go" | "java" | "rb" | "php" | "cs" | "swift" | "kt" | "scala" | "sh" | "ps1"
            | "html" | "css" | "sql" | "lua" => Kind::Code,
            "toml" | "json" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "xml" | "env"
            | "properties" | "lock" => Kind::Setting,
            "md" | "txt" | "log" | "rst" | "adoc" | "csv" => Kind::Text,
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" | "webp" => Kind::Picture,
            "code-workspace" => Kind::Workspace,
            _ => Kind::Other,
        }
    }

    /// The name of the kind, for the person.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Code => "代码",
            Kind::Setting => "配置",
            Kind::Text => "文本",
            Kind::Picture => "图片",
            Kind::Workspace => "工作区",
            Kind::Other => "文件",
        }
    }
}

/// The colour of the mark of a kind.
///
/// Every kind has a colour of its own, so that the kinds can be told apart
/// without reading the names.
pub fn color_of(kind: Kind) -> Color {
    match kind {
        Kind::Code => Color::rgb(0.36, 0.64, 0.95),
        Kind::Setting => Color::rgb(0.95, 0.70, 0.30),
        Kind::Text => Color::rgb(0.44, 0.78, 0.56),
        Kind::Picture => Color::rgb(0.82, 0.50, 0.86),
        Kind::Workspace => Color::rgb(0.40, 0.78, 0.80),
        Kind::Other => Color::rgb(0.55, 0.59, 0.67),
    }
}

/// Draws the mark of a kind.
///
/// Every kind has a shape of its own as well as a colour: a block of colour
/// for code, a dial for a setting, a disc for a piece of text, a frame for a
/// picture and a wedge for a file the editor does not know.
pub fn draw(scene: &mut FrameScene, viewport: Rect, rect: Rect, kind: Kind) {
    let color = color_of(kind);
    match kind {
        Kind::Code => {
            scene.push_quad(rect, color, viewport);
        }
        Kind::Setting => {
            icons::ring(
                scene,
                viewport,
                rect,
                rect.size.width / 2.0,
                rect.size.width / 4.0,
                color,
            );
        }
        Kind::Text => {
            icons::disc(
                scene,
                viewport,
                rect,
                rect.size.width / 2.0 - 0.5,
                color,
            );
        }
        Kind::Picture => {
            icons::outline(scene, viewport, rect, 1.0, color);
        }
        Kind::Workspace => {
            // A frame with a bar down the middle: a window that shows two
            // things at once, which is what a workspace is.
            icons::outline(scene, viewport, rect, 1.0, color);
            scene.push_quad(
                Rect::new(
                    rect.origin.x + rect.size.width / 2.0 - 0.5,
                    rect.origin.y,
                    1.0,
                    rect.size.height,
                ),
                color,
                viewport,
            );
        }
        Kind::Other => {
            icons::triangle(
                scene,
                viewport,
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                true,
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind of a file with the given name.
    fn kind_of(name: &str) -> Kind {
        Kind::of(Path::new(name))
    }

    #[test]
    fn the_name_of_the_file_says_what_kind_of_file_it_is() {
        assert_eq!(kind_of("main.rs"), Kind::Code);
        assert_eq!(kind_of("Cargo.toml"), Kind::Setting);
        assert_eq!(kind_of("notes.md"), Kind::Text);
        assert_eq!(kind_of("logo.png"), Kind::Picture);
        assert_eq!(kind_of("mystery.qqq"), Kind::Other);
    }

    #[test]
    fn the_letters_of_the_name_do_not_have_to_be_small() {
        assert_eq!(kind_of("MAIN.RS"), Kind::Code);
        assert_eq!(kind_of("Photo.PNG"), Kind::Picture);
    }

    #[test]
    fn a_file_without_a_name_is_a_file_the_editor_does_not_know() {
        assert_eq!(kind_of("LICENSE"), Kind::Other);
        assert_eq!(kind_of(""), Kind::Other);
    }

    #[test]
    fn a_workspace_file_has_a_mark_of_its_own() {
        assert_eq!(kind_of("project.code-workspace"), Kind::Workspace);
        assert_eq!(kind_of("PROJECT.CODE-WORKSPACE"), Kind::Workspace);
    }

    #[test]
    fn every_kind_has_a_colour_of_its_own() {
        let kinds = [
            Kind::Code,
            Kind::Setting,
            Kind::Text,
            Kind::Picture,
            Kind::Workspace,
            Kind::Other,
        ];
        for (index, kind) in kinds.iter().enumerate() {
            for other in kinds.iter().skip(index + 1) {
                assert_ne!(color_of(*kind), color_of(*other), "the colours differ");
            }
        }
    }

    #[test]
    fn every_kind_is_drawn() {
        let mut scene = FrameScene::default();
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let rect = Rect::new(10.0, 10.0, SIZE, SIZE);
        for kind in [
            Kind::Code,
            Kind::Setting,
            Kind::Text,
            Kind::Picture,
            Kind::Workspace,
            Kind::Other,
        ] {
            let before = scene.quads.len();
            draw(&mut scene, viewport, rect, kind);
            assert!(scene.quads.len() > before, "the mark is drawn");
        }
    }

    #[test]
    fn the_kinds_are_told_apart_by_their_shape_too() {
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let rect = Rect::new(10.0, 10.0, SIZE, SIZE);
        let mut shapes: Vec<String> = Vec::new();
        for kind in [
            Kind::Code,
            Kind::Setting,
            Kind::Text,
            Kind::Picture,
            Kind::Workspace,
            Kind::Other,
        ] {
            let mut scene = FrameScene::default();
            draw(&mut scene, viewport, rect, kind);
            let shape: Vec<String> = scene
                .quads
                .iter()
                .map(|quad| format!("{:?}", quad.bounds))
                .collect();
            shapes.push(shape.join(" "));
        }
        // No two kinds are drawn the same way, so that the shapes are told
        // apart even without the colours.
        for (index, shape) in shapes.iter().enumerate() {
            for other in shapes.iter().skip(index + 1) {
                assert_ne!(shape, other, "the shapes differ");
            }
        }
    }
}
