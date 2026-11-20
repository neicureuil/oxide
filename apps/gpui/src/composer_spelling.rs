//! Misspelled words of the draft, from the system's spell checker, as AppKit's continuous spell checking underlines
//! them in the macOS composer. The word being typed is left alone until the caret leaves it.
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{Encode, Encoding, msg_send};
use objc2_foundation::NSString;
use std::ops::Range;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRange {
    location: usize,
    length: usize,
}
unsafe impl Encode for NSRange {
    const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}

/// Byte ranges of the misspelled words of `text`, except the one holding `caret`; call it on the main thread.
pub fn misspelled(text: &str, caret: usize) -> Vec<Range<usize>> {
    let Some(class) = AnyClass::get(c"NSSpellChecker") else {
        return Vec::new();
    };
    if text.trim().is_empty() {
        return Vec::new();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let string = NSString::from_str(text);
    let mut found = Vec::new();
    let mut start = 0usize;
    unsafe {
        let checker: *mut AnyObject = msg_send![class, sharedSpellChecker];
        if checker.is_null() {
            return Vec::new();
        }
        // The checker wraps around to the start: stop once it goes back.
        while start < units.len() && found.len() < 200 {
            let range: NSRange =
                msg_send![checker, checkSpellingOfString: &*string, startingAt: start as isize];
            if range.length == 0 || range.location == usize::MAX || range.location < start {
                break;
            }
            found.push(range.location..range.location + range.length);
            start = range.location + range.length;
        }
    }
    let byte = |unit: usize| String::from_utf16_lossy(&units[..unit.min(units.len())]).len();
    found
        .into_iter()
        .map(|r| byte(r.start)..byte(r.end))
        .filter(|r| !(r.start <= caret && caret <= r.end))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_misspelled_words_but_not_the_one_being_typed() {
        let text = "un tezt qwrtzk ici";
        let found = super::misspelled(text, text.len());
        assert!(
            found.iter().any(|r| &text[r.clone()] == "qwrtzk"),
            "{found:?}"
        );
        // The caret inside a word keeps it from being marked.
        assert!(
            !super::misspelled(text, 10)
                .iter()
                .any(|r| &text[r.clone()] == "qwrtzk")
        );
    }
}
