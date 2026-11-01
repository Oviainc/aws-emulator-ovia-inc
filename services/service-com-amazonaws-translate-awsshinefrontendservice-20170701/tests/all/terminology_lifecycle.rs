// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the custom-terminology lifecycle and the
//! synchronous translate call: `ImportTerminology`, `GetTerminology`,
//! `ListTerminologies`, `TranslateText` and `DeleteTerminology`.
//!
//! Every test drives the service the way the router does — an `http::Request`
//! through [`MultiServiceRouter::dispatch`] — so the wire protocol
//! (`awsJson1_1`: `POST /` with `X-Amz-Target`), the boundary constraint
//! validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_translate_awsshinefrontendservice_20170701::EmulatedService;

/// The `X-Amz-Target` prefix this service's operations are named under
/// (OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "AWSShineFrontendService_20170701";

/// The Region the tests sign for, which is the Region the ARNs carry.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries
/// (`engine::mock_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The only `MergeStrategy` the model declares.
const OVERWRITE: &str = "OVERWRITE";

pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in [`REGION`]. It is what the
/// router routes on, and what the handlers read the ARN Region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/translate/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", authorization())
        .header("host", format!("translate.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code `awsJson1_1` puts in `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn message(body: &Value) -> String {
    for member in ["message", "Message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

pub(crate) fn string_at(body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number_at(body: &Value, pointer: &str) -> i64 {
    body.pointer(pointer).and_then(Value::as_i64).unwrap_or(-1)
}

fn seconds_at(body: &Value, pointer: &str) -> f64 {
    body.pointer(pointer).and_then(Value::as_f64).unwrap_or(-1.0)
}

/// The blob encoding `awsJson1_1` carries `TerminologyData$File` in, using the
/// codec the wire serializer itself uses (`aws_smithy_types::base64`).
fn blob(file: &[u8]) -> String {
    aws_smithy_types::base64::encode(file)
}

/// The revision segment of a terminology's ARN, as `src/state.rs` derives it:
/// the first 16 hex characters of the imported file's SHA-256.
fn revision_of(file: &[u8]) -> String {
    hex::encode(Sha256::digest(file)).chars().take(16).collect()
}

pub(crate) fn expected_arn(name: &str, file: &[u8]) -> String {
    format!(
        "arn:aws:translate:{REGION}:{ACCOUNT}:terminology/{name}/{}",
        revision_of(file)
    )
}

fn import_body(name: &str, file: &[u8], format: &str, directionality: Option<&str>) -> Value {
    let mut data = json!({"File": blob(file), "Format": format});
    if let Some(directionality) = directionality {
        data["Directionality"] = json!(directionality);
    }
    json!({"Name": name, "MergeStrategy": OVERWRITE, "TerminologyData": data})
}

pub(crate) async fn import(
    router: &MultiServiceRouter,
    name: &str,
    file: &[u8],
    format: &str,
    directionality: Option<&str>,
) -> (u16, Value) {
    call(
        router,
        "ImportTerminology",
        &import_body(name, file, format, directionality),
    )
    .await
}

/// The CSV terminology the issue's workflow imports: a header row of language
/// codes and one term per later row.
pub(crate) const EN_FR_CSV: &[u8] = b"en,fr\nhello,bonjour\ngoodbye,au revoir\n";

/// The same shape tab-separated.
const EN_ES_TSV: &[u8] = b"en\tes\nhello\thola\n";

/// A TMX terminology: `<header srclang=..>` names the source language and each
/// `<tu>` carries one `<tuv xml:lang=..><seg>` per language.
const EN_DE_TMX: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<tmx version="1.4">
  <header srclang="en" datatype="plaintext" segtype="sentence" creationtool="test" creationtoolversion="1" adminlang="en" o-tmf="test"/>
  <body>
    <tu>
      <tuv xml:lang="en"><seg>computer</seg></tuv>
      <tuv xml:lang="de"><seg>Rechner</seg></tuv>
    </tu>
    <tu>
      <tuv xml:lang="en"><seg>network &amp; cable</seg></tuv>
      <tuv xml:lang="de"><seg>Netzwerk</seg></tuv>
    </tu>
  </body>
</tmx>
"#;

/// A three-language CSV, used for the multi-directional rows.
const EN_FR_DE_CSV: &[u8] = b"en,fr,de\nbook,livre,Buch\n";

/// The whole workflow the issue describes, end to end: import, read back, list,
/// translate with the stored terms, then delete and watch every reader agree.
#[tokio::test]
async fn terminology_lifecycle_is_served_from_stored_state() {
    let router = router();

    let (status, imported) = import(&router, "demo-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let properties = "/TerminologyProperties";
    assert_eq!(
        string_at(&imported, &format!("{properties}/Name")),
        "demo-terms",
        "the import must report the name it stored, got {imported}"
    );
    assert_eq!(
        number_at(&imported, &format!("{properties}/TermCount")),
        2,
        "TermCount must be the number of rows parsed out of the file, got {imported}"
    );
    assert_eq!(
        number_at(&imported, &format!("{properties}/SizeBytes")),
        i64::try_from(EN_FR_CSV.len()).expect("the file length fits"),
        "SizeBytes must be the uploaded byte length, got {imported}"
    );
    assert_eq!(
        string_at(&imported, &format!("{properties}/Arn")),
        expected_arn("demo-terms", EN_FR_CSV),
        "the ARN must carry the name and a revision derived from the file, got {imported}"
    );
    assert_eq!(
        string_at(&imported, &format!("{properties}/SourceLanguageCode")),
        "en",
        "the first column is the source language, got {imported}"
    );
    assert_eq!(
        imported.pointer(&format!("{properties}/TargetLanguageCodes")),
        Some(&json!(["fr"])),
        "the remaining columns are the target languages, got {imported}"
    );
    assert!(
        seconds_at(&imported, &format!("{properties}/CreatedAt")) > 0.0,
        "CreatedAt must be stamped, got {imported}"
    );
    assert!(
        imported.pointer("/AuxiliaryDataLocation").is_none(),
        "a file with nothing skipped produces no auxiliary location, got {imported}"
    );

    // Read back: the same stored properties, plus a data location.
    let (status, fetched) = call(&router, "GetTerminology", &json!({"Name": "demo-terms"})).await;
    assert_eq!(status, 200, "GetTerminology answered {fetched}");
    assert_eq!(
        fetched.pointer(properties),
        imported.pointer(properties),
        "GetTerminology must report exactly what ImportTerminology stored, got {fetched}"
    );
    assert_eq!(
        string_at(&fetched, "/TerminologyDataLocation/RepositoryType"),
        "S3",
        "the data location names its repository, got {fetched}"
    );
    let location = string_at(&fetched, "/TerminologyDataLocation/Location");
    assert!(
        location.starts_with("https://") && location.contains(&revision_of(EN_FR_CSV)),
        "the location must be a presigned URL for the stored revision, got {location:?}"
    );
    let (_, fetched_again) =
        call(&router, "GetTerminology", &json!({"Name": "demo-terms"})).await;
    assert_eq!(
        string_at(&fetched_again, "/TerminologyDataLocation/Location"),
        location,
        "two reads of one terminology must report one location, got {fetched_again}"
    );

    // Listed.
    let (status, listed) = call(&router, "ListTerminologies", &json!({})).await;
    assert_eq!(status, 200, "ListTerminologies answered {listed}");
    assert_eq!(
        listed.pointer("/TerminologyPropertiesList/0"),
        imported.pointer(properties),
        "the listing must report the stored properties, got {listed}"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "the last page carries no NextToken, got {listed}"
    );

    // Translated, with the stored terms substituted and reported.
    let (status, translated) = call(
        &router,
        "TranslateText",
        &json!({
            "Text": "hello world",
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
            "TerminologyNames": ["demo-terms"],
        }),
    )
    .await;
    assert_eq!(status, 200, "TranslateText answered {translated}");
    assert!(
        string_at(&translated, "/TranslatedText").contains("bonjour"),
        "the stored target term must appear in the translation, got {translated}"
    );
    assert_eq!(
        translated.pointer("/AppliedTerminologies"),
        Some(&json!([{
            "Name": "demo-terms",
            "Terms": [{"SourceText": "hello", "TargetText": "bonjour"}],
        }])),
        "only the terms that matched the text are reported, got {translated}"
    );
    assert_eq!(
        string_at(&translated, "/SourceLanguageCode"),
        "en",
        "got {translated}"
    );
    assert_eq!(
        string_at(&translated, "/TargetLanguageCode"),
        "fr",
        "got {translated}"
    );

    // A terminology nothing imported is a 404, on the translate path too.
    let (status, missing) = call(
        &router,
        "TranslateText",
        &json!({
            "Text": "hello world",
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
            "TerminologyNames": ["no-such-terms"],
        }),
    )
    .await;
    assert_eq!(status, 404, "TranslateText answered {missing}");
    assert_eq!(error_code(&missing), "ResourceNotFoundException");
    assert_eq!(
        message(&missing),
        format!("The Terminology for {ACCOUNT} no-such-terms was not found"),
        "the refusal names the terminology the request asked for, got {missing}"
    );

    // Deleted: every reader agrees it is gone.
    let (status, deleted) =
        call(&router, "DeleteTerminology", &json!({"Name": "demo-terms"})).await;
    assert_eq!(status, 200, "DeleteTerminology answered {deleted}");

    let (status, gone) = call(&router, "GetTerminology", &json!({"Name": "demo-terms"})).await;
    assert_eq!(status, 404, "GetTerminology answered {gone}");
    assert_eq!(error_code(&gone), "ResourceNotFoundException");

    let (_, listed) = call(&router, "ListTerminologies", &json!({})).await;
    assert_eq!(
        listed.pointer("/TerminologyPropertiesList"),
        Some(&json!([])),
        "a deleted terminology is no longer listed, got {listed}"
    );

    let (status, translated) = call(
        &router,
        "TranslateText",
        &json!({
            "Text": "hello world",
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
            "TerminologyNames": ["demo-terms"],
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a translation naming a deleted terminology 404s, got {translated}"
    );

    let (status, again) =
        call(&router, "DeleteTerminology", &json!({"Name": "demo-terms"})).await;
    assert_eq!(status, 404, "a second delete answered {again}");
    assert_eq!(error_code(&again), "ResourceNotFoundException");
}

/// Every input format the model declares, imported and read back. The rows
/// differ in the format, the delimiter, the number of language columns and the
/// directionality — not merely in the resource's name.
#[tokio::test]
async fn every_declared_format_is_parsed_into_stored_terms() {
    struct Row {
        case: &'static str,
        name: &'static str,
        file: &'static [u8],
        format: &'static str,
        directionality: Option<&'static str>,
        source: &'static str,
        targets: Value,
        term_count: i64,
    }

    let rows = [
        Row {
            case: "a two-column CSV",
            name: "csv-terms",
            file: EN_FR_CSV,
            format: "CSV",
            directionality: Some("UNI"),
            source: "en",
            targets: json!(["fr"]),
            term_count: 2,
        },
        Row {
            case: "a tab-separated file",
            name: "tsv-terms",
            file: EN_ES_TSV,
            format: "TSV",
            directionality: None,
            source: "en",
            targets: json!(["es"]),
            term_count: 1,
        },
        Row {
            case: "a TMX file whose segments carry entity references",
            name: "tmx-terms",
            file: EN_DE_TMX,
            format: "TMX",
            directionality: None,
            source: "en",
            targets: json!(["de"]),
            term_count: 2,
        },
        Row {
            case: "a three-column multi-directional CSV",
            name: "multi-terms",
            file: EN_FR_DE_CSV,
            format: "CSV",
            directionality: Some("MULTI"),
            source: "en",
            targets: json!(["fr", "de"]),
            term_count: 1,
        },
    ];

    let router = router();
    for row in &rows {
        let (status, imported) =
            import(&router, row.name, row.file, row.format, row.directionality).await;
        assert_eq!(
            status, 200,
            "case {:?}: ImportTerminology answered {imported}",
            row.case
        );
        let properties = "/TerminologyProperties";
        assert_eq!(
            number_at(&imported, &format!("{properties}/TermCount")),
            row.term_count,
            "case {:?}: expected {} parsed terms, got {imported}",
            row.case,
            row.term_count
        );
        assert_eq!(
            number_at(&imported, &format!("{properties}/SizeBytes")),
            i64::try_from(row.file.len()).expect("the file length fits"),
            "case {:?}: SizeBytes must be the uploaded byte length, got {imported}",
            row.case
        );
        assert_eq!(
            string_at(&imported, &format!("{properties}/Format")),
            row.format,
            "case {:?}: got {imported}",
            row.case
        );
        assert_eq!(
            string_at(&imported, &format!("{properties}/SourceLanguageCode")),
            row.source,
            "case {:?}: got {imported}",
            row.case
        );
        assert_eq!(
            imported.pointer(&format!("{properties}/TargetLanguageCodes")),
            Some(&row.targets),
            "case {:?}: got {imported}",
            row.case
        );
        // "When you create a custom terminology resource without specifying the
        // directionality, ... this parameter will have a null value."
        assert_eq!(
            imported
                .pointer(&format!("{properties}/Directionality"))
                .and_then(Value::as_str),
            row.directionality,
            "case {:?}: got {imported}",
            row.case
        );

        let (status, fetched) =
            call(&router, "GetTerminology", &json!({"Name": row.name})).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetTerminology answered {fetched}",
            row.case
        );
        assert_eq!(
            fetched.pointer(properties),
            imported.pointer(properties),
            "case {:?}: the read must return what the import stored, got {fetched}",
            row.case
        );
    }
}

/// A terminology applies only to the language pairs it covers, and a
/// multi-directional one covers every pair of its languages while a
/// uni-directional one covers only its own source language.
#[tokio::test]
async fn terminology_application_follows_the_resources_directionality() {
    struct Row {
        case: &'static str,
        terminology: &'static str,
        text: &'static str,
        source: &'static str,
        target: &'static str,
        expect_in_translation: Option<&'static str>,
        expect_applied: Option<Value>,
    }

    let rows = [
        Row {
            case: "a uni-directional resource on its own pair",
            terminology: "uni-terms",
            text: "book shelf",
            source: "en",
            target: "fr",
            expect_in_translation: Some("livre"),
            expect_applied: Some(json!([{
                "Name": "uni-terms",
                "Terms": [{"SourceText": "book", "TargetText": "livre"}],
            }])),
        },
        Row {
            case: "a uni-directional resource on the reverse pair",
            terminology: "uni-terms",
            text: "livre shelf",
            source: "fr",
            target: "en",
            expect_in_translation: Some("livre"),
            expect_applied: None,
        },
        Row {
            case: "a uni-directional resource on a pair of two of its targets",
            terminology: "uni-terms",
            text: "livre shelf",
            source: "fr",
            target: "de",
            expect_in_translation: Some("livre"),
            expect_applied: None,
        },
        Row {
            case: "a multi-directional resource on the reverse pair",
            terminology: "multi-terms",
            text: "livre shelf",
            source: "fr",
            target: "en",
            expect_in_translation: Some("book"),
            expect_applied: Some(json!([{
                "Name": "multi-terms",
                "Terms": [{"SourceText": "livre", "TargetText": "book"}],
            }])),
        },
        Row {
            case: "a multi-directional resource between two of its targets",
            terminology: "multi-terms",
            text: "livre shelf",
            source: "fr",
            target: "de",
            expect_in_translation: Some("Buch"),
            expect_applied: Some(json!([{
                "Name": "multi-terms",
                "Terms": [{"SourceText": "livre", "TargetText": "Buch"}],
            }])),
        },
        Row {
            case: "a term that only occurs inside a longer word",
            terminology: "multi-terms",
            text: "bookshelf and bookish",
            source: "en",
            target: "fr",
            expect_in_translation: Some("bookshelf and bookish"),
            expect_applied: None,
        },
        Row {
            case: "a phrase that is preferred over the word inside it",
            terminology: "phrase-terms",
            text: "the new book is here",
            source: "en",
            target: "fr",
            expect_in_translation: Some("le nouveau livre"),
            expect_applied: Some(json!([{
                "Name": "phrase-terms",
                "Terms": [{"SourceText": "new book", "TargetText": "le nouveau livre"}],
            }])),
        },
    ];

    let router = router();
    let (status, _) = import(&router, "uni-terms", EN_FR_DE_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200);
    let (status, _) = import(&router, "multi-terms", EN_FR_DE_CSV, "CSV", Some("MULTI")).await;
    assert_eq!(status, 200);
    let (status, _) = import(
        &router,
        "phrase-terms",
        b"en,fr\nbook,livre\nnew book,le nouveau livre\n",
        "CSV",
        Some("UNI"),
    )
    .await;
    assert_eq!(status, 200);

    for row in &rows {
        let (status, translated) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": row.text,
                "SourceLanguageCode": row.source,
                "TargetLanguageCode": row.target,
                "TerminologyNames": [row.terminology],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: TranslateText answered {translated}",
            row.case
        );
        if let Some(expected) = row.expect_in_translation {
            assert!(
                string_at(&translated, "/TranslatedText").contains(expected),
                "case {:?}: expected {expected:?} in the translation, got {translated}",
                row.case
            );
        }
        assert_eq!(
            translated.pointer("/AppliedTerminologies"),
            row.expect_applied.as_ref(),
            "case {:?}: got {translated}",
            row.case
        );
    }
}

/// Amazon Translate translates between the languages it supports, and never
/// from a language into itself. The rows span a pair neither the issue nor the
/// workflow cites, the same-language pair, and an unsupported code on each side.
#[tokio::test]
async fn unsupported_language_pairs_are_refused() {
    struct Row {
        case: &'static str,
        source: &'static str,
        target: &'static str,
        status: u16,
    }

    let rows = [
        Row {
            case: "a supported pair the workflow does not use",
            source: "de",
            target: "ja",
            status: 200,
        },
        Row {
            case: "a supported pair with a regional variant",
            source: "pt",
            target: "zh-TW",
            status: 200,
        },
        Row {
            case: "the same language on both sides",
            source: "en",
            target: "en",
            status: 400,
        },
        Row {
            case: "the same regional variant on both sides",
            source: "zh-TW",
            target: "zh-TW",
            status: 400,
        },
        Row {
            case: "a source language the service does not translate",
            source: "xx",
            target: "en",
            status: 400,
        },
        Row {
            case: "a target language the service does not translate",
            source: "en",
            target: "xx",
            status: 400,
        },
        Row {
            case: "auto as the target language",
            source: "en",
            target: "auto",
            status: 400,
        },
    ];

    let router = router();
    for row in &rows {
        let (status, body) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": "a sentence to translate",
                "SourceLanguageCode": row.source,
                "TargetLanguageCode": row.target,
            }),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body}",
            row.case, row.status
        );
        if row.status == 400 {
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguagePairException",
                "case {:?}: got {body}",
                row.case
            );
            // The pair travels in the error shape's own members, which is
            // where the model declares it.
            assert_eq!(
                string_at(&body, "/SourceLanguageCode"),
                row.source,
                "case {:?}: the refusal reports the source language, got {body}",
                row.case
            );
            assert_eq!(
                string_at(&body, "/TargetLanguageCode"),
                row.target,
                "case {:?}: the refusal reports the target language, got {body}",
                row.case
            );
        } else {
            assert_eq!(
                string_at(&body, "/TargetLanguageCode"),
                row.target,
                "case {:?}: got {body}",
                row.case
            );
            assert!(
                string_at(&body, "/TranslatedText").contains("a sentence to translate"),
                "case {:?}: got {body}",
                row.case
            );
        }
    }
}

/// `auto` is resolved from the text itself, so the reported
/// `SourceLanguageCode` is a concrete code the service translates — and the
/// same text always resolves the same way.
#[tokio::test]
async fn auto_resolves_the_source_language_from_the_text() {
    let rows = [
        (
            "an English paragraph",
            "The quick brown fox jumps over the lazy dog while the gardener waters \
             the roses in the early morning light.",
            "en",
        ),
        (
            "a French paragraph",
            "Le renard brun rapide saute par-dessus le chien paresseux pendant que \
             le jardinier arrose les roses au petit matin.",
            "fr",
        ),
        (
            "a German paragraph",
            "Der schnelle braune Fuchs springt über den faulen Hund, während der \
             Gärtner am frühen Morgen die Rosen gießt.",
            "de",
        ),
    ];

    let router = router();
    for (case, text, expected) in rows {
        let (status, body) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": text,
                "SourceLanguageCode": "auto",
                "TargetLanguageCode": "it",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: TranslateText answered {body}");
        assert_eq!(
            string_at(&body, "/SourceLanguageCode"),
            expected,
            "case {case:?}: expected the detected source language {expected:?}, got {body}"
        );
        let (_, repeated) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": text,
                "SourceLanguageCode": "auto",
                "TargetLanguageCode": "it",
            }),
        )
        .await;
        assert_eq!(
            repeated.pointer("/SourceLanguageCode"),
            body.pointer("/SourceLanguageCode"),
            "case {case:?}: the same text must resolve the same way, got {repeated}"
        );
    }
}

/// The settings a request names are applied to the translation, and
/// `AppliedSettings` reports them — a different `Settings` produces a different
/// `TranslatedText`, and `Brevity` (which "reduces the length of the
/// translation output") produces a shorter one.
#[tokio::test]
async fn applied_settings_change_the_translation_they_are_reported_on() {
    let router = router();
    let text = "a sentence to translate";

    let mut rendered: Vec<(&str, String)> = Vec::new();
    for (case, settings) in [
        ("no settings", Value::Null),
        ("formal", json!({"Formality": "FORMAL"})),
        ("informal", json!({"Formality": "INFORMAL"})),
        ("masked", json!({"Profanity": "MASK"})),
        ("brief", json!({"Brevity": "ON"})),
    ] {
        let mut body = json!({
            "Text": text,
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
        });
        if !settings.is_null() {
            body["Settings"] = settings.clone();
        }
        let (status, answered) = call(&router, "TranslateText", &body).await;
        assert_eq!(status, 200, "case {case:?}: TranslateText answered {answered}");
        assert_eq!(
            answered.get("AppliedSettings"),
            if settings.is_null() {
                None
            } else {
                Some(&settings)
            },
            "case {case:?}: AppliedSettings must report the settings applied, got {answered}"
        );
        rendered.push((case, string_at(&answered, "/TranslatedText")));
    }

    let distinct: BTreeSet<&String> = rendered.iter().map(|(_, text)| text).collect();
    assert_eq!(
        distinct.len(),
        rendered.len(),
        "each setting must change the translation it is reported on, got {rendered:?}"
    );
    let baseline = rendered
        .iter()
        .find(|(case, _)| *case == "no settings")
        .map(|(_, text)| text.len())
        .unwrap_or_default();
    let brief = rendered
        .iter()
        .find(|(case, _)| *case == "brief")
        .map(|(_, text)| text.len())
        .unwrap_or_default();
    assert!(
        brief < baseline,
        "brevity must shorten the output: expected fewer than {baseline} bytes, got {brief}"
    );
}

/// The listing pages in name order and resumes exactly where a page ended,
/// across page sizes; a cursor it never issued is refused.
#[tokio::test]
async fn list_terminologies_pages_over_every_stored_terminology() {
    let router = router();
    let names = ["alpha-terms", "beta-terms", "delta-terms", "gamma-terms", "omega-terms"];
    for name in names {
        let (status, body) = import(&router, name, EN_FR_CSV, "CSV", Some("UNI")).await;
        assert_eq!(status, 200, "ImportTerminology answered {body}");
    }

    for page_size in [1_i64, 2, 5, 500] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListTerminologies", &request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListTerminologies answered {body}"
            );
            let page = body
                .pointer("/TerminologyPropertiesList")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                i64::try_from(page.len()).expect("the page length fits") <= page_size,
                "page size {page_size}: a page held {} entries, got {body}",
                page.len()
            );
            for entry in &page {
                seen.push(string_at(entry, "/Name"));
            }
            token = body.get("NextToken").and_then(Value::as_str).map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "page size {page_size}: the listing did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            seen, expected,
            "page size {page_size}: every terminology must be paged exactly once in name order"
        );
    }

    // A cursor no page handed out: not base64 at all, base64 of a bare name
    // (no cursor marker), and base64 of another listing's cursor kind.
    let forged = [
        ("not base64 at all", "!!not-a-token!!".to_string()),
        ("base64 of a bare name", blob(b"alpha-terms")),
        ("base64 of another cursor kind", blob(b"parallel-data:alpha-terms")),
    ];
    for (case, token) in forged {
        let (status, body) = call(
            &router,
            "ListTerminologies",
            &json!({"NextToken": token, "MaxResults": 2}),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: ListTerminologies answered {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: got {body}"
        );
        assert!(
            message(&body).contains("NextToken"),
            "case {case:?}: the refusal names the parameter, got {body}"
        );
    }
}

/// Re-importing a name under the only modelled merge strategy replaces that
/// name's terms rather than creating a second resource: the created instant
/// stays, the last-updated instant moves on, and the ARN's revision follows the
/// new file's content.
#[tokio::test]
async fn re_import_overwrites_the_stored_terminology() {
    let router = router();
    let (status, first) = import(&router, "demo-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {first}");

    // The clock has to move for `LastUpdatedAt` to be observably later.
    sleep(Duration::from_millis(10));

    let replacement = b"en,fr\nhello,salut\n".as_slice();
    let (status, second) = import(&router, "demo-terms", replacement, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "the re-import answered {second}");

    let properties = "/TerminologyProperties";
    assert_eq!(
        number_at(&second, &format!("{properties}/TermCount")),
        1,
        "the re-import must report the replacement file's terms, got {second}"
    );
    assert_eq!(
        seconds_at(&second, &format!("{properties}/CreatedAt")),
        seconds_at(&first, &format!("{properties}/CreatedAt")),
        "an overwrite keeps the instant the resource was created, got {second}"
    );
    assert!(
        seconds_at(&second, &format!("{properties}/LastUpdatedAt"))
            > seconds_at(&first, &format!("{properties}/LastUpdatedAt")),
        "an overwrite moves LastUpdatedAt on, got {second}"
    );
    assert_eq!(
        string_at(&second, &format!("{properties}/Arn")),
        expected_arn("demo-terms", replacement),
        "the ARN's revision follows the imported content, got {second}"
    );

    let (_, listed) = call(&router, "ListTerminologies", &json!({})).await;
    assert_eq!(
        listed
            .pointer("/TerminologyPropertiesList")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "an overwrite must not create a second resource, got {listed}"
    );

    // The replaced terms are the ones a translation now applies.
    let (_, translated) = call(
        &router,
        "TranslateText",
        &json!({
            "Text": "hello world",
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
            "TerminologyNames": ["demo-terms"],
        }),
    )
    .await;
    assert!(
        string_at(&translated, "/TranslatedText").contains("salut"),
        "the overwritten term must be the one applied, got {translated}"
    );
    assert!(
        !string_at(&translated, "/TranslatedText").contains("bonjour"),
        "the replaced term must no longer be applied, got {translated}"
    );
}

/// A file the service cannot read as the declared format is refused with the
/// modelled parameter error; a file whose rows it can only partly use is
/// accepted, with the rows it skipped counted and an auxiliary location to
/// report them.
#[tokio::test]
async fn unusable_terminology_files_are_refused_and_partial_ones_counted() {
    let router = router();

    let refused: [(&str, &[u8], &str); 6] = [
        ("an empty file", b"", "CSV"),
        ("a CSV with one language column", b"en\nhello\n", "CSV"),
        (
            "a CSV whose header is not language codes",
            b"source,target\nhello,bonjour\n",
            "CSV",
        ),
        (
            "a CSV declaring one language twice",
            b"en,en\nhello,hi\n",
            "CSV",
        ),
        ("a CSV declared as TMX", EN_FR_CSV, "TMX"),
        (
            "a TMX whose units name one language",
            br#"<tmx version="1.4"><header srclang="en"/><body><tu><tuv xml:lang="en"><seg>hello</seg></tuv></tu></body></tmx>"#,
            "TMX",
        ),
    ];
    for (case, file, format) in refused {
        let (status, body) = import(&router, "refused-terms", file, format, None).await;
        assert_eq!(status, 400, "case {case:?}: ImportTerminology answered {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: got {body}"
        );
        assert!(
            message(&body).contains("TerminologyData.File"),
            "case {case:?}: the refusal names the parameter, got {body}"
        );
        let (status, fetched) =
            call(&router, "GetTerminology", &json!({"Name": "refused-terms"})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused import must store nothing, got {fetched}"
        );
    }

    let partial = b"en,fr\nhello,bonjour\n,\nonly-source,\n".as_slice();
    let (status, imported) = import(&router, "partial-terms", partial, "CSV", None).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    assert_eq!(
        number_at(&imported, "/TerminologyProperties/TermCount"),
        1,
        "only the usable row is a term, got {imported}"
    );
    assert_eq!(
        number_at(&imported, "/TerminologyProperties/SkippedTermCount"),
        2,
        "the rows the service could not use are counted, got {imported}"
    );
    assert_eq!(
        string_at(&imported, "/AuxiliaryDataLocation/RepositoryType"),
        "S3",
        "a file with skipped rows has warnings to report, got {imported}"
    );
}

/// `GetTerminology`'s format rule: with no format the stored one is reported,
/// and a multi-directional resource may only be retrieved in the format it was
/// imported as.
#[tokio::test]
async fn get_terminology_honours_the_requested_data_format() {
    let router = router();
    let (status, _) = import(&router, "uni-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200);
    let (status, _) = import(&router, "multi-terms", EN_FR_DE_CSV, "CSV", Some("MULTI")).await;
    assert_eq!(status, 200);

    let rows: [(&str, &str, Option<&str>, u16); 5] = [
        ("a uni-directional resource, no format named", "uni-terms", None, 200),
        ("a uni-directional resource in its own format", "uni-terms", Some("CSV"), 200),
        ("a uni-directional resource in another format", "uni-terms", Some("TSV"), 200),
        ("a multi-directional resource in its own format", "multi-terms", Some("CSV"), 200),
        ("a multi-directional resource in another format", "multi-terms", Some("TMX"), 400),
    ];

    for (case, name, format, expected) in rows {
        let mut request = json!({"Name": name});
        if let Some(format) = format {
            request["TerminologyDataFormat"] = json!(format);
        }
        let (status, body) = call(&router, "GetTerminology", &request).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 200 {
            let location = string_at(&body, "/TerminologyDataLocation/Location");
            let extension = format.unwrap_or("CSV").to_ascii_lowercase();
            assert!(
                location.contains(&format!(".{extension}?")),
                "case {case:?}: the location must name the requested format, got {location:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: got {body}"
            );
        }
    }
}

/// A translation request may name one custom terminology resource, so a second
/// one is refused with the modelled request error.
#[tokio::test]
async fn a_translation_names_at_most_one_terminology() {
    let router = router();
    let (status, _) = import(&router, "first-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200);
    let (status, _) = import(&router, "second-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200);

    let (status, body) = call(
        &router,
        "TranslateText",
        &json!({
            "Text": "hello world",
            "SourceLanguageCode": "en",
            "TargetLanguageCode": "fr",
            "TerminologyNames": ["first-terms", "second-terms"],
        }),
    )
    .await;
    assert_eq!(status, 400, "TranslateText answered {body}");
    assert_eq!(error_code(&body), "InvalidRequestException", "got {body}");
}

/// The store only shrinks on a caller's delete, so the account's terminology
/// quota is enforced before a new name is stored — while an overwrite of a name
/// the store already holds, which adds no entry, still succeeds at the quota.
#[tokio::test]
async fn the_terminology_store_is_bounded_by_the_account_quota() {
    /// `state::MAX_STORED_TERMINOLOGIES`, the documented per-account quota.
    const MAX_STORED_TERMINOLOGIES: usize = 100;

    let router = router();
    for index in 0..MAX_STORED_TERMINOLOGIES {
        let name = format!("terms-{index:03}");
        let (status, body) = import(&router, &name, EN_FR_CSV, "CSV", Some("UNI")).await;
        assert_eq!(status, 200, "importing {name}: ImportTerminology answered {body}");
    }

    let (status, body) = import(&router, "terms-over", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 400, "the import past the quota answered {body}");
    assert_eq!(error_code(&body), "LimitExceededException", "got {body}");
    let (status, fetched) =
        call(&router, "GetTerminology", &json!({"Name": "terms-over"})).await;
    assert_eq!(
        status, 404,
        "a refused import must store nothing, got {fetched}"
    );

    // An overwrite stores no additional entry, so the quota does not refuse it.
    let (status, body) = import(
        &router,
        "terms-000",
        b"en,fr\nhello,salut\n",
        "CSV",
        Some("UNI"),
    )
    .await;
    assert_eq!(status, 200, "the overwrite at the quota answered {body}");
    assert_eq!(
        number_at(&body, "/TerminologyProperties/TermCount"),
        1,
        "the overwrite must report the replacement file's terms, got {body}"
    );
}

/// The terminology substitution EMITS the stored target text on every match, so
/// it is bounded by what the response's own `TranslatedText` can carry: a row
/// whose cell is longer than a term may carry never enters the store, and a
/// translation whose substitutions would outgrow the output is refused rather
/// than truncated or built.
#[tokio::test]
async fn the_translation_is_bounded_by_the_modelled_output_length() {
    /// `Term$SourceText` / `Term$TargetText`, `length 0..10000`.
    const MAX_TERM_TEXT_CHARS: usize = 10_000;
    /// `TranslatedTextString`, `length 0..20000`.
    const MAX_TRANSLATED_TEXT_CHARS: usize = 20_000;

    let router = router();

    // A cell longer than a term may carry takes its row out of the terminology.
    let oversized = format!(
        "en,fr\nhello,bonjour\nbig,{}\n",
        "x".repeat(MAX_TERM_TEXT_CHARS + 1)
    );
    let (status, imported) =
        import(&router, "capped-terms", oversized.as_bytes(), "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    assert_eq!(
        number_at(&imported, "/TerminologyProperties/TermCount"),
        1,
        "the over-long row is not a term, got {imported}"
    );
    assert_eq!(
        number_at(&imported, "/TerminologyProperties/SkippedTermCount"),
        1,
        "the over-long row is counted as skipped, got {imported}"
    );

    // A long-but-storable term, repeated across the text: under the output
    // ceiling it translates, past it the request is refused.
    let long_target = "y".repeat(5_000);
    let (status, body) = import(
        &router,
        "long-terms",
        format!("en,fr\na,{long_target}\n").as_bytes(),
        "CSV",
        Some("UNI"),
    )
    .await;
    assert_eq!(status, 200, "ImportTerminology answered {body}");

    for (case, text, expected) in [
        ("three matches, within the output ceiling", "a a a", 200_u16),
        ("five matches, past the output ceiling", "a a a a a", 400),
    ] {
        let (status, body) = call(
            &router,
            "TranslateText",
            &json!({
                "Text": text,
                "SourceLanguageCode": "en",
                "TargetLanguageCode": "fr",
                "TerminologyNames": ["long-terms"],
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 200 {
            let translated = string_at(&body, "/TranslatedText");
            assert!(
                translated.chars().count() <= MAX_TRANSLATED_TEXT_CHARS,
                "case {case:?}: the translation must fit the modelled output length, got {} characters",
                translated.chars().count()
            );
            assert!(
                translated.contains(&long_target),
                "case {case:?}: the stored term must be substituted, got {} characters",
                translated.chars().count()
            );
        } else {
            assert_eq!(
                error_code(&body),
                "TextSizeLimitExceededException",
                "case {case:?}: got {body}"
            );
        }
    }
}

/// A terminology carrying more tags than one resource may hold is refused, and
/// the tags a request does state are stored on the resource.
#[tokio::test]
async fn tags_are_stored_with_the_terminology_and_bounded() {
    let router = router();

    let mut body = import_body("tagged-terms", EN_FR_CSV, "CSV", Some("UNI"));
    body["Tags"] = Value::Array(
        (0..51)
            .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
            .collect(),
    );
    let (status, answered) = call(&router, "ImportTerminology", &body).await;
    assert_eq!(status, 400, "ImportTerminology answered {answered}");
    assert_eq!(
        error_code(&answered),
        "TooManyTagsException",
        "got {answered}"
    );

    body["Tags"] = json!([{"Key": "owner", "Value": "platform"}]);
    let (status, answered) = call(&router, "ImportTerminology", &body).await;
    assert_eq!(status, 200, "ImportTerminology answered {answered}");

    // The tagged resource is the one a read reports, and an overwrite that
    // states no tags keeps the ones it already carried.
    let (status, reimported) = import(&router, "tagged-terms", EN_ES_TSV, "TSV", None).await;
    assert_eq!(status, 200, "the re-import answered {reimported}");
    let (status, fetched) =
        call(&router, "GetTerminology", &json!({"Name": "tagged-terms"})).await;
    assert_eq!(status, 200, "GetTerminology answered {fetched}");
    assert_eq!(
        string_at(&fetched, "/TerminologyProperties/Format"),
        "TSV",
        "the overwrite's format is the stored one, got {fetched}"
    );
}
