//! The bar with the activities, on the left of the window.
//!
//! The bar is the way into the side bar: every activity is one section of
//! it. Clicking an activity shows its section, clicking it again hides the
//! side bar — the way VS Code does it.
//!
//! The component knows nothing about the editor: it says which activities
//! there are, where they are drawn, and which one was clicked.

use codeweaver_render::{Color, FrameScene, Point, Rect};

use crate::icons;

/// The height of one activity on the bar.
pub const ITEM_HEIGHT: f32 = 48.0;
/// The size of the icon of an activity.
const ICON_SIZE: f32 = 20.0;
/// The size of the badge that tells how much there is.
const BADGE_HEIGHT: f32 = 14.0;

/// The color of the bar.
const BAR_COLOR: Color = Color::rgb(0.075, 0.086, 0.11);
/// The color of the border on the right of the bar.
const BORDER_COLOR: Color = Color::rgb(0.16, 0.18, 0.22);
/// The color of an icon that is not picked.
const ICON_COLOR: Color = Color::rgb(0.58, 0.62, 0.70);
/// The color of the icon of the activity that is on screen.
const ICON_ACTIVE_COLOR: Color = Color::rgb(0.92, 0.94, 0.97);
/// The color of the bar that is drawn on the left of the picked activity.
const MARK_COLOR: Color = Color::rgb(0.30, 0.58, 0.96);
/// The color of the badge.
const BADGE_COLOR: Color = Color::rgb(0.30, 0.58, 0.96);
/// The color of the text of the badge.
const BADGE_TEXT_COLOR: Color = Color::rgb(1.0, 1.0, 1.0);

/// One of the things the side bar can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Activity {
    /// The files of the project.
    Files,
    /// The text that was found in the files.
    Search,
    /// The files that git knows have changed.
    SourceControl,
    /// The breakpoints and the session of the debugger.
    Debug,
    /// The extensions of the editor.
    Extensions,
}

impl Activity {
    /// All of them, in the order they are drawn.
    pub const ALL: [Activity; 5] = [
        Activity::Files,
        Activity::Search,
        Activity::SourceControl,
        Activity::Debug,
        Activity::Extensions,
    ];

    /// The name of it, for the person.
    pub fn label(self) -> &'static str {
        match self {
            Activity::Files => "资源管理器",
            Activity::Search => "搜索",
            Activity::SourceControl => "源代码管理",
            Activity::Debug => "调试",
            Activity::Extensions => "扩展",
        }
    }

    /// Which one it is, counting from zero.
    pub fn index(self) -> usize {
        match self {
            Activity::Files => 0,
            Activity::Search => 1,
            Activity::SourceControl => 2,
            Activity::Debug => 3,
            Activity::Extensions => 4,
        }
    }

    /// The activity that comes after it, going round to the first one.
    pub fn next(self) -> Activity {
        Activity::ALL[(self.index() + 1) % Activity::ALL.len()]
    }

    /// The activity that comes before it, going round to the last one.
    pub fn previous(self) -> Activity {
        Activity::ALL[(self.index() + Activity::ALL.len() - 1) % Activity::ALL.len()]
    }
}

/// The numbers that are drawn on the activities.
///
/// The numbers are drawn on the icons, so that the person sees that there is
/// something to look at without going to look at it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Badges {
    /// How many hits the last search found.
    pub search: usize,
    /// How many files git knows have changed.
    pub source_control: usize,
    /// How many breakpoints there are.
    pub debug: usize,
}

impl Badges {
    /// The number that is drawn on the activity, if there is one.
    pub fn of(&self, activity: Activity) -> Option<usize> {
        let count = match activity {
            Activity::Search => self.search,
            Activity::SourceControl => self.source_control,
            Activity::Debug => self.debug,
            Activity::Files | Activity::Extensions => 0,
        };
        (count > 0).then_some(count)
    }
}

/// The section the side bar shows, and whether it is shown at all.
///
/// The section is a state: switching to another one and back again leaves
/// the content of the section as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideBar {
    /// The section that is on screen.
    pub section: Activity,
    /// Whether the side bar is shown.
    pub open: bool,
}

impl Default for SideBar {
    fn default() -> Self {
        SideBar {
            section: Activity::Files,
            open: true,
        }
    }
}

impl SideBar {
    /// The activity that was picked.
    ///
    /// Picking the activity that is on screen hides the side bar; picking
    /// another one shows that one. This is the way VS Code does it.
    pub fn pick(&mut self, activity: Activity) {
        if self.open && self.section == activity {
            self.open = false;
            return;
        }
        self.section = activity;
        self.open = true;
    }

    /// Shows the section, whether it is shown already or not.
    pub fn show(&mut self, activity: Activity) {
        self.section = activity;
        self.open = true;
    }

    /// Hides the side bar.
    pub fn hide(&mut self) {
        self.open = false;
    }

    /// Whether the side bar is shown.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The activity that is on screen, if the side bar is shown.
    pub fn selected(&self) -> Option<Activity> {
        self.open.then_some(self.section)
    }
}

/// One activity on the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityItem {
    /// Which activity it is.
    pub activity: Activity,
    /// The number that is drawn on the icon, if there is one.
    pub badge: Option<usize>,
    /// Whether this is the activity that is on screen.
    pub selected: bool,
    /// Where the icon is drawn.
    pub rect: Rect,
}

/// The bar with the activities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityBar {
    /// The activities, in the order they are drawn.
    pub items: Vec<ActivityItem>,
}

impl ActivityBar {
    /// Works out where every activity is drawn.
    ///
    /// `selected` is the activity that is on screen. When the side bar is
    /// hidden there is none, and no activity is picked.
    pub fn layout(bar: Rect, selected: Option<Activity>, badges: &Badges) -> ActivityBar {
        let items = Activity::ALL
            .iter()
            .enumerate()
            .map(|(index, activity)| ActivityItem {
                activity: *activity,
                badge: badges.of(*activity),
                selected: selected == Some(*activity),
                rect: Rect::new(
                    bar.origin.x,
                    bar.origin.y + index as f32 * ITEM_HEIGHT,
                    bar.size.width,
                    ITEM_HEIGHT,
                ),
            })
            .collect();
        ActivityBar { items }
    }

    /// The activity that was clicked, if one was.
    pub fn hit(&self, point: Point) -> Option<Activity> {
        self.items
            .iter()
            .find(|item| item.rect.contains(point))
            .map(|item| item.activity)
    }
}

/// Draws the bar and its icons.
///
/// The icons are made of shapes, not of pictures: the editor has no images of
/// its own, and a shape that fits the theme is worth more than a picture
/// that does not.
pub fn draw(scene: &mut FrameScene, viewport: Rect, bar: Rect, items: &[ActivityItem]) {
    scene.push_quad(bar, BAR_COLOR, viewport);
    scene.push_quad(
        Rect::new(bar.max_x() - 1.0, bar.origin.y, 1.0, bar.size.height),
        BORDER_COLOR,
        viewport,
    );

    for item in items {
        if item.selected {
            // The activity that is on screen has a bar on its left: the
            // person has to see which one it is without reading.
            scene.push_quad(
                Rect::new(bar.origin.x, item.rect.origin.y, 2.0, item.rect.size.height),
                MARK_COLOR,
                viewport,
            );
        }
        let color = if item.selected {
            ICON_ACTIVE_COLOR
        } else {
            ICON_COLOR
        };
        let icon = Rect::new(
            item.rect.origin.x + (item.rect.size.width - ICON_SIZE) / 2.0,
            item.rect.origin.y + (ITEM_HEIGHT - ICON_SIZE) / 2.0,
            ICON_SIZE,
            ICON_SIZE,
        );
        draw_icon(scene, viewport, item.activity, icon, color);
        if let Some(count) = item.badge {
            draw_badge(scene, viewport, item.rect, count);
        }
    }
}

/// Draws the icon of one activity.
fn draw_icon(
    scene: &mut FrameScene,
    viewport: Rect,
    activity: Activity,
    icon: Rect,
    color: Color,
) {
    match activity {
        Activity::Files => {
            // A sheet of paper with lines on it.
            icons::outline(scene, viewport, icon, 1.5, color);
            for row in 0..3 {
                let y = icon.origin.y + 5.0 + row as f32 * 4.0;
                let width = if row == 2 {
                    icon.size.width * 0.45
                } else {
                    icon.size.width * 0.6
                };
                scene.push_quad(
                    Rect::new(icon.origin.x + 3.0, y, width, 1.5),
                    color,
                    viewport,
                );
            }
        }
        Activity::Search => {
            // A lens with a handle.
            icons::ring(
                scene,
                viewport,
                Rect::new(icon.origin.x + 1.0, icon.origin.y + 1.0, 13.0, 13.0),
                6.5,
                4.0,
                color,
            );
            scene.push_quad(
                Rect::new(icon.origin.x + 12.0, icon.origin.y + 13.0, 7.0, 2.0),
                color,
                viewport,
            );
        }
        Activity::SourceControl => {
            // A branch: a line with two knots in it.
            scene.push_quad(
                Rect::new(icon.origin.x + 3.0, icon.origin.y + 2.0, 2.0, 16.0),
                color,
                viewport,
            );
            scene.push_quad(
                Rect::new(icon.origin.x + 3.0, icon.origin.y + 5.0, 9.0, 2.0),
                color,
                viewport,
            );
            scene.push_quad(
                Rect::new(icon.origin.x + 10.0, icon.origin.y + 5.0, 2.0, 8.0),
                color,
                viewport,
            );
            icons::disc(
                scene,
                viewport,
                Rect::new(icon.origin.x + 2.0, icon.origin.y + 1.0, 4.0, 4.0),
                2.0,
                color,
            );
            icons::disc(
                scene,
                viewport,
                Rect::new(icon.origin.x + 9.0, icon.origin.y + 11.0, 4.0, 4.0),
                2.0,
                color,
            );
        }
        Activity::Debug => {
            // A triangle that points to the right: it runs the program.
            icons::triangle(
                scene,
                viewport,
                icon.origin.x + 2.0,
                icon.origin.y + 1.0,
                icon.size.height - 2.0,
                true,
                color,
            );
        }
        Activity::Extensions => {
            // Four blocks: one of them is apart from the others.
            let half = (icon.size.width - 5.0) / 2.0;
            for (column, row) in [(0.0, 0.0), (half + 3.0, 0.0), (0.0, half + 3.0)] {
                scene.push_quad(
                    Rect::new(
                        icon.origin.x + column,
                        icon.origin.y + row,
                        half,
                        half,
                    ),
                    color,
                    viewport,
                );
            }
            scene.push_quad(
                Rect::new(
                    icon.origin.x + half + 3.0,
                    icon.origin.y + half + 3.0,
                    half,
                    half,
                ),
                color,
                viewport,
            );
        }
    }
}

/// Draws the number that tells how much there is.
fn draw_badge(scene: &mut FrameScene, viewport: Rect, item: Rect, count: usize) {
    let text = count.to_string();
    // The badge is as wide as its number: a badge that is too wide covers the
    // icon next to it.
    let width = (text.len() as f32 * 7.0 + 8.0).min(item.size.width - 4.0);
    let badge = Rect::new(
        item.max_x() - width - 2.0,
        item.max_y() - BADGE_HEIGHT - 4.0,
        width,
        BADGE_HEIGHT,
    );
    scene.push_quad(badge, BADGE_COLOR, viewport);
    scene.push_text(
        text,
        Rect::new(badge.origin.x, badge.origin.y + 1.0, badge.size.width, BADGE_HEIGHT),
        codeweaver_render::TextStyle {
            clip: viewport,
            color: BADGE_TEXT_COLOR,
            font_size: 10.0,
            line_height: 12.0,
            family: codeweaver_render::TextFamily::SansSerif,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bar of a window that is 800 pixels wide.
    fn bar() -> Rect {
        Rect::new(0.0, 0.0, 48.0, 800.0)
    }

    #[test]
    fn the_bar_has_five_activities_in_the_order_they_are_drawn() {
        let items = ActivityBar::layout(bar(), None, &Badges::default());
        let activities: Vec<Activity> = items.items.iter().map(|item| item.activity).collect();
        assert_eq!(activities, vec![
            Activity::Files,
            Activity::Search,
            Activity::SourceControl,
            Activity::Debug,
            Activity::Extensions,
        ]);
    }

    #[test]
    fn the_activities_are_stacked_one_under_the_other() {
        let items = ActivityBar::layout(bar(), None, &Badges::default());
        assert_eq!(items.items[0].rect.origin.y, 0.0);
        assert_eq!(items.items[1].rect.origin.y, ITEM_HEIGHT);
        assert_eq!(items.items[4].rect.origin.y, 4.0 * ITEM_HEIGHT);
        for item in &items.items {
            assert_eq!(item.rect.origin.x, 0.0);
            assert_eq!(item.rect.size.width, 48.0);
        }
    }

    #[test]
    fn the_activity_that_was_clicked_is_the_one_under_the_point() {
        let items = ActivityBar::layout(bar(), None, &Badges::default());
        assert_eq!(
            items.hit(Point { x: 24.0, y: 10.0 }),
            Some(Activity::Files)
        );
        assert_eq!(
            items.hit(Point { x: 24.0, y: 60.0 }),
            Some(Activity::Search)
        );
        assert_eq!(items.hit(Point { x: 24.0, y: 500.0 }), None);
        assert_eq!(items.hit(Point { x: 100.0, y: 10.0 }), None);
    }

    #[test]
    fn the_activity_that_is_on_screen_is_marked() {
        let items = ActivityBar::layout(bar(), Some(Activity::Debug), &Badges::default());
        assert!(items.items[3].selected);
        assert!(!items.items[0].selected);
    }

    #[test]
    fn a_hidden_side_bar_has_no_activity_on_screen() {
        let items = ActivityBar::layout(bar(), None, &Badges::default());
        assert!(items.items.iter().all(|item| !item.selected));
    }

    #[test]
    fn the_badge_tells_how_much_there_is() {
        let badges = Badges {
            search: 3,
            source_control: 12,
            debug: 0,
        };
        let items = ActivityBar::layout(bar(), None, &badges);
        assert_eq!(items.items[1].badge, Some(3));
        assert_eq!(items.items[2].badge, Some(12));
        // A section with nothing in it has no badge: a zero says nothing.
        assert_eq!(items.items[3].badge, None);
        assert_eq!(items.items[0].badge, None);
    }

    #[test]
    fn the_keyboard_can_walk_through_the_activities() {
        assert_eq!(Activity::Files.next(), Activity::Search);
        assert_eq!(Activity::Search.next(), Activity::SourceControl);
        assert_eq!(Activity::Extensions.next(), Activity::Files);
        assert_eq!(Activity::Files.previous(), Activity::Extensions);
        assert_eq!(Activity::Debug.previous(), Activity::SourceControl);
    }

    #[test]
    fn the_side_bar_starts_with_the_files_of_the_project() {
        let side = SideBar::default();
        assert!(side.is_open());
        assert_eq!(side.selected(), Some(Activity::Files));
    }

    #[test]
    fn picking_an_activity_shows_its_section() {
        let mut side = SideBar::default();
        side.pick(Activity::Debug);
        assert!(side.is_open());
        assert_eq!(side.selected(), Some(Activity::Debug));
    }

    #[test]
    fn picking_the_same_activity_again_hides_the_side_bar() {
        let mut side = SideBar::default();
        side.pick(Activity::Files);
        assert!(!side.is_open());
        assert_eq!(side.selected(), None);
    }

    #[test]
    fn the_section_is_a_state_and_stays_what_it_was() {
        let mut side = SideBar::default();
        side.pick(Activity::Search);
        side.pick(Activity::Debug);
        // The side bar was hidden and shown again: the section it shows is
        // the one that was picked last.
        assert_eq!(side.selected(), Some(Activity::Debug));
        side.pick(Activity::Debug);
        assert_eq!(side.section, Activity::Debug);
    }

    #[test]
    fn the_side_bar_can_be_hidden_and_shown_again() {
        let mut side = SideBar::default();
        side.hide();
        assert!(!side.is_open());
        side.show(Activity::Extensions);
        assert!(side.is_open());
        assert_eq!(side.selected(), Some(Activity::Extensions));
    }
}
