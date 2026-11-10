// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ATTRIBUTE-level ontology linking of `InferSNOMEDCT`.
//!
//! The rule under test: a concept list is read off the stretch of text it is reported on.
//! The modelled `SNOMEDCTAttribute$SNOMEDCTConcepts` is "the SNOMED-CT concepts specific
//! to an attribute", while `SNOMEDCTEntity$SNOMEDCTConcepts` is what the ENTITY could
//! refer to — so an attribute reports the concepts ITS OWN written form names (the
//! qualifier value of a modifier, the body structure of a site), never the concepts of the
//! entity it modifies, and an attribute whose written form the ontology data this pass
//! carries holds no row for reports no concept list at all rather than a borrowed or
//! invented one.
//!
//! Each table spans a CLASS of attribute rather than one document: every attribute type
//! the pass can place, both polarities of the rule (a written form with an ontology row
//! and one without), and the spellings a subtype qualifier is written in. Every request
//! goes over the wire the service actually speaks (awsJson1_1, `POST /` with
//! `X-Amz-Target: ComprehendMedical_20181030.InferSNOMEDCT`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The most candidates one stretch of text reports.
const CANDIDATE_LIMIT: usize = 5;

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

/// Infer against `text` and return the parsed answer, asserting the call succeeded.
async fn infer(router: &MultiServiceRouter, text: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.InferSNOMEDCT"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({"Text": text})).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answer: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {text:?}: expected 200, got {status} {answer}"
    );
    answer
}

/// The entities of an answer.
fn entities(case: &str, answer: &Value) -> Vec<Value> {
    answer["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: an answer carries an Entities list, got {answer}"))
}

/// The attributes of an entity.
fn attributes(entity: &Value) -> Vec<Value> {
    entity["Attributes"].as_array().cloned().unwrap_or_default()
}

/// The attribute of `answer` written exactly as `written`, with the entity reporting it.
fn attribute_of(case: &str, answer: &Value, written: &str) -> (Value, Value) {
    entities(case, answer)
        .into_iter()
        .flat_map(|entity| {
            attributes(&entity)
                .into_iter()
                .map(move |attribute| (entity.clone(), attribute))
        })
        .find(|(_, attribute)| attribute["Text"].as_str() == Some(written))
        .unwrap_or_else(|| panic!("case {case:?}: expected the attribute {written:?}, got {answer}"))
}

/// The `Code` of every concept of `span`'s list under `member`.
fn concept_codes(span: &Value, member: &str) -> Vec<String> {
    span[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|concept| concept["Code"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the linked-attribute table: the text, the attribute as written, and the
/// concept id and fully specified name its own written form names.
type LinkedAttribute = (&'static str, &'static str, &'static str, &'static str);

/// An attribute reports the concepts its OWN written form names, and none of the concepts
/// of the entity it modifies.
///
/// The rows cover every attribute type this pass links: the subtype qualifier it splits
/// off a condition's name (digit and roman-numeral spelling), the acuity, quality and
/// direction modifiers, and the body structure reported as a site. Sources: the
/// qualifier-value and body-structure hierarchies of the SNOMED CT US edition, which is
/// where this crate's ontology data for those written forms comes from.
#[tokio::test]
async fn an_attribute_links_the_concepts_of_its_own_written_form() {
    let router = router();
    let rows: &[LinkedAttribute] = &[
        (
            "Patient has type 2 diabetes mellitus.",
            "type 2",
            "258195006",
            "Type 2 (qualifier value)",
        ),
        (
            "Known type I diabetes mellitus.",
            "type I",
            "258194007",
            "Type 1 (qualifier value)",
        ),
        (
            "Chronic cough with wheezing.",
            "Chronic",
            "90734009",
            "Chronic (qualifier value)",
        ),
        (
            "Acute bronchitis on examination.",
            "Acute",
            "263654008",
            "Acute (qualifier value)",
        ),
        (
            "Recurrent bronchitis each winter.",
            "Recurrent",
            "255227004",
            "Recurrent (qualifier value)",
        ),
        (
            "Moderate asthma reported.",
            "Moderate",
            "6736007",
            "Moderate (severity modifier) (qualifier value)",
        ),
        (
            "Severe chest pain on arrival.",
            "Severe",
            "24484000",
            "Severe (severity modifier) (qualifier value)",
        ),
        (
            "Right pneumonia on the radiograph.",
            "Right",
            "24028007",
            "Right (qualifier value)",
        ),
        (
            "Lower back pain for a week.",
            "Lower",
            "261122009",
            "Lower (qualifier value)",
        ),
        (
            "Left lung pneumonia.",
            "lung",
            "39607008",
            "Lung structure (body structure)",
        ),
    ];

    for (text, written, code, description) in rows {
        let case = format!("{text:?} {written:?}");
        let answer = infer(&router, text).await;
        let (entity, attribute) = attribute_of(&case, &answer, written);
        let reported = concept_codes(&attribute, "SNOMEDCTConcepts");
        assert!(
            !reported.is_empty() && reported.len() <= CANDIDATE_LIMIT,
            "case {case:?}: an attribute the ontology data names carries 1..={CANDIDATE_LIMIT} \
             concepts, got {reported:?} in {attribute}"
        );
        let leading = (
            attribute["SNOMEDCTConcepts"][0]["Code"]
                .as_str()
                .unwrap_or_default(),
            attribute["SNOMEDCTConcepts"][0]["Description"]
                .as_str()
                .unwrap_or_default(),
        );
        assert_eq!(
            leading,
            (*code, *description),
            "case {case:?}: expected the concept its own text names, got {leading:?} in \
             {attribute}"
        );
        let entity_codes = concept_codes(&entity, "SNOMEDCTConcepts");
        assert!(
            reported.iter().all(|code| !entity_codes.contains(code)),
            "case {case:?}: an attribute reports none of the concepts of the entity it \
             modifies {entity_codes:?}, got {reported:?}"
        );

        // Every concept of the list is a well-formed ontology row, the scores rank the
        // candidates most likely first, and they divide the attribute's own confidence —
        // the modelled `Score` of a concept reports "the likelihood of the match".
        let confidence = attribute["Score"].as_f64().unwrap_or_default();
        let mut previous = f64::INFINITY;
        let mut total = 0.0;
        let mut seen = BTreeSet::new();
        for concept in attribute["SNOMEDCTConcepts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let code = concept["Code"].as_str().unwrap_or_default().to_string();
            // Source: the SNOMED CT identifier specification — a concept id is 6 to 18
            // digits and never starts with a zero.
            assert!(
                (6..=18).contains(&code.chars().count())
                    && code.chars().all(|digit| digit.is_ascii_digit())
                    && !code.starts_with('0'),
                "case {case:?}: {code:?} is not a SNOMED CT concept id, in {attribute}"
            );
            assert!(
                seen.insert(code.clone()),
                "case {case:?}: {code:?} is reported twice, in {attribute}"
            );
            // A SNOMED CT description is the fully specified name: the term followed by
            // the semantic tag of its hierarchy, in parentheses.
            let name = concept["Description"].as_str().unwrap_or_default();
            assert!(
                name.ends_with(')') && name.contains(" ("),
                "case {case:?}: {name:?} is not a fully specified name, in {attribute}"
            );
            let score = concept["Score"].as_f64().unwrap_or_default();
            assert!(
                score > 0.0 && score < previous,
                "case {case:?}: candidate scores descend, got {score} after {previous} in \
                 {attribute}"
            );
            previous = score;
            total += score;
        }
        assert!(
            (total - confidence).abs() < 1.0e-4,
            "case {case:?}: the candidates divide the attribute's confidence {confidence} \
             between them, got {total}"
        );
    }
}

/// An attribute whose written form the ontology data this pass carries holds no row for
/// reports NO concept list — the member is absent rather than carrying the concepts of the
/// entity it modifies, or a concept the ontology does not hold.
///
/// The rows span the attribute types that name a measurement rather than a concept (the
/// value a test produced and the unit it was measured in) and modifier vocabulary the
/// ontology data carries no qualifier value for.
#[tokio::test]
async fn an_attribute_the_ontology_does_not_name_reports_no_concepts() {
    let router = router();
    // (text, the attribute as written, its modelled type)
    let rows: &[(&str, &str, &str)] = &[
        ("A1c 7.2 percent today.", "7.2", "TEST_VALUE"),
        ("A1c 7.2 percent today.", "percent", "TEST_UNIT"),
        ("Blood pressure 140/90 mmHg on arrival.", "140/90", "TEST_VALUE"),
        ("Blood pressure 140/90 mmHg on arrival.", "mmHg", "TEST_UNIT"),
        ("Burning chest pain on exertion.", "Burning", "QUALITY"),
        ("Progressive dyspnea over two weeks.", "Progressive", "ACUITY"),
    ];

    for (text, written, attribute_type) in rows {
        let case = format!("{text:?} {written:?}");
        let answer = infer(&router, text).await;
        let (entity, attribute) = attribute_of(&case, &answer, written);
        assert_eq!(
            attribute["Type"].as_str(),
            Some(*attribute_type),
            "case {case:?}: wrong attribute type, got {attribute}"
        );
        assert!(
            attribute["SNOMEDCTConcepts"].is_null(),
            "case {case:?}: an attribute the ontology data does not name reports no \
             concepts, got {attribute}"
        );
        // The entity it belongs to still reports its own.
        assert!(
            !concept_codes(&entity, "SNOMEDCTConcepts").is_empty(),
            "case {case:?}: the entity still reports its own concepts, got {entity}"
        );
    }
}

/// Across a whole note, no attribute of any entity repeats a concept of the entity it
/// modifies: the two lists answer different questions, so a handler that filled an
/// attribute's list from its entity's fails here on every entity at once.
#[tokio::test]
async fn no_attribute_of_a_note_borrows_its_entity_concepts() {
    let router = router();
    let notes = [
        "Patient has type 2 diabetes mellitus with chronic kidney disease. \
         Severe left chest pain on arrival; A1c 7.2 percent.",
        "Known type I diabetes mellitus and bilateral lower lung pneumonia. \
         Blood pressure 140/90 mmHg, heart rate 110 bpm.",
        "Acute bronchitis with productive cough. Chronic asthma since childhood.",
    ];
    for text in notes {
        let answer = infer(&router, text).await;
        let found = entities(text, &answer);
        assert!(
            !found.is_empty(),
            "case {text:?}: the note names clinical concepts, got {answer}"
        );
        for entity in found {
            let entity_codes = concept_codes(&entity, "SNOMEDCTConcepts");
            for attribute in attributes(&entity) {
                let reported = concept_codes(&attribute, "SNOMEDCTConcepts");
                assert!(
                    reported.iter().all(|code| !entity_codes.contains(code)),
                    "case {text:?}: the attribute {} reports a concept of its entity \
                     {entity_codes:?}, got {reported:?}",
                    attribute["Text"]
                );
            }
        }
    }
}
