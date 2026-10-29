// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW MANY characters a run of XML character data stands for, at
//! both of the service's XML readers: `SynthesizeSpeech`'s `ssml` `Text`, which bills
//! them, and `PutLexicon`'s PLS `Content`, which measures a `<lexeme>`'s replacement
//! with them.
//!
//! RULE: the characters an XML document's character data carries are the characters it
//! RESOLVES to — a character written as an entity or numeric character reference is the
//! one character it stands for, a CDATA section carries its characters verbatim and
//! resolves nothing, and markup (an element, a comment, a processing instruction)
//! carries none — so the same character data is the same number of characters whichever
//! of this service's two readers reads it; only what each reader does with that number
//! differs.
//!
//! The table spans the class the rule generalises over: the form the characters arrive
//! in (literal text, each predefined entity, a decimal, a hexadecimal and a non-ASCII
//! character reference, a literal multi-byte character, a CDATA section, an empty one,
//! and mixtures), what sits between them (a comment, a processing instruction), and how
//! many of them there are (none, a few, exactly the published per-lexeme quota). Each
//! row is checked at BOTH readers, which is what makes it a test of the one shared rule
//! rather than of either copy of it.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    MAX_LEXEME_CHARACTERS, PLS_NAMESPACE, PLS_VERSION,
};

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// One run of character data, and how many characters it stands for.
///
/// `characters` is counted BY HAND, so the expectation is independent of the reader
/// under test.
struct Row {
    case: &'static str,
    markup: String,
    characters: usize,
}

/// The character data every row below is built from.
fn rows() -> Vec<Row> {
    let row = |case: &'static str, markup: &str, characters: usize| Row {
        case,
        markup: markup.to_string(),
        characters,
    };
    vec![
        row("literal ASCII text", "abc", 3),
        // XML 1.0 predefines exactly these five general entities.
        row("a predefined entity reference", "&amp;", 1),
        row(
            "every predefined entity reference",
            "&amp;&lt;&gt;&quot;&apos;",
            5,
        ),
        row("a decimal character reference", "&#120;", 1),
        row("a hexadecimal character reference", "&#x78;", 1),
        row("a character reference past ASCII", "&#233;", 1),
        // Characters, not bytes: this one is two bytes of UTF-8.
        row("a literal multi-byte character", "é", 1),
        row("a multi-byte character spelled both ways", "é&#233;", 2),
        row("a CDATA section", "<![CDATA[abcd]]>", 4),
        // A CDATA section resolves nothing, so this is the five characters it spells.
        row(
            "a CDATA section spelling a reference verbatim",
            "<![CDATA[&amp;]]>",
            5,
        ),
        row("an empty CDATA section", "<![CDATA[]]>", 0),
        row(
            "text, references and a CDATA section mixed",
            "a&amp;b<![CDATA[cd]]>e",
            6,
        ),
        // Markup is not character data, so neither of these is counted; the text they
        // break apart still is.
        row(
            "character data split by a comment and a processing instruction",
            "ab<!--a comment--><?instruction value?>cd",
            4,
        ),
        row("a reference standing for a markup character", "&lt;speak&gt;", 7),
        // Whitespace inside character data is character data, wherever it sits.
        row("whitespace around and between text", " a b ", 5),
        // The published per-lexeme quota itself, so the rule is checked on a bound and
        // — through the one-character-longer document the PLS test builds — one past it.
        row(
            "text exactly as long as the per-lexeme quota",
            &"y".repeat(MAX_LEXEME_CHARACTERS),
            MAX_LEXEME_CHARACTERS,
        ),
    ]
}

/// A PLS lexicon with one `<lexeme>` whose `<alias>` replacement is `replacement`.
fn pls(replacement: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><lexeme><grapheme>G</grapheme>\
         <alias>{replacement}</alias></lexeme></lexicon>"
    )
}

/// RULE (the billing reader): `SynthesizeSpeech` bills the characters an `ssml`
/// document's character data resolves to, so every row's billed count is exactly the
/// characters that row stands for — however they are spelled and whatever markup sits
/// between them.
#[tokio::test]
async fn ssml_bills_the_characters_its_character_data_stands_for() {
    let polly = Polly::new();
    for row in rows() {
        let text = format!("<speak>{}</speak>", row.markup);
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {:?} ({text:?}): expected the document to be synthesized, got {:?}",
            row.case,
            String::from_utf8_lossy(response.body())
        );
        let billed = header(&response, "x-amzn-requestcharacters");
        assert_eq!(
            billed,
            row.characters.to_string(),
            "case {:?} ({text:?}): expected {} billed characters, got {billed:?}",
            row.case,
            row.characters
        );
    }
}

/// RULE (the measuring reader): `PutLexicon` measures a replacement against the same
/// resolved characters, so a replacement whose character data stands for exactly the
/// per-lexeme quota is stored and one standing for a single character more is refused
/// with `MaxLexemeLengthExceededException` — which pins the measurement of each row to
/// the same number the billing reader reached for it.
#[tokio::test]
async fn pls_measures_a_replacement_with_the_characters_its_content_stands_for() {
    let polly = Polly::new();
    for (index, row) in rows().into_iter().enumerate() {
        let padding = MAX_LEXEME_CHARACTERS - row.characters;
        // At the quota: the row's character data plus enough plain text to reach it.
        let name = format!("AtQuota{index:02}");
        let content = pls(&format!("{}{}", row.markup, "x".repeat(padding)));
        let put = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {:?} ({} characters plus {padding} of padding): expected the \
             document to be stored, got {:?}",
            row.case,
            row.characters,
            json(&put)
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            (
                got["Lexicon"]["Content"].as_str(),
                got["LexiconAttributes"]["LexemesCount"].as_u64()
            ),
            (Some(content.as_str()), Some(1)),
            "case {:?}: the stored document must read back as it was put, got {got:?}",
            row.case
        );

        // One character past the quota: the same row, one more character of padding.
        let over_name = format!("PastQuota{index:02}");
        let over = pls(&format!("{}{}", row.markup, "x".repeat(padding + 1)));
        let response = polly.put_lexicon(&over_name, &over).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "MaxLexemeLengthExceededException".to_string()),
            "case {:?} ({} characters plus {} of padding): expected the lexeme-quota \
             refusal, got {:?}",
            row.case,
            row.characters,
            padding + 1,
            json(&response)
        );
        let read_back = polly.get_lexicon(&over_name).await;
        assert_eq!(
            (read_back.status().as_u16(), error_code(&read_back)),
            (404, "LexiconNotFoundException".to_string()),
            "case {:?}: a refused put must store nothing, got {:?}",
            row.case,
            json(&read_back)
        );
    }
}
