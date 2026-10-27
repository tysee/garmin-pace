//! src/interface/terminal/brand.rs draws Pace's ginger cat mascot beside the title.
//! Half-block pixel art needs no image protocol; a four-pixel face keeps compact headers short.

use std::fmt::{Result, Write};

// Face at two pixels per terminal row: outline k, fur p/h, stripes r, ears n, muzzle w, glints g.
const CAT: [&[u8; 18]; 10] = [
    b"..kk..........kk..",
    b".khpk........kphk.",
    b".kpnpkkkkkkkkpnpk.",
    b".kppprpppppprpppk.",
    b"kpppprpprrpprppppk",
    b"kppkkkppppppkkkppk",
    b"kpkgkkkppppkgkkkpk",
    b"kpkkkkkpwwpkkkkkpk",
    b".kppkkpwwnwwpkkpk.",
    b"..kkkkkkwwkkkkkk..",
];

const MINI: [&[u8; 8]; 4] = [b".k....k.", b".kpkkpk.", b"kpkppkpk", b".kppppk."];

pub(super) fn tall(width: usize, height: usize, compact: bool) -> bool {
    !compact && width >= 76 && height >= 22
}

pub(super) fn rows(tall: bool) -> usize {
    if tall { CAT.len() / 2 } else { MINI.len() / 2 }
}

pub(super) fn draw(frame: &mut String, tall: bool, title: &str) -> Result {
    let title = if title.is_empty() {
        String::new()
    } else {
        format!(" / {title}")
    };
    if tall {
        for (row, [top_row, bottom_row]) in CAT.as_chunks::<2>().0.iter().enumerate() {
            frame.push(' ');
            for (&top, &bottom) in top_row.iter().zip(*bottom_row) {
                pixel(frame, top, bottom)?;
            }
            match row {
                1 => write!(frame, "   \x1b[1mPace{title}\x1b[0m")?,
                2 => frame.push_str("   \x1b[90m(for Garmin)\x1b[0m"),
                _ => {}
            }
            frame.push_str("\r\n");
        }
    } else {
        for (row, [top_row, bottom_row]) in MINI.as_chunks::<2>().0.iter().enumerate() {
            frame.push(' ');
            for (&top, &bottom) in top_row.iter().zip(*bottom_row) {
                pixel(frame, top, bottom)?;
            }
            if row == 0 {
                write!(frame, "  \x1b[1mPace{title}\x1b[0m")?;
            } else {
                frame.push_str("  \x1b[90m(for Garmin)\x1b[0m");
            }
            frame.push_str("\r\n");
        }
    }
    Ok(())
}

fn color(pixel: u8) -> Option<u8> {
    match pixel {
        b'p' => Some(208), // Ginger fur.
        b'h' => Some(215), // Ear highlights.
        b'r' => Some(130), // Tabby stripes.
        b'n' => Some(211), // Inner ears and nose.
        b'w' => Some(230), // Muzzle.
        b'g' => Some(231), // Eye glints.
        b'k' => Some(236), // Outline and eyes.
        _ => None,
    }
}

fn pixel(frame: &mut String, top: u8, bottom: u8) -> Result {
    match (color(top), color(bottom)) {
        (Some(top), Some(bottom)) => write!(frame, "\x1b[38;5;{top};48;5;{bottom}m▀\x1b[0m"),
        (Some(top), None) => write!(frame, "\x1b[38;5;{top}m▀\x1b[0m"),
        (None, Some(bottom)) => write!(frame, "\x1b[38;5;{bottom}m▄\x1b[0m"),
        (None, None) => {
            frame.push(' ');
            Ok(())
        }
    }
}
