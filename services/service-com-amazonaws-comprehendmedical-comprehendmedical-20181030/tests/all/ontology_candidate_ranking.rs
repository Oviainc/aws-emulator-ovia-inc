// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the synchronous inference answers LINK a mention to an
//! ontology, which is what `InferICD10CM`, `InferRxNorm` and `InferSNOMEDCT` exist for.
//!
//! The rules under test:
//!
//! * a linked mention reports the candidates of ITS OWN concept family, most likely
//!   first, as many as the ontology offers up to five, with strictly descending scores
//!   that divide the mention's own confidence between them — so a client can page the
//!   candidates for review and tell a confident match from a long-tail one;
//! * a qualifier written as part of a condition's name carries the qualifier-value
//!   concept it names in its own right FIRST and then the concepts of the mention it
//!   qualifies;
//! * an age counts how old the person is, so a medical-condition analysis reports it
//!   neither as a time expression of its own nor as the time of a condition, while a
//!   time expression beside a condition is still reported;
//! * a span the pass placed but the operation reports no entity for still takes its place
//!   in the response-wide `Id` sequence, and the ids rise with the offsets of the spans
//!   they number;
//! * every operation reports the version of its own model, in the documented n.n.n form,
//!   and `InferSNOMEDCT` reports the revision of the ontology it linked against.
//!
//! Each table spans a CLASS of input — several concept families, all three ontologies,
//! both polarities of each rule — rather than one note. Every request goes over the wire
//! the service actually speaks (awsJson1_1, `POST /` with
//! `X-Amz-Target: ComprehendMedical_20181030.<Operation>`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The most candidates one mention reports, which is what the ontology-linking answers
/// page for review.
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

/// Call `operation` with `text` and return the parsed JSON answer, asserting it succeeded.
async fn infer(router: &MultiServiceRouter, operation: &str, text: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
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
        "case {operation} {text:?}: expected 200, got {status} {answer}"
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

/// The entity of `answer` written as `mention`.
fn mention_entity(case: &str, answer: &Value, mention: &str) -> Value {
    entities(case, answer)
        .into_iter()
        .find(|entity| entity["Text"].as_str() == Some(mention))
        .unwrap_or_else(|| panic!("case {case:?}: expected the mention {mention:?}, got {answer}"))
}

/// The candidate list `span` reports, under the member name the ontology's own concept
/// list carries.
fn candidates(case: &str, span: &Value, member: &str) -> Vec<Value> {
    span[member]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {span}"))
}

/// A score the answer reports.
fn score(span: &Value, member: &str) -> f64 {
    span[member].as_f64().unwrap_or_default()
}

/// Assert `list` is a ranked candidate list of the mention `span`: at least `least`
/// candidates and never more than the limit, strictly descending scores in (0, 1), and
/// the mention's own confidence divided between them.
fn assert_ranked_candidates(case: &str, span: &Value, list: &[Value], least: usize) {
    assert!(
        list.len() >= least && list.len() <= CANDIDATE_LIMIT,
        "case {case:?}: expected {least}..={CANDIDATE_LIMIT} ranked candidates, got {} in \
         {span}",
        list.len()
    );
    let scores: Vec<f64> = list.iter().map(|concept| score(concept, "Score")).collect();
    assert!(
        scores.iter().all(|value| *value > 0.0 && *value < 1.0),
        "case {case:?}: every candidate score is a confidence in (0, 1), got {scores:?}"
    );
    assert!(
        scores.windows(2).all(|pair| pair[0] > pair[1]),
        "case {case:?}: the candidates are ranked most likely first, got {scores:?}"
    );
    let total: f64 = scores.iter().sum();
    let confidence = score(span, "Score");
    assert!(
        (total - confidence).abs() < 1.0e-4,
        "case {case:?}: the candidates divide the mention's confidence {confidence} between \
         them, got {total} from {scores:?}"
    );
}

/// One linked-mention row: the operation, the text, the mention the entity reports, the
/// member its candidate list is carried under, how many candidates the ontology offers
/// for that family at least, and the string every candidate of the family carries —
/// the ICD-10-CM category code the family lives under, or (where the ontology's codes are
/// opaque numbers) a word of the concept's own name.
type CandidateRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    usize,
    Option<Family>,
);

/// What makes a candidate part of the mention's own concept family.
#[derive(Debug, Clone, Copy)]
enum Family {
    /// Every candidate's `Code` starts with this, which for ICD-10-CM is the category
    /// the family is classified under.
    Code(&'static str),
    /// Every candidate's `Description` carries this word, which is how a family shows in
    /// an ontology whose codes are opaque identifiers.
    Description(&'static str),
}

/// A linked mention reports the ranked candidates of its own concept family.
///
/// The rows span all three ontologies and several families each, so a handler that
/// answered with one canned list, truncated the list, or attached one family's codes to
/// another family's text fails here.
#[tokio::test]
async fn a_linked_mention_reports_the_ranked_candidates_of_its_own_family() {
    let router = router();
    // Sources: the ICD-10-CM tabular list (Centers for Disease Control) for the category
    // codes, the SNOMED CT US edition for the concept names, and the RxNorm release for
    // the ingredient and brand concepts — the three ontologies the operations link to.
    let rows: &[CandidateRow] = &[
        (
            "InferICD10CM",
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("E11")),
        ),
        (
            "InferICD10CM",
            "Patient has type 1 diabetes mellitus.",
            "diabetes mellitus",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("E10")),
        ),
        (
            "InferICD10CM",
            "Patient has hypertension.",
            "hypertension",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("I1")),
        ),
        (
            "InferICD10CM",
            "Patient has congestive heart failure.",
            "congestive heart failure",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("I50")),
        ),
        (
            "InferICD10CM",
            "Patient has asthma.",
            "asthma",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("J45")),
        ),
        (
            "InferICD10CM",
            "Patient has pneumonia.",
            "pneumonia",
            "ICD10CMConcepts",
            5,
            Some(Family::Code("J1")),
        ),
        (
            "InferICD10CM",
            "Patient reports a cough.",
            "cough",
            "ICD10CMConcepts",
            3,
            Some(Family::Code("R05")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("diabet")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has hypertension.",
            "hypertension",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("hyperten")),
        ),
        (
            "InferSNOMEDCT",
            "Patient had a myocardial infarction.",
            "myocardial infarction",
            "SNOMEDCTConcepts",
            5,
            Some(Family::Description("myocardial")),
        ),
        (
            "InferSNOMEDCT",
            "Patient has asthma.",
            "asthma",
            "SNOMEDCTConcepts",
            3,
            Some(Family::Description("asthma")),
        ),
        (
            "InferSNOMEDCT",
            "Patient reports a cough.",
            "cough",
            "SNOMEDCTConcepts",
            3,
            Some(Family::Description("cough")),
        ),
        (
            "InferRxNorm",
            "He takes metformin 500 mg orally twice daily.",
            "metformin",
            "RxNormConcepts",
            1,
            Some(Family::Description("metformin")),
        ),
        (
            "InferRxNorm",
            "He takes insulin glargine 20 units nightly.",
            "insulin glargine",
            "RxNormConcepts",
            2,
            Some(Family::Description("insulin")),
        ),
        (
            // A brand name is linked to its own brand concept first and then to the
            // ingredient it delivers, so the family is the brand, not one word.
            "InferRxNorm",
            "He takes Tylenol 500 mg as needed.",
            "Tylenol",
            "RxNormConcepts",
            2,
            None,
        ),
    ];

    for (operation, text, mention, member, least, family) in rows {
        let case = format!("{operation} {text:?}");
        let answer = infer(&router, operation, text).await;
        let entity = mention_entity(&case, &answer, mention);
        let list = candidates(&case, &entity, member);
        assert_ranked_candidates(&case, &entity, &list, *least);
        for concept in &list {
            let code = concept["Code"].as_str().unwrap_or_default();
            let description = concept["Description"].as_str().unwrap_or_default();
            assert!(
                !code.is_empty() && !description.is_empty(),
                "case {case:?}: every candidate names a code and its description, got \
                 {concept}"
            );
            match family {
                Some(Family::Code(prefix)) => assert!(
                    code.starts_with(prefix),
                    "case {case:?}: the candidates of {mention:?} belong to the family \
                     {prefix}, got {code:?} ({description:?})"
                ),
                Some(Family::Description(word)) => assert!(
                    description.to_lowercase().contains(word),
                    "case {case:?}: the candidates of {mention:?} name the concept family \
                     {word:?}, got {description:?} ({code:?})"
                ),
                None => {}
            }
        }
    }
}

/// The candidates of a qualifier split off a condition's name: the qualifier value it
/// names in its own right, and then the concepts of the mention it qualifies.
///
/// The rows are the digit and the roman-numeral spelling of two subtypes, so a handler
/// that reported a single candidate for the qualifier fails here.
#[tokio::test]
async fn a_qualifier_continues_the_candidates_of_the_entity_it_qualifies() {
    let router = router();
    let rows: &[(&str, &str, &str)] = &[
        ("Patient has type 2 diabetes mellitus.", "type 2", "Type 2"),
        ("Patient has type 1 diabetes mellitus.", "type 1", "Type 1"),
        (
            "Patient has type II diabetes mellitus.",
            "type II",
            "Type 2",
        ),
    ];

    for (text, qualifier, qualifier_value) in rows {
        let case = format!("InferSNOMEDCT {text:?}");
        let answer = infer(&router, "InferSNOMEDCT", text).await;
        let entity = mention_entity(&case, &answer, "diabetes mellitus");
        let entity_codes: Vec<String> = candidates(&case, &entity, "SNOMEDCTConcepts")
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default().to_string())
            .collect();
        let attribute = attributes(&entity)
            .into_iter()
            .find(|attribute| attribute["Text"].as_str() == Some(qualifier))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the qualifier attribute {qualifier:?}, got {answer}")
            });
        let list = candidates(&case, &attribute, "SNOMEDCTConcepts");
        assert_ranked_candidates(&case, &attribute, &list, 2);

        let leading = list[0]["Description"].as_str().unwrap_or_default();
        assert_eq!(
            leading,
            format!("{qualifier_value} (qualifier value)"),
            "case {case:?}: the qualifier's own concept is its leading candidate, got \
             {leading:?}"
        );
        let continued: Vec<String> = list[1..]
            .iter()
            .map(|concept| concept["Code"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            continued.iter().all(|code| entity_codes.contains(code)),
            "case {case:?}: the qualifier's remaining candidates are the concepts of the \
             mention it qualifies {entity_codes:?}, got {continued:?}"
        );
    }
}

/// Every span `operation` reports for `text`, entities and attributes together, as
/// `(BeginOffset, Id, Type, Text)`.
async fn reported_spans(
    router: &MultiServiceRouter,
    operation: &str,
    text: &str,
) -> Vec<(i64, i64, String, String)> {
    let case = format!("{operation} {text:?}");
    let answer = infer(router, operation, text).await;
    let mut spans = Vec::new();
    for entity in entities(&case, &answer) {
        for span in std::iter::once(entity.clone()).chain(attributes(&entity)) {
            spans.push((
                span["BeginOffset"].as_i64().unwrap_or_default(),
                span["Id"].as_i64().unwrap_or_default(),
                span["Type"].as_str().unwrap_or_default().to_string(),
                span["Text"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    spans.sort_unstable();
    spans
}

/// An age is how old the person is, not the time course of a condition: a
/// medical-condition analysis reports it as neither entity nor attribute, while a time
/// expression written beside a condition is still reported. The age span is still placed,
/// so it takes its place in the response-wide `Id` sequence.
///
/// The rows are the written forms an age takes ("65-year-old", "Age: 72", "45 yo") in
/// both operations that place time expressions, plus the opposite polarity: a duration
/// that really is the time course of the condition.
#[tokio::test]
async fn an_age_is_not_the_time_course_of_a_condition() {
    let router = router();
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "InferICD10CM",
            "Patient is a 65-year-old male with type 2 diabetes mellitus.",
            "65",
            None,
        ),
        (
            "InferICD10CM",
            "Age: 72. Patient has asthma.",
            "72",
            None,
        ),
        (
            "InferICD10CM",
            "45 yo male with pneumonia.",
            "45",
            None,
        ),
        (
            "InferSNOMEDCT",
            "Patient is a 65-year-old male with type 2 diabetes mellitus.",
            "65",
            None,
        ),
        (
            // The same analysis still reports the duration a condition has lasted.
            "InferICD10CM",
            "Patient reports fever for three weeks.",
            "99",
            Some("three weeks"),
        ),
    ];

    for (operation, text, age, timed) in rows {
        let case = format!("{operation} {text:?}");
        let spans = reported_spans(&router, operation, text).await;
        assert!(
            !spans.is_empty(),
            "case {case:?}: the text names a condition, got no span"
        );
        assert!(
            spans.iter().all(|(_, _, _, written)| written != age),
            "case {case:?}: an age is not a span of a medical-condition analysis, got \
             {spans:?}"
        );
        match timed {
            Some(duration) => assert!(
                spans
                    .iter()
                    .any(|(_, _, kind, written)| written == duration
                        && (kind == "TIME_EXPRESSION" || kind == "TIME_TO_DX_NAME")),
                "case {case:?}: the time course of the condition is still reported, got \
                 {spans:?}"
            ),
            None => assert!(
                spans
                    .iter()
                    .all(|(_, _, kind, _)| kind != "TIME_EXPRESSION"
                        && kind != "TIME_TO_DX_NAME"),
                "case {case:?}: an age is the age of the person, not a time expression of \
                 a condition, got {spans:?}"
            ),
        }

        // The ids rise with the offsets of the spans they number, and the age the pass
        // placed without reporting has taken the first of them.
        let ids: Vec<i64> = spans.iter().map(|(_, id, _, _)| *id).collect();
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the Ids rise with the offsets of the spans they number, got \
             {spans:?}"
        );
        if timed.is_none() {
            assert!(
                ids.first().is_some_and(|first| *first > 1),
                "case {case:?}: the age span the pass placed takes the first Id, so the \
                 first reported span carries a later one, got {spans:?}"
            );
        }
    }
}

/// A value written under a field label of its own identifier class is established by two
/// independent pieces of evidence — the characters take the class's written form AND the
/// record says which class the value is — so `DetectPHI` reports it with more confidence
/// than the same written form standing on its own.
///
/// The rows are three identifier classes, each in both polarities, so a handler whose
/// `Score` is one constant per detection class fails here.
#[tokio::test]
async fn a_labelled_value_is_reported_with_more_confidence_than_a_bare_one() {
    let router = router();
    // (the class's mention, the note that labels it, the note that does not)
    let rows: &[(&str, &str, &str)] = &[
        (
            "617-555-0199",
            "Phone: 617-555-0199.",
            "Reach him on 617-555-0199.",
        ),
        (
            "03/14/2024",
            "Date of visit: 03/14/2024.",
            "He felt unwell around 03/14/2024.",
        ),
        (
            "j.smith@example.com",
            "Email: j.smith@example.com.",
            "Write to j.smith@example.com.",
        ),
    ];

    for (mention, labelled, bare) in rows {
        let case = format!("DetectPHI {mention:?}");
        let under_label = infer(&router, "DetectPHI", labelled).await;
        let on_its_own = infer(&router, "DetectPHI", bare).await;
        let with_label = score(&mention_entity(&case, &under_label, mention), "Score");
        let without = score(&mention_entity(&case, &on_its_own, mention), "Score");
        assert!(
            with_label > without,
            "case {case:?}: a value written under a field label of its class carries more \
             evidence than the same form alone, got {with_label} under {labelled:?} and \
             {without} in {bare:?}"
        );
        assert!(
            with_label < 1.0 && without > 0.0,
            "case {case:?}: both detections report a confidence in (0, 1), got \
             {with_label} and {without}"
        );
    }
}

/// Every one of the four synchronous operations reports the version of ITS OWN model, in
/// the documented n.n.n form and stable from call to call, and `InferSNOMEDCT` reports
/// the revision of the ontology it linked against.
#[tokio::test]
async fn every_operation_reports_the_version_of_its_own_model() {
    let router = router();
    let text = "Patient has type 2 diabetes mellitus and takes metformin 500 mg daily. \
                Reach him on 617-555-0199.";
    for operation in ["DetectPHI", "InferICD10CM", "InferRxNorm", "InferSNOMEDCT"] {
        let first = infer(&router, operation, text).await;
        let second = infer(&router, operation, text).await;
        let version = first["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.chars().all(|digit| digit.is_numeric())),
            "case {operation}: ModelVersion is the model's version in n.n.n form, got \
             {version:?}"
        );
        assert_eq!(
            second["ModelVersion"].as_str().unwrap_or_default(),
            version,
            "case {operation}: the model behind an analysis does not change between two \
             calls, got {second}"
        );
    }

    let answer = infer(&router, "InferSNOMEDCT", text).await;
    let details = &answer["SNOMEDCTDetails"];
    // The edition and the language are what the modelled members document: the US
    // edition, and US English for every operation of the service.
    assert_eq!(
        details["Edition"].as_str(),
        Some("US"),
        "case InferSNOMEDCT: the revision reports the edition it linked against, got \
         {details}"
    );
    assert_eq!(
        details["Language"].as_str(),
        Some("en"),
        "case InferSNOMEDCT: the revision reports the language of its descriptions, got \
         {details}"
    );
    let version_date = details["VersionDate"].as_str().unwrap_or_default();
    let fields: Vec<&str> = version_date.split('-').collect();
    assert!(
        fields.len() == 3
            && fields
                .iter()
                .all(|field| !field.is_empty() && field.chars().all(|digit| digit.is_numeric())),
        "case InferSNOMEDCT: the revision reports the release date of the ontology, got \
         {version_date:?}"
    );
}
