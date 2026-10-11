//! On-device extractive summaries of messages and conversations.
//!
//! The summarizer removes quoted text, attribution lines ("On … wrote:"),
//! forwarded/original-message blocks, signatures and greetings, splits the
//! remaining text into sentences (aware of abbreviations, decimal numbers and
//! URLs) and ranks them by content word frequency, position and length. The
//! best sentences are returned in their original order.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;

static STOP_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    const EN: &str = "a about above after again against all also am an and any are aren't as at be because been before being below between both but by \
        can can't cannot could couldn't did didn't do does doesn't doing don't down during each few for from further get got had hadn't has hasn't have \
        haven't having he he'd he'll he's her here here's hers herself him himself his how how's i i'd i'll i'm i've if in into is isn't it it's its itself \
        just let's me more most mustn't my myself no nor not now of off on once only or other ought our ours ourselves out over own same shan't she she'd \
        she'll she's should shouldn't so some such than that that's the their theirs them themselves then there there's these they they'd they'll they're \
        they've this those through to too under until up us very was wasn't we we'd we'll we're we've were weren't what what's when when's where where's \
        which while who who's whom why why's will with won't would wouldn't you you'd you'll you're you've your yours yourself yourselves hi hello hey \
        thanks thank please dear regards best cheers just really also yes ok okay like one would could";
    const DE: &str = "aber alle allem allen aller alles als also am an ander andere anderem anderen anderer anderes anderm andern anders auch auf aus bei bin \
        bis bist da damit dann das dass daß dein deine deinem deinen deiner dem den denn der des dessen dich die dies diese diesem diesen dieser dieses dir \
        doch dort du durch ein eine einem einen einer eines einig einige einigem einigen einiger einiges einmal er es etwas euch euer eure für gegen gewesen \
        hab habe haben hat hatte hatten hier hin hinter ich ihm ihn ihnen ihr ihre ihrem ihren ihrer ihres im in indem ins ist jede jedem jeden jeder jedes \
        jene jenem jenen jener jenes jetzt kann kein keine keinem keinen keiner keines können könnte machen man manche mein meine meinem meinen meiner mich \
        mir mit muss musste nach nicht nichts noch nun nur ob oder ohne sehr sein seine seinem seinen seiner sich sie sind so solche soll sollte sondern \
        sonst über um und uns unser unsere unter viel vom von vor während war waren warst was weg weil weiter welche welchem welchen welcher welches wenn \
        werde werden wie wieder will wir wird wirst wo wollen wollte würde würden zu zum zur zwar zwischen hallo liebe lieber grüße gruß danke bitte ja";
    EN.split_whitespace().chain(DE.split_whitespace()).collect()
});

/// Abbreviations (lowercase, without the trailing period) after which a period does not end a sentence.
static ABBREVIATIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "mr mrs ms dr prof sr jr st vs etc e.g i.e eg ie approx appt dept est inc ltd co corp no nr fig cf al ca bzw z.b zb d.h dh u.a ua usw ggf evtl inkl \
     max min tel str hr fr jan feb mar apr jun jul aug sep sept oct okt nov dec dez mon tue wed thu fri sat sun mo di mi do fr sa so p.s ps a.m p.m am pm"
        .split_whitespace()
        .collect()
});

/// One sentence extracted from a text together with its position information.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    pub text: String,
    /// Index of the paragraph the sentence belongs to.
    pub paragraph: usize,
    /// Index of the sentence inside the whole text.
    pub index: usize,
    /// First sentence of its paragraph.
    pub paragraph_start: bool,
}

fn is_attribution(line: &str) -> bool {
    let l = line.trim();
    let lower = l.to_lowercase();
    (lower.starts_with("on ") && (lower.ends_with("wrote:") || lower.ends_with("writes:")))
        || (lower.starts_with("am ") && (lower.ends_with("schrieb:") || lower.contains(" schrieb ")))
        || (lower.ends_with("wrote:") && l.len() < 200)
        || (lower.ends_with("schrieb:") && l.len() < 200)
}

fn is_block_separator(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    lower.starts_with("-----original message")
        || lower.starts_with("----- original message")
        || lower.starts_with("-----ursprüngliche nachricht")
        || lower.starts_with("begin forwarded message")
        || lower.starts_with("---------- forwarded message")
        || lower.starts_with("-------- weitergeleitete nachricht")
        || lower.starts_with("________________________________")
        || lower == "sent from my phone"
}

/// Kind of a short salutation line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Salutation {
    Greeting,
    Closing,
}

fn salutation(line: &str) -> Option<Salutation> {
    let l = line.trim();
    let lower = l.to_lowercase();
    let words = l.split_whitespace().count();
    const GREETINGS: [&str; 12] = ["hi", "hello", "hey", "dear", "good", "morning", "hallo", "liebe", "lieber", "sehr", "moin", "servus"];
    const CLOSINGS: [&str; 14] = ["thanks", "thank", "best", "cheers", "regards", "kind", "warm", "sincerely", "yours", "viele", "beste", "danke", "liebe", "herzliche"];
    let first = lower.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
    if words <= 4 && (l.ends_with(',') || l.ends_with('!')) {
        if GREETINGS.contains(&first) && !(first == "liebe" && lower.contains("grüße")) {
            return Some(Salutation::Greeting);
        }
        if CLOSINGS.contains(&first) {
            return Some(Salutation::Closing);
        }
        return (words <= 2).then_some(Salutation::Greeting);
    }
    matches!(
        lower.as_str(),
        "thanks" | "thanks!" | "cheers" | "best" | "regards" | "vg" | "lg" | "mfg" | "gruß" | "grüße" | "viele grüße" | "liebe grüße" | "best regards" | "kind regards"
    )
    .then_some(Salutation::Closing)
}

/// Remove quoted text, signatures and similar noise; return paragraphs of running text.
pub fn clean_text(text: &str) -> Vec<String> {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, paragraphs: &mut Vec<String>| {
        let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            paragraphs.push(t);
        }
        cur.clear();
    };
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if line == "-- " || trimmed == "--" || is_block_separator(line) || is_attribution(line) {
            break;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut cur, &mut paragraphs);
            continue;
        }
        match salutation(trimmed) {
            Some(Salutation::Closing) if !paragraphs.is_empty() || !cur.is_empty() => break,
            Some(_) => {
                flush(&mut cur, &mut paragraphs);
                continue;
            }
            None => {}
        }
        // Bullet items become their own paragraph.
        let is_bullet = trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("• ") || numbered_item(trimmed);
        if is_bullet {
            flush(&mut cur, &mut paragraphs);
            let item = trimmed.trim_start_matches(['-', '*', '•']).trim_start();
            let mut s = item.to_string();
            if !s.ends_with(['.', '!', '?', ':']) {
                s.push('.');
            }
            cur.push_str(&s);
            flush(&mut cur, &mut paragraphs);
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(trimmed);
    }
    flush(&mut cur, &mut paragraphs);
    paragraphs
}

fn numbered_item(s: &str) -> bool {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    !digits.is_empty() && digits.len() <= 2 && (s[digits.len()..].starts_with(". ") || s[digits.len()..].starts_with(") "))
}

/// Split a paragraph into sentences.
pub fn split_sentences(paragraph: &str) -> Vec<String> {
    let chars: Vec<(usize, char)> = paragraph.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if matches!(c, '.' | '!' | '?' | '…') {
            // Swallow runs like "?!" or "..." and closing quotes / brackets.
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '.' | '!' | '?' | '…' | '"' | '\'' | ')' | '”' | '’' | '»') {
                j += 1;
            }
            let at_end = j >= chars.len();
            let followed_by_space = !at_end && chars[j].1.is_whitespace();
            if at_end || followed_by_space {
                let mut is_break = true;
                if c == '.' && j == i + 1 {
                    // Look at the token before the period.
                    let token_start = paragraph[..pos].rfind(char::is_whitespace).map(|p| p + 1).unwrap_or(0);
                    let token = paragraph[token_start..pos].trim_start_matches(['(', '"', '\'']).to_lowercase();
                    let single_letter = token.chars().count() == 1 && token.chars().all(|c| c.is_alphabetic());
                    let ordinal = !token.is_empty() && token.chars().all(|c| c.is_ascii_digit()) && token.len() <= 2 && next_is_lowercase_or_month(&chars, j);
                    if ABBREVIATIONS.contains(token.as_str()) || single_letter || ordinal {
                        is_break = false;
                    }
                }
                if is_break && !at_end {
                    // Next sentence should start with an uppercase letter, digit, quote or bracket.
                    let mut k = j;
                    while k < chars.len() && chars[k].1.is_whitespace() {
                        k += 1;
                    }
                    if k < chars.len() {
                        let n = chars[k].1;
                        if n.is_lowercase() && c == '.' {
                            is_break = false;
                        }
                    }
                }
                if is_break {
                    let end = if at_end { paragraph.len() } else { chars[j].0 };
                    let s = paragraph[start..end].trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                    }
                    start = end;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    let rest = paragraph[start..].trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

fn next_is_lowercase_or_month(chars: &[(usize, char)], j: usize) -> bool {
    let mut k = j;
    while k < chars.len() && chars[k].1.is_whitespace() {
        k += 1;
    }
    if k >= chars.len() {
        return false;
    }
    let word: String = chars[k..].iter().map(|(_, c)| *c).take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    const MONTHS: [&str; 16] = ["januar", "februar", "märz", "april", "mai", "juni", "juli", "august", "september", "oktober", "november", "dezember", "jan", "feb", "okt", "dez"];
    chars[k].1.is_lowercase() || MONTHS.contains(&word.as_str())
}

/// Extract sentences from a text after cleaning.
pub fn sentences(text: &str) -> Vec<Sentence> {
    let mut out = Vec::new();
    for (pi, para) in clean_text(text).iter().enumerate() {
        for (si, s) in split_sentences(para).into_iter().enumerate() {
            let index = out.len();
            out.push(Sentence { text: s, paragraph: pi, index, paragraph_start: si == 0 });
        }
    }
    out
}

/// Lowercase content words of a sentence (stop words, numbers and URLs removed).
pub fn content_words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .filter(|w| !w.contains("://") && !w.starts_with("www.") && !w.contains('@'))
        .flat_map(|w| w.split(|c: char| !(c.is_alphanumeric() || c == '\'')).map(|x| x.trim_matches('\'').to_lowercase()).collect::<Vec<_>>())
        .filter(|w| w.chars().count() >= 2 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP_WORDS.contains(w.as_str()))
        .collect()
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Score sentences; `freq` holds content word frequencies over the whole text.
fn score_sentence(s: &Sentence, freq: &HashMap<String, f32>, max_freq: f32, total: usize) -> f32 {
    let words = content_words(&s.text);
    let n = word_count(&s.text);
    if words.is_empty() || n < 3 {
        return 0.0;
    }
    let mut seen = HashSet::new();
    let sum: f32 = words.iter().filter(|w| seen.insert(w.as_str())).map(|w| freq.get(w).copied().unwrap_or(0.0) / max_freq).sum();
    let mut score = sum / (words.len() as f32).sqrt();
    // Position: the beginning of a message usually carries the point.
    if s.index == 0 {
        score *= 1.35;
    } else if s.paragraph_start {
        score *= 1.12;
    }
    if total > 4 && s.index + 1 == total {
        score *= 1.05;
    }
    // Length: penalize fragments and run-on sentences.
    if n < 5 {
        score *= 0.6;
    } else if n > 45 {
        score *= 0.7;
    }
    // Requests and questions are often what the reader needs to act on.
    if s.text.ends_with('?') {
        score *= 1.1;
    }
    score
}

fn frequencies<'a>(sents: impl Iterator<Item = &'a Sentence>) -> (HashMap<String, f32>, f32) {
    let mut freq: HashMap<String, f32> = HashMap::new();
    for s in sents {
        for w in content_words(&s.text) {
            *freq.entry(w).or_default() += 1.0;
        }
    }
    let max = freq.values().copied().fold(1.0f32, f32::max);
    (freq, max)
}

fn pick(scored: &mut [(usize, f32)], max_sentences: usize) -> Vec<usize> {
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut chosen: Vec<usize> = scored.iter().take(max_sentences).map(|(i, _)| *i).collect();
    chosen.sort_unstable();
    chosen
}

/// Summarize a message body into at most `max_sentences` sentences.
pub fn summarize(text: &str, max_sentences: usize) -> String {
    if max_sentences == 0 {
        return String::new();
    }
    let sents = sentences(text);
    if sents.len() <= max_sentences {
        return sents.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    }
    let (freq, max) = frequencies(sents.iter());
    let mut scored: Vec<(usize, f32)> = sents.iter().map(|s| (s.index, score_sentence(s, &freq, max, sents.len()))).collect();
    pick(&mut scored, max_sentences).into_iter().map(|i| sents[i].text.as_str()).collect::<Vec<_>>().join(" ")
}

/// Summarize a conversation: `texts` are the message bodies in chronological order.
///
/// Word frequencies are computed over the whole conversation; the opening of each
/// message gets a position bonus and the latest message a recency bonus, so that the
/// summary reflects both how the conversation started and where it stands.
pub fn summarize_thread(texts: &[String], max_sentences: usize) -> String {
    if max_sentences == 0 || texts.is_empty() {
        return String::new();
    }
    let per_message: Vec<Vec<Sentence>> = texts.iter().map(|t| sentences(t)).collect();
    let all: Vec<(usize, &Sentence)> = per_message.iter().enumerate().flat_map(|(mi, v)| v.iter().map(move |s| (mi, s))).collect();
    if all.len() <= max_sentences {
        return all.iter().map(|(_, s)| s.text.as_str()).collect::<Vec<_>>().join(" ");
    }
    let (freq, max) = frequencies(all.iter().map(|(_, s)| *s));
    let last = texts.len() - 1;
    let mut seen_texts = HashSet::new();
    let mut scored: Vec<(usize, f32)> = all
        .iter()
        .enumerate()
        .map(|(gi, (mi, s))| {
            let mut sc = score_sentence(s, &freq, max, per_message[*mi].len());
            if *mi == last {
                sc *= 1.2;
            }
            // Avoid repeating identical sentences (e.g. repeated boilerplate).
            if !seen_texts.insert(s.text.to_lowercase()) {
                sc = 0.0;
            }
            (gi, sc)
        })
        .collect();
    pick(&mut scored, max_sentences).into_iter().map(|i| all[i].1.text.as_str()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sentences_with_abbreviations_and_urls() {
        let s = split_sentences("Dr. Miller arrives at 3.30 p.m. tomorrow. Please see https://example.com/a.b for details! Is that ok? Yes.");
        assert_eq!(s, vec!["Dr. Miller arrives at 3.30 p.m. tomorrow.", "Please see https://example.com/a.b for details!", "Is that ok?", "Yes."]);
        let de = split_sentences("Wir treffen uns am 12. März z.B. im Büro. Danach essen wir.");
        assert_eq!(de.len(), 2);
    }

    #[test]
    fn cleaning_removes_quotes_and_signature() {
        let text = "Hi Anna,\n\nThe budget review moved to Thursday.\nPlease bring the numbers.\n\nBest,\nTom\n-- \nTom Example\nCEO\n\nOn Mon, 6 Oct 2025, Anna wrote:\n> old stuff";
        let paras = clean_text(text);
        assert_eq!(paras, vec!["The budget review moved to Thursday. Please bring the numbers.".to_string()]);
    }

    #[test]
    fn summary_picks_relevant_sentences_in_order() {
        let text = "The quarterly budget review has moved to Thursday at 10 in the large meeting room.\n\
            I had a nice weekend at the lake.\n\
            For the budget review, please prepare the updated marketing budget figures.\n\
            The weather was great.\n\
            We also need the travel budget numbers before the review on Thursday.\n\
            My cat says hi.";
        let s = summarize(text, 2);
        assert!(s.starts_with("The quarterly budget review"), "{s}");
        assert!(s.contains("budget"), "{s}");
        assert!(!s.contains("cat"));
        assert!(!s.contains("weather"));
        // Short texts are returned as they are.
        assert_eq!(summarize("Just one line here.", 3), "Just one line here.");
        assert_eq!(summarize("", 3), "");
    }

    #[test]
    fn thread_summary_covers_conversation() {
        let texts = vec![
            "Can we move the website launch to next Friday? The design team needs more time for the new landing page.".to_string(),
            "Friday works for me. I will tell the hosting provider about the new launch date.\n\nOn Tue, Bob wrote:\n> Can we move the website launch".to_string(),
            "Great, the launch is confirmed for Friday. The landing page will be ready on Wednesday for a final review.".to_string(),
        ];
        let s = summarize_thread(&texts, 2);
        assert!(s.contains("launch"), "{s}");
        assert!(!s.contains("> Can we"));
        assert_eq!(summarize_thread(&[], 3), "");
    }
}
