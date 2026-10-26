// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InferRxNorm`: the synchronous ontology linking that places
//! medication mentions in clinical text and links each one to the RxNorm concepts it
//! could refer to.
//!
//! The rule under test is that the answer is computed from the SUBMITTED TEXT: every
//! reported entity is a stretch of that text, its offsets delimit exactly the characters
//! it reports, the prescription written around it decides its attributes, the context
//! around it decides its traits, and a text that names no medication reports no entity.
//! Each table below therefore spans a class of input — several medications, several
//! written forms of one medication, each attribute type, each context cue, both ends of
//! the accepted length — rather than one document.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The members of the modelled `RxNormEntityType` enum.
const ENTITY_TYPES: &[&str] = &["BRAND_NAME", "GENERIC_NAME"];

/// The members of the modelled `RxNormAttributeType` enum.
const ATTRIBUTE_TYPES: &[&str] = &[
    "DOSAGE",
    "DURATION",
    "FORM",
    "FREQUENCY",
    "RATE",
    "ROUTE_OR_MODE",
    "STRENGTH",
];

/// The members of the modelled `RxNormTraitName` enum.
const TRAIT_NAMES: &[&str] = &["NEGATION", "PAST_HISTORY"];

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
    let (status, answer) = call(router, "InferRxNorm", &json!({"Text": text})).await;
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

/// The entity of `answer` whose reported text is `mention`.
fn mention_entity(case: &str, answer: &Value, mention: &str) -> Value {
    entities(answer)
        .into_iter()
        .find(|entity| entity["Text"].as_str() == Some(mention))
        .unwrap_or_else(|| panic!("case {case:?}: expected the mention {mention:?}, got {answer}"))
}

/// The characters `begin..end` of `text`, which is what a reported offset pair
/// delimits: the modelled `BeginOffset`/`EndOffset` are 0-based CHARACTER offsets.
fn by_characters(text: &str, begin: usize, end: usize) -> String {
    text.chars()
        .skip(begin)
        .take(end.saturating_sub(begin))
        .collect()
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

/// Every offset pair in `answer` delimits exactly the `Text` it is reported with, every
/// reported value is a member of the enum the model declares for it, and every score is
/// a confidence — on entities and on their attributes alike.
fn assert_answer_is_well_formed(case: &str, text: &str, answer: &Value) {
    for entity in entities(answer) {
        assert_eq!(
            entity["Category"].as_str(),
            Some("MEDICATION"),
            "case {case:?}: a medication is in the MEDICATION category, got {entity}"
        );
        let entity_type = entity["Type"].as_str().unwrap_or_default();
        assert!(
            ENTITY_TYPES.contains(&entity_type),
            "case {case:?}: {entity_type:?} is not a modelled entity type, got {entity}"
        );
        let mut spans = vec![entity.clone()];
        let attributes = entity["Attributes"].as_array().cloned().unwrap_or_default();
        for attribute in &attributes {
            let attribute_type = attribute["Type"].as_str().unwrap_or_default();
            assert!(
                ATTRIBUTE_TYPES.contains(&attribute_type),
                "case {case:?}: {attribute_type:?} is not a modelled attribute type, \
                 got {attribute}"
            );
            let relationship = attribute["RelationshipScore"].as_f64().unwrap_or_default();
            assert!(
                relationship > 0.0 && relationship <= 1.0,
                "case {case:?}: RelationshipScore must be in (0, 1], got {relationship} \
                 in {attribute}"
            );
        }
        spans.extend(attributes);
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
            let score = span["Score"].as_f64().unwrap_or_default();
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: the confidence must be in (0, 1], got {score} in {span}"
            );
            for name in trait_names(&span) {
                assert!(
                    TRAIT_NAMES.contains(&name.as_str()),
                    "case {case:?}: {name:?} is not a modelled trait name, got {span}"
                );
            }
        }
    }
}

/// A medication mention is reported as the stretch of text that names it, in the
/// `MEDICATION` category, typed by whether that written form is the brand or the generic
/// name, and linked to the RxNorm concepts the mention could refer to — most likely
/// first.
///
/// The rows span several drug classes, several written forms of one medication (generic
/// name, salt form, abbreviation, brand name), a written form of several tokens, and both
/// modelled entity types, so a handler that answered for one document rather than
/// computing from the text fails here.
#[tokio::test]
async fn a_medication_mention_is_linked_to_its_rxnorm_concepts() {
    let router = router();
    // (text, the mention as written, the RxCUI the mention names first, its entity type)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "Patient takes metformin for diabetes.",
            "metformin",
            "6809",
            "GENERIC_NAME",
        ),
        (
            "Started on Metformin hydrochloride.",
            "Metformin hydrochloride",
            "6809",
            "GENERIC_NAME",
        ),
        (
            "Continue lisinopril.",
            "lisinopril",
            "29046",
            "GENERIC_NAME",
        ),
        ("Also on HCTZ.", "HCTZ", "5487", "GENERIC_NAME"),
        ("Takes ASA for prophylaxis.", "ASA", "1191", "GENERIC_NAME"),
        (
            "Patient injects insulin glargine at night.",
            "insulin glargine",
            "274783",
            "GENERIC_NAME",
        ),
        (
            "Ceftriaxone given in the ED.",
            "Ceftriaxone",
            "2193",
            "GENERIC_NAME",
        ),
        (
            "Takes Tylenol for the headache.",
            "Tylenol",
            "202433",
            "BRAND_NAME",
        ),
        ("On Lipitor since 2019.", "Lipitor", "153165", "BRAND_NAME"),
        ("Uses a Ventolin inhaler.", "Ventolin", "236719", "BRAND_NAME"),
    ];

    for (text, mention, code, entity_type) in cases {
        let answer = infer(&router, text).await;
        let entity = mention_entity(text, &answer, mention);
        assert_eq!(
            entity["Type"].as_str(),
            Some(entity_type),
            "case {text:?}: wrong entity type, got {entity}"
        );
        let concepts = entity["RxNormConcepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            concepts.first().and_then(|concept| concept["Code"].as_str()),
            Some(code),
            "case {text:?}: expected the RxCUI {code} first, got {entity}"
        );
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// Every candidate an entity reports is an RxNorm concept identifier with the name RxNorm
/// gives that concept, the list is ordered by descending likelihood, and it is bounded.
#[tokio::test]
async fn concept_candidates_are_well_formed_ranked_and_bounded() {
    let router = router();
    let cases = [
        "Meds: metformin, lisinopril, atorvastatin.",
        "Takes Coumadin and Lasix.",
        "Ozempic weekly and Januvia daily.",
        "Albuterol and prednisone for the exacerbation.",
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        assert!(
            !found.is_empty(),
            "case {text:?}: the text names medications, got {answer}"
        );
        for entity in found {
            let concepts = entity["RxNormConcepts"]
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
                    !code.is_empty() && code.chars().all(|digit| digit.is_ascii_digit()),
                    "case {text:?}: {code:?} is not written as an RxNorm concept identifier"
                );
                assert!(
                    concept["Description"]
                        .as_str()
                        .is_some_and(|description| !description.is_empty()),
                    "case {text:?}: every candidate carries its RxNorm name, got {concept}"
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

/// One attribute row: the modelled attribute type and the part of the prescription as it
/// is written.
type ExpectedAttribute = (&'static str, &'static str);

/// One case of the attribute table: the text, the mention, and the attributes the
/// mention must carry.
type AttributeCase = (&'static str, &'static str, Vec<ExpectedAttribute>);

/// The prescription written around a medication is reported as the attributes of it, each
/// typed by what that part of the prescription says: how much is in a dose, how much is
/// taken at once, in what form, by what route, how often, how fast and for how long.
///
/// The rows span every member of the modelled attribute-type enum and both the written-out
/// and the abbreviated way a prescription states each one.
#[tokio::test]
async fn the_prescription_around_a_medication_is_reported_as_its_attributes() {
    let router = router();
    // (text, the mention, the attribute type -> that part of the sig as written)
    let cases: Vec<AttributeCase> = vec![
        (
            "Metformin 500 mg twice daily.",
            "Metformin",
            vec![("STRENGTH", "500 mg"), ("FREQUENCY", "twice daily")],
        ),
        (
            "Metformin 500mg BID.",
            "Metformin",
            vec![("STRENGTH", "500mg"), ("FREQUENCY", "BID")],
        ),
        (
            "Take 1 tablet of Tylenol 500 mg by mouth every 8 hours for 10 days.",
            "Tylenol",
            vec![
                ("DOSAGE", "1 tablet"),
                ("STRENGTH", "500 mg"),
                ("ROUTE_OR_MODE", "by mouth"),
                ("FREQUENCY", "every 8 hours"),
                ("DURATION", "for 10 days"),
            ],
        ),
        (
            "Insulin glargine 20 units subcutaneously at bedtime.",
            "Insulin glargine",
            vec![
                ("DOSAGE", "20 units"),
                ("ROUTE_OR_MODE", "subcutaneously"),
                ("FREQUENCY", "at bedtime"),
            ],
        ),
        (
            "Albuterol inhaler 2 puffs q6h prn.",
            "Albuterol",
            vec![
                ("FORM", "inhaler"),
                ("DOSAGE", "2 puffs"),
                ("FREQUENCY", "q6h"),
                ("FREQUENCY", "prn"),
            ],
        ),
        (
            "Heparin drip at 1000 units/hr IV.",
            "Heparin",
            vec![("RATE", "1000 units/hr"), ("ROUTE_OR_MODE", "IV")],
        ),
        (
            "Vancomycin 15 mg/kg IV every 12 hours for 14 days.",
            "Vancomycin",
            vec![
                ("STRENGTH", "15 mg/kg"),
                ("ROUTE_OR_MODE", "IV"),
                ("FREQUENCY", "every 12 hours"),
                ("DURATION", "for 14 days"),
            ],
        ),
        (
            "Prednisone 20 mg po daily x 5 days.",
            "Prednisone",
            vec![
                ("STRENGTH", "20 mg"),
                ("ROUTE_OR_MODE", "po"),
                ("FREQUENCY", "daily"),
                ("DURATION", "x 5 days"),
            ],
        ),
        (
            "Fluticasone nasal spray, two sprays every morning.",
            "Fluticasone",
            vec![
                ("ROUTE_OR_MODE", "nasal"),
                ("FORM", "spray"),
                ("DOSAGE", "two sprays"),
                ("FREQUENCY", "every morning"),
            ],
        ),
    ];

    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let entity = mention_entity(text, &answer, mention);
        let attributes = entity["Attributes"].as_array().cloned().unwrap_or_default();
        for (attribute_type, written) in expected {
            assert!(
                attributes.iter().any(|attribute| {
                    attribute["Type"].as_str() == Some(attribute_type)
                        && attribute["Text"].as_str() == Some(written)
                }),
                "case {text:?}: expected a {attribute_type} attribute {written:?}, got {entity}"
            );
        }
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// A prescription belongs to the medication it is written for: the parts of one text that
/// names two medications are reported on the name each part follows, and a part written in
/// a clause that names no medication is reported on none.
#[tokio::test]
async fn each_part_of_a_prescription_is_reported_on_the_medication_it_follows() {
    let router = router();
    let text = "Metformin 500 mg BID and lisinopril 10 mg daily. Follow up in two weeks.";
    let answer = infer(&router, text).await;

    // (the mention, the attributes that belong to it)
    let expected: Vec<(&str, Vec<ExpectedAttribute>)> = vec![
        (
            "Metformin",
            vec![("STRENGTH", "500 mg"), ("FREQUENCY", "BID")],
        ),
        (
            "lisinopril",
            vec![("STRENGTH", "10 mg"), ("FREQUENCY", "daily")],
        ),
    ];
    for (mention, attributes) in expected {
        let entity = mention_entity(text, &answer, mention);
        let reported: Vec<(String, String)> = entity["Attributes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|attribute| {
                (
                    attribute["Type"].as_str().unwrap_or_default().to_string(),
                    attribute["Text"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let wanted: Vec<(String, String)> = attributes
            .into_iter()
            .map(|(attribute_type, written)| (attribute_type.to_string(), written.to_string()))
            .collect();
        assert_eq!(
            reported, wanted,
            "case {mention:?}: expected exactly its own part of the prescription, got {entity}"
        );
    }
    assert_answer_is_well_formed("two prescriptions", text, &answer);
}

/// The words around a mention license its contextual traits, and only the traits this
/// ontology models are reported: the shared pass also recognises a family reference and a
/// hedge, which the modelled `RxNormTraitName` enum does not hold.
#[tokio::test]
async fn context_cues_license_the_traits_this_ontology_models() {
    let router = router();
    // (text, the mention, the traits the context licenses)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "Patient denies taking aspirin.",
            "aspirin",
            vec!["NEGATION"],
        ),
        ("No warfarin given.", "warfarin", vec!["NEGATION"]),
        (
            "Negative for metformin use.",
            "metformin",
            vec!["NEGATION"],
        ),
        (
            "History of amiodarone and warfarin.",
            "warfarin",
            vec!["PAST_HISTORY"],
        ),
        (
            "Previously on lisinopril.",
            "lisinopril",
            vec!["PAST_HISTORY"],
        ),
        (
            "Prior metoprolol therapy.",
            "metoprolol",
            vec!["PAST_HISTORY"],
        ),
        // The family cue and the hedge of the shared pass are not members of this
        // ontology's trait enum, so neither is reported; the history cue in the same
        // clause still is.
        (
            "Family history of metformin use.",
            "metformin",
            vec!["PAST_HISTORY"],
        ),
        ("Possible ibuprofen overuse.", "ibuprofen", vec![]),
        ("Patient takes metformin daily.", "metformin", vec![]),
        // A cue in one clause does not reach the next.
        ("No warfarin. Takes aspirin.", "aspirin", vec![]),
    ];
    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let entity = mention_entity(text, &answer, mention);
        assert_eq!(
            trait_names(&entity),
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {text:?}: wrong traits, got {entity}"
        );
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// Offsets are CHARACTER offsets into the submitted text, not byte offsets: a text
/// carrying multi-byte characters before the mention still reports offsets that delimit
/// it.
#[tokio::test]
async fn offsets_are_character_offsets_into_the_submitted_text() {
    let router = router();
    let cases = [
        "Takes metformin 500 mg daily.",
        "Patiënt — naïve to treatment — takes metformin 500 mg daily.",
        "\u{1f489} insulin glargine 20 units nightly",
        "Meds:\n  metformin 500 mg BID\n  lisinopril 10 mg daily",
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        assert!(
            !entities(&answer).is_empty(),
            "case {text:?}: the text names a medication, got {answer}"
        );
        assert_answer_is_well_formed(text, text, &answer);
    }
}

/// A text that names no medication reports no entity — and the response still reports the
/// model version, in the format the model documents.
#[tokio::test]
async fn a_text_with_no_medication_reports_no_entity() {
    let router = router();
    let cases = [
        "The weather is fine today.",
        "Patient is 44 years old and lives alone.",
        // Nothing here names a medication, so the dosing words belong to nothing and are
        // reported on nothing.
        "Take 2 tablets by mouth twice daily for 10 days.",
        "a",
        &"a".repeat(10_000),
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        assert_eq!(
            entities(&answer),
            Vec::<Value>::new(),
            "case {:?}: nothing in this text names a medication, got {answer}",
            &text[..text.len().min(40)]
        );
        let version = answer["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3
                && parts.iter().all(|part| {
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
    let text = "Discharge meds: metformin 1000 mg BID, lisinopril 20 mg daily, \
                atorvastatin 40 mg at bedtime. Ventolin inhaler 2 puffs q6h prn.";
    let answer = infer(&router, text).await;
    let found = entities(&answer);
    assert!(
        found.len() >= 4,
        "the text names four medications, got {} in {answer}",
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

    let mut ids = Vec::new();
    for entity in &found {
        ids.push(entity["Id"].as_i64().unwrap_or_default());
        for attribute in entity["Attributes"].as_array().cloned().unwrap_or_default() {
            ids.push(attribute["Id"].as_i64().unwrap_or_default());
        }
    }
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every Id in one response is unique, got {ids:?}"
    );
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "the Ids of one response increase monotonically, got {ids:?}"
    );
    assert_answer_is_well_formed("a discharge list", text, &answer);
}

/// The inference reads nothing and writes nothing: the same text always gets the same
/// answer, whatever was inferred in between.
#[tokio::test]
async fn the_same_text_always_gets_the_same_answer() {
    let router = router();
    let text = "Metformin 500 mg po BID for 30 days.";
    let first = infer(&router, text).await;
    let _ = infer(&router, "Lantus 20 units subcutaneously at bedtime.").await;
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
            format!("{} metformin", "a".repeat(10_000 - " metformin".len())),
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
        let (status, answer) = call(&router, "InferRxNorm", &json!({"Text": text})).await;
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
