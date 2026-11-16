//! The applications on one long page: grouped by kind under the latest started (the
//! shelf), or the kind chosen in a list beside them (the sidebar). The wheel scrolls it,
//! the arrows move from one application to the next, and typing narrows it down.

use std::collections::HashMap;

use smithay::input::keyboard::Keysym;

use super::*;

/// How many of the latest started are kept, and shown first.
pub(super) const RECENT: usize = 8;
const SHELF_CELL: (i32, i32) = (104, 108);
const SHELF_ICON: i32 = 56;
const SIDE_CELL: (i32, i32) = (132, 124);
const SIDE_ICON: i32 = 64;
const HEADING: i32 = 46;
const SIDEBAR_WIDTH: i32 = 264;
const SIDEBAR_ROW: i32 = 34;
pub(super) const SIDEBAR_RADIUS: i32 = 16;
/// How far one turn of the wheel scrolls, in the wheel's units.
const SCROLL_STEP: f64 = 3.0;
/// The kinds, in the order they are shown; the last is for what says none.
const KINDS: [&str; 8] = [
    "網際網路",
    "工具程式",
    "辦公",
    "圖片與影音",
    "開發",
    "遊戲",
    "系統",
    "資料夾",
];

/// What the long page was drawn with: what was typed, how far it was scrolled, what the
/// pointer and the arrows were on, the kind chosen, and its lines.
pub(super) type ListDrawn = (String, i64, Option<usize>, Option<usize>, usize, Vec<Line>);
/// What the sidebar was drawn with: its items, the one chosen and the one pointed at.
pub(super) type SidebarDrawn = (Vec<(String, usize)>, usize, Option<usize>);

/// A line of the long page: a kind's name with how many there are, or a row of them.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Line {
    Heading(String, usize),
    Icons(Vec<usize>),
}

impl Line {
    fn height(&self, cell: (i32, i32)) -> i32 {
        match self {
            Line::Heading(..) => HEADING,
            Line::Icons(_) => cell.1,
        }
    }
}

fn kind_label(kind: &str) -> &str {
    if kind == "資料夾" {
        "其他"
    } else {
        kind
    }
}

fn cell(style: AppsView) -> (i32, i32) {
    if style == AppsView::Sidebar {
        SIDE_CELL
    } else {
        SHELF_CELL
    }
}

fn icon(style: AppsView) -> i32 {
    if style == AppsView::Sidebar {
        SIDE_ICON
    } else {
        SHELF_ICON
    }
}

/// Where the long page, its search field and the sidebar go on the output.
pub(super) fn layout(style: AppsView, area: Rectangle<i32, Logical>) -> Layout {
    let bottom = area.loc.y + area.size.h - crate::dock::DOCK_SPACE - 12;
    let (cell_width, _) = cell(style);
    if style == AppsView::Sidebar {
        let sidebar = Rectangle::new(
            (area.loc.x + 16, area.loc.y + 16).into(),
            (SIDEBAR_WIDTH, bottom - area.loc.y - 16).into(),
        );
        let search = Rectangle::new(
            sidebar.loc + Point::from((12, 12)),
            (SIDEBAR_WIDTH - 24, 36).into(),
        );
        let left = sidebar.loc.x + SIDEBAR_WIDTH + 40;
        let columns = ((area.loc.x + area.size.w - 40 - left) / cell_width).clamp(3, 8) as usize;
        let grid = Rectangle::new(
            (left, area.loc.y + 40).into(),
            (columns as i32 * cell_width, bottom - area.loc.y - 40).into(),
        );
        return Layout {
            search,
            grid,
            columns,
            rows: 0,
            sidebar: Some(sidebar),
        };
    }
    let columns = ((area.size.w - 120) / cell_width).clamp(4, 10) as usize;
    let width = columns as i32 * cell_width;
    let left = area.loc.x + (area.size.w - width) / 2;
    let search = Rectangle::new((left, area.loc.y + 40).into(), (width, 44).into());
    let top = search.loc.y + search.size.h + 20;
    Layout {
        search,
        grid: Rectangle::new((left, top).into(), (width, bottom - top).into()),
        columns,
        rows: 0,
        sidebar: None,
    }
}

impl AppGrid {
    pub(super) fn recent_apps(&self) -> Vec<usize> {
        self.recent
            .iter()
            .filter_map(|id| self.apps.iter().position(|app| app.id == *id))
            .take(RECENT)
            .collect()
    }

    /// The sidebar's items: all of them, the latest started, then each kind there is.
    pub(super) fn sidebar_items(&self) -> Vec<(String, Vec<usize>)> {
        let mut items = vec![
            ("全部".to_string(), (0..self.apps.len()).collect()),
            ("最近使用".to_string(), self.recent_apps()),
        ];
        items.extend(
            self.kinds()
                .into_iter()
                .map(|(kind, apps)| (kind_label(kind).to_string(), apps)),
        );
        items
    }

    fn kinds(&self) -> Vec<(&'static str, Vec<usize>)> {
        KINDS
            .iter()
            .map(|kind| {
                (
                    *kind,
                    (0..self.apps.len())
                        .filter(|&app| self.apps[app].category == *kind)
                        .collect(),
                )
            })
            .filter(|(_, apps): &(&str, Vec<usize>)| !apps.is_empty())
            .collect()
    }

    /// The long page's lines: what matches what was typed; on the shelf, the latest
    /// started then every kind; in the sidebar, the kind chosen.
    pub(super) fn lines(&self, columns: usize) -> Vec<Line> {
        let rows = |apps: &[usize]| -> Vec<Line> {
            apps.chunks(columns.max(1))
                .map(|row| Line::Icons(row.to_vec()))
                .collect()
        };
        if !self.query.is_empty() {
            return rows(&self.matches());
        }
        let mut lines = Vec::new();
        let groups: Vec<(String, Vec<usize>)> = if self.style == AppsView::Sidebar {
            self.sidebar_items()
                .into_iter()
                .nth(self.category)
                .into_iter()
                .collect()
        } else {
            let mut groups = vec![("最近使用".to_string(), self.recent_apps())];
            groups.extend(
                self.kinds()
                    .into_iter()
                    .map(|(kind, apps)| (kind_label(kind).to_string(), apps)),
            );
            groups.into_iter().filter(|(_, apps)| !apps.is_empty()).collect()
        };
        for (name, apps) in groups {
            lines.push(Line::Heading(name, apps.len()));
            lines.extend(rows(&apps));
        }
        lines
    }

    /// Every application on the long page, in the order the arrows go through them.
    fn listed(&self, columns: usize) -> Vec<usize> {
        self.lines(columns)
            .into_iter()
            .flat_map(|line| match line {
                Line::Icons(apps) => apps,
                Line::Heading(..) => Vec::new(),
            })
            .collect()
    }

    /// The application at a point of the long page.
    fn app_at(&self, layout: &Layout, pos: Point<f64, Logical>) -> Option<usize> {
        if !layout.grid.to_f64().contains(pos) {
            return None;
        }
        let cell = cell(self.style);
        let mut top = 0.0;
        let y = pos.y - f64::from(layout.grid.loc.y) + self.scroll;
        for line in self.lines(layout.columns) {
            let height = f64::from(line.height(cell));
            if y >= top && y < top + height {
                let Line::Icons(apps) = line else {
                    return None;
                };
                let column = ((pos.x - f64::from(layout.grid.loc.x)) / f64::from(cell.0)) as usize;
                return apps.get(column).copied();
            }
            top += height;
        }
        None
    }

    /// Which of the sidebar's items is at a point.
    fn item_at(&self, layout: &Layout, pos: Point<f64, Logical>) -> Option<usize> {
        let sidebar = layout.sidebar?.to_f64();
        if !sidebar.contains(pos) {
            return None;
        }
        let first = sidebar.loc.y + f64::from(item_top());
        let index = ((pos.y - first) / f64::from(SIDEBAR_ROW)).floor();
        (index >= 0.0 && (index as usize) < self.sidebar_items().len()).then_some(index as usize)
    }

    /// How far down the page goes past what is in view.
    fn max_scroll(&self, layout: &Layout) -> f64 {
        let cell = cell(self.style);
        let height: i32 = self
            .lines(layout.columns)
            .iter()
            .map(|line| line.height(cell))
            .sum();
        f64::from((height - layout.grid.size.h).max(0))
    }

    /// Scrolls so that the application the arrows came to is in view.
    fn scroll_to(&mut self, layout: &Layout, app: usize) {
        let cell = cell(self.style);
        let mut top = 0;
        for line in self.lines(layout.columns) {
            let height = line.height(cell);
            if matches!(&line, Line::Icons(apps) if apps.contains(&app)) {
                let (top, bottom) = (f64::from(top), f64::from(top + height));
                if top < self.scroll {
                    self.scroll = top;
                } else if bottom > self.scroll + f64::from(layout.grid.size.h) {
                    self.scroll = bottom - f64::from(layout.grid.size.h);
                }
                return;
            }
            top += height;
        }
    }
}

/// Where the sidebar's items start, under its search field.
fn item_top() -> i32 {
    12 + 36 + 16
}

impl Nekoland {
    /// Draws the long page and the sidebar again when what they show changed.
    pub(super) fn refresh_list(
        &mut self,
        layout: &Layout,
        pointer: Option<Point<f64, Logical>>,
    ) -> Option<()> {
        let apps = self.overview.as_mut()?.apps.as_mut()?;
        let max = apps.max_scroll(layout);
        apps.scroll = apps.scroll.clamp(0.0, max);
        let hovered = pointer.and_then(|pos| apps.app_at(layout, pos));
        apps.hovered = hovered;
        let lines = apps.lines(layout.columns);
        let key = (
            apps.query.clone(),
            apps.scroll.round() as i64,
            hovered,
            apps.focus,
            apps.category,
            lines.clone(),
        );
        let style = apps.style;
        let (scroll, focus) = (apps.scroll, apps.focus);
        let items: Vec<(String, usize)> = apps
            .sidebar_items()
            .into_iter()
            .map(|(name, apps)| (name, apps.len()))
            .collect();
        let sidebar_key = (
            items.clone(),
            apps.category,
            pointer.and_then(|pos| apps.item_at(layout, pos)),
        );
        let sidebar_stale = layout.sidebar.is_some()
            && apps
                .sidebar_picture
                .as_ref()
                .is_none_or(|(drawn, _)| *drawn != sidebar_key);
        let list_stale = apps.list_drawn.as_ref() != Some(&key) || apps.list_picture.is_none();
        if list_stale {
            // The pictures of what is in view.
            let cell = cell(style);
            let mut top = 0.0;
            let mut shown = Vec::new();
            for line in &lines {
                let height = f64::from(line.height(cell));
                if top + height > scroll && top < scroll + f64::from(layout.grid.size.h) {
                    if let Line::Icons(row) = line {
                        shown.extend(row.iter().copied());
                    }
                }
                top += height;
            }
            let tiles: HashMap<usize, Tile> = shown
                .into_iter()
                .filter_map(|app| Some((app, self.grid_tile(Entry::App(app), icon(style))?)))
                .collect();
            let picture = draw_list(layout, style, &lines, scroll, &tiles, hovered, focus);
            let apps = self.overview.as_mut()?.apps.as_mut()?;
            apps.list_picture = picture;
            apps.list_drawn = Some(key);
        }
        if sidebar_stale {
            let sidebar = layout.sidebar?;
            let apps = self.overview.as_mut()?.apps.as_mut()?;
            apps.sidebar_picture = draw_sidebar(sidebar.size, &items, sidebar_key.1, sidebar_key.2)
                .map(|picture| (sidebar_key, picture));
        }
        Some(())
    }

    pub(super) fn list_press(&mut self, pos: Point<f64, Logical>) {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let Some(layout) = apps.layout else {
            return;
        };
        if layout.search.to_f64().contains(pos) {
            let query = apps.query.clone();
            config::spawn(&format!(
                "{} {}{}",
                self.config.launcher,
                self.config.launcher_search,
                config::quote(&query)
            ));
            self.close_overview(None);
            return;
        }
        if let Some(item) = apps.item_at(&layout, pos) {
            apps.category = item;
            apps.scroll = 0.0;
            apps.focus = None;
            apps.query.clear();
            return;
        }
        if layout
            .sidebar
            .is_some_and(|sidebar| sidebar.to_f64().contains(pos))
        {
            return;
        }
        match apps.app_at(&layout, pos) {
            Some(app) => apps.press = Some((Pressed::App(app), pos)),
            None if layout.grid.to_f64().contains(pos) => {}
            None => self.close_overview(None),
        }
    }

    /// A press far enough becomes a drag, which can only keep the application in the Dock.
    pub(super) fn list_motion(&mut self, pos: Point<f64, Logical>) {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        if let Some((Pressed::App(app), start)) = apps.press {
            if (pos.x - start.x).hypot(pos.y - start.y) > DRAG_THRESHOLD {
                let half = f64::from(HELD_SIZE) / 2.0;
                apps.held = Some(Held {
                    entry: Entry::App(app),
                    from: Source::Grid,
                    grab: Point::from((half, half)),
                });
                apps.press = None;
                apps.held_picture = None;
            }
        }
        if let Some(Held {
            entry: Entry::App(app),
            ..
        }) = apps.held
        {
            let id = apps.apps[app].id.clone();
            self.offer_to_dock(id);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        if apps.held.is_some() && apps.held_picture.is_none() {
            let entry = apps.held.as_ref().map(|held| held.entry);
            let tile = entry.and_then(|entry| self.grid_tile(entry, HELD_SIZE));
            if let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) {
                apps.held_picture = tile.and_then(|tile| draw_held(&tile));
            }
        }
    }

    pub(super) fn list_release(&mut self, pos: Point<f64, Logical>) {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let layout = apps.layout;
        if let Some((Pressed::App(app), _)) = apps.press.take() {
            if layout.and_then(|layout| apps.app_at(&layout, pos)) == Some(app) {
                self.start_grid_app(app, None);
            }
            return;
        }
        let Some(held) = apps.held.take() else {
            return;
        };
        apps.held_picture = None;
        let Entry::App(app) = held.entry else {
            return;
        };
        let id = apps.apps[app].id.clone();
        self.drop_on_dock(id, pos);
    }

    /// The arrows move from one application to the next, Return starts it, Page Up and
    /// Down scroll; typing narrows the page down, Escape clears it or leaves.
    pub(super) fn list_key(&mut self, sym: Keysym, typed: Option<char>) -> bool {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return false;
        };
        let Some(layout) = apps.layout else {
            return false;
        };
        let listed = apps.listed(layout.columns);
        let at = apps
            .focus
            .and_then(|focus| listed.iter().position(|app| *app == focus));
        let step = |delta: isize| -> Option<usize> {
            let at = at.map_or(0, |at| {
                at.saturating_add_signed(delta)
                    .min(listed.len().saturating_sub(1))
            });
            listed.get(at).copied()
        };
        let columns = layout.columns as isize;
        let moved = match sym {
            Keysym::Left => step(-1),
            Keysym::Right => step(if at.is_some() { 1 } else { 0 }),
            Keysym::Up => step(-columns),
            Keysym::Down => step(if at.is_some() { columns } else { 0 }),
            Keysym::Page_Up | Keysym::Page_Down => {
                let page = f64::from(layout.grid.size.h) * if sym == Keysym::Page_Up { -0.8 } else { 0.8 };
                apps.scroll += page;
                return true;
            }
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(app) = apps.focus.or(apps.hovered).or_else(|| listed.first().copied()) {
                    self.start_grid_app(app, None);
                }
                return true;
            }
            Keysym::Escape if !apps.query.is_empty() => {
                apps.query.clear();
                apps.scroll = 0.0;
                return true;
            }
            Keysym::Escape => {
                self.close_overview(None);
                return true;
            }
            Keysym::BackSpace => {
                apps.query.pop();
                apps.scroll = 0.0;
                apps.focus = None;
                return true;
            }
            _ => match typed {
                Some(character) if !character.is_control() => {
                    apps.query.push(character);
                    apps.scroll = 0.0;
                    apps.focus = None;
                    return true;
                }
                _ => return false,
            },
        };
        if let Some(app) = moved {
            apps.focus = Some(app);
            apps.scroll_to(&layout, app);
        }
        true
    }

    pub(super) fn list_scroll(&mut self, amount: f64) -> bool {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return false;
        };
        apps.scroll += amount * SCROLL_STEP;
        true
    }
}

/// The part of the long page in view: each kind's name and count, then its applications,
/// the pointed-at one shaded and the one the arrows came to ringed.
fn draw_list(
    layout: &Layout,
    style: AppsView,
    lines: &[Line],
    scroll: f64,
    tiles: &HashMap<usize, Tile>,
    hovered: Option<usize>,
    focus: Option<usize>,
) -> Option<Picture> {
    let size = layout.grid.size;
    let cell = cell(style);
    let surface = text::canvas(size.w.max(1), size.h.max(1)).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        let mut top = -scroll;
        for line in lines {
            let height = f64::from(line.height(cell));
            if top + height < 0.0 || top > f64::from(size.h) {
                top += height;
                continue;
            }
            match line {
                Line::Heading(name, count) => {
                    let heading =
                        text_layout(&cr, name, if style == AppsView::Sidebar { 16.0 } else { 11.0 });
                    let bold = if style == AppsView::Sidebar {
                        "Bold 16"
                    } else {
                        "Bold 11"
                    };
                    heading.set_font_description(Some(&crate::text::font(&format!("{FONT} {bold}"))));
                    let (width, height_text) = heading.pixel_size();
                    let baseline = top + f64::from(HEADING) - f64::from(height_text) - 10.0;
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.92);
                    cr.move_to(4.0, baseline);
                    pangocairo::functions::show_layout(&cr, &heading);
                    let number = text_layout(&cr, &count.to_string(), 9.0);
                    let (_, number_height) = number.pixel_size();
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.5);
                    cr.move_to(
                        f64::from(width) + 12.0,
                        baseline + f64::from(height_text - number_height) - 1.0,
                    );
                    pangocairo::functions::show_layout(&cr, &number);
                }
                Line::Icons(apps) => {
                    for (column, app) in apps.iter().enumerate() {
                        let x = f64::from(column as i32 * cell.0);
                        if hovered == Some(*app) || focus == Some(*app) {
                            text::rounded_rectangle(
                                &cr,
                                x + 3.0,
                                top + 2.0,
                                f64::from(cell.0) - 6.0,
                                f64::from(cell.1) - 6.0,
                                12.0,
                            );
                            cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                            cr.fill_preserve().ok()?;
                            if focus == Some(*app) {
                                cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
                                cr.set_line_width(1.5);
                                cr.stroke().ok()?;
                            } else {
                                cr.new_path();
                            }
                        }
                        if let Some(tile) = tiles.get(app) {
                            paint_tile_in(&cr, x, top, tile, cell.0, icon(style))?;
                        }
                    }
                }
            }
            top += height;
        }
    }
    text::picture(surface)
}

/// The sidebar on its pane: every item with its count, the chosen one solid.
fn draw_sidebar(
    size: Size<i32, Logical>,
    items: &[(String, usize)],
    chosen: usize,
    hovered: Option<usize>,
) -> Option<Picture> {
    let surface = text::canvas(size.w, size.h).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint_pane(
            &cr,
            f64::from(size.w),
            f64::from(size.h),
            f64::from(SIDEBAR_RADIUS),
            true,
            Tier::Menu,
        );
        let (ink, paper) = ((0.945, 0.937, 0.918), (0.090, 0.090, 0.102));
        for (index, (name, count)) in items.iter().enumerate() {
            let y = f64::from(item_top() + index as i32 * SIDEBAR_ROW);
            let (x, width) = (12.0, f64::from(size.w) - 24.0);
            let lit = index == chosen;
            if lit || hovered == Some(index) {
                text::rounded_rectangle(&cr, x, y, width, f64::from(SIDEBAR_ROW - 2), 8.0);
                if lit {
                    cr.set_source_rgb(ink.0, ink.1, ink.2);
                } else {
                    cr.set_source_rgba(ink.0, ink.1, ink.2, 0.10);
                }
                cr.fill().ok()?;
            }
            let color = if lit { paper } else { ink };
            let label = text_layout(&cr, name, 10.0);
            let (_, height) = label.pixel_size();
            let middle = y + f64::from(SIDEBAR_ROW - 2) / 2.0;
            cr.set_source_rgb(color.0, color.1, color.2);
            cr.move_to(x + 12.0, middle - f64::from(height) / 2.0);
            pangocairo::functions::show_layout(&cr, &label);
            let number = text_layout(&cr, &count.to_string(), 9.0);
            let (number_width, number_height) = number.pixel_size();
            cr.set_source_rgba(color.0, color.1, color.2, if lit { 0.6 } else { 0.5 });
            cr.move_to(
                x + width - 12.0 - f64::from(number_width),
                middle - f64::from(number_height) / 2.0,
            );
            pangocairo::functions::show_layout(&cr, &number);
        }
    }
    text::picture(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidebar_leaves_room_for_the_page() {
        let area = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let side = layout(AppsView::Sidebar, area);
        let sidebar = side.sidebar.unwrap();
        assert!(side.grid.loc.x >= sidebar.loc.x + sidebar.size.w);
        assert!(side.grid.loc.x + side.grid.size.w <= 1920);
        let shelf = layout(AppsView::Shelf, area);
        assert!(shelf.sidebar.is_none());
        assert_eq!(shelf.grid.size.w, shelf.columns as i32 * SHELF_CELL.0);
    }
}
