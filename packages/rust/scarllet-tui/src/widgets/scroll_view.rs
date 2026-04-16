use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

/// An item that can be measured and painted inside a `ScrollView`.
pub trait ScrollItem {
    /// The item's total height in terminal rows.
    fn height(&self) -> u16;
    /// Renders the item into the given area of the buffer.
    fn render_ref(&self, area: Rect, buf: &mut Buffer);
}

/// Persistent scroll position and content height for a `ScrollView`.
#[derive(Debug, Clone, Default)]
pub struct ScrollViewState {
    pub offset_y: u16,
    pub content_height: u16,
}

impl ScrollViewState {
    /// Returns a state at scroll position zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the maximum valid scroll offset for the current content height.
    pub fn max_offset(&self, viewport_height: u16) -> u16 {
        self.content_height.saturating_sub(viewport_height)
    }

    /// Adjusts the scroll offset so the item at `item_y` is fully visible.
    pub fn ensure_visible(&mut self, item_y: u16, item_height: u16, viewport_height: u16) {
        if item_height >= viewport_height {
            self.offset_y = (item_y + item_height).saturating_sub(viewport_height);
        } else if item_y < self.offset_y {
            self.offset_y = item_y;
        } else if item_y + item_height > self.offset_y + viewport_height {
            self.offset_y = (item_y + item_height).saturating_sub(viewport_height);
        }
    }

    /// Pins the scroll position to the bottom (auto-follow mode).
    pub fn scroll_to_bottom(&mut self) {
        self.offset_y = u16::MAX;
    }
}

/// Stateless renderer that paints a list of `ScrollItem`s into a viewport with clipping.
pub struct ScrollView;

impl ScrollView {
    /// Lays out items vertically with the given gap, clips to the viewport,
    /// and auto-follows when the user was at the bottom before new content arrived.
    pub fn render(
        area: Rect,
        buf: &mut Buffer,
        state: &mut ScrollViewState,
        items: &[&dyn ScrollItem],
        gap: u16,
    ) {
        let viewport_height = area.height;
        let width = area.width;

        if items.is_empty() || viewport_height == 0 || width == 0 {
            state.content_height = 0;
            return;
        }

        let mut positions: Vec<u16> = Vec::with_capacity(items.len());
        let mut y: u16 = 0;
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                y = y.saturating_add(gap);
            }
            positions.push(y);
            y = y.saturating_add(item.height());
        }
        let total_height = y;

        let old_max = state.content_height.saturating_sub(viewport_height);
        let was_at_bottom = state.offset_y >= old_max;

        state.content_height = total_height;
        let new_max = state.max_offset(viewport_height);

        if was_at_bottom {
            state.offset_y = new_max;
        } else {
            state.offset_y = state.offset_y.min(new_max);
        }

        let scroll_y = state.offset_y;

        for (i, item) in items.iter().enumerate() {
            let item_y = positions[i];
            let item_h = item.height();
            let item_bottom = item_y.saturating_add(item_h);

            if item_bottom <= scroll_y || item_y >= scroll_y.saturating_add(viewport_height) {
                continue;
            }

            if item_y >= scroll_y && item_bottom <= scroll_y.saturating_add(viewport_height) {
                let screen_y = item_y - scroll_y;
                let render_area = Rect::new(area.x, area.y + screen_y, width, item_h);
                item.render_ref(render_area, buf);
            } else {
                let temp_area = Rect::new(0, 0, width, item_h);
                let mut temp_buf = Buffer::empty(temp_area);
                item.render_ref(temp_area, &mut temp_buf);

                let vis_start = scroll_y.saturating_sub(item_y);
                let vis_end = scroll_y
                    .saturating_add(viewport_height)
                    .saturating_sub(item_y)
                    .min(item_h);

                for ty in vis_start..vis_end {
                    let screen_y = (item_y + ty).saturating_sub(scroll_y);
                    for x in 0..width {
                        if let Some(src) = temp_buf.cell(Position::new(x, ty)) {
                            if let Some(dest) =
                                buf.cell_mut(Position::new(area.x + x, area.y + screen_y))
                            {
                                *dest = src.clone();
                            }
                        }
                    }
                }
            }
        }
    }
}
