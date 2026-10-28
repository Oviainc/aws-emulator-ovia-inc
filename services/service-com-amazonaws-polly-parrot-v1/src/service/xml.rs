// SPDX-License-Identifier: CC0-1.0

//! What XML itself says about the texts this service reads as XML documents.
//!
//! Both of the service's XML readers — `PutLexicon`'s PLS `Content` and
//! `SynthesizeSpeech`'s `ssml` `Text` — have to answer the same question about the
//! characters that sit OUTSIDE a document element, so the answer lives here once
//! rather than in each operation.

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

#[cfg(test)]
mod tests {
    use super::is_xml_whitespace;

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
}
