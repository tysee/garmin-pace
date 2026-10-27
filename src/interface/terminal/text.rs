//! src/interface/terminal/text.rs fits escaped text to bounded terminal columns.
//! Clipping is visible, editing tails retain the cursor, and wrapping preserves words.

pub(super) fn cells(text: &str) -> usize {
    text.chars().map(cell_width).sum()
}

// East Asian wide and fullwidth scripts and emoji take two cells; other symbols one.
fn cell_width(c: char) -> usize {
    match u32::from(c) {
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300.. => 2,
        _ => 1,
    }
}

fn escaped(text: &str) -> String {
    let mut clean = String::new();
    for c in text.chars() {
        if matches!(c, '\'' | '"' | '\\') {
            clean.push(c);
        } else {
            clean.extend(c.escape_debug());
        }
    }
    clean
}

pub(super) fn clipped(text: &str, width: usize) -> String {
    let clean = escaped(text);
    if cells(&clean) <= width {
        return clean;
    }
    let mut used = 0;
    let mut result: String = clean
        .chars()
        .take_while(|c| {
            used += cell_width(*c);
            used <= width.saturating_sub(3)
        })
        .collect();
    result.push_str(&".".repeat(width.min(3)));
    result
}

pub(super) fn tail(text: &str, width: usize) -> String {
    if cells(text) <= width {
        return text.into();
    }
    let mut used = 0;
    let suffix: Vec<_> = text
        .chars()
        .rev()
        .take_while(|c| {
            used += cell_width(*c);
            used < width
        })
        .collect();
    format!("<{}", suffix.into_iter().rev().collect::<String>())
}

pub(super) fn wrapped(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let clean = escaped(line);
        let mut current = String::new();
        let mut used = 0;
        for word in clean.split_inclusive(' ') {
            if used > 0 && used + cells(word) > width {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            for c in word.chars() {
                if used + cell_width(c) > width && !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    used = 0;
                }
                current.push(c);
                used += cell_width(c);
            }
        }
        lines.push(current);
    }
    lines
}
