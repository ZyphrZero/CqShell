use unicode_width::UnicodeWidthStr;

/// The cursor always sits on a UTF-8 boundary. Width-based clipping keeps CJK input visible.
#[derive(Debug, Clone, Default)]
pub(super) struct InputBuffer {
    pub text: String,
    pub cursor: usize,
}
impl InputBuffer {
    pub fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .last()
                .map_or(0, |(at, _)| at);
        }
    }
    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }
    pub fn backspace(&mut self) {
        let old = self.cursor;
        self.left();
        self.text.drain(self.cursor..old);
    }
    pub fn delete(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text.drain(self.cursor..self.cursor + c.len_utf8());
        }
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
    pub fn replace(&mut self, text: String) {
        *self = Self::new(text);
    }
    pub fn visible(&self, width: usize) -> (String, u16) {
        let room = width.saturating_sub(1);
        let mut start = 0;
        while self.text[start..self.cursor].width() > room {
            start += self.text[start..].chars().next().unwrap().len_utf8();
        }
        let mut end = self.cursor;
        while let Some(c) = self.text[end..].chars().next() {
            let next = end + c.len_utf8();
            if self.text[start..next].width() > width {
                break;
            }
            end = next;
        }
        (
            self.text[start..end].into(),
            self.text[start..self.cursor].width() as u16,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_and_scrolling_preserve_unicode_boundaries() {
        let mut input = InputBuffer::new("中a文".into());
        input.left();
        input.backspace();
        input.insert('字');
        assert_eq!(input.text, "中字文");
        input.delete();
        assert_eq!(input.text, "中字");
        input.replace("abc中文路径".into());
        let (visible, cursor) = input.visible(6);
        assert!(visible.ends_with("路径"));
        assert!(cursor < 6);
        input.clear();
        assert_eq!(input.cursor, 0);
    }
}
