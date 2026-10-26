// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DetectPHI`: the synchronous pass that reports the protected
//! health information written in clinical text.
//!
//! The rule under test is that every reported entity is a stretch of the SUBMITTED
//! TEXT — placed either by the written form an identifier class takes, or by the words
//! standing around an otherwise ambiguous value — that its offsets delimit exactly, and
//! that a text carrying no identifier reports no entity. Each table below therefore
//! spans a CLASS of input: every identifier class the operation reports, several written
//! forms of the same class, and texts whose words merely resemble one.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The category every entity of this operation is in, as the modelled `EntityType`
/// enum spells it.
const CATEGORY_PHI: &str = "PROTECTED_HEALTH_INFORMATION";

/// A router carrying only this service, over its own engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call `operation` with `body` and return the HTTP status and the parsed JSON answer.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Detect against `text` and return the whole answer, asserting the call succeeded.
async fn detect(router: &MultiServiceRouter, text: &str) -> Value {
    let (status, answer) = call(router, "DetectPHI", &json!({"Text": text})).await;
    assert_eq!(
        status, 200,
        "case {text:?}: expected 200, got {status} {answer}"
    );
    answer
}

/// The entities of an answer.
fn entities(answer: &Value) -> Vec<Value> {
    answer["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("an answer must carry an Entities list, got {answer}"))
}

/// The `(Type, Text)` pair of every entity of an answer, as a set: which identifier
/// classes the pass placed, and which characters each one reports.
fn typed_texts(answer: &Value) -> BTreeSet<(String, String)> {
    entities(answer)
        .iter()
        .map(|entity| {
            (
                entity["Type"].as_str().unwrap_or_default().to_string(),
                entity["Text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The characters `begin..end` of `text`, which is what a reported offset pair
/// delimits: the modelled `BeginOffset`/`EndOffset` are 0-based CHARACTER offsets.
fn by_characters(text: &str, begin: usize, end: usize) -> String {
    text.chars()
        .skip(begin)
        .take(end.saturating_sub(begin))
        .collect()
}

/// Everything every answer of this operation must satisfy, whatever the text: each
/// entity is in the PHI category, carries one of the modelled identifier types, reports
/// a confidence in (0, 1], is delimited by its own offsets, and the entities are written
/// in order with unique, increasing ids.
fn assert_answer_is_well_formed(case: &str, text: &str, answer: &Value) {
    // The modelled `EntitySubType` members that name an identifier class of protected
    // health information, as the operation's documentation lists them.
    let types = [
        "NAME",
        "AGE",
        "DATE",
        "PHONE_OR_FAX",
        "EMAIL",
        "ID",
        "URL",
        "ADDRESS",
        "PROFESSION",
    ];
    let found = entities(answer);
    let mut previous_begin = 0u64;
    let mut previous_id: Option<i64> = None;
    for entity in &found {
        assert_eq!(
            entity["Category"].as_str(),
            Some(CATEGORY_PHI),
            "case {case:?}: every entity of this operation is PHI, got {entity}"
        );
        let entity_type = entity["Type"].as_str().unwrap_or_default();
        assert!(
            types.contains(&entity_type),
            "case {case:?}: {entity_type:?} is not an identifier class this operation \
             reports, got {entity}"
        );
        let score = entity["Score"].as_f64().unwrap_or_default();
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: the confidence must be in (0, 1], got {score} in {entity}"
        );
        let begin = entity["BeginOffset"].as_u64().unwrap_or_default();
        let end = entity["EndOffset"].as_u64().unwrap_or_default();
        let written = by_characters(text, begin as usize, end as usize);
        assert_eq!(
            Some(written.as_str()),
            entity["Text"].as_str(),
            "case {case:?}: the offsets {begin}..{end} must delimit the reported text, \
             got {written:?} in {entity}"
        );
        assert!(
            begin >= previous_begin,
            "case {case:?}: entities are reported in the order they are written, got \
             {begin} after {previous_begin}"
        );
        previous_begin = begin;
        let id = entity["Id"].as_i64().unwrap_or_default();
        assert!(
            previous_id.is_none_or(|earlier| earlier < id),
            "case {case:?}: the ids of one response increase monotonically, got {id} \
             after {previous_id:?}"
        );
        previous_id = Some(id);
    }
}

/// One row of the detection table: the submitted text and every entity it carries, as
/// `(Type, Text)` pairs.
type DetectionCase = (&'static str, Vec<(&'static str, &'static str)>);

/// Each identifier class is placed from the characters the text itself carries, and a
/// class written several ways is placed from each of them.
///
/// The rows span every identifier class the operation reports and several written forms
/// of the ambiguous ones, so a handler that answered for one document rather than
/// computing from the text fails here. The names include one no lexicon of this crate
/// lists (`Okonkwo`), placed because the title in front of it names a person.
#[tokio::test]
async fn every_identifier_class_is_placed_from_the_submitted_text() {
    let router = router();
    let cases: Vec<DetectionCase> = vec![
        (
            "Patient John Doe reports no pain.",
            vec![("NAME", "John Doe")],
        ),
        ("Mr. Okonkwo was seen today.", vec![("NAME", "Okonkwo")]),
        (
            "Dr. Alice Nguyen signed the note.",
            vec![("NAME", "Alice Nguyen")],
        ),
        ("Name: Jane Roe-Smith", vec![("NAME", "Jane Roe-Smith")]),
        ("The patient is a 45-year-old male.", vec![("AGE", "45")]),
        ("Patient is 7 months old.", vec![("AGE", "7")]),
        ("Age: 62, no complaints.", vec![("AGE", "62")]),
        ("A 38 y/o presents with fever.", vec![("AGE", "38")]),
        ("Admitted on 2021-03-15.", vec![("DATE", "2021-03-15")]),
        ("Follow up on 03/15/2021.", vec![("DATE", "03/15/2021")]),
        ("Seen on March 15, 2021.", vec![("DATE", "March 15, 2021")]),
        ("Seen on 15-Mar-2021.", vec![("DATE", "15-Mar-2021")]),
        ("Discharged in June 2020.", vec![("DATE", "June 2020")]),
        (
            "Call the clinic at (555) 123-4567.",
            vec![("PHONE_OR_FAX", "(555) 123-4567")],
        ),
        (
            "Fax the report to 555.123.4567 today.",
            vec![("PHONE_OR_FAX", "555.123.4567")],
        ),
        (
            "Results sent to jane.roe@example.com.",
            vec![("EMAIL", "jane.roe@example.com")],
        ),
        (
            "Portal: https://example.com/records/42",
            vec![("URL", "https://example.com/records/42")],
        ),
        ("MRN: 1234567 on file.", vec![("ID", "1234567")]),
        ("SSN 123-45-6789 verified.", vec![("ID", "123-45-6789")]),
        (
            "Lives at 123 Main Street, Springfield, IL 62704.",
            vec![("ADDRESS", "123 Main Street, Springfield, IL 62704")],
        ),
        ("He works as a truck driver.", vec![("PROFESSION", "truck driver")]),
        ("Patient is a teacher.", vec![("PROFESSION", "teacher")]),
        ("Occupation: welder.", vec![("PROFESSION", "welder")]),
    ];

    for (text, expected) in cases {
        let answer = detect(&router, text).await;
        let found = typed_texts(&answer);
        let wanted: BTreeSet<(String, String)> = expected
            .iter()
            .map(|(phi_type, written)| (phi_type.to_string(), written.to_string()))
            .collect();
        assert_eq!(
            found, wanted,
            "case {text:?}: expected {wanted:?}, got {answer}"
        );
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// A text that carries no protected health information reports no entity — and the
/// response still reports the model version, in the format the model documents.
///
/// The rows are the words a clinical note writes that RESEMBLE an identifier: a month
/// name that is also an ordinary word, a measurement written like a date, a number
/// with no age cue, an occupation mentioned as somebody else's, and a note's own
/// capitalized headings.
#[tokio::test]
async fn a_text_with_no_identifier_reports_no_entity() {
    let router = router();
    let cases = [
        "The patient reports mild headache and nausea.",
        "The patient may return in 2 weeks.",
        "Blood pressure was 120/80 and pulse 88.",
        "Patient takes 2 tablets daily.",
        "Seen by the nurse for a dressing change.",
        "Patient Education Materials were provided.",
        "No fever, cough, or chest pain.",
        "a",
        &"a".repeat(20_000),
    ];
    for text in cases {
        let answer = detect(&router, text).await;
        assert_eq!(
            entities(&answer),
            Vec::<Value>::new(),
            "case {:?}: nothing in this text identifies a person, got {answer}",
            text.chars().take(48).collect::<String>()
        );
        let version = answer["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3
                && parts.iter().all(|part| {
                    !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit())
                }),
            "the model version must be written X.X.X, got {version:?}"
        );
        assert!(
            answer["PaginationToken"].is_null(),
            "nothing was truncated, so no page token is reported, got {answer}"
        );
    }
}

/// A text carrying several identifiers reports each of them once, in writing order.
#[tokio::test]
async fn several_identifiers_in_one_text_are_each_reported_once() {
    let router = router();
    let text = "Patient John Doe, a 45-year-old teacher, was seen on 2021-03-15. \
                MRN: 4455667. Reach him at (555) 123-4567 or john.doe@example.com.";
    let answer = detect(&router, text).await;
    assert_answer_is_well_formed("several identifiers", text, &answer);
    let found = typed_texts(&answer);
    for expected in [
        ("NAME", "John Doe"),
        ("AGE", "45"),
        ("PROFESSION", "teacher"),
        ("DATE", "2021-03-15"),
        ("ID", "4455667"),
        ("PHONE_OR_FAX", "(555) 123-4567"),
        ("EMAIL", "john.doe@example.com"),
    ] {
        let wanted = (expected.0.to_string(), expected.1.to_string());
        assert!(
            found.contains(&wanted),
            "case {text:?}: expected {wanted:?}, got {found:?}"
        );
    }
    assert_eq!(
        entities(&answer).len(),
        found.len(),
        "case {text:?}: one stretch of text reports one entity, got {answer}"
    );
}

/// Offsets are CHARACTER offsets into the submitted text, not byte offsets: a text
/// carrying multi-byte characters before an identifier still reports offsets that
/// delimit it.
#[tokio::test]
async fn offsets_are_character_offsets_into_the_submitted_text() {
    let router = router();
    let cases = [
        "Patiënt — John Doe — is 45 years old.",
        "\u{1f321} 39°C: seen on 2021-03-15 by Dr. Nguyen.",
        "Notes:\n  MRN: 7788991\n  reach at jane.roe@example.com",
    ];
    for text in cases {
        let answer = detect(&router, text).await;
        assert!(
            !entities(&answer).is_empty(),
            "case {text:?}: the text carries an identifier, got {answer}"
        );
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// The pass reads nothing and writes nothing: the same text always gets the same
/// answer, whatever was analysed in between, so two callers never see each other's
/// results.
#[tokio::test]
async fn the_same_text_always_gets_the_same_answer() {
    let router = router();
    let text = "Patient John Doe, 45 years old, seen on 2021-03-15.";
    let first = detect(&router, text).await;
    let other = detect(&router, "Mrs. Okonkwo lives at 123 Main Street.").await;
    let second = detect(&router, text).await;
    assert_eq!(
        first, second,
        "case {text:?}: the pass holds no state, so the answer must not change"
    );
    assert_ne!(
        typed_texts(&first),
        typed_texts(&other),
        "two texts are analysed independently, got {other}"
    );
}

/// A text outside the modelled length is refused with this service's own client error,
/// and both ends of the accepted length are still served.
#[tokio::test]
async fn a_text_outside_the_modelled_length_is_refused() {
    let router = router();
    let cases: Vec<(&str, String, u16, &str)> = vec![
        ("empty text", String::new(), 400, "InvalidRequestException"),
        (
            "one character, the shortest accepted text",
            "a".to_string(),
            200,
            "",
        ),
        (
            "20000 characters, the longest accepted text",
            format!("{} John Doe", "a".repeat(20_000 - " John Doe".len())),
            200,
            "",
        ),
        (
            "20001 characters, one past the limit",
            "a".repeat(20_001),
            400,
            "TextSizeLimitExceededException",
        ),
    ];
    for (name, text, expected_status, expected_code) in cases {
        let (status, answer) = call(&router, "DetectPHI", &json!({"Text": text})).await;
        assert_eq!(
            status, expected_status,
            "case {name:?}: expected {expected_status}, got {status} {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer["__type"].as_str(),
                Some(expected_code),
                "case {name:?}: expected {expected_code}, got {answer}"
            );
        } else {
            assert!(
                answer["Entities"].is_array(),
                "case {name:?}: an accepted text is analysed, got {answer}"
            );
        }
    }
}
