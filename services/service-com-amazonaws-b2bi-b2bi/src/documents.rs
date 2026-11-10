// SPDX-License-Identifier: CC0-1.0

//! Whether a document really is the file format the request declared it to be.
//!
//! The test-harness operations (`TestConversion`, `TestMapping`) are handed a document
//! inline together with the name of its format, and both have to refuse content that is
//! not that format before they do anything else with it. The check lives here, once, so
//! the two cannot disagree about what "valid JSON" or "well-formed XML" means.
//!
//! Declared as a submodule of [`crate::state`] because `src/lib.rs` is generated and
//! cannot take a new top-level `mod`.
//!
//! Both checks delegate to a maintained parser — `serde_json` for JSON, `quick-xml` for
//! XML — rather than to hand-written syntax rules, and report that parser's own message.
//! For JSON that message is the whole refusal ([`json_parse_diagnostic`]): the parser
//! states what the parse stopped on and where, and a service surfaces that diagnostic
//! rather than its own account of the member the document arrived in.
//!
//! Beside them sits [`to_document_text`], the JSON LAYOUT a generated mapping template is
//! written in: `CreateStarterMappingTemplate`'s `mappingTemplate` is modelled as a plain
//! string and carries a whole document as text, and that text is compared byte for byte by
//! the clients that read it back.

/// The `FileFormat` / `ConversionSourceFormat` value naming a JSON document.
pub const JSON: &str = "JSON";

/// The `FileFormat` / `ConversionSourceFormat` value naming an XML document.
pub const XML: &str = "XML";

/// The extra `FileFormat` value that names no document format at all
/// (`FileFormat` is `JSON | NOT_USED | XML`; `NOT_USED` exists for the deprecated
/// `CreateTransformer$fileFormat` member, which a transformer using `inputConversion`
/// leaves unset).
pub const NOT_USED: &str = "NOT_USED";

/// Parse `content` as a JSON document, answering the parser's own diagnostic of where the
/// parse stopped on failure ([`json_parse_diagnostic`]).
pub fn parse_json(content: &str) -> Result<::serde_json::Value, String> {
    ::serde_json::from_str(content).map_err(|failure| json_parse_diagnostic(content, &failure))
}

/// WHERE a JSON document the caller submitted stopped parsing, as the JSON parser states
/// it: what the parse stopped on, what belonged there instead, and the position — with the
/// document itself withheld.
///
/// This is the WHOLE sentence a request carrying an unparsable JSON document is refused
/// with: it names no input member, because the diagnostic is the parser's own rather than
/// this service's account of the member, and it reports the source as redacted rather than
/// echoing a document that may be sensitive back to the caller.
///
/// Every reading in it — the offending character and its character code, the word a parse
/// stopped on where a value belongs, the expectation at that position, and the line and
/// column — is the tree's shared one ([`::service_common::json_parse_fault`]), taken from
/// the parse that actually ran, so two services handed one document cannot disagree about
/// where it went wrong.
pub fn json_parse_diagnostic(content: &str, failure: &::serde_json::Error) -> String {
    use ::service_common::json_parse_fault as fault;

    // A document that stops on an unquoted word where a value belongs is reported by
    // naming that WORD, at the position the word starts; every other fault names the
    // character the parse stopped on, or the end of the input, at the position the parse
    // itself reports.
    let (diagnosis, line, column) = match fault::unrecognized_token(content, failure) {
        Some((word, line, column)) => (
            format!(
                "Unrecognized token '{word}': was expecting ({})",
                fault::VALUE_CLASSES
            ),
            line,
            column,
        ),
        None => {
            let (line, column) = fault::stopped_at(content, failure);
            (fault::unexpected_input(content, failure), line, column)
        }
    };
    format!(
        "{diagnosis} at {}",
        fault::location_clause(fault::REDACTED_SOURCE, line, column)
    )
}

/// Render `document` as the pretty-printed JSON TEXT a GENERATED mapping template is
/// recorded with, answering `serde_json`'s own message on failure.
///
/// `CreateStarterMappingTemplate`'s `mappingTemplate` is modelled as a plain `String` and
/// carries a whole JSON document as text, and the layout of that text is part of what a
/// caller reads back and compares, so it is written here, once, rather than inside the
/// generator:
///
/// * two spaces of indentation per level of nesting;
/// * a space on BOTH sides of the colon between a member's name and its value;
/// * an array on the line it opens on, its elements separated by a comma and a space.
///
/// `serde_json`'s own pretty layout — no space before the colon, every array element on a
/// line of its own — is what the MAPPED document of a `TestMapping` answer is recorded
/// with, so that operation writes its text with `serde_json::to_string_pretty` and the two
/// layouts stay apart.
pub fn to_document_text<T: ::serde::Serialize + ?Sized>(document: &T) -> Result<String, String> {
    let mut text = Vec::new();
    let mut serializer =
        ::serde_json::Serializer::with_formatter(&mut text, DocumentFormatter::default());
    document
        .serialize(&mut serializer)
        .map_err(|failure| failure.to_string())?;
    String::from_utf8(text).map_err(|failure| failure.to_string())
}

/// The layout [`to_document_text`] writes, as a `serde_json` formatter.
///
/// `serde_json`'s own pretty formatter writes no space before the colon and puts every
/// element of an array on a line of its own, so the layout is spelled out here instead.
#[derive(Debug, Default)]
struct DocumentFormatter {
    /// How deeply the value being written nests.
    depth: usize,
    /// Whether the container being closed wrote a member.
    wrote_member: bool,
}

impl DocumentFormatter {
    /// Two spaces per level of nesting.
    fn indent<W: ?Sized + ::std::io::Write>(&self, writer: &mut W) -> ::std::io::Result<()> {
        for _ in 0..self.depth {
            writer.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl ::serde_json::ser::Formatter for DocumentFormatter {
    fn begin_object<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> ::std::io::Result<()> {
        self.depth += 1;
        self.wrote_member = false;
        writer.write_all(b"{")
    }

    fn begin_object_key<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> ::std::io::Result<()> {
        writer.write_all(if first { b"\n" } else { b",\n" })?;
        self.indent(writer)
    }

    fn begin_object_value<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> ::std::io::Result<()> {
        writer.write_all(b" : ")
    }

    fn end_object_value<W: ?Sized + ::std::io::Write>(
        &mut self,
        _writer: &mut W,
    ) -> ::std::io::Result<()> {
        self.wrote_member = true;
        Ok(())
    }

    fn end_object<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> ::std::io::Result<()> {
        self.depth -= 1;
        if self.wrote_member {
            writer.write_all(b"\n")?;
            self.indent(writer)?;
        }
        self.wrote_member = true;
        writer.write_all(b"}")
    }

    fn begin_array<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> ::std::io::Result<()> {
        // An array stays on the line it opens on, so it adds no level of indentation to
        // the objects inside it.
        self.wrote_member = false;
        writer.write_all(b"[")
    }

    fn begin_array_value<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> ::std::io::Result<()> {
        writer.write_all(if first { b" " } else { b", " })
    }

    fn end_array_value<W: ?Sized + ::std::io::Write>(
        &mut self,
        _writer: &mut W,
    ) -> ::std::io::Result<()> {
        self.wrote_member = true;
        Ok(())
    }

    fn end_array<W: ?Sized + ::std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> ::std::io::Result<()> {
        if self.wrote_member {
            writer.write_all(b" ")?;
        }
        self.wrote_member = true;
        writer.write_all(b"]")
    }
}

/// Check that `content` is a well-formed XML document: a prolog, exactly ONE root
/// element whose tags are balanced and correctly nested, and nothing but whitespace,
/// comments and processing instructions around it (XML 1.0 §2.1, `document ::= prolog
/// element Misc*`).
///
/// The balance and nesting come from `quick-xml`'s own `check_end_names`; this walk adds
/// the two conditions an event reader does not raise by itself, so that a document with
/// no element at all (`""`) or a bare string (`not xml`) is not mistaken for XML.
pub fn check_xml(content: &str) -> Result<(), String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(content);
    reader.config_mut().check_end_names = true;

    let mut depth: usize = 0;
    let mut roots: usize = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(_)) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Empty(_)) => {
                if depth == 0 {
                    roots += 1;
                }
            }
            Ok(Event::Text(text)) => {
                // The raw bytes are enough here: the only question is whether anything
                // other than whitespace sits OUTSIDE the root element.
                let raw = String::from_utf8_lossy(text.as_ref());
                if depth == 0 && !raw.trim().is_empty() {
                    return Err(format!(
                        "character data outside the root element: {:?}",
                        raw.trim()
                    ));
                }
            }
            // A CDATA section is character data too, and XML allows none around the root
            // element: handled beside `Text` so the two cannot be classified differently.
            Ok(Event::CData(section)) => {
                if depth == 0 {
                    return Err(format!(
                        "character data outside the root element: {:?}",
                        String::from_utf8_lossy(section.as_ref())
                    ));
                }
            }
            Ok(_) => {}
            Err(failure) => return Err(failure.to_string()),
        }
    }

    match roots {
        1 if depth == 0 => Ok(()),
        // Reaching the end of the document with elements still open: `check_end_names`
        // only catches a MISMATCHED end tag, not a missing one.
        1 => Err(format!(
            "the document ends with {depth} element(s) still open"
        )),
        0 => Err("the document has no root element".to_string()),
        found => Err(format!(
            "the document has {found} root elements, and XML allows exactly one"
        )),
    }
}
