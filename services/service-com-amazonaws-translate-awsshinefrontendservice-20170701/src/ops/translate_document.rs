// SPDX-License-Identifier: CC0-1.0

//! @op TranslateDocument
//!
//! Translates the request's document from the source language into the target
//! language, keeping the document in the format it arrived in: unformatted text
//! is translated whole, an HTML document keeps its markup and only the text in
//! its elements is translated, and a Word document is rewritten part by part
//! inside its own package.
//!
//! As in `TranslateText`, the translation itself is a deterministic synthetic
//! transform marked as such — this emulator hosts no translation model — but
//! everything around it is real: which terminology applies, which of its terms
//! matched, the markup the translated text is carried in, and the resolved
//! source language all come from the request and from persisted state.
//!
//! The transform, the stored terminologies it applies and the language table it
//! resolves against live in [`crate::state`], shared with `TranslateText`, so
//! one text translates the same way whether it arrives as text or inside a
//! document.
//!
//! ---
//!
//! # Modeled contract for `TranslateDocument`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Translates the input document from the source language to the target language. This
//! synchronous operation supports text, HTML, or Word documents as the input document.
//! TranslateDocument supports translations from English to any supported language, and from
//! any supported language to English. Therefore, specify either the source language code or
//! the target language code as “en” (English). If you set the Formality parameter, the
//! request will fail if the target language does not support formality. For a list of
//! target languages that support formality, see Setting formality.
//!
//! ## Input
//!
//! * `Document` (Document) **required** — The content and content type for the document to be
//!   translated. The document size must not exceed 100 KB.
//! * `Settings` (TranslationSettings) — Settings to configure your translation output. You
//!   can configure the following options: Brevity: not supported. Formality: sets the
//!   formality level of the output text. Profanity: masks profane words and phrases in your
//!   translation output.
//! * `SourceLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code for
//!   the language of the source text. For a list of supported language codes, see Supported
//!   languages. To have Amazon Translate determine the source language of your text, you can
//!   specify auto in the SourceLanguageCode field. If you specify auto, Amazon Translate will
//!   call Amazon Comprehend to determine the source language. If you specify auto, you must
//!   send the TranslateDocument request in a region that supports Amazon Comprehend.
//!   Otherwise, the request returns an error indicating that autodetect is not supported.
//! * `TargetLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code
//!   requested for the translated document. For a list of supported language codes, see
//!   Supported languages.
//! * `TerminologyNames` (ResourceNameList) — The name of a terminology list file to add to
//!   the translation job. This file provides source terms and the desired translation for
//!   each term. A terminology list can contain a maximum of 256 terms. You can use one custom
//!   terminology resource in your translation request. Use the ListTerminologies operation to
//!   get the available terminology lists. For more information about custom terminology
//!   lists, see Custom terminology.
//!
//! ## Output
//!
//! * `AppliedSettings` (TranslationSettings) — Settings to configure your translation output.
//!   You can configure the following options: Brevity: reduces the length of the translation
//!   output for most translations. Available for TranslateText only. Formality: sets the
//!   formality level of the translation output. Profanity: masks profane words and phrases in
//!   the translation output.
//! * `AppliedTerminologies` (AppliedTerminologyList) — The names of the custom terminologies
//!   applied to the input text by Amazon Translate to produce the translated text document.
//! * `SourceLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code of
//!   the source document.
//! * `TargetLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code of
//!   the translated document.
//! * `TranslatedDocument` (TranslatedDocument) **required** — The document containing the
//!   translated content. The document format matches the source document format.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidRequestException` (HTTP 400) — The request that you made is not valid. Check
//!   your request to determine why it's not valid and then retry the request.
//! * `LimitExceededException` (HTTP 400) — The specified limit has been exceeded. Review your
//!   request and retry it with a quantity below the stated limit.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `ServiceUnavailableException` (HTTP 503) — The Amazon Translate service is temporarily
//!   unavailable. Wait a bit and then retry your request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.
//! * `UnsupportedLanguagePairException` (HTTP 400) — Amazon Translate does not support
//!   translation from the language of the source text into the requested target language. For
//!   more information, see Supported languages.

#[allow(unused_imports)]
use super::types::*;

use std::io::{Cursor, Read, Write};

use crate::state::{
    applied_term, invalid_request, is_supported_language, language_for_detector_code,
    limit_exceeded, resolve_terminologies, service_state, substitute, terminology_not_found,
    translation_marker, unsupported_language_pair, AUTO_LANGUAGE_CODE, MAX_APPLIED_TERMS,
    MAX_DOCUMENT_BYTES, MAX_TERMINOLOGIES_PER_TRANSLATION, TERMINOLOGY_RESOURCE,
};

/// The language code a document translation carries on one side of its pair:
/// "TranslateDocument supports translations from English to any supported
/// language, and from any supported language to English. Therefore, specify
/// either the source language code or the target language code as `en`"
/// (`smithy_query operation=TranslateDocument`).
const PIVOT_LANGUAGE_CODE: &str = "en";

/// Unformatted text, of which "Amazon Translate translates every character in
/// the content" (`smithy_query shape=Document member=ContentType`).
const CONTENT_TYPE_PLAIN: &str = "text/plain";

/// HTML content, of which "Amazon Translate translates only the text in the
/// HTML element" (same member).
const CONTENT_TYPE_HTML: &str = "text/html";

/// A Word document (.docx), which is an Office Open XML package (same member).
const CONTENT_TYPE_WORD: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// The HTML elements whose character data is not the document's text: a script
/// and a stylesheet are code the document carries, not text it reads.
const NON_TEXT_HTML_ELEMENTS: [&[u8]; 2] = [b"script", b"style"];

/// The WordprocessingML element one run's text lives in (`w:t`): inside a Word
/// document's package, that element's character data is the document's text and
/// everything else is the package's own structure.
const WORD_TEXT_ELEMENT: &[u8] = b"t";

/// How many uncompressed bytes this handler reads out of one Word document's
/// package.
///
/// A packaged part expands as it is read, by as much as the compression achieved
/// — so a request at the document limit could otherwise make this handler
/// materialise an unbounded amount of XML. The ceiling is this emulator's own
/// rather than a documented quota, and it is set well above the markup a
/// document at that limit carries.
const MAX_PACKAGE_BYTES: usize = 10 * MAX_DOCUMENT_BYTES;

/// How long a translated document this handler builds, in characters.
///
/// A translation EMITS the stored terminology's text rather than copying the
/// request's, so without a ceiling a document at the size limit could make the
/// answer arbitrarily larger than the request that asked for it. The model
/// states no length for the translated document — it is legitimately longer
/// than the source, since the source's terms are replaced by their translations
/// — so the ceiling is this emulator's own, at the same multiple of the document
/// limit the package extraction is held to.
const MAX_TRANSLATED_DOCUMENT_CHARS: usize = 10 * MAX_DOCUMENT_BYTES;

/// Why a document could not be translated.
enum DocumentFailure {
    /// The document's bytes are not the format its content type declares.
    Malformed(String),
    /// The document, or the translation of it, is larger than one synchronous
    /// translation carries.
    TooLarge,
}

impl DocumentFailure {
    /// The modelled refusal this failure is answered with: a document that is
    /// not what its content type says it is, is a request the caller has to fix;
    /// one that is too large has passed a limit of this operation.
    fn into_error(self, content_type: &str) -> TranslateDocumentError {
        match self {
            Self::Malformed(reason) => TranslateDocumentErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "The document content is not valid {content_type} content ({reason}). \
                         Check your request to determine why it's not valid and then retry \
                         the request."
                    )),
                },
            )
            .into(),
            Self::TooLarge => {
                TranslateDocumentErrorModeled::LimitExceededException(limit_exceeded()).into()
            }
        }
    }
}

/// The `InvalidRequestException` a request naming a document format this
/// operation does not translate is refused with: the content type the request
/// declared, and the three a document may be in
/// (`smithy_query shape=Document member=ContentType`).
fn unsupported_content_type(content_type: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: Some(format!(
            "Unsupported content type: {content_type}. Supported content types are \
             {CONTENT_TYPE_PLAIN}, {CONTENT_TYPE_HTML} and {CONTENT_TYPE_WORD}."
        )),
    }
}

/// Which text nodes of a markup document are the document's own text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextScope {
    /// Every text node outside the elements that carry code — HTML.
    ElementText,
    /// Only the text of a run — WordprocessingML.
    RunText,
}

/// A document translation in progress: the terms it substitutes, how much
/// translated text the answer may still carry, the marker the document is
/// marked synthetic with, and the terms that have matched so far.
struct Translation<'t> {
    pairs: &'t [(String, String)],
    /// The marker, until the document's first text has carried it.
    marker: Option<String>,
    budget: usize,
    applied: Vec<(&'t str, &'t str)>,
    overflowed: bool,
}

impl<'t> Translation<'t> {
    fn new(pairs: &'t [(String, String)], marker: String) -> Self {
        Self {
            pairs,
            budget: MAX_TRANSLATED_DOCUMENT_CHARS.saturating_sub(marker.chars().count()),
            marker: Some(marker),
            applied: Vec::new(),
            overflowed: false,
        }
    }

    /// Translate one text segment of the document, accumulating the terms that
    /// matched and what is left of the answer's size.
    ///
    /// Every segment of one document is translated against the same remaining
    /// budget, so a document made of many small segments is bounded exactly as
    /// one made of a single large one.
    fn segment(&mut self, text: &str) -> String {
        let (translated, applied, overflowed) = substitute(text, self.pairs, self.budget);
        self.overflowed = self.overflowed || overflowed;
        self.budget = self.budget.saturating_sub(translated.chars().count());
        for (source, target) in applied {
            if !self.applied.iter().any(|(seen, _)| *seen == source)
                && self.applied.len() < MAX_APPLIED_TERMS
            {
                self.applied.push((source, target));
            }
        }
        // One marker per document: it goes in front of the first segment that
        // carries text, so the answer is marked exactly once however much
        // markup its text is spread across.
        if text.trim().is_empty() {
            return translated;
        }
        match self.marker.take() {
            Some(marker) => format!("{marker}{translated}"),
            None => translated,
        }
    }
}

/// Write one event back exactly as it was read.
fn write_back(
    writer: &mut ::quick_xml::Writer<Vec<u8>>,
    event: ::quick_xml::events::Event<'_>,
) -> Result<(), DocumentFailure> {
    writer.write_event(event).map_err(|error| {
        DocumentFailure::Malformed(format!("the markup could not be written back ({error})"))
    })
}

/// Translate the text of a markup document, writing every other event back as
/// it was read, so the answer's markup is the request's markup.
///
/// The parsing and the re-serialisation are the `quick-xml` crate's; this only
/// decides which text nodes are the document's own text, by `scope`.
fn rewrite_markup(
    content: &[u8],
    scope: TextScope,
    translate: &mut dyn FnMut(&str) -> String,
) -> Result<Vec<u8>, DocumentFailure> {
    let mut reader = ::quick_xml::Reader::from_reader(content);
    // HTML is not well-formed XML — a void element never closes — so the
    // end-name check is off: an unclosed element is written back as it came
    // rather than failing the translation.
    reader.config_mut().check_end_names = false;
    let mut writer = ::quick_xml::Writer::new(Vec::new());
    let mut buffer = Vec::new();
    let mut inside_code = 0usize;
    let mut inside_run_text = 0usize;

    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            DocumentFailure::Malformed(format!("the markup is not well-formed ({error})"))
        })?;
        match event {
            ::quick_xml::events::Event::Eof => break,
            ::quick_xml::events::Event::Start(tag) => {
                let local = tag.local_name();
                let name = local.as_ref();
                if NON_TEXT_HTML_ELEMENTS.contains(&name) {
                    inside_code = inside_code.saturating_add(1);
                }
                if name == WORD_TEXT_ELEMENT {
                    inside_run_text = inside_run_text.saturating_add(1);
                }
                write_back(&mut writer, ::quick_xml::events::Event::Start(tag))?;
            }
            ::quick_xml::events::Event::End(tag) => {
                let local = tag.local_name();
                let name = local.as_ref();
                if NON_TEXT_HTML_ELEMENTS.contains(&name) {
                    inside_code = inside_code.saturating_sub(1);
                }
                if name == WORD_TEXT_ELEMENT {
                    inside_run_text = inside_run_text.saturating_sub(1);
                }
                write_back(&mut writer, ::quick_xml::events::Event::End(tag))?;
            }
            ::quick_xml::events::Event::Text(text) => {
                let translatable = match scope {
                    TextScope::ElementText => inside_code == 0,
                    TextScope::RunText => inside_run_text > 0,
                };
                if !translatable {
                    write_back(&mut writer, ::quick_xml::events::Event::Text(text))?;
                } else {
                    // The two steps the vendored XML codec takes for a text
                    // value: decode the bytes, then resolve the entity
                    // references `quick-xml` leaves verbatim.
                    let decoded = text.xml10_content().map_err(|error| {
                        DocumentFailure::Malformed(format!(
                            "a text node could not be read ({error})"
                        ))
                    })?;
                    match ::quick_xml::escape::unescape(&decoded) {
                        Ok(unescaped) => {
                            let translated = translate(unescaped.as_ref());
                            // Only the two characters that cannot stand in
                            // character data are escaped again, so the markup
                            // around the translated text is the markup the
                            // request wrote and no other character is rewritten.
                            let escaped = ::quick_xml::escape::minimal_escape(&translated);
                            write_back(
                                &mut writer,
                                ::quick_xml::events::Event::Text(
                                    ::quick_xml::events::BytesText::from_escaped(escaped),
                                ),
                            )?;
                        }
                        // HTML declares entity references XML does not
                        // (`&nbsp;` and its kin). A node carrying one of those
                        // is translated in the form it was written in and
                        // written back verbatim, so an entity this codec cannot
                        // resolve survives the translation instead of failing
                        // it.
                        Err(_) => {
                            let translated = translate(decoded.as_ref());
                            write_back(
                                &mut writer,
                                ::quick_xml::events::Event::Text(
                                    ::quick_xml::events::BytesText::from_escaped(translated),
                                ),
                            )?;
                        }
                    }
                }
            }
            other => write_back(&mut writer, other)?,
        }
        buffer.clear();
    }

    Ok(writer.into_inner())
}

/// Whether a part of a Word document's package carries run text, and so is a
/// part this handler rewrites. Every other part — styles, fonts, the package's
/// relationships — is copied through byte for byte.
fn carries_run_text(name: &str, part: &[u8]) -> bool {
    name.starts_with("word/")
        && name.ends_with(".xml")
        && part.windows(4).any(|window| window == b"<w:t")
}

/// Translate a Word document: every part of its package is written back into a
/// package of the same entries, and the run text of the parts that carry any is
/// translated.
///
/// Reading and writing the package format is the `zip` crate's job; what this
/// adds is the bound on how much it may unpack.
fn rewrite_word(
    content: &[u8],
    translate: &mut dyn FnMut(&str) -> String,
) -> Result<Vec<u8>, DocumentFailure> {
    let mut archive = ::zip::ZipArchive::new(Cursor::new(content)).map_err(|error| {
        DocumentFailure::Malformed(format!("the package could not be opened ({error})"))
    })?;
    let mut package = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = ::zip::write::SimpleFileOptions::default();
    let mut extracted = 0usize;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            DocumentFailure::Malformed(format!("a package part could not be read ({error})"))
        })?;
        // zip reports an entry's name as a `Result`, because a name that is not
        // valid UTF-8 cannot be reported as a string.
        let name = entry
            .name()
            .map_err(|error| {
                DocumentFailure::Malformed(format!(
                    "a package part name could not be read ({error})"
                ))
            })?
            .to_string();
        if name.ends_with('/') {
            package.add_directory(name, options).map_err(|error| {
                DocumentFailure::Malformed(format!(
                    "a package directory could not be written back ({error})"
                ))
            })?;
            continue;
        }
        // One byte past what is left of the extraction ceiling, so a part that
        // fills it exactly is still read whole and only a part that passes it
        // is refused.
        let remaining = MAX_PACKAGE_BYTES.saturating_sub(extracted);
        let mut part = Vec::new();
        let read = u64::try_from(remaining.saturating_add(1)).unwrap_or(u64::MAX);
        Read::take(&mut entry, read)
            .read_to_end(&mut part)
            .map_err(|error| {
                DocumentFailure::Malformed(format!("a package part could not be read ({error})"))
            })?;
        if part.len() > remaining {
            return Err(DocumentFailure::TooLarge);
        }
        extracted = extracted.saturating_add(part.len());

        let rewritten = if carries_run_text(&name, &part) {
            rewrite_markup(&part, TextScope::RunText, translate)?
        } else {
            part
        };
        package.start_file(name, options).map_err(|error| {
            DocumentFailure::Malformed(format!("a package part could not be written ({error})"))
        })?;
        package.write_all(&rewritten).map_err(|error| {
            DocumentFailure::Malformed(format!("a package part could not be written ({error})"))
        })?;
    }

    let finished = package.finish().map_err(|error| {
        DocumentFailure::Malformed(format!("the package could not be written ({error})"))
    })?;
    Ok(finished.into_inner())
}

/// Translate unformatted text, of which every character is content.
fn rewrite_plain(
    content: &[u8],
    translate: &mut dyn FnMut(&str) -> String,
) -> Result<Vec<u8>, DocumentFailure> {
    let text = ::std::str::from_utf8(content).map_err(|error| {
        DocumentFailure::Malformed(format!("the content is not valid UTF-8 text ({error})"))
    })?;
    Ok(translate(text).into_bytes())
}

/// Translate the document in the format its content type declares, handing each
/// of its text segments to `translate` and keeping everything else as it was:
/// "The document format matches the source document format".
fn rewrite_document(
    content: &[u8],
    content_type: &str,
    translate: &mut dyn FnMut(&str) -> String,
) -> Result<Vec<u8>, DocumentFailure> {
    match content_type {
        CONTENT_TYPE_HTML => rewrite_markup(content, TextScope::ElementText, translate),
        CONTENT_TYPE_WORD => rewrite_word(content, translate),
        // The handler admits three content types, so what is left here is
        // unformatted text.
        _ => rewrite_plain(content, translate),
    }
}

/// The source language of a document whose request asked for `auto`.
///
/// The detection reads the document's own text — the text the translation would
/// translate, not its markup or its package — and is the `whatlang` crate's
/// statistical identifier over that text's script and trigrams, so the resolved
/// code is computed from the document and the same document always resolves the
/// same way.
fn detect_source_language(
    content: &[u8],
    content_type: &str,
    target_language_code: &str,
) -> Result<String, TranslateDocumentError> {
    let mut text = String::new();
    {
        // The rewritten bytes are discarded: this pass reads the document's
        // text through the same traversal the translation uses, so the
        // detection sees exactly what would be translated.
        let mut collect = |segment: &str| {
            text.push_str(segment);
            segment.to_string()
        };
        rewrite_document(content, content_type, &mut collect)
            .map_err(|failure| failure.into_error(content_type))?;
    }
    ::whatlang::detect_lang(&text)
        .and_then(|detected| language_for_detector_code(detected.code()))
        .map(ToString::to_string)
        .ok_or_else(|| {
            // This operation models no low-confidence error, and a language
            // this service does not translate has no supported code to report a
            // source as: either way the pair the request asked for is what it is
            // refused by.
            TranslateDocumentErrorModeled::UnsupportedLanguagePairException(
                unsupported_language_pair(AUTO_LANGUAGE_CODE, target_language_code),
            )
            .into()
        })
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TranslateDocumentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TranslateDocumentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#TranslateDocument")]
#[derive(Debug)]
pub enum TranslateDocumentErrorModeled {
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    TooManyRequestsException(TooManyRequestsException),
    UnsupportedLanguagePairException(UnsupportedLanguagePairException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::translate_document::TranslateDocumentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TranslateDocumentError = crate::scaffolded::Scaffolded<TranslateDocumentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TranslateDocumentErrorModeled> for TranslateDocumentError {
    fn from(modeled: TranslateDocumentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TranslateDocument` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TranslateDocument<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TranslateDocument<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TranslateDocument<Eng>
{
    type Input = TranslateDocumentInput;
    type Output = TranslateDocumentOutput;
    type Error = TranslateDocumentError;

    const NAME: &'static str = "TranslateDocument";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSLATE_DOCUMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSLATE_DOCUMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            TranslateDocumentErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        // "The document size must not exceed 100 KB" is the length the document
        // member is modelled with, so a document past it is refused before this
        // handler runs, in the operation's own error class
        // (`crate::service::oversize_document`).
        //
        // A media type is case-insensitive, and the member's pattern admits no
        // parameters after the subtype, so the declared format is the whole
        // value.
        let content_type = input.document.content_type.to_ascii_lowercase();
        if ![CONTENT_TYPE_PLAIN, CONTENT_TYPE_HTML, CONTENT_TYPE_WORD]
            .contains(&content_type.as_str())
        {
            return Err(TranslateDocumentErrorModeled::InvalidRequestException(
                unsupported_content_type(&input.document.content_type),
            )
            .into());
        }

        let requested_terminologies = input.terminology_names.clone().unwrap_or_default();
        // "You can use one custom terminology resource in your translation
        // request."
        if requested_terminologies.len() > MAX_TERMINOLOGIES_PER_TRANSLATION {
            return Err(TranslateDocumentErrorModeled::InvalidRequestException(
                invalid_request(),
            )
            .into());
        }

        // `auto` asks the service to determine the source language from the
        // document itself, so the resolved code is computed from the text the
        // document carries rather than echoed back from the request.
        let source_language_code = if input.source_language_code == AUTO_LANGUAGE_CODE {
            detect_source_language(
                &input.document.content,
                &content_type,
                &input.target_language_code,
            )?
        } else {
            input.source_language_code.clone()
        };
        let target_language_code = input.target_language_code.clone();

        // Amazon Translate translates between the languages it supports and
        // never from a language into itself; a document translation is from
        // English into a supported language or from a supported language into
        // English, so one side of the pair is English.
        if source_language_code == target_language_code
            || !is_supported_language(&source_language_code)
            || !is_supported_language(&target_language_code)
            || (source_language_code != PIVOT_LANGUAGE_CODE
                && target_language_code != PIVOT_LANGUAGE_CODE)
        {
            return Err(
                TranslateDocumentErrorModeled::UnsupportedLanguagePairException(
                    unsupported_language_pair(&source_language_code, &target_language_code),
                )
                .into(),
            );
        }

        let resolved = {
            let store = state.lock();
            resolve_terminologies(
                &store,
                &requested_terminologies,
                &source_language_code,
                &target_language_code,
            )
            .map_err(|name| {
                TranslateDocumentErrorModeled::ResourceNotFoundException(terminology_not_found(
                    TERMINOLOGY_RESOURCE,
                    &name,
                ))
            })?
        };

        // "Brevity: not supported" for a document, and an unsupported brevity
        // request is one the "translation proceeds with the brevity setting
        // turned off" for (`smithy_query shape=TranslationSettings
        // member=Brevity`) — so it is dropped rather than refused, and
        // `AppliedSettings` reports the settings the translation really applied.
        let applied_settings = input
            .settings
            .clone()
            .map(|settings| TranslationSettings {
                brevity: None,
                ..settings
            })
            .filter(|settings| settings.formality.is_some() || settings.profanity.is_some());

        let marker = translation_marker(
            &source_language_code,
            &target_language_code,
            applied_settings.as_ref(),
        );

        // The rewrite below is bounded by the document limit, but it is the only
        // caller-driven work this handler does: yield first, so a timeout or a
        // cancellation has a point to land on.
        ::tokio::task::yield_now().await;

        let pairs: &[(String, String)] = resolved
            .first()
            .map_or(&[], |terminology| terminology.pairs.as_slice());
        let mut translation = Translation::new(pairs, marker);
        let content = {
            let mut translate = |text: &str| translation.segment(text);
            rewrite_document(&input.document.content, &content_type, &mut translate)
                .map_err(|failure| failure.into_error(&content_type))?
        };
        if translation.overflowed {
            // The translated document no longer fits what one synchronous
            // translation carries, which is this operation's own size refusal
            // rather than a truncated answer.
            return Err(
                TranslateDocumentErrorModeled::LimitExceededException(limit_exceeded()).into(),
            );
        }

        let applied_terminologies: Vec<AppliedTerminology> = resolved
            .iter()
            .filter(|_| !translation.applied.is_empty())
            .map(|terminology| AppliedTerminology {
                name: Some(terminology.name.clone()),
                terms: Some(
                    translation
                        .applied
                        .iter()
                        .map(|(source, target)| applied_term(source, target))
                        .collect(),
                ),
            })
            .collect();

        Ok(TranslateDocumentOutput {
            translated_document: TranslatedDocument { content },
            source_language_code,
            target_language_code,
            // Reported only when the request named a terminology that matched,
            // as on the text path.
            applied_terminologies: (!applied_terminologies.is_empty())
                .then_some(applied_terminologies),
            applied_settings,
        })
    }
}
