// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InferICD10CM`: the synchronous ontology linking that places
//! medical-condition mentions in clinical text and links each one to the ICD-10-CM
//! codes it could refer to.
//!
//! The rule under test is that the answer is computed from the SUBMITTED TEXT: every
//! reported entity is a stretch of that text, its offsets delimit exactly the characters
//! it reports, the context around it decides its traits and attributes, and a text that
//! names no condition reports no entity. Each table below therefore spans a class of
//! input — several conditions, several written forms of one condition, both ends of the
//! accepted length, each context cue, each attribute type — rather than one document.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

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

/// Infer against `text` and return the whole answer, asserting the call succeeded.
async fn infer(router: &MultiServiceRouter, text: &str) -> Value {
    let (status, answer) = call(router, "InferICD10CM", &json!({"Text": text})).await;
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

/// The characters `begin..end` of `text`, which is what a reported offset pair
/// delimits: the modelled `BeginOffset`/`EndOffset` are 0-based CHARACTER offsets.
fn by_characters(text: &str, begin: usize, end: usize) -> String {
    text.chars().skip(begin).take(end.saturating_sub(begin)).collect()
}

/// Every offset pair in `answer` delimits exactly the `Text` it is reported with, on
/// entities and on their attributes alike.
fn assert_offsets_delimit_their_text(case: &str, text: &str, answer: &Value) {
    for entity in entities(answer) {
        let mut spans = vec![entity.clone()];
        spans.extend(
            entity["Attributes"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        for span in spans {
            let reported = span["Text"].as_str().unwrap_or_default();
            let begin = span["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = span["EndOffset"].as_u64().unwrap_or_default() as usize;
            let written = by_characters(text, begin, end);
            assert_eq!(
                written, reported,
                "case {case:?}: the offsets {begin}..{end} must delimit the reported text, \
                 got {written:?} for {reported:?} in {span}"
            );
        }
    }
}

/// Whether `code` is written the way an ICD-10-CM code is: a letter, two digits, and
/// optionally a dotted extension. Source: the structure of the ICD-10-CM tabular list
/// (a category of one letter and two characters, optionally extended after a period).
fn is_icd10cm_code(code: &str) -> bool {
    let characters: Vec<char> = code.chars().collect();
    if characters.len() < 3 {
        return false;
    }
    if !characters[0].is_ascii_uppercase()
        || !characters[1].is_ascii_digit()
        || !characters[2].is_ascii_alphanumeric()
    {
        return false;
    }
    match characters.len() {
        3 => true,
        _ => {
            characters[3] == '.'
                && characters.len() > 4
                && characters[4..].iter().all(|c| c.is_ascii_alphanumeric())
        }
    }
}

/// The names of an entity's or attribute's traits.
fn trait_names(span: &Value) -> Vec<String> {
    span["Traits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A medical-condition mention is reported as the stretch of text that names it, typed
/// `DX_NAME` in the `MEDICAL_CONDITION` category, and linked to the ICD-10-CM concepts
/// that mention could refer to — most likely first.
///
/// The rows span several body systems, several written forms of one condition (full
/// name, short name, abbreviation), and both a diagnosis name and a symptom name, so a
/// handler that answered for one document rather than computing from the text fails
/// here.
#[tokio::test]
async fn a_condition_mention_is_linked_to_its_icd10cm_concepts() {
    let router = router();
    // (text, the mention as written, the code a coder reaches first, the trait a bare
    // mention licenses)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "E11.9",
            "DIAGNOSIS",
        ),
        (
            "The patient was diagnosed with diabetes.",
            "diabetes",
            "E11.9",
            "DIAGNOSIS",
        ),
        ("History of T2DM.", "T2DM", "E11.9", "DIAGNOSIS"),
        (
            "Patient has type 1 diabetes.",
            "diabetes",
            "E10.9",
            "DIAGNOSIS",
        ),
        ("Known hypertension.", "hypertension", "I10", "DIAGNOSIS"),
        ("He has COPD.", "COPD", "J44.9", "DIAGNOSIS"),
        (
            "Treated for a urinary tract infection.",
            "urinary tract infection",
            "N39.0",
            "DIAGNOSIS",
        ),
        ("Patient reports fever.", "fever", "R50.9", "SYMPTOM"),
        ("Cough noted.", "Cough", "R05.9", "SYMPTOM"),
        (
            "Pitting edema on examination.",
            "edema",
            "R60.9",
            "SIGN",
        ),
    ];

    for (text, mention, code, base_trait) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = found
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(mention))
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the mention {mention:?}, got {answer}")
            });
        assert_eq!(
            entity["Category"].as_str(),
            Some("MEDICAL_CONDITION"),
            "case {text:?}: a condition is in the MEDICAL_CONDITION category, got {entity}"
        );
        assert_eq!(
            entity["Type"].as_str(),
            Some("DX_NAME"),
            "case {text:?}: a condition mention is typed DX_NAME, got {entity}"
        );
        let score = entity["Score"].as_f64().unwrap_or_default();
        assert!(
            score > 0.0 && score <= 1.0,
            "case {text:?}: the confidence must be in (0, 1], got {score}"
        );
        let concepts = entity["ICD10CMConcepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            concepts.first().and_then(|concept| concept["Code"].as_str()),
            Some(code),
            "case {text:?}: expected {code} first, got {entity}"
        );
        assert!(
            trait_names(entity).contains(&base_trait.to_string()),
            "case {text:?}: expected the trait {base_trait}, got {:?}",
            trait_names(entity)
        );
        assert_offsets_delimit_their_text(text, text, &answer);
    }
}

/// Every candidate an entity reports is a well-formed ICD-10-CM code with its long
/// description, the list is ordered by descending likelihood, and it is bounded.
#[tokio::test]
async fn concept_candidates_are_well_formed_ranked_and_bounded() {
    let router = router();
    let cases = [
        "Patient has diabetes, hypertension and asthma.",
        "Reports chest pain and shortness of breath.",
        "Assessment: congestive heart failure with chronic kidney disease.",
        "Patient has a migraine and reports nausea.",
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        assert!(
            !found.is_empty(),
            "case {text:?}: the text names conditions, got {answer}"
        );
        for entity in found {
            let concepts = entity["ICD10CMConcepts"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !concepts.is_empty() && concepts.len() <= 5,
                "case {text:?}: expected between 1 and 5 candidates, got {}",
                concepts.len()
            );
            let mut previous = f64::INFINITY;
            for concept in &concepts {
                let code = concept["Code"].as_str().unwrap_or_default();
                assert!(
                    is_icd10cm_code(code),
                    "case {text:?}: {code:?} is not written as an ICD-10-CM code"
                );
                assert!(
                    concept["Description"]
                        .as_str()
                        .is_some_and(|description| !description.is_empty()),
                    "case {text:?}: every candidate carries its long description, got {concept}"
                );
                let score = concept["Score"].as_f64().unwrap_or_default();
                assert!(
                    score > 0.0 && score < previous,
                    "case {text:?}: candidate scores must descend, got {score} after {previous}"
                );
                previous = score;
            }
        }
    }
}

/// A longer written form of the same concept is a more specific mention of it, so it is
/// reported with the higher confidence.
#[tokio::test]
async fn a_more_specific_written_form_is_reported_with_more_confidence() {
    let router = router();
    let pairs = [
        ("Patient has diabetes.", "Patient has type 2 diabetes."),
        (
            "Patient has type 2 diabetes.",
            "Patient has type 2 diabetes mellitus.",
        ),
    ];
    for (shorter, longer) in pairs {
        let short_score = entities(&infer(&router, shorter).await)[0]["Score"]
            .as_f64()
            .unwrap_or_default();
        let long_score = entities(&infer(&router, longer).await)[0]["Score"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            long_score > short_score,
            "case {longer:?} against {shorter:?}: the more specific mention must score \
             higher, got {long_score} against {short_score}"
        );
    }
}

/// Offsets are CHARACTER offsets into the submitted text, not byte offsets: a text
/// carrying multi-byte characters before the mention still reports offsets that
/// delimit it.
#[tokio::test]
async fn offsets_are_character_offsets_into_the_submitted_text() {
    let router = router();
    let cases = [
        "Patient reports fever.",
        "Patiënt — naïve to treatment — reports fever.",
        "\u{1f321} 39°C: fever.",
        "Notes:\n  severe left chest pain\n  chronic cough",
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        assert!(
            !entities(&answer).is_empty(),
            "case {text:?}: the text names a condition, got {answer}"
        );
        assert_offsets_delimit_their_text(text, text, &answer);
    }
}

/// The words around a mention license its contextual traits, and a cue never reaches
/// past the clause it is written in.
#[tokio::test]
async fn context_cues_license_the_traits_they_scope() {
    let router = router();
    // (text, the mention, the trait the context licenses)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("Patient denies chest pain.", "chest pain", "NEGATION"),
        ("No fever.", "fever", "NEGATION"),
        ("Negative for pneumonia.", "pneumonia", "NEGATION"),
        ("Family history of diabetes.", "diabetes", "PERTAINS_TO_FAMILY"),
        ("Mother had a stroke.", "stroke", "PERTAINS_TO_FAMILY"),
        ("At risk of diabetes.", "diabetes", "HYPOTHETICAL"),
        ("Possible pneumonia.", "pneumonia", "LOW_CONFIDENCE"),
        ("Suspected influenza.", "influenza", "LOW_CONFIDENCE"),
    ];
    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = found
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(mention))
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the mention {mention:?}, got {answer}")
            });
        assert!(
            trait_names(entity).contains(&expected.to_string()),
            "case {text:?}: expected the trait {expected}, got {:?}",
            trait_names(entity)
        );
    }

    // A mention with nothing hedging it carries only the trait its own written form
    // licenses, and a cue in one clause does not reach the next.
    for (text, mention) in [
        ("Patient has pneumonia.", "pneumonia"),
        ("No fever. Patient has pneumonia.", "pneumonia"),
    ] {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = found
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(mention))
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the mention {mention:?}, got {answer}")
            });
        assert_eq!(
            trait_names(entity),
            vec!["DIAGNOSIS".to_string()],
            "case {text:?}: nothing in this clause hedges the mention, got {entity}"
        );
    }
}

/// One attribute row: the modelled attribute type and the modifier as it is written.
type ExpectedAttribute = (&'static str, &'static str);

/// One case of the attribute table: the text, the mention, and the attributes the
/// mention must carry.
type AttributeCase = (&'static str, &'static str, Vec<ExpectedAttribute>);

/// A modifier standing beside a mention is reported as an attribute of it, with the
/// attribute type its vocabulary carries and the relationship that type has to a
/// condition.
#[tokio::test]
async fn a_modifier_beside_a_mention_is_reported_as_its_attribute() {
    let router = router();
    // (text, the mention, the attribute type -> the modifier as written)
    let cases: Vec<AttributeCase> = vec![
        (
            "Severe left chest pain.",
            "chest pain",
            vec![("QUALITY", "Severe"), ("DIRECTION", "left")],
        ),
        (
            "Chronic cough for three weeks.",
            "cough",
            vec![("ACUITY", "Chronic"), ("TIME_TO_DX_NAME", "three weeks")],
        ),
        (
            "Left lower lung pneumonia.",
            "pneumonia",
            vec![
                ("DIRECTION", "Left"),
                ("DIRECTION", "lower"),
                ("SYSTEM_ORGAN_SITE", "lung"),
            ],
        ),
        (
            "Two seizure episodes yesterday.",
            "seizure",
            vec![("QUANTITY", "Two"), ("TIME_TO_DX_NAME", "yesterday")],
        ),
    ];

    // How the modelled relationship type reports each attribute type.
    let relationship = BTreeMap::from([
        ("QUALITY", "QUALITY"),
        ("SYSTEM_ORGAN_SITE", "SYSTEM_ORGAN_SITE"),
        ("DIRECTION", "OVERLAP"),
        ("ACUITY", "OVERLAP"),
        ("QUANTITY", "OVERLAP"),
        ("TIME_TO_DX_NAME", "OVERLAP"),
    ]);

    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = found
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(mention))
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the mention {mention:?}, got {answer}")
            });
        let attributes = entity["Attributes"].as_array().cloned().unwrap_or_default();
        for (attribute_type, written) in expected {
            let attribute = attributes
                .iter()
                .find(|attribute| {
                    attribute["Type"].as_str() == Some(attribute_type)
                        && attribute["Text"].as_str() == Some(written)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {text:?}: expected a {attribute_type} attribute {written:?}, \
                         got {entity}"
                    )
                });
            assert_eq!(
                attribute["RelationshipType"].as_str(),
                relationship.get(attribute_type).copied(),
                "case {text:?}: wrong relationship for {attribute_type}, got {attribute}"
            );
            for member in ["Score", "RelationshipScore"] {
                let value = attribute[member].as_f64().unwrap_or_default();
                assert!(
                    value > 0.0 && value <= 1.0,
                    "case {text:?}: {member} must be in (0, 1], got {value} in {attribute}"
                );
            }
        }
        assert_offsets_delimit_their_text(text, text, &answer);
    }
}

/// A text that names no medical condition reports no entity — and the response still
/// reports the model version, in the format the model documents.
#[tokio::test]
async fn a_text_with_no_condition_reports_no_entity() {
    let router = router();
    let cases = [
        "The weather is fine today.",
        "Patient is 44 years old and lives alone.",
        "Follow up in two weeks.",
        "a",
        &"a".repeat(10_000),
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        assert_eq!(
            entities(&answer),
            Vec::<Value>::new(),
            "case {:?}: nothing in this text names a condition, got {answer}",
            &text[..text.len().min(40)]
        );
        let version = answer["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3 && parts.iter().all(|part| {
                !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit())
            }),
            "the model version must be written n.n.n, got {version:?}"
        );
        assert!(
            answer["PaginationToken"].is_null(),
            "nothing was truncated, so no page token is reported, got {answer}"
        );
    }
}

/// Several mentions in one text are reported in the order they are written, and every
/// `Id` in the response — entities and attributes alike — is unique and increasing, as
/// the modelled member documents.
#[tokio::test]
async fn mentions_are_ordered_and_every_id_is_unique_and_increasing() {
    let router = router();
    let text = "Patient with chronic asthma reports severe chest pain and fever. \
                History of type 2 diabetes mellitus and hypertension.";
    let answer = infer(&router, text).await;
    let found = entities(&answer);
    assert!(
        found.len() >= 5,
        "the text names five conditions, got {} in {answer}",
        found.len()
    );

    let offsets: Vec<u64> = found
        .iter()
        .map(|entity| entity["BeginOffset"].as_u64().unwrap_or_default())
        .collect();
    let mut sorted = offsets.clone();
    sorted.sort_unstable();
    assert_eq!(
        offsets, sorted,
        "entities must be reported in the order they are written, got {offsets:?}"
    );

    // `Id` numbers the spans of the document in the order they are WRITTEN, entities and
    // their attributes sharing the one sequence, so the ids rise with the offsets rather
    // than with the order the entity list nests them in.
    let mut spans: Vec<(i64, i64)> = Vec::new();
    for entity in &found {
        spans.push((
            entity["BeginOffset"].as_i64().unwrap_or_default(),
            entity["Id"].as_i64().unwrap_or_default(),
        ));
        for attribute in entity["Attributes"].as_array().cloned().unwrap_or_default() {
            spans.push((
                attribute["BeginOffset"].as_i64().unwrap_or_default(),
                attribute["Id"].as_i64().unwrap_or_default(),
            ));
        }
    }
    spans.sort_unstable();
    let ids: Vec<i64> = spans.iter().map(|(_, id)| *id).collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every Id in one response is unique, got {ids:?}"
    );
    assert!(
        ids.first().is_some_and(|first| *first >= 1),
        "the first Id of one response is at least 1, got {ids:?}"
    );
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "the Ids of one response increase with the offsets of the spans they number, got \
         {spans:?}"
    );
    assert_offsets_delimit_their_text("several mentions", text, &answer);
}

/// The inference reads nothing and writes nothing: the same text always gets the same
/// answer, whatever was inferred in between.
#[tokio::test]
async fn the_same_text_always_gets_the_same_answer() {
    let router = router();
    let text = "Patient denies chest pain but reports chronic cough for three weeks.";
    let first = infer(&router, text).await;
    let _ = infer(&router, "Known hypertension and type 2 diabetes mellitus.").await;
    let second = infer(&router, text).await;
    assert_eq!(
        first, second,
        "case {text:?}: inference holds no state, so the answer must not change"
    );
}

/// A text outside the modelled length is refused with this service's own client error,
/// and the longest accepted text is still served.
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
            "10000 characters, the longest accepted text",
            format!("{} diabetes", "a".repeat(10_000 - " diabetes".len())),
            200,
            "",
        ),
        (
            "10001 characters, one past the limit",
            "a".repeat(10_001),
            400,
            "TextSizeLimitExceededException",
        ),
    ];
    for (name, text, expected_status, expected_code) in cases {
        let (status, answer) = call(&router, "InferICD10CM", &json!({"Text": text})).await;
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
