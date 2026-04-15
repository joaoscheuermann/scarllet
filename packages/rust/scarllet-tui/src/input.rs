use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct VisualLine {
    pub byte_start: usize,
    pub byte_end: usize,
    #[allow(dead_code)]
    pub visual_width: u16,
}

#[derive(Debug, Clone, Default)]
pub struct InputState {
    text: String,
    cursor_byte_offset: usize,
    selection_anchor: Option<usize>,
    pub vertical_scroll: u16,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor_byte_offset = self.text.len();
        self.selection_anchor = None;
        self.vertical_scroll = 0;
    }

    #[allow(dead_code)]
    pub fn cursor_byte_offset(&self) -> usize {
        self.cursor_byte_offset
    }

    pub fn selection_range(&self) -> Option<(usize, usize)> {
        if let Some(anchor) = self.selection_anchor {
            if anchor != self.cursor_byte_offset {
                return Some((
                    anchor.min(self.cursor_byte_offset),
                    anchor.max(self.cursor_byte_offset),
                ));
            }
        }
        None
    }

    pub fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.selection_range() {
            self.text.replace_range(start..end, "");
            self.cursor_byte_offset = start;
            self.selection_anchor = None;
            true
        } else {
            false
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let c = if c == '\r' { '\n' } else { c };
        self.delete_selection();
        self.text.insert(self.cursor_byte_offset, c);
        self.cursor_byte_offset += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        let cleaned = s.replace("\r\n", "\n").replace('\r', "\n");
        self.delete_selection();
        self.text.insert_str(self.cursor_byte_offset, &cleaned);
        self.cursor_byte_offset += cleaned.len();
    }

    fn delete_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor_byte_offset == 0 {
            return;
        }
        let (prev_idx, _) = self
            .text
            .grapheme_indices(true)
            .rev()
            .find(|(i, _)| *i < self.cursor_byte_offset)
            .unwrap_or((0, ""));

        self.text
            .replace_range(prev_idx..self.cursor_byte_offset, "");
        self.cursor_byte_offset = prev_idx;
    }

    fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor_byte_offset == self.text.len() {
            return;
        }
        if let Some((_, grapheme)) = self
            .text
            .grapheme_indices(true)
            .find(|(i, _)| *i == self.cursor_byte_offset)
        {
            let end = self.cursor_byte_offset + grapheme.len();
            self.text.replace_range(self.cursor_byte_offset..end, "");
        }
    }

    pub fn visual_lines(&self, wrap_width: u16) -> Vec<VisualLine> {
        let wrap_width = wrap_width.max(1) as usize;
        let mut lines = Vec::new();
        let mut byte_offset = 0;

        let parts: Vec<&str> = self.text.split('\n').collect();
        for (i, line) in parts.iter().enumerate() {
            if i > 0 {
                byte_offset += 1; // for '\n'
            }
            if line.is_empty() {
                lines.push(VisualLine {
                    byte_start: byte_offset,
                    byte_end: byte_offset,
                    visual_width: 0,
                });
                continue;
            }

            let mut current_start = byte_offset;
            let mut current_width = 0;
            let mut current_end = byte_offset;

            for g in line.graphemes(true) {
                let gw = g.width();
                if current_width + gw > wrap_width && current_width > 0 {
                    lines.push(VisualLine {
                        byte_start: current_start,
                        byte_end: current_end,
                        visual_width: current_width as u16,
                    });
                    current_start = current_end;
                    current_width = 0;
                }
                current_width += gw;
                current_end += g.len();
            }
            if current_start < current_end {
                lines.push(VisualLine {
                    byte_start: current_start,
                    byte_end: current_end,
                    visual_width: current_width as u16,
                });
            }
            byte_offset += line.len();
        }
        if lines.is_empty() {
            lines.push(VisualLine {
                byte_start: 0,
                byte_end: 0,
                visual_width: 0,
            });
        }
        lines
    }

    pub fn cursor_visual_position(&self, wrap_width: u16) -> (u16, u16) {
        let lines = self.visual_lines(wrap_width);
        for (i, line) in lines.iter().enumerate() {
            if self.cursor_byte_offset >= line.byte_start && self.cursor_byte_offset < line.byte_end
            {
                let slice = &self.text[line.byte_start..self.cursor_byte_offset];
                return (slice.width() as u16, i as u16);
            }
        }
        // Fallback: Check if cursor is at the very end of a line
        for (i, line) in lines.iter().enumerate().rev() {
            if self.cursor_byte_offset == line.byte_end {
                let slice = &self.text[line.byte_start..self.cursor_byte_offset];
                return (slice.width() as u16, i as u16);
            }
        }
        (0, 0)
    }

    pub fn is_at_top(&self, wrap_width: u16) -> bool {
        let (_, row) = self.cursor_visual_position(wrap_width);
        row == 0
    }

    fn set_selection(&mut self, shift: bool) {
        if shift {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor_byte_offset);
            }
        } else {
            self.selection_anchor = None;
        }
    }

    fn move_horizontal(&mut self, delta: i32, shift: bool) {
        self.set_selection(shift);
        if delta < 0 {
            if self.cursor_byte_offset > 0 {
                if let Some((prev_idx, _)) = self
                    .text
                    .grapheme_indices(true)
                    .rev()
                    .find(|(i, _)| *i < self.cursor_byte_offset)
                {
                    self.cursor_byte_offset = prev_idx;
                }
            }
        } else if delta > 0 {
            if self.cursor_byte_offset < self.text.len() {
                if let Some((i, g)) = self
                    .text
                    .grapheme_indices(true)
                    .find(|(i, _)| *i >= self.cursor_byte_offset)
                {
                    self.cursor_byte_offset = i + g.len();
                }
            }
        }
    }

    fn move_vertical(&mut self, delta: i32, shift: bool, wrap_width: u16) {
        self.set_selection(shift);
        let lines = self.visual_lines(wrap_width);
        let (current_col, current_row) = self.cursor_visual_position(wrap_width);

        let target_row = current_row as i32 + delta;
        if target_row < 0 || target_row >= lines.len() as i32 {
            return;
        }

        let target_line = &lines[target_row as usize];
        let mut new_offset = target_line.byte_start;
        let mut current_w = 0;

        let slice = &self.text[target_line.byte_start..target_line.byte_end];
        for g in slice.graphemes(true) {
            let gw = g.width() as u16;
            if current_w + gw > current_col {
                break;
            }
            current_w += gw;
            new_offset += g.len();
        }
        self.cursor_byte_offset = new_offset;
    }

    pub fn handle_key_event(&mut self, key: KeyEvent, wrap_width: u16) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Left => self.move_horizontal(-1, shift),
            KeyCode::Right => self.move_horizontal(1, shift),
            KeyCode::Up => self.move_vertical(-1, shift, wrap_width),
            KeyCode::Down => self.move_vertical(1, shift, wrap_width),
            KeyCode::Backspace => self.delete_backward(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Home => {
                self.set_selection(shift);
                let lines = self.visual_lines(wrap_width);
                let (_, current_row) = self.cursor_visual_position(wrap_width);
                if let Some(line) = lines.get(current_row as usize) {
                    self.cursor_byte_offset = line.byte_start;
                }
            }
            KeyCode::End => {
                self.set_selection(shift);
                let lines = self.visual_lines(wrap_width);
                let (_, current_row) = self.cursor_visual_position(wrap_width);
                if let Some(line) = lines.get(current_row as usize) {
                    self.cursor_byte_offset = line.byte_end;
                }
            }
            KeyCode::Char(c) => self.insert_char(c),
            _ => {}
        }
    }
}
