//! The editor's user interface layer.
//!
//! The UI layer owns the state, the layout, the hit testing and the focus,
//! and hands the render layer an immutable picture of one frame. It never
//! touches the GPU, the file system or the agent: those are services it
//! asks, not things it owns.
//!
//! - [`layout`] turns a buffer into the visual lines that fit the viewport;
//! - [`measure`] is how the layout asks the renderer where characters are;
//! - [`view`] holds the scroll position and the caret's shape.

pub mod layout;
pub mod measure;
pub mod view;

pub use layout::{TextLayout, VisualLine};
pub use measure::{LineMetrics, MonospaceMeasurer, TextMeasurer};
pub use view::{caret_shape, selection_line, CaretShape, ViewState};
