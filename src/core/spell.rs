//! Spell checking using system word lists (hunspell dictionaries or /usr/share/dict).
//! STUB: to be implemented.

pub struct SpellChecker;

impl SpellChecker {
    /// Load the best available dictionary for the given language (e.g. "en_US").
    pub fn load(_lang: &str) -> SpellChecker {
        SpellChecker
    }
    pub fn is_available(&self) -> bool {
        false
    }
    pub fn check(&self, _word: &str) -> bool {
        true
    }
    /// Byte ranges of misspelled words in text.
    pub fn misspelled(&self, _text: &str) -> Vec<(usize, usize)> {
        Vec::new()
    }
    pub fn suggest(&self, _word: &str, _max: usize) -> Vec<String> {
        Vec::new()
    }
    /// Add a word to the user's personal dictionary.
    pub fn learn(&mut self, _word: &str) {}
}
