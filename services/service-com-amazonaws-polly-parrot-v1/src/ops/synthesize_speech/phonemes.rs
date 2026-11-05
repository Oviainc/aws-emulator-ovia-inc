// SPDX-License-Identifier: CC0-1.0

//! The front end of the synthesiser: the phoneme inventory, what each phoneme sounds
//! like, which viseme it shows, and how a written word becomes a sequence of them.
//!
//! A phoneme carries everything the renderer needs to produce it — whether the vocal
//! folds vibrate, where its formants sit, how long it lasts and, for a consonant, where
//! its frication or burst noise is centred — so [`super::render`] reads acoustics only
//! from this table and never from the text.
//!
//! The formant values are the classic measured averages for English (the first three
//! formants of each vowel, and the consonant loci), and the durations are the usual
//! segment lengths for read speech; both are properties of the language, not of any
//! request. The viseme of each phoneme is the mouth shape its class shows, from the
//! viseme value set Amazon Polly's speech-mark documentation publishes
//! (`sil`, `p`, `t`, `S`, `T`, `f`, `k`, `i`, `r`, `s`, `u`, `@`, `a`, `e`, `E`, `o`,
//! `O`).
//!
//! The letter-to-sound rules are English spelling rules, applied longest-match-first
//! at each position of the word. They are rules, not a dictionary: no word, phrase or
//! recorded value is stored here, and an unknown spelling still comes out as the
//! phonemes its letters make.

/// How a phoneme is produced, which is what decides the source the renderer excites it
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manner {
    /// An open, voiced sound: a glottal source through the formants.
    Vowel,
    /// A voiced consonant with an open-ish tract: `l`, `r`, `w`, `y`.
    Approximant,
    /// A voiced consonant with the nasal tract coupled in: `m`, `n`, `ng`.
    Nasal,
    /// Turbulent noise at a constriction, voiced (`z`) or not (`s`).
    Fricative,
    /// A closure followed by a burst, voiced (`b`) or not (`p`); the affricates
    /// `ch`/`jh` are a closure followed by frication.
    Stop,
    /// Breath noise through the tract: `h`.
    Aspirate,
}

/// One phoneme of the inventory, with the acoustics the renderer excites.
#[derive(Debug, Clone, Copy)]
pub struct Phoneme {
    /// How the inventory and the letter-to-sound rules name it.
    pub id: &'static str,
    /// The viseme this phoneme shows, from the published viseme value set.
    pub viseme: &'static str,
    /// How it is produced.
    pub manner: Manner,
    /// Whether the vocal folds vibrate during it.
    pub voiced: bool,
    /// The first three formants, in Hz: the resonances of the tract holding this shape.
    pub formants: [f32; 3],
    /// Where the formants move to by the end of the phoneme — a diphthong or a glide
    /// moves, a steady vowel does not.
    pub glide_to: Option<[f32; 3]>,
    /// How long the phoneme lasts at a normal speaking rate, in milliseconds.
    pub duration_ms: u16,
    /// Where this phoneme's frication or burst noise is centred, in Hz; 0 for a
    /// phoneme that carries no noise.
    pub noise_hz: f32,
}

impl Phoneme {
    /// The loudness this phoneme is produced at, relative to an open vowel.
    pub fn amplitude(&self) -> f32 {
        match self.manner {
            Manner::Vowel => 1.0,
            Manner::Approximant => 0.75,
            Manner::Nasal => 0.6,
            Manner::Fricative => 0.35,
            Manner::Stop => 0.5,
            Manner::Aspirate => 0.25,
        }
    }
}

/// Shorthand for one row of [`INVENTORY`].
#[allow(clippy::too_many_arguments)]
const fn p(
    id: &'static str,
    viseme: &'static str,
    manner: Manner,
    voiced: bool,
    formants: [f32; 3],
    glide_to: Option<[f32; 3]>,
    duration_ms: u16,
    noise_hz: f32,
) -> Phoneme {
    Phoneme {
        id,
        viseme,
        manner,
        voiced,
        formants,
        glide_to,
        duration_ms,
        noise_hz,
    }
}

/// Every phoneme the synthesiser can produce.
pub const INVENTORY: &[Phoneme] = &[
    // Vowels. The formants are the measured averages of English vowels; a diphthong
    // carries the second target it moves to.
    p("iy", "i", Manner::Vowel, true, [270.0, 2300.0, 3000.0], None, 110, 0.0),
    p("ih", "i", Manner::Vowel, true, [400.0, 1900.0, 2550.0], None, 95, 0.0),
    p(
        "ey",
        "e",
        Manner::Vowel,
        true,
        [480.0, 2000.0, 2650.0],
        Some([350.0, 2200.0, 3000.0]),
        160,
        0.0,
    ),
    p("eh", "E", Manner::Vowel, true, [530.0, 1850.0, 2500.0], None, 110, 0.0),
    p("ae", "a", Manner::Vowel, true, [660.0, 1700.0, 2400.0], None, 120, 0.0),
    p("aa", "a", Manner::Vowel, true, [730.0, 1100.0, 2450.0], None, 120, 0.0),
    p("ao", "O", Manner::Vowel, true, [570.0, 840.0, 2410.0], None, 120, 0.0),
    p(
        "ow",
        "o",
        Manner::Vowel,
        true,
        [500.0, 900.0, 2300.0],
        Some([350.0, 800.0, 2250.0]),
        160,
        0.0,
    ),
    p("uh", "u", Manner::Vowel, true, [440.0, 1020.0, 2240.0], None, 95, 0.0),
    p("uw", "u", Manner::Vowel, true, [300.0, 870.0, 2240.0], None, 110, 0.0),
    p("ah", "a", Manner::Vowel, true, [640.0, 1200.0, 2400.0], None, 100, 0.0),
    p("ax", "@", Manner::Vowel, true, [500.0, 1500.0, 2500.0], None, 70, 0.0),
    p("er", "r", Manner::Vowel, true, [490.0, 1350.0, 1690.0], None, 110, 0.0),
    p(
        "ay",
        "a",
        Manner::Vowel,
        true,
        [730.0, 1100.0, 2450.0],
        Some([400.0, 1900.0, 2550.0]),
        180,
        0.0,
    ),
    p(
        "aw",
        "a",
        Manner::Vowel,
        true,
        [730.0, 1100.0, 2450.0],
        Some([300.0, 870.0, 2240.0]),
        180,
        0.0,
    ),
    p(
        "oy",
        "O",
        Manner::Vowel,
        true,
        [570.0, 840.0, 2410.0],
        Some([400.0, 1900.0, 2550.0]),
        180,
        0.0,
    ),
    // Approximants.
    p("l", "t", Manner::Approximant, true, [360.0, 1300.0, 2900.0], None, 70, 0.0),
    p("r", "r", Manner::Approximant, true, [310.0, 1060.0, 1380.0], None, 70, 0.0),
    p("w", "u", Manner::Approximant, true, [290.0, 610.0, 2150.0], None, 60, 0.0),
    p("y", "i", Manner::Approximant, true, [260.0, 2070.0, 3020.0], None, 60, 0.0),
    // Nasals.
    p("m", "p", Manner::Nasal, true, [250.0, 1100.0, 2200.0], None, 70, 0.0),
    p("n", "t", Manner::Nasal, true, [250.0, 1700.0, 2600.0], None, 70, 0.0),
    p("ng", "k", Manner::Nasal, true, [250.0, 2300.0, 2900.0], None, 75, 0.0),
    // Fricatives. A voiced fricative carries both voicing and noise.
    p("f", "f", Manner::Fricative, false, [400.0, 1100.0, 2200.0], None, 95, 4500.0),
    p("v", "f", Manner::Fricative, true, [400.0, 1100.0, 2200.0], None, 80, 4500.0),
    p("th", "T", Manner::Fricative, false, [400.0, 1400.0, 2400.0], None, 90, 6000.0),
    p("dh", "T", Manner::Fricative, true, [400.0, 1400.0, 2400.0], None, 70, 6000.0),
    p("s", "s", Manner::Fricative, false, [400.0, 1700.0, 2600.0], None, 110, 5500.0),
    p("z", "s", Manner::Fricative, true, [400.0, 1700.0, 2600.0], None, 95, 5500.0),
    p("sh", "S", Manner::Fricative, false, [400.0, 1800.0, 2500.0], None, 115, 2800.0),
    p("zh", "S", Manner::Fricative, true, [400.0, 1800.0, 2500.0], None, 95, 2800.0),
    p("hh", "k", Manner::Aspirate, false, [500.0, 1500.0, 2500.0], None, 70, 1800.0),
    // Stops, at their place-of-articulation locus, and the two affricates.
    p("p", "p", Manner::Stop, false, [400.0, 1100.0, 2150.0], None, 80, 900.0),
    p("b", "p", Manner::Stop, true, [400.0, 1100.0, 2150.0], None, 70, 900.0),
    p("t", "t", Manner::Stop, false, [400.0, 1700.0, 2600.0], None, 80, 4000.0),
    p("d", "t", Manner::Stop, true, [400.0, 1700.0, 2600.0], None, 70, 4000.0),
    p("k", "k", Manner::Stop, false, [300.0, 2000.0, 2600.0], None, 85, 2200.0),
    p("g", "k", Manner::Stop, true, [300.0, 2000.0, 2600.0], None, 75, 2200.0),
    p("ch", "S", Manner::Stop, false, [400.0, 1800.0, 2500.0], None, 115, 2800.0),
    p("jh", "S", Manner::Stop, true, [400.0, 1800.0, 2500.0], None, 100, 2800.0),
];

/// The viseme a pause shows, from the published viseme value set.
pub const SILENCE_VISEME: &str = "sil";

/// The phoneme `id` names, or `None` when the inventory has no such phoneme.
pub fn phoneme(id: &str) -> Option<&'static Phoneme> {
    INVENTORY.iter().find(|entry| entry.id == id)
}

/// The phoneme a vowel becomes when a silent final `e` lengthens it (`mat` → `mate`),
/// or `None` for a vowel that does not lengthen.
fn lengthened(id: &str) -> Option<&'static str> {
    Some(match id {
        "ae" => "ey",
        "eh" => "iy",
        "ih" => "ay",
        "aa" => "ow",
        "ah" => "uw",
        _ => return None,
    })
}

/// One spelling and the phonemes it stands for.
struct Rule {
    /// The letters, lower-cased.
    letters: &'static str,
    /// The phonemes they are read as.
    phonemes: &'static [&'static str],
}

/// Shorthand for a rule row.
const fn r(letters: &'static str, phonemes: &'static [&'static str]) -> Rule {
    Rule { letters, phonemes }
}

/// Spellings read this way only at the END of a word.
const FINAL_RULES: &[Rule] = &[
    r("tion", &["sh", "ax", "n"]),
    r("sion", &["zh", "ax", "n"]),
    r("ture", &["ch", "er"]),
    r("ough", &["ao"]),
    r("ing", &["ih", "ng"]),
    r("ies", &["iy", "z"]),
    r("ed", &["d"]),
    r("es", &["z"]),
    r("le", &["ax", "l"]),
    r("re", &["er"]),
    r("ce", &["s"]),
    r("ge", &["jh"]),
    r("y", &["iy"]),
];

/// Spellings read this way anywhere in a word, longest match first.
const RULES: &[Rule] = &[
    r("eigh", &["ey"]),
    r("ough", &["ao"]),
    r("sch", &["s", "k"]),
    r("tch", &["ch"]),
    r("igh", &["ay"]),
    r("ai", &["ey"]),
    r("ay", &["ey"]),
    r("au", &["ao"]),
    r("aw", &["ao"]),
    r("ea", &["iy"]),
    r("ee", &["iy"]),
    r("ei", &["iy"]),
    r("eu", &["uw"]),
    r("ew", &["uw"]),
    r("ey", &["ey"]),
    r("ie", &["iy"]),
    r("oa", &["ow"]),
    r("oe", &["ow"]),
    r("oi", &["oy"]),
    r("oo", &["uw"]),
    r("ou", &["aw"]),
    r("ow", &["ow"]),
    r("oy", &["oy"]),
    r("ue", &["uw"]),
    r("ui", &["uw"]),
    r("ar", &["aa", "r"]),
    r("er", &["er"]),
    r("ir", &["er"]),
    r("or", &["ao", "r"]),
    r("ur", &["er"]),
    r("ch", &["ch"]),
    r("ck", &["k"]),
    r("gh", &["g"]),
    r("gn", &["n"]),
    r("kn", &["n"]),
    r("ng", &["ng"]),
    r("ph", &["f"]),
    r("qu", &["k", "w"]),
    r("sh", &["sh"]),
    r("th", &["th"]),
    r("wh", &["w"]),
    r("wr", &["r"]),
    r("bb", &["b"]),
    r("cc", &["k"]),
    r("dd", &["d"]),
    r("ff", &["f"]),
    r("gg", &["g"]),
    r("ll", &["l"]),
    r("mm", &["m"]),
    r("nn", &["n"]),
    r("pp", &["p"]),
    r("rr", &["r"]),
    r("ss", &["s"]),
    r("tt", &["t"]),
];

/// The phonemes one letter stands for on its own.
const LETTERS: &[Rule] = &[
    r("a", &["ae"]),
    r("b", &["b"]),
    r("c", &["k"]),
    r("d", &["d"]),
    r("e", &["eh"]),
    r("f", &["f"]),
    r("g", &["g"]),
    r("h", &["hh"]),
    r("i", &["ih"]),
    r("j", &["jh"]),
    r("k", &["k"]),
    r("l", &["l"]),
    r("m", &["m"]),
    r("n", &["n"]),
    r("o", &["aa"]),
    r("p", &["p"]),
    r("q", &["k"]),
    r("r", &["r"]),
    r("s", &["s"]),
    r("t", &["t"]),
    r("u", &["ah"]),
    r("v", &["v"]),
    r("w", &["w"]),
    r("x", &["k", "s"]),
    r("y", &["ih"]),
    r("z", &["z"]),
];

/// How each letter is NAMED, for the spelling-out a `say-as` of characters asks for.
pub fn letter_name(letter: char) -> Option<&'static str> {
    Some(match letter.to_ascii_lowercase() {
        'a' => "ay",
        'b' => "bee",
        'c' => "see",
        'd' => "dee",
        'e' => "ee",
        'f' => "eff",
        'g' => "gee",
        'h' => "aitch",
        'i' => "eye",
        'j' => "jay",
        'k' => "kay",
        'l' => "ell",
        'm' => "em",
        'n' => "en",
        'o' => "oh",
        'p' => "pee",
        'q' => "cue",
        'r' => "are",
        's' => "ess",
        't' => "tee",
        'u' => "you",
        'v' => "vee",
        'w' => "double you",
        'x' => "ex",
        'y' => "why",
        'z' => "zee",
        _ => return None,
    })
}

/// Whether `letter` spells a vowel.
fn is_vowel_letter(letter: char) -> bool {
    matches!(letter, 'a' | 'e' | 'i' | 'o' | 'u')
}

/// The phonemes `word` is read as, by English spelling rules.
///
/// The word is lower-cased and its letters are read left to right, taking the longest
/// spelling that matches at each position: a word-final spelling first ([`FINAL_RULES`]),
/// then a multi-letter one ([`RULES`]), then the letter on its own ([`LETTERS`]), with
/// the soft readings of `c`, `g` and `s` and the silent final `e` handled where the
/// letter's neighbours decide them. Anything that is not a letter contributes nothing,
/// and a word that yields nothing at all is read as a single schwa, so every word spoken
/// carries at least one phoneme.
pub fn pronounce(word: &str) -> Vec<&'static Phoneme> {
    let letters: Vec<char> = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect();
    if letters.is_empty() {
        return Vec::new();
    }
    // A final `e` after a consonant is silent and lengthens the word's last vowel
    // (`hop` → `hope`), which is why it is taken off before the letters are read.
    let mut body = letters.as_slice();
    let mut lengthen_last_vowel = false;
    if body.len() >= 3 && body[body.len() - 1] == 'e' && !is_vowel_letter(body[body.len() - 2]) {
        body = &body[..body.len() - 1];
        lengthen_last_vowel = true;
    }

    let mut ids: Vec<&'static str> = Vec::new();
    let mut index = 0usize;
    while index < body.len() {
        let rest = &body[index..];
        let at_start = index == 0;
        // The soft readings: `c` and `g` before `e`, `i` or `y`, and `s` between
        // vowels. They are decided by the next letter, so they come before the tables.
        let next = rest.get(1).copied();
        let soft_follower = next.is_some_and(|c| matches!(c, 'e' | 'i' | 'y'));
        if rest[0] == 'c' && soft_follower {
            ids.push("s");
            index += 1;
            continue;
        }
        if rest[0] == 'g' && soft_follower && rest.len() > 2 {
            ids.push("jh");
            index += 1;
            continue;
        }
        if rest[0] == 'y' && at_start && rest.len() > 1 {
            ids.push("y");
            index += 1;
            continue;
        }
        if let Some(rule) = longest_match(FINAL_RULES, rest, true)
            .or_else(|| longest_match(RULES, rest, false))
            .or_else(|| longest_match(LETTERS, rest, false))
        {
            ids.extend_from_slice(rule.phonemes);
            index += rule.letters.chars().count();
            continue;
        }
        index += 1;
    }

    if lengthen_last_vowel {
        if let Some(position) = ids
            .iter()
            .rposition(|id| phoneme(id).is_some_and(|ph| ph.manner == Manner::Vowel))
        {
            if let Some(longer) = lengthened(ids[position]) {
                ids[position] = longer;
            }
        }
    }

    let spoken: Vec<&'static Phoneme> = ids.iter().filter_map(|id| phoneme(id)).collect();
    if spoken.is_empty() {
        // A word of letters the rules read as nothing is still spoken, as the neutral
        // vowel: silence would drop it from the stream altogether.
        return phoneme("ax").into_iter().collect();
    }
    spoken
}

/// The longest rule of `table` that matches the start of `rest`, where `whole_word`
/// requires the match to run to the end of the word.
fn longest_match<'t>(table: &'t [Rule], rest: &[char], whole_word: bool) -> Option<&'t Rule> {
    table
        .iter()
        .filter(|rule| {
            let letters: Vec<char> = rule.letters.chars().collect();
            letters.len() <= rest.len()
                && rest[..letters.len()] == letters[..]
                && (!whole_word || letters.len() == rest.len())
        })
        .max_by_key(|rule| rule.letters.chars().count())
}

/// The phonemes an SSML `<phoneme ph>` or a PLS `<phoneme>` spells, in IPA or X-SAMPA.
///
/// Both alphabets are read with the same longest-match-first scan: a symbol the table
/// carries becomes its phoneme, a length or stress mark is skipped, and a symbol the
/// table does not carry contributes nothing rather than guessing. `None` when the
/// string spells no phoneme at all, which leaves the caller to speak the text instead.
pub fn from_phonetic(spelling: &str, alphabet: &str) -> Option<Vec<&'static Phoneme>> {
    let table = if alphabet == "x-sampa" {
        X_SAMPA
    } else {
        IPA
    };
    let symbols: Vec<char> = spelling.chars().collect();
    let mut out: Vec<&'static Phoneme> = Vec::new();
    let mut index = 0usize;
    while index < symbols.len() {
        let rest = &symbols[index..];
        let matched = table
            .iter()
            .filter(|(symbol, _)| {
                let chars: Vec<char> = symbol.chars().collect();
                chars.len() <= rest.len() && rest[..chars.len()] == chars[..]
            })
            .max_by_key(|(symbol, _)| symbol.chars().count());
        match matched {
            Some((symbol, id)) => {
                if let Some(found) = phoneme(id) {
                    out.push(found);
                }
                index += symbol.chars().count();
            }
            None => index += 1,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// IPA symbols, as the phoneme sets published for English spell them.
const IPA: &[(&str, &str)] = &[
    ("iː", "iy"),
    ("i", "iy"),
    ("ɪ", "ih"),
    ("eɪ", "ey"),
    ("ɛ", "eh"),
    ("e", "eh"),
    ("æ", "ae"),
    ("ɑː", "aa"),
    ("ɑ", "aa"),
    ("ɒ", "ao"),
    ("ɔː", "ao"),
    ("ɔɪ", "oy"),
    ("ɔ", "ao"),
    ("oʊ", "ow"),
    ("əʊ", "ow"),
    ("ʊ", "uh"),
    ("uː", "uw"),
    ("u", "uw"),
    ("ʌ", "ah"),
    ("ə", "ax"),
    ("ɜː", "er"),
    ("ɝ", "er"),
    ("ɚ", "er"),
    ("aɪ", "ay"),
    ("aʊ", "aw"),
    ("p", "p"),
    ("b", "b"),
    ("t", "t"),
    ("d", "d"),
    ("k", "k"),
    ("ɡ", "g"),
    ("g", "g"),
    ("tʃ", "ch"),
    ("dʒ", "jh"),
    ("m", "m"),
    ("n", "n"),
    ("ŋ", "ng"),
    ("f", "f"),
    ("v", "v"),
    ("θ", "th"),
    ("ð", "dh"),
    ("s", "s"),
    ("z", "z"),
    ("ʃ", "sh"),
    ("ʒ", "zh"),
    ("h", "hh"),
    ("l", "l"),
    ("ɹ", "r"),
    ("r", "r"),
    ("w", "w"),
    ("j", "y"),
];

/// X-SAMPA symbols — the ASCII spelling of the same phonemes.
const X_SAMPA: &[(&str, &str)] = &[
    ("i:", "iy"),
    ("i", "iy"),
    ("I", "ih"),
    ("eI", "ey"),
    ("E", "eh"),
    ("e", "eh"),
    ("{", "ae"),
    ("A:", "aa"),
    ("A", "aa"),
    ("Q", "ao"),
    ("O:", "ao"),
    ("OI", "oy"),
    ("O", "ao"),
    ("oU", "ow"),
    ("@U", "ow"),
    ("U", "uh"),
    ("u:", "uw"),
    ("u", "uw"),
    ("V", "ah"),
    ("@", "ax"),
    ("3:", "er"),
    ("3`", "er"),
    ("@`", "er"),
    ("aI", "ay"),
    ("aU", "aw"),
    ("p", "p"),
    ("b", "b"),
    ("t", "t"),
    ("d", "d"),
    ("k", "k"),
    ("g", "g"),
    ("tS", "ch"),
    ("dZ", "jh"),
    ("m", "m"),
    ("n", "n"),
    ("N", "ng"),
    ("f", "f"),
    ("v", "v"),
    ("T", "th"),
    ("D", "dh"),
    ("s", "s"),
    ("z", "z"),
    ("S", "sh"),
    ("Z", "zh"),
    ("h", "hh"),
    ("l", "l"),
    ("r", "r"),
    ("w", "w"),
    ("j", "y"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every phoneme a rule or an alphabet names has to be in the inventory, or the
    /// rule silently drops a sound.
    #[test]
    fn every_rule_names_a_phoneme_of_the_inventory() {
        let rule_ids = FINAL_RULES
            .iter()
            .chain(RULES)
            .chain(LETTERS)
            .flat_map(|rule| rule.phonemes.iter().copied());
        let alphabet_ids = IPA.iter().chain(X_SAMPA).map(|(_, id)| *id);
        for id in rule_ids.chain(alphabet_ids) {
            assert!(
                phoneme(id).is_some(),
                "phoneme {id:?} is named by a rule but is not in the inventory"
            );
        }
    }

    /// Every viseme the inventory shows has to be one of the published values.
    #[test]
    fn every_viseme_is_a_published_value() {
        // The viseme value set of Amazon Polly's speech-mark documentation.
        const PUBLISHED: &[&str] = &[
            "sil", "p", "t", "S", "T", "f", "k", "i", "r", "s", "u", "@", "a", "e", "E", "o", "O",
        ];
        assert!(PUBLISHED.contains(&SILENCE_VISEME), "the pause viseme");
        for entry in INVENTORY {
            assert!(
                PUBLISHED.contains(&entry.viseme),
                "phoneme {:?} shows viseme {:?}, which is not published",
                entry.id,
                entry.viseme
            );
        }
    }

    /// The rules read a word as a sequence of sounds, and two different words are read
    /// differently.
    #[test]
    fn words_are_read_as_phonemes() {
        for (word, expected) in [
            ("hello", vec!["hh", "eh", "l", "aa"]),
            ("ship", vec!["sh", "ih", "p"]),
            ("make", vec!["m", "ey", "k"]),
            ("city", vec!["s", "ih", "t", "iy"]),
        ] {
            let read: Vec<&str> = pronounce(word).iter().map(|ph| ph.id).collect();
            assert_eq!(read, expected, "word {word:?}: read as {read:?}");
        }
        assert_ne!(
            pronounce("cat").iter().map(|ph| ph.id).collect::<Vec<_>>(),
            pronounce("dog").iter().map(|ph| ph.id).collect::<Vec<_>>(),
            "two different words must be read as different phonemes"
        );
    }

    /// A phonetic spelling is read in either published alphabet.
    #[test]
    fn phonetic_spellings_are_read_in_both_alphabets() {
        for (spelling, alphabet) in [("ˈtoʊmeɪtoʊ", "ipa"), ("\"toUmeItoU", "x-sampa")] {
            let read: Vec<&str> = from_phonetic(spelling, alphabet)
                .unwrap_or_default()
                .iter()
                .map(|ph| ph.id)
                .collect();
            assert_eq!(
                read,
                vec!["t", "ow", "m", "ey", "t", "ow"],
                "spelling {spelling:?} in {alphabet:?}: read as {read:?}"
            );
        }
        assert!(
            from_phonetic("", "ipa").is_none(),
            "an empty spelling names no phonemes"
        );
    }
}
