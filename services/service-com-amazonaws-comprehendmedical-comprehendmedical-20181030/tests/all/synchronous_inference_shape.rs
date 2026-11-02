// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SHAPE of the synchronous inference answers: `DetectPHI`,
//! `InferICD10CM`, `InferRxNorm` and `InferSNOMEDCT`.
//!
//! The rules under test are the ones that hold across all four operations rather than
//! inside any one of them:
//!
//! * every `Id` of one response numbers a span of the document: the ids are unique
//!   within the response, shared by entities and their attributes, counted from 1 and
//!   rising with the offsets of the spans they number;
//! * a mention is segmented at the concept's NAME: a subtype qualifier written in front
//!   of it is reported as that entity's own `QUALITY` attribute, linked to the concepts
//!   the qualifier names in its own right, and the entity keeps the concepts of the whole
//!   written form;
//! * a SNOMED CT concept's `Description` is the ontology's fully specified name — the
//!   term followed by its semantic tag in parentheses;
//! * every PHI mention an identifier class licenses is reported, a named organization
//!   included, and each one's `Score` is read off that detection rather than fixed per
//!   class;
//! * a measured amount beside a medication is the amount ordered unless the sig also
//!   counts the dose units, in which case the amount is the strength of each of them;
//! * a cue that places a mention in time reaches the mention it introduces and no
//!   further.
//!
//! Each table spans a CLASS of input — several written forms, several ontologies, both
//! polarities of each rule — rather than one document, so a handler that answered for one
//! note fails here. Every request goes over the wire the service actually speaks
//! (awsJson1_1, `POST /` with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The semantic tags a SNOMED CT fully specified name can end with, as the release writes
/// them. Source: the semantic tags of the SNOMED CT hierarchies this service's concept
/// tables draw on (disorder, finding, body structure, observable entity, procedure,
/// regime/therapy, substance, qualifier value).
const SEMANTIC_TAGS: &[&str] = &[
    "disorder",
    "finding",
    "body structure",
    "observable entity",
    "procedure",
    "regime/therapy",
    "substance",
    "qualifier value",
];

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

/// The characters `begin..end` of `text`, which is what a reported offset pair delimits:
/// both modelled offset members are 0-based CHARACTER offsets.
fn by_characters(text: &str, begin: usize, end: usize) -> String {
    text.chars()
        .skip(begin)
        .take(end.saturating_sub(begin))
        .collect()
}

/// The offsets of `span` delimit exactly the `Text` it reports.
fn assert_offsets_delimit_text(case: &str, text: &str, span: &Value) {
    let reported = span["Text"].as_str().unwrap_or_default();
    let begin = span["BeginOffset"].as_u64().unwrap_or_default() as usize;
    let end = span["EndOffset"].as_u64().unwrap_or_default() as usize;
    let written = by_characters(text, begin, end);
    assert_eq!(
        written, reported,
        "case {case:?}: the offsets {begin}..{end} must delimit the reported text, got \
         {written:?} for {reported:?} in {span}"
    );
}

/// The names of a span's traits.
fn trait_names(span: &Value) -> Vec<String> {
    span["Traits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A note that names a PHI mention, two conditions with a subtype qualifier, a
/// prescription and a procedure, so each of the four operations has entities AND
/// attributes to number.
const SHARED_NOTE: &str = "Patient is a 62-year-old male with a history of type 2 diabetes \
                           mellitus and severe hypertension, seen at Boston General Hospital \
                           on 03/14/2024. He takes metformin 500 mg orally twice daily. \
                           Reach him on 617-555-0199.";

/// Every `Id` of one response numbers a span of the document: the ids are unique within
/// the response and rise with the offsets of the spans they number, entities and their
/// attributes sharing the one sequence, as the modelled `Id` members document.
///
/// The rows are all four synchronous operations, so an operation that restarted the
/// sequence per entity — or numbered from 0 — fails here.
#[tokio::test]
async fn every_id_of_one_response_numbers_the_spans_in_the_order_they_are_written() {
    let router = router();
    for operation in ["DetectPHI", "InferICD10CM", "InferRxNorm", "InferSNOMEDCT"] {
        let answer = infer(&router, operation, SHARED_NOTE).await;
        let found = entities(operation, &answer);
        assert!(
            !found.is_empty(),
            "case {operation}: the note names entities of every one of these \
             operations, got {answer}"
        );
        let mut spans: Vec<(i64, i64)> = Vec::new();
        for entity in &found {
            spans.push((
                entity["BeginOffset"].as_i64().unwrap_or_default(),
                entity["Id"].as_i64().unwrap_or_default(),
            ));
            for attribute in attributes(entity) {
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
            "case {operation}: every Id of one response is unique, got {ids:?} in {answer}"
        );
        assert!(
            ids.first().is_some_and(|first| *first >= 1),
            "case {operation}: the sequence is counted from 1, got {ids:?} in {answer}"
        );
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "case {operation}: the Ids must rise with the offsets of the spans they \
             number, got {spans:?} in {answer}"
        );
    }
}

/// One qualifier row: the text, the entity the condition NAME is reported as, the
/// qualifier reported as its `QUALITY` attribute (or `None` where the form carries none),
/// and the concept code the whole written form still links to in each ontology.
type QualifierRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
);

/// A subtype qualifier written in front of a condition is reported as that entity's own
/// `QUALITY` attribute, the entity spanning only the condition NAME — and the entity
/// still links to the concepts the WHOLE written form names, because the qualifier is
/// what tells the subtype's codes from the unspecified ones.
///
/// The rows span the digit and the roman-numeral spellings of a subtype, a qualifier on a
/// one-word and on a two-word condition name, and a written form carrying no qualifier at
/// all (which must report no `QUALITY` attribute).
#[tokio::test]
async fn a_subtype_qualifier_is_the_quality_attribute_of_the_condition_name() {
    let router = router();
    // (text, the entity as written, the qualifier as written, the ICD-10-CM code, the
    // SNOMED CT code)
    let cases: Vec<QualifierRow> = vec![
        (
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            Some("type 2"),
            "E11.9",
            "44054006",
        ),
        (
            "Known type 1 diabetes.",
            "diabetes",
            Some("type 1"),
            "E10.9",
            "46635009",
        ),
        (
            "Assessment: Type II diabetes.",
            "diabetes",
            Some("Type II"),
            "E11.9",
            "44054006",
        ),
        (
            "Patient has diabetes mellitus.",
            "diabetes mellitus",
            None,
            "E11.9",
            "73211009",
        ),
    ];

    for (text, mention, qualifier, icd10cm, snomedct) in cases {
        for (operation, concepts_member, code) in [
            ("InferICD10CM", "ICD10CMConcepts", icd10cm),
            ("InferSNOMEDCT", "SNOMEDCTConcepts", snomedct),
        ] {
            let case = format!("{operation} {text}");
            let answer = infer(&router, operation, text).await;
            let entity = mention_entity(&case, &answer, mention);
            assert_offsets_delimit_text(&case, text, &entity);
            assert_eq!(
                entity["Type"].as_str(),
                Some("DX_NAME"),
                "case {case:?}: the condition name is typed DX_NAME, got {entity}"
            );
            assert_eq!(
                entity[concepts_member][0]["Code"].as_str(),
                Some(code),
                "case {case:?}: the whole written form names {code} first, got {entity}"
            );
            let quality: Vec<Value> = attributes(&entity)
                .into_iter()
                .filter(|attribute| attribute["Type"].as_str() == Some("QUALITY"))
                .collect();
            match qualifier {
                Some(written) => {
                    assert_eq!(
                        quality.len(),
                        1,
                        "case {case:?}: expected exactly the qualifier as a QUALITY \
                         attribute, got {entity}"
                    );
                    assert_eq!(
                        quality[0]["Text"].as_str(),
                        Some(written),
                        "case {case:?}: expected the QUALITY attribute {written:?}, got \
                         {entity}"
                    );
                    assert_offsets_delimit_text(&case, text, &quality[0]);
                    let relationship = quality[0]["RelationshipScore"].as_f64().unwrap_or_default();
                    assert!(
                        relationship > 0.0 && relationship <= 1.0,
                        "case {case:?}: RelationshipScore must be in (0, 1], got \
                         {relationship} in {entity}"
                    );
                }
                None => assert!(
                    quality.is_empty(),
                    "case {case:?}: nothing qualifies this mention, got {entity}"
                ),
            }
        }
    }
}

/// A SNOMED CT attribute carries the concepts it is itself linked to, ranked by
/// likelihood: a qualifier is a concept of the ontology in its own right, which is how a
/// client codes it separately from the condition it qualifies.
///
/// The rows are the subtype spellings the qualifier table holds, including the
/// roman-numeral ones.
#[tokio::test]
async fn a_snomedct_attribute_carries_its_own_ranked_concepts() {
    let router = router();
    // (text, the qualifier as written, the qualifier-value concept it names)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("Patient has type 2 diabetes mellitus.", "type 2", "258195006"),
        ("Known type 1 diabetes.", "type 1", "258194007"),
        ("Assessment: Type II diabetes.", "Type II", "258195006"),
        ("History of Type I diabetes mellitus.", "Type I", "258194007"),
    ];
    for (text, written, code) in cases {
        let answer = infer(&router, "InferSNOMEDCT", text).await;
        let found = entities(text, &answer);
        let attribute = found
            .iter()
            .flat_map(attributes)
            .find(|attribute| attribute["Text"].as_str() == Some(written))
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the attribute {written:?}, got {answer}")
            });
        let concepts = attribute["SNOMEDCTConcepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !concepts.is_empty() && concepts.len() <= 5,
            "case {text:?}: an attribute the lexicon placed carries between 1 and 5 \
             concepts, got {attribute}"
        );
        assert_eq!(
            concepts[0]["Code"].as_str(),
            Some(code),
            "case {text:?}: expected the qualifier-value concept {code}, got {attribute}"
        );
        let mut previous = f64::INFINITY;
        for concept in &concepts {
            let score = concept["Score"].as_f64().unwrap_or_default();
            assert!(
                score > 0.0 && score < previous,
                "case {text:?}: attribute concept scores must descend, got {score} after \
                 {previous} in {attribute}"
            );
            previous = score;
        }
    }
}

/// Every SNOMED CT `Description` is the ontology's FULLY SPECIFIED NAME: the concept's
/// term followed by its semantic tag in parentheses, which is the string a terminology
/// release carries — so a client joining the answer against a release matches a row.
///
/// The texts span every modelled category (a condition, a body structure, a test, a
/// procedure, a treatment) and both levels concepts are reported at (entities and their
/// attributes).
#[tokio::test]
async fn every_snomedct_description_is_a_fully_specified_name() {
    let router = router();
    let cases = [
        "Patient has type 2 diabetes mellitus and hypertension.",
        "Severe left chest pain with wheezing.",
        "Hemoglobin A1c 9.2 percent and a complete blood count were ordered.",
        "Prior appendectomy and a colonoscopy.",
        "Started on dialysis and physical therapy.",
        "Chest x-ray of the lung showed pneumonia.",
    ];
    for text in cases {
        let answer = infer(&router, "InferSNOMEDCT", text).await;
        let found = entities(text, &answer);
        assert!(
            !found.is_empty(),
            "case {text:?}: the text names clinical concepts, got {answer}"
        );
        let mut seen = 0usize;
        for entity in &found {
            let mut lists = vec![entity["SNOMEDCTConcepts"].clone()];
            lists.extend(
                attributes(entity)
                    .iter()
                    .map(|attribute| attribute["SNOMEDCTConcepts"].clone()),
            );
            for list in lists {
                for concept in list.as_array().cloned().unwrap_or_default() {
                    let description = concept["Description"].as_str().unwrap_or_default();
                    let tag = description
                        .rsplit_once(" (")
                        .and_then(|(_, tail)| tail.strip_suffix(')'))
                        .unwrap_or_default();
                    assert!(
                        SEMANTIC_TAGS.contains(&tag),
                        "case {text:?}: {description:?} must end with a SNOMED CT semantic \
                         tag in parentheses, got the tag {tag:?}"
                    );
                    assert!(
                        description.len() > tag.len() + 3,
                        "case {text:?}: a fully specified name carries a term before its \
                         tag, got {description:?}"
                    );
                    seen += 1;
                }
            }
        }
        assert!(
            seen > 0,
            "case {text:?}: the answer links its mentions to concepts, got {answer}"
        );
    }
}

/// A named organization is protected health information: the facility a note names is a
/// location identifier of the document, which is the identifier class the modelled
/// `ADDRESS` sub-type reports, so it is reported wherever it is written.
///
/// The rows span several organization designators and both a two-word and a longer
/// proper name; the negative rows are the designator written as the common noun it also
/// is, which names no organization.
#[tokio::test]
async fn a_named_organization_is_reported_as_protected_health_information() {
    let router = router();
    // (text, the organization as written, or None where the text names none)
    let cases: Vec<(&str, Option<&str>)> = vec![
        (
            "Admitted to Boston General Hospital overnight.",
            Some("Boston General Hospital"),
        ),
        (
            "Follow up at the Cedar Valley Medical Center.",
            Some("Cedar Valley Medical Center"),
        ),
        (
            "Seen in the Riverside Infirmary.",
            Some("Riverside Infirmary"),
        ),
        (
            "Specimen sent to Northside Laboratories.",
            Some("Northside Laboratories"),
        ),
        (
            "He teaches at Fairview College.",
            Some("Fairview College"),
        ),
        ("Hospital course was uneventful.", None),
        ("Discharged from the clinic in good condition.", None),
    ];
    for (text, organization) in cases {
        let answer = infer(&router, "DetectPHI", text).await;
        let found = entities(text, &answer);
        let addresses: Vec<&Value> = found
            .iter()
            .filter(|entity| entity["Type"].as_str() == Some("ADDRESS"))
            .collect();
        match organization {
            Some(written) => {
                let entity = addresses
                    .iter()
                    .find(|entity| entity["Text"].as_str() == Some(written))
                    .unwrap_or_else(|| {
                        panic!("case {text:?}: expected the organization {written:?}, got {answer}")
                    });
                assert_eq!(
                    entity["Category"].as_str(),
                    Some("PROTECTED_HEALTH_INFORMATION"),
                    "case {text:?}: a PHI entity is in the PHI category, got {entity}"
                );
                assert_offsets_delimit_text(text, text, entity);
            }
            None => assert!(
                addresses.is_empty(),
                "case {text:?}: the designator here names no organization, got {answer}"
            ),
        }
    }
}

/// A PHI `Score` is read off the detection it reports rather than fixed per identifier
/// class: two mentions established the same way but written out to different extents are
/// reported with different confidences, and every confidence stays in (0, 1).
#[tokio::test]
async fn a_phi_confidence_is_read_off_the_detection() {
    let router = router();
    // (text, the two mentions of ONE identifier class whose written forms differ)
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "Call him on 617-555-0199 or on +1 617-555-0188.",
            "617-555-0199",
            "+1 617-555-0188",
        ),
        (
            "Seen at Fairview Clinic and at the Cedar Valley Medical Center.",
            "Fairview Clinic",
            "Cedar Valley Medical Center",
        ),
    ];
    for (text, shorter, longer) in cases {
        let answer = infer(&router, "DetectPHI", text).await;
        let short_score = mention_entity(text, &answer, shorter)["Score"]
            .as_f64()
            .unwrap_or_default();
        let long_score = mention_entity(text, &answer, longer)["Score"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            long_score > short_score,
            "case {text:?}: the more fully written mention carries more confidence, got \
             {long_score} for {longer:?} against {short_score} for {shorter:?}"
        );
        for entity in entities(text, &answer) {
            let score = entity["Score"].as_f64().unwrap_or_default();
            assert!(
                score > 0.0 && score < 1.0,
                "case {text:?}: a confidence is in (0, 1), got {score} in {entity}"
            );
        }
    }
}

/// One sig row: the text, the medication, and the attribute type each part of the
/// prescription must be reported with.
type SigRow = (&'static str, &'static str, Vec<(&'static str, &'static str)>);

/// A measured amount written beside a medication is the amount of medication ORDERED —
/// the modelled `DOSAGE` — unless the same prescription also counts the dose units, in
/// which case the amount is how much active ingredient each of those units holds, the
/// modelled `STRENGTH`. An amount measured per a unit of volume or mass is a strength
/// too; one measured per a unit of TIME is the `RATE` it is given at.
///
/// The rows span all four of those cases and both the spaced and the joined way an amount
/// is written.
#[tokio::test]
async fn a_measured_amount_is_the_dose_ordered_unless_the_sig_counts_dose_units() {
    let router = router();
    let cases: Vec<SigRow> = vec![
        (
            "Metformin 500 mg orally twice daily.",
            "Metformin",
            vec![("DOSAGE", "500 mg")],
        ),
        (
            "Lisinopril 10mg daily.",
            "Lisinopril",
            vec![("DOSAGE", "10mg")],
        ),
        (
            "Take 2 tablets of Tylenol 325 mg every 6 hours.",
            "Tylenol",
            vec![("DOSAGE", "2 tablets"), ("STRENGTH", "325 mg")],
        ),
        (
            "Vancomycin 15 mg/kg IV every 12 hours.",
            "Vancomycin",
            vec![("STRENGTH", "15 mg/kg")],
        ),
        (
            "Heparin drip at 1000 units/hr IV.",
            "Heparin",
            vec![("RATE", "1000 units/hr")],
        ),
    ];
    for (text, mention, expected) in cases {
        let answer = infer(&router, "InferRxNorm", text).await;
        let entity = mention_entity(text, &answer, mention);
        for (attribute_type, written) in expected {
            let attribute = attributes(&entity)
                .into_iter()
                .find(|attribute| attribute["Text"].as_str() == Some(written))
                .unwrap_or_else(|| {
                    panic!("case {text:?}: expected the part {written:?}, got {entity}")
                });
            assert_eq!(
                attribute["Type"].as_str(),
                Some(attribute_type),
                "case {text:?}: {written:?} is reported as {attribute_type}, got {attribute}"
            );
            assert_offsets_delimit_text(text, text, &attribute);
        }
    }
}

/// A cue that places a mention in TIME reaches the mention it introduces and no further:
/// "history of" says when the mention it introduces was recorded, so a mention written
/// past other content in the same clause is not placed in the past by it.
///
/// The rows span both ontologies that model such a trait, both cues (the patient's
/// history and the future) and both polarities of the government rule.
#[tokio::test]
async fn a_time_cue_scopes_only_the_mention_it_introduces() {
    let router = router();
    // (operation, text, the mention, the trait the cue licenses on it, if any)
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "InferSNOMEDCT",
            "History of myocardial infarction.",
            "myocardial infarction",
            Some("PAST_HISTORY"),
        ),
        (
            "InferSNOMEDCT",
            "Prior appendectomy.",
            "appendectomy",
            Some("PAST_HISTORY"),
        ),
        // The qualifier of the condition stands between the cue and the condition name,
        // so the cue introduces the qualifier and not the name.
        (
            "InferSNOMEDCT",
            "Patient with a history of type 2 diabetes mellitus.",
            "diabetes mellitus",
            None,
        ),
        (
            "InferSNOMEDCT",
            "History of pneumonia and asthma.",
            "asthma",
            None,
        ),
        (
            "InferSNOMEDCT",
            "Scheduled for a colonoscopy.",
            "colonoscopy",
            Some("FUTURE"),
        ),
        (
            "InferRxNorm",
            "History of warfarin therapy.",
            "warfarin",
            Some("PAST_HISTORY"),
        ),
        (
            "InferRxNorm",
            "History of aspirin and warfarin.",
            "warfarin",
            None,
        ),
    ];
    for (operation, text, mention, expected) in cases {
        let case = format!("{operation} {text}");
        let answer = infer(&router, operation, text).await;
        let entity = mention_entity(&case, &answer, mention);
        let names = trait_names(&entity);
        for licensed in ["PAST_HISTORY", "FUTURE"] {
            let wanted = expected == Some(licensed);
            assert_eq!(
                names.contains(&licensed.to_string()),
                wanted,
                "case {case:?}: the trait {licensed} must be {}, got {names:?}",
                if wanted { "reported" } else { "absent" }
            );
        }
    }
}
