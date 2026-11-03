// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TranslateDocument`: the document a translation
//! answers with, in each format a document may arrive in, and the rules a
//! document translation is refused by.
//!
//! Every test drives the service the way the router does, through the harness
//! `terminology_lifecycle` already builds (`router`, `call`, `error_code`,
//! `message`, `import`), so the wire protocol (`awsJson1_1`, with the document
//! content as a base64 blob), the boundary constraint validation and the
//! handlers are all exercised.
//!
//! Each test asserts a RULE over a set of inputs rather than one recorded
//! request: every content type the model documents, both directions of the
//! pivot language and the pairs outside it, the detection of the source
//! language from documents in several languages, the settings a document
//! translation can and cannot apply, and the document limit on both sides.

use std::io::{Cursor, Read, Write};

use serde_json::{json, Value};

use crate::terminology_lifecycle::{
    call, error_code, import, message, router, string_at, ACCOUNT, EN_FR_CSV,
};

/// Unformatted text (`smithy_query shape=Document member=ContentType`).
const CONTENT_TYPE_PLAIN: &str = "text/plain";

/// HTML content (same member).
const CONTENT_TYPE_HTML: &str = "text/html";

/// A Word document (same member).
const CONTENT_TYPE_WORD: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// The documented size limit on one synchronous translation's document, in
/// BYTES: "The document size must not exceed 100 KB"
/// (`smithy_query shape=TranslateDocumentRequest member=Document`).
const MAX_DOCUMENT_BYTES: usize = 100 * 1024;

/// A paragraph of English, long enough for the statistical detector the service
/// resolves `auto` with to place it.
const ENGLISH_TEXT: &str = "The quick brown fox jumps over the lazy dog while the gardener \
                            waters the roses in the early morning light.";

/// The same, in French.
const FRENCH_TEXT: &str = "Le rapide renard brun saute par-dessus le chien paresseux pendant \
                           que le jardinier arrose les roses tôt le matin.";

/// The name the tests import [`EN_FR_CSV`] under.
const TERMS: &str = "doc-terms";

/// A document's content as `awsJson1_1` carries a blob, using the codec the
/// wire serializer itself uses (`aws_smithy_types::base64`).
fn blob(content: &[u8]) -> String {
    aws_smithy_types::base64::encode(content)
}

fn document_body(
    content: &[u8],
    content_type: &str,
    source: &str,
    target: &str,
    terminologies: &[&str],
) -> Value {
    let mut body = json!({
        "Document": {"Content": blob(content), "ContentType": content_type},
        "SourceLanguageCode": source,
        "TargetLanguageCode": target,
    });
    if !terminologies.is_empty() {
        body["TerminologyNames"] = json!(terminologies);
    }
    body
}

/// The bytes of the document a translation answered with.
fn translated_document(body: &Value) -> Vec<u8> {
    aws_smithy_types::base64::decode(string_at(body, "/TranslatedDocument/Content"))
        .expect("the answer carries the translated document as a base64 blob")
}

/// The translated document as text, for the formats whose content is text.
fn translated_text(body: &Value) -> String {
    String::from_utf8(translated_document(body))
        .expect("a text document's translation is UTF-8 text")
}

/// The `[Content_Types].xml` part of a minimal Office Open XML package, which
/// carries no run text and so is copied through unchanged.
const CONTENT_TYPES_PART: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>";

/// The `word/document.xml` part carrying `paragraphs`, one run each: inside a
/// Word document the run text (`w:t`) is the document's own text.
fn word_document_part(paragraphs: &[&str]) -> String {
    let body: String = paragraphs
        .iter()
        .map(|text| format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:body>{body}</w:body></w:document>"
    )
}

/// A Word document carrying `paragraphs`, as a real Office Open XML package:
/// the archive format is the `zip` crate's, here and in the handler that reads
/// it back.
fn word_document(paragraphs: &[&str]) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (name, part) in [
            ("[Content_Types].xml", CONTENT_TYPES_PART.to_string()),
            ("word/document.xml", word_document_part(paragraphs)),
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .expect("the part starts");
            writer
                .write_all(part.as_bytes())
                .expect("the part is written");
        }
        writer.finish().expect("the package is written");
    }
    buffer.into_inner()
}

/// One part of a package, as text.
fn package_part(package: &[u8], name: &str) -> String {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(package)).expect("the answer is a readable package");
    let mut part = archive
        .by_name(name)
        .unwrap_or_else(|error| panic!("the package carries {name}: {error}"));
    let mut text = String::new();
    part.read_to_string(&mut text).expect("the part is text");
    text
}

/// The document a translation answers with is in the format the request's
/// document was in: unformatted text is translated whole, an HTML document
/// keeps its markup and only the text in its elements is translated, and a Word
/// document comes back as a package of the same parts with its run text
/// translated.
///
/// The rows span every content type the model documents, and inside HTML the
/// forms that decide WHICH text is the document's own: indentation between
/// elements, a script element, and entity references both XML and HTML declare.
#[tokio::test]
async fn a_document_is_translated_in_the_format_it_arrived_in() {
    struct Row {
        case: &'static str,
        content_type: &'static str,
        content: Vec<u8>,
        /// The document the translation must answer with, as text.
        expected: String,
        /// The terms the answer must report as applied.
        applied: Value,
    }

    let rows = vec![
        Row {
            case: "unformatted text",
            content_type: CONTENT_TYPE_PLAIN,
            content: b"hello world".to_vec(),
            expected: "[mt:en->fr] bonjour world".to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [{"SourceText": "hello", "TargetText": "bonjour"}],
            }]),
        },
        Row {
            case: "unformatted text carrying no stored term",
            content_type: CONTENT_TYPE_PLAIN,
            content: b"a sentence of plain text".to_vec(),
            expected: "[mt:en->fr] a sentence of plain text".to_string(),
            applied: Value::Null,
        },
        Row {
            case: "an HTML document",
            content_type: CONTENT_TYPE_HTML,
            content: b"<html><body><p>hello world</p></body></html>".to_vec(),
            expected: "<html><body><p>[mt:en->fr] bonjour world</p></body></html>".to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [{"SourceText": "hello", "TargetText": "bonjour"}],
            }]),
        },
        Row {
            case: "an indented HTML document of two paragraphs",
            content_type: CONTENT_TYPE_HTML,
            content: b"<html>\n  <body>\n    <p>hello world</p>\n    <p>goodbye</p>\n  </body>\n</html>"
                .to_vec(),
            expected: "<html>\n  <body>\n    <p>[mt:en->fr] bonjour world</p>\n    \
                       <p>au revoir</p>\n  </body>\n</html>"
                .to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [
                    {"SourceText": "hello", "TargetText": "bonjour"},
                    {"SourceText": "goodbye", "TargetText": "au revoir"},
                ],
            }]),
        },
        Row {
            case: "an HTML document carrying a script",
            content_type: CONTENT_TYPE_HTML,
            content: b"<html><body><p>hello</p><script>var hello = 1;</script></body></html>"
                .to_vec(),
            expected: "<html><body><p>[mt:en->fr] bonjour</p><script>var hello = 1;</script>\
                       </body></html>"
                .to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [{"SourceText": "hello", "TargetText": "bonjour"}],
            }]),
        },
        Row {
            case: "an HTML document carrying an XML entity reference",
            content_type: CONTENT_TYPE_HTML,
            content: b"<p>hello &amp; goodbye</p>".to_vec(),
            expected: "<p>[mt:en->fr] bonjour &amp; au revoir</p>".to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [
                    {"SourceText": "hello", "TargetText": "bonjour"},
                    {"SourceText": "goodbye", "TargetText": "au revoir"},
                ],
            }]),
        },
        Row {
            case: "an HTML document carrying an entity only HTML declares",
            content_type: CONTENT_TYPE_HTML,
            content: b"<p>hello&nbsp;goodbye</p>".to_vec(),
            expected: "<p>[mt:en->fr] bonjour&nbsp;au revoir</p>".to_string(),
            applied: json!([{
                "Name": TERMS,
                "Terms": [
                    {"SourceText": "hello", "TargetText": "bonjour"},
                    {"SourceText": "goodbye", "TargetText": "au revoir"},
                ],
            }]),
        },
    ];

    let router = router();
    let (status, imported) = import(&router, TERMS, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");

    for row in &rows {
        let (status, answered) = call(
            &router,
            "TranslateDocument",
            &document_body(&row.content, row.content_type, "en", "fr", &[TERMS]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: TranslateDocument answered {answered}",
            row.case
        );
        let translated = translated_text(&answered);
        assert_eq!(
            translated, row.expected,
            "case {:?}: expected {:?}, got {translated:?}",
            row.case, row.expected
        );
        assert_eq!(
            answered.get("AppliedTerminologies"),
            row.applied.as_array().map(|_| &row.applied),
            "case {:?}: the answer must report the terms that matched, got {answered}",
            row.case
        );
        assert_eq!(
            string_at(&answered, "/SourceLanguageCode"),
            "en",
            "case {:?}: got {answered}",
            row.case
        );
        assert_eq!(
            string_at(&answered, "/TargetLanguageCode"),
            "fr",
            "case {:?}: got {answered}",
            row.case
        );
    }

    // A Word document is the same rule over a package: the answer is a package
    // of the same parts, its run text translated and every other part the
    // bytes it came in as.
    let document = word_document(&["hello world", "goodbye"]);
    let (status, answered) = call(
        &router,
        "TranslateDocument",
        &document_body(&document, CONTENT_TYPE_WORD, "en", "fr", &[TERMS]),
    )
    .await;
    assert_eq!(status, 200, "TranslateDocument answered {answered}");
    let package = translated_document(&answered);
    assert_eq!(
        package_part(&package, "word/document.xml"),
        word_document_part(&["[mt:en->fr] bonjour world", "au revoir"]),
        "the run text must be translated in place, got {answered}"
    );
    assert_eq!(
        package_part(&package, "[Content_Types].xml"),
        CONTENT_TYPES_PART,
        "a part carrying no run text must come back as it went in, got {answered}"
    );
    assert_eq!(
        answered.pointer("/AppliedTerminologies"),
        Some(&json!([{
            "Name": TERMS,
            "Terms": [
                {"SourceText": "hello", "TargetText": "bonjour"},
                {"SourceText": "goodbye", "TargetText": "au revoir"},
            ],
        }])),
        "got {answered}"
    );
}

/// One text translates the same way whether it arrives as text or inside a
/// plain-text document: both operations resolve the same stored terminology and
/// apply the same transform, so the document path is not a second translation
/// rule.
#[tokio::test]
async fn a_plain_text_document_translates_as_the_same_text_does() {
    let router = router();
    let (status, imported) = import(&router, TERMS, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");

    for case in [
        "hello world",
        "goodbye",
        "hello and goodbye, in one sentence",
        "nothing stored matches this sentence",
    ] {
        let (status, as_text) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": case,
                "SourceLanguageCode": "en",
                "TargetLanguageCode": "fr",
                "TerminologyNames": [TERMS],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: TranslateText answered {as_text}");
        let (status, as_document) = call(
            &router,
            "TranslateDocument",
            &document_body(case.as_bytes(), CONTENT_TYPE_PLAIN, "en", "fr", &[TERMS]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TranslateDocument answered {as_document}"
        );
        let document = translated_text(&as_document);
        assert_eq!(
            document,
            string_at(&as_text, "/TranslatedText"),
            "case {case:?}: the document must carry the translation of that text, got \
             {document:?}"
        );
        assert_eq!(
            as_document.get("AppliedTerminologies"),
            as_text.get("AppliedTerminologies"),
            "case {case:?}: both paths report the same applied terms, got {as_document}"
        );
    }
}

/// A document translation is from English into a supported language or from a
/// supported language into English, so a pair with English on neither side — and
/// a pair naming a language this service does not translate, or one language
/// twice — is refused with the modelled pair error, which names the pair in its
/// message and in its own members.
#[tokio::test]
async fn a_document_translation_has_english_on_one_side_of_its_pair() {
    struct Row {
        case: &'static str,
        source: &'static str,
        target: &'static str,
        status: u16,
    }

    let rows = [
        Row {
            case: "English into a supported language",
            source: "en",
            target: "fr",
            status: 200,
        },
        Row {
            case: "a supported language into English",
            source: "de",
            target: "en",
            status: 200,
        },
        Row {
            case: "English into a regional variant",
            source: "en",
            target: "pt-PT",
            status: 200,
        },
        Row {
            case: "a regional variant into English",
            source: "zh-TW",
            target: "en",
            status: 200,
        },
        Row {
            case: "two supported languages, neither English",
            source: "fr",
            target: "de",
            status: 400,
        },
        Row {
            case: "English into English",
            source: "en",
            target: "en",
            status: 400,
        },
        Row {
            case: "a language this service does not translate, into English",
            source: "xh",
            target: "en",
            status: 400,
        },
        Row {
            case: "English into a language this service does not translate",
            source: "en",
            target: "xh",
            status: 400,
        },
    ];

    let router = router();
    for row in &rows {
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(
                ENGLISH_TEXT.as_bytes(),
                CONTENT_TYPE_PLAIN,
                row.source,
                row.target,
                &[],
            ),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body}",
            row.case, row.status
        );
        if row.status != 200 {
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguagePairException",
                "case {:?}: got {body}",
                row.case
            );
            assert_eq!(
                message(&body),
                format!(
                    "Unsupported language pair: {} to {}.",
                    row.source, row.target
                ),
                "case {:?}: the refusal names the pair it rejected, got {body}",
                row.case
            );
            assert_eq!(
                string_at(&body, "/SourceLanguageCode"),
                row.source,
                "case {:?}: got {body}",
                row.case
            );
            assert_eq!(
                string_at(&body, "/TargetLanguageCode"),
                row.target,
                "case {:?}: got {body}",
                row.case
            );
        }
    }
}

/// A document is translated in one of the three formats the model documents,
/// whatever case its media type is written in; any other format is refused as
/// an invalid request naming the content type the request declared.
#[tokio::test]
async fn only_the_documented_content_types_are_translated() {
    let rows: [(&str, &str, u16); 7] = [
        ("unformatted text", CONTENT_TYPE_PLAIN, 200),
        ("HTML", CONTENT_TYPE_HTML, 200),
        ("a Word document", CONTENT_TYPE_WORD, 200),
        ("a media type in upper case", "TEXT/PLAIN", 200),
        ("a document format of its own", "application/pdf", 400),
        ("another text format", "text/xml", 400),
        ("an image", "image/png", 400),
    ];

    let router = router();
    for (case, content_type, expected) in rows {
        let content = if content_type == CONTENT_TYPE_WORD {
            word_document(&["hello world"])
        } else {
            b"hello world".to_vec()
        };
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(&content, content_type, "en", "fr", &[]),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: got {body}"
            );
            assert_eq!(
                message(&body),
                format!(
                    "Unsupported content type: {content_type}. Supported content types are \
                     {CONTENT_TYPE_PLAIN}, {CONTENT_TYPE_HTML} and {CONTENT_TYPE_WORD}."
                ),
                "case {case:?}: the refusal names the content type it rejected, got {body}"
            );
        }
    }
}

/// Content that is not the format its content type declares is an invalid
/// request, not a translation of whatever bytes it carried.
#[tokio::test]
async fn content_that_is_not_its_declared_format_is_refused() {
    let rows: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "text that is not UTF-8",
            CONTENT_TYPE_PLAIN,
            vec![0x68, 0x69, 0xff, 0xfe],
        ),
        (
            "a Word content type over text",
            CONTENT_TYPE_WORD,
            b"hello world".to_vec(),
        ),
        (
            "a Word content type over a truncated package",
            CONTENT_TYPE_WORD,
            word_document(&["hello world"])[..20].to_vec(),
        ),
    ];

    let router = router();
    for (case, content_type, content) in &rows {
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(content, content_type, "en", "fr", &[]),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: TranslateDocument answered {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: got {body}"
        );
        assert!(
            message(&body).starts_with(&format!(
                "The document content is not valid {content_type} content ("
            )),
            "case {case:?}: the refusal must name the format it could not read, got {body}"
        );
    }
}

/// `auto` asks the service to determine the source language of the document, so
/// the code the answer reports is computed from the document's own text — the
/// text a translation would translate, not the markup or the package around it
/// — and the same document always resolves the same way.
#[tokio::test]
async fn the_source_language_of_a_document_is_detected_from_its_text() {
    let rows: Vec<(&str, &str, Vec<u8>, &str, &str)> = vec![
        (
            "an English plain-text document",
            CONTENT_TYPE_PLAIN,
            ENGLISH_TEXT.as_bytes().to_vec(),
            "fr",
            "en",
        ),
        (
            "a French plain-text document",
            CONTENT_TYPE_PLAIN,
            FRENCH_TEXT.as_bytes().to_vec(),
            "en",
            "fr",
        ),
        (
            "a French HTML document, whose markup is not its text",
            CONTENT_TYPE_HTML,
            format!("<html><body><p>{FRENCH_TEXT}</p></body></html>")
                .into_bytes(),
            "en",
            "fr",
        ),
        (
            "a French Word document",
            CONTENT_TYPE_WORD,
            word_document(&[FRENCH_TEXT]),
            "en",
            "fr",
        ),
    ];

    let router = router();
    for (case, content_type, content, target, expected) in &rows {
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(content, content_type, "auto", target, &[]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TranslateDocument answered {body}"
        );
        assert_eq!(
            string_at(&body, "/SourceLanguageCode"),
            *expected,
            "case {case:?}: expected the detected source language {expected:?}, got {body}"
        );
        let (_, repeated) = call(
            &router,
            "TranslateDocument",
            &document_body(content, content_type, "auto", target, &[]),
        )
        .await;
        assert_eq!(
            repeated.pointer("/SourceLanguageCode"),
            body.pointer("/SourceLanguageCode"),
            "case {case:?}: the same document must resolve the same way, got {repeated}"
        );
    }

    // A document whose text no language can be read off is a pair the service
    // cannot translate, and the refusal reports the source as what the request
    // asked for.
    let (status, body) = call(
        &router,
        "TranslateDocument",
        &document_body(b"...", CONTENT_TYPE_PLAIN, "auto", "fr", &[]),
    )
    .await;
    assert_eq!(status, 400, "TranslateDocument answered {body}");
    assert_eq!(
        error_code(&body),
        "UnsupportedLanguagePairException",
        "got {body}"
    );
    assert_eq!(
        string_at(&body, "/SourceLanguageCode"),
        "auto",
        "got {body}"
    );
}

/// `AppliedSettings` reports the settings the translation really applied:
/// formality and profanity are applied to a document, and brevity — which this
/// operation does not support — is dropped, so the translation proceeds with it
/// turned off rather than being refused.
#[tokio::test]
async fn brevity_is_not_applied_to_a_document_translation() {
    let rows: [(&str, Value, Value); 5] = [
        ("no settings", Value::Null, Value::Null),
        (
            "formality",
            json!({"Formality": "FORMAL"}),
            json!({"Formality": "FORMAL"}),
        ),
        (
            "profanity",
            json!({"Profanity": "MASK"}),
            json!({"Profanity": "MASK"}),
        ),
        ("brevity alone", json!({"Brevity": "ON"}), Value::Null),
        (
            "brevity beside formality",
            json!({"Brevity": "ON", "Formality": "INFORMAL"}),
            json!({"Formality": "INFORMAL"}),
        ),
    ];

    let router = router();
    let mut rendered: Vec<(&str, String)> = Vec::new();
    for (case, settings, expected) in &rows {
        let mut body = document_body(b"hello world", CONTENT_TYPE_PLAIN, "en", "fr", &[]);
        if !settings.is_null() {
            body["Settings"] = settings.clone();
        }
        let (status, answered) = call(&router, "TranslateDocument", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TranslateDocument answered {answered}"
        );
        assert_eq!(
            answered.get("AppliedSettings"),
            if expected.is_null() {
                None
            } else {
                Some(expected)
            },
            "case {case:?}: AppliedSettings must report the settings applied, got {answered}"
        );
        rendered.push((case, translated_text(&answered)));
    }

    // A setting the translation applies changes the document; brevity, which it
    // does not apply, leaves the document the one an unset request gets.
    let unset = rendered
        .iter()
        .find(|(case, _)| *case == "no settings")
        .map(|(_, document)| document.clone())
        .expect("the unset row ran");
    for (case, document) in &rendered {
        let same = *document == unset;
        let expected_same = *case == "no settings" || *case == "brevity alone";
        assert_eq!(
            same, expected_same,
            "case {case:?}: expected the document to {} the unset one, got {document:?}",
            if expected_same { "match" } else { "differ from" }
        );
    }
}

/// A terminology a document translation names is resolved against stored state:
/// a name nothing imported is a 404 naming it, a second terminology in one
/// request is refused, and a deleted terminology stops being resolvable.
#[tokio::test]
async fn a_document_translation_resolves_its_terminologies_from_state() {
    let router = router();
    let (status, body) = import(&router, TERMS, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {body}");
    let (status, body) = import(&router, "other-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {body}");

    let (status, body) = call(
        &router,
        "TranslateDocument",
        &document_body(b"hello world", CONTENT_TYPE_PLAIN, "en", "fr", &["absent-terms"]),
    )
    .await;
    assert_eq!(status, 404, "TranslateDocument answered {body}");
    assert_eq!(error_code(&body), "ResourceNotFoundException", "got {body}");
    assert_eq!(
        message(&body),
        format!("The Terminology for {ACCOUNT} absent-terms was not found"),
        "the refusal names the terminology the request asked for, got {body}"
    );

    let (status, body) = call(
        &router,
        "TranslateDocument",
        &document_body(
            b"hello world",
            CONTENT_TYPE_PLAIN,
            "en",
            "fr",
            &[TERMS, "other-terms"],
        ),
    )
    .await;
    assert_eq!(status, 400, "TranslateDocument answered {body}");
    assert_eq!(error_code(&body), "InvalidRequestException", "got {body}");

    let (status, body) = call(&router, "DeleteTerminology", &json!({"Name": TERMS})).await;
    assert_eq!(status, 200, "DeleteTerminology answered {body}");
    let (status, body) = call(
        &router,
        "TranslateDocument",
        &document_body(b"hello world", CONTENT_TYPE_PLAIN, "en", "fr", &[TERMS]),
    )
    .await;
    assert_eq!(
        status, 404,
        "a translation naming a deleted terminology 404s, got {body}"
    );
}

/// The document limit is a BYTE limit on the document the request carries: a
/// document at the limit is translated and one past it is refused, in every
/// format and whatever the character width of its text.
#[tokio::test]
async fn a_document_past_the_size_limit_is_refused() {
    let plain_at_limit = vec![b'a'; MAX_DOCUMENT_BYTES];
    let plain_over_limit = vec![b'a'; MAX_DOCUMENT_BYTES + 1];
    // Two bytes per character, so the document is well inside any character
    // count and still past the byte limit.
    let wide_over_limit = "é".repeat(MAX_DOCUMENT_BYTES).into_bytes();
    let markup_over_limit = format!("<p>{}</p>", "a".repeat(MAX_DOCUMENT_BYTES)).into_bytes();

    let rows: Vec<(&str, &str, &[u8], u16)> = vec![
        (
            "unformatted text at the limit",
            CONTENT_TYPE_PLAIN,
            &plain_at_limit,
            200,
        ),
        (
            "unformatted text one byte past the limit",
            CONTENT_TYPE_PLAIN,
            &plain_over_limit,
            400,
        ),
        (
            "multi-byte text past the limit",
            CONTENT_TYPE_PLAIN,
            &wide_over_limit,
            400,
        ),
        (
            "HTML past the limit",
            CONTENT_TYPE_HTML,
            &markup_over_limit,
            400,
        ),
    ];

    let router = router();
    for (case, content_type, content, expected) in &rows {
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(content, content_type, "en", "fr", &[]),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "LimitExceededException",
                "case {case:?}: got {body}"
            );
            assert_eq!(
                message(&body),
                "The specified limit has been exceeded. Review your request and retry it with \
                 a quantity below the stated limit.",
                "case {case:?}: got {body}"
            );
        }
    }
}

/// A translation whose stored terms would grow the document past the length one
/// answer carries is refused rather than answered with a truncated document.
#[tokio::test]
async fn a_translation_that_grows_past_the_output_ceiling_is_refused() {
    let router = router();
    // One term whose target text is the longest a term may carry, so a handful
    // of matches is enough to pass the document limit.
    let long_target = "z".repeat(10_000);
    let (status, body) = import(
        &router,
        "long-terms",
        format!("en,fr\na,{long_target}\n").as_bytes(),
        "CSV",
        Some("UNI"),
    )
    .await;
    assert_eq!(status, 200, "ImportTerminology answered {body}");

    for (case, matches, expected) in [
        ("fifty matches, within the ceiling", 50_usize, 200_u16),
        ("two hundred matches, past the ceiling", 200, 400),
    ] {
        let text = vec!["a"; matches].join(" ");
        let (status, body) = call(
            &router,
            "TranslateDocument",
            &document_body(
                text.as_bytes(),
                CONTENT_TYPE_PLAIN,
                "en",
                "fr",
                &["long-terms"],
            ),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "LimitExceededException",
                "case {case:?}: got {body}"
            );
        } else {
            let translated = translated_text(&body);
            assert!(
                translated.contains(&long_target),
                "case {case:?}: the stored target term must appear in the document, got \
                 {} characters",
                translated.chars().count()
            );
        }
    }
}
