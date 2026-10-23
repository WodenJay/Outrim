//! Text counts in one fixed encoding; special-token literals remain ordinary text.

pub(crate) const ENCODING: &str = "o200k_base";

pub(crate) fn count(text: &str) -> u64 {
    tiktoken::get_encoding(ENCODING)
        .expect("o200k_base vocabulary is compiled in")
        .count(text) as u64
}

#[derive(Clone, Copy)]
pub(crate) struct Counts {
    pub before: u64,
    pub after: u64,
}

impl Counts {
    pub(crate) fn new(original: &str, final_text: &str) -> Self {
        let before = count(original);
        Self {
            before,
            after: if original == final_text {
                before
            } else {
                count(final_text)
            },
        }
    }
}
