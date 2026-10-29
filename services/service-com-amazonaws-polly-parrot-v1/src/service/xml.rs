// SPDX-License-Identifier: CC0-1.0

//! What XML itself says about the texts this service reads as XML documents.
//!
//! Both of the service's XML readers — `PutLexicon`'s PLS `Content` and
//! `SynthesizeSpeech`'s `ssml` `Text` — have to answer the same questions about the
//! characters a text carries: which of them XML admits OUTSIDE a document element
//! ([`is_xml_whitespace`]), and how many characters one event of character data stands
//! for ([`character_data_length`]). Both answers live here once rather than in each
//! operation, so neither reader can drift from the other.

/// Whether `text` is nothing but XML whitespace.
///
/// XML 1.0's `S` production is exactly four characters — space, tab, carriage return
/// and line feed — and they are the only character data a well-formed document may
/// carry outside its document element. Unicode calls many more characters blank (a
/// no-break space, a line separator, an ideographic space), and XML does not: each of
/// those is ordinary character data, so a reader that trimmed them would admit a text
/// that is not one XML document.
pub(crate) fn is_xml_whitespace(text: &str) -> bool {
    text.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// How many characters of text content one XML event carries.
///
/// This is the whole of "how long is this character data", and it is the same question
/// at both readers: `SynthesizeSpeech` adds the answer to the characters it bills and
/// `PutLexicon` adds it to the replacement element it is measuring, so the rule is
/// written once here and each caller keeps only its own accumulator.
///
/// A text event is UNESCAPED before its characters are counted, because a character
/// written as an entity reference or a numeric character reference is the one character
/// it stands for, exactly as a literal one is. A CDATA section carries the characters it
/// holds verbatim — that is what CDATA is for — so nothing in it is resolved and a
/// sequence that merely looks like a reference counts as the characters it literally
/// spells. A reference arrives on its own, because the reader BREAKS character data at
/// every entity or character reference, so a loop that read only text events would count
/// none of the characters a document spells with `&amp;` or `&#NN;`; the reference
/// carries its NAME, so it is reconstructed as `&name;` and resolved, which makes a
/// predefined entity and a numeric character reference resolve the same way. Every other
/// event is markup or the document's own syntax and carries no text content, so it
/// counts nothing.
///
/// `Err` carries why the character data is not well-formed XML: an entity nothing
/// declares, or a character reference denoting no code point, stands for nothing at all,
/// which is a property of the document and not of what is being measured — so it is an
/// error here whether or not the caller has anything open to add the count to.
pub(crate) fn character_data_length(
    event: &quick_xml::events::Event<'_>,
) -> Result<usize, String> {
    use quick_xml::events::Event;
    match event {
        Event::Text(text) => {
            let raw = String::from_utf8_lossy(text.as_ref()).to_string();
            let decoded = quick_xml::escape::unescape(&raw).map_err(|e| e.to_string())?;
            Ok(decoded.chars().count())
        }
        Event::CData(text) => Ok(String::from_utf8_lossy(text.as_ref()).chars().count()),
        Event::GeneralRef(reference) => {
            let name = reference.decode().map_err(|e| e.to_string())?;
            let reconstructed = format!("&{name};");
            let resolved =
                quick_xml::escape::unescape(&reconstructed).map_err(|e| e.to_string())?;
            Ok(resolved.chars().count())
        }
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::{character_data_length, is_xml_whitespace};

    #[test]
    fn only_xmls_own_four_whitespace_characters_count() {
        for text in ["", " ", "\t", "\r", "\n", " \t\r\n "] {
            assert!(
                is_xml_whitespace(text),
                "case {text:?}: XML's S production admits this"
            );
        }
        // Unicode whitespace that XML's S production does NOT admit, plus ordinary
        // character data for contrast.
        for text in [
            "\u{a0}", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}", "\u{3000}", "x", " x ",
        ] {
            assert!(
                !is_xml_whitespace(text),
                "case {text:?}: this is character data, not XML whitespace"
            );
        }
    }

    /// Read `xml` the way both of the service's readers do and total what each event
    /// contributes, so the helper is exercised over every event kind a document emits.
    fn character_data_of(xml: &str) -> Result<usize, String> {
        let mut reader = quick_xml::Reader::from_str(xml);
        let mut total = 0usize;
        loop {
            let event = reader.read_event().map_err(|e| e.to_string())?;
            if matches!(event, quick_xml::events::Event::Eof) {
                break;
            }
            total = total.saturating_add(character_data_length(&event)?);
        }
        Ok(total)
    }

    #[test]
    fn an_event_counts_the_characters_its_character_data_stands_for() {
        // case, document, how many characters of text content it carries
        let table: [(&str, &str, usize); 11] = [
            ("markup only", "<r><c/></r>", 0),
            ("literal text", "<r>abc</r>", 3),
            ("a predefined entity reference", "<r>&amp;</r>", 1),
            ("a decimal character reference", "<r>&#120;</r>", 1),
            ("a hexadecimal character reference", "<r>&#x78;</r>", 1),
            ("a character reference past ASCII", "<r>&#233;</r>", 1),
            ("a literal non-ASCII character", "<r>é</r>", 1),
            ("a CDATA section", "<r><![CDATA[abcd]]></r>", 4),
            // CDATA resolves nothing, so this is the five characters it spells.
            ("a reference spelled inside CDATA", "<r><![CDATA[&amp;]]></r>", 5),
            ("text, references and CDATA mixed", "<r>a&amp;b<![CDATA[cd]]>e</r>", 6),
            // A comment and a processing instruction are not text content.
            ("comments and instructions", "<r><!--c--><?pi v?>ab</r>", 2),
        ];
        for (case, xml, expected) in table {
            let counted = character_data_of(xml).expect("the document is well-formed");
            assert_eq!(
                counted, expected,
                "case {case:?} ({xml:?}): expected {expected} characters, got {counted}"
            );
        }
        // A reference that stands for nothing is not well-formed XML wherever it sits.
        for xml in ["<r>&nosuchentity;</r>", "<r>&#x110000;</r>", "<r>&#;</r>"] {
            assert!(
                character_data_of(xml).is_err(),
                "case {xml:?}: a reference standing for nothing is an error"
            );
        }
    }
}
