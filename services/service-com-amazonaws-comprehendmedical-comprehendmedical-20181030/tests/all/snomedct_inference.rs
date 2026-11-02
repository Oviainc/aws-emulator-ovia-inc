// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InferSNOMEDCT`: the synchronous ontology linking that places
//! clinical concepts in medical text and links each one to the SNOMED CT concepts it
//! could refer to.
//!
//! The rule under test is that the answer is computed from the SUBMITTED TEXT: every
//! reported entity is a stretch of that text, its offsets delimit exactly the characters
//! it reports, its category and type say what kind of concept it is, the context around
//! it decides its traits, the words beside it become its attributes, the response counts
//! the characters it was given, and a text that names no clinical concept reports no
//! entity. Each table below therefore spans a class of input — every entity category,
//! every entity type, each context cue, each attribute type, both ends of the accepted
//! length — rather than one document.
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
    let (status, answer) = call(router, "InferSNOMEDCT", &json!({"Text": text})).await;
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

/// The entity of `answer` written exactly as `mention`.
fn entity_of<'a>(case: &str, answer: &'a Value, found: &'a [Value], mention: &str) -> &'a Value {
    found
        .iter()
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

/// Every offset pair in `answer` delimits exactly the `Text` it is reported with, on
/// entities and on their attributes alike.
fn assert_offsets_delimit_their_text(case: &str, text: &str, answer: &Value) {
    for entity in entities(answer) {
        let mut spans = vec![entity.clone()];
        spans.extend(entity["Attributes"].as_array().cloned().unwrap_or_default());
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

/// Whether `code` is written the way a SNOMED CT concept identifier is. Source: the
/// SNOMED CT identifier specification — a concept id is a string of 6 to 18 digits, and
/// it never starts with a zero.
fn is_snomedct_concept_id(code: &str) -> bool {
    (6..=18).contains(&code.chars().count())
        && code.chars().all(|digit| digit.is_ascii_digit())
        && !code.starts_with('0')
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

/// One row of the catalog table: the text, the mention as written, the modelled category
/// and type the mention is reported with (the type is absent for a body structure), the
/// concept id a reader of the ontology reaches first, and the trait a bare mention
/// licenses.
type CatalogRow = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
);

/// A mention of a clinical concept is reported as the stretch of text that names it, in
/// the modelled category and type of that kind of concept, and linked to the SNOMED CT
/// concepts the mention could refer to — most likely first.
///
/// The rows span every member of the modelled category enum (`MEDICAL_CONDITION`,
/// `ANATOMY`, `TEST_TREATMENT_PROCEDURE`), every member of the type enum (`DX_NAME`,
/// `TEST_NAME`, `PROCEDURE_NAME`, `TREATMENT_NAME`), every base trait a condition can
/// license, and several written forms of one concept (full name, synonym,
/// abbreviation) — so a handler that answered for one document rather than computing
/// from the text fails here.
#[tokio::test]
async fn a_mention_is_linked_to_its_snomedct_concepts() {
    let router = router();
    let cases: Vec<CatalogRow> = vec![
        (
            "Patient has type 2 diabetes mellitus.",
            "diabetes mellitus",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "44054006",
            Some("DIAGNOSIS"),
        ),
        (
            "The patient was diagnosed with diabetes.",
            "diabetes",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "73211009",
            Some("DIAGNOSIS"),
        ),
        (
            "Known T1DM.",
            "T1DM",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "46635009",
            Some("DIAGNOSIS"),
        ),
        (
            "Admitted with congestive heart failure.",
            "congestive heart failure",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "42343007",
            Some("DIAGNOSIS"),
        ),
        (
            "Patient reports fever.",
            "fever",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "386661006",
            Some("SYMPTOM"),
        ),
        (
            "Pitting edema on examination.",
            "edema",
            "MEDICAL_CONDITION",
            Some("DX_NAME"),
            "267038008",
            Some("SIGN"),
        ),
        (
            "The abdomen was soft and nontender.",
            "abdomen",
            "ANATOMY",
            None,
            "113345001",
            None,
        ),
        (
            "Examination of the knee was unremarkable.",
            "knee",
            "ANATOMY",
            None,
            "72696002",
            None,
        ),
        (
            "We will obtain a complete blood count.",
            "complete blood count",
            "TEST_TREATMENT_PROCEDURE",
            Some("TEST_NAME"),
            "26604007",
            None,
        ),
        (
            "An EKG was obtained.",
            "EKG",
            "TEST_TREATMENT_PROCEDURE",
            Some("TEST_NAME"),
            "29303009",
            None,
        ),
        (
            "Patient underwent a colonoscopy.",
            "colonoscopy",
            "TEST_TREATMENT_PROCEDURE",
            Some("PROCEDURE_NAME"),
            "73761001",
            None,
        ),
        (
            "Status post coronary artery bypass graft.",
            "coronary artery bypass graft",
            "TEST_TREATMENT_PROCEDURE",
            Some("PROCEDURE_NAME"),
            "232717009",
            None,
        ),
        (
            "Continue oxygen therapy.",
            "oxygen therapy",
            "TEST_TREATMENT_PROCEDURE",
            Some("TREATMENT_NAME"),
            "57485005",
            None,
        ),
        (
            "Started on physical therapy.",
            "physical therapy",
            "TEST_TREATMENT_PROCEDURE",
            Some("TREATMENT_NAME"),
            "91251008",
            None,
        ),
    ];

    for (text, mention, category, entity_type, code, base_trait) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = entity_of(text, &answer, &found, mention);
        assert_eq!(
            entity["Category"].as_str(),
            Some(category),
            "case {text:?}: wrong category, got {entity}"
        );
        assert_eq!(
            entity["Type"].as_str(),
            entity_type,
            "case {text:?}: wrong type, got {entity}"
        );
        let score = entity["Score"].as_f64().unwrap_or_default();
        assert!(
            score > 0.0 && score <= 1.0,
            "case {text:?}: the confidence must be in (0, 1], got {score}"
        );
        let concepts = entity["SNOMEDCTConcepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            concepts.first().and_then(|concept| concept["Code"].as_str()),
            Some(code),
            "case {text:?}: expected the concept {code} first, got {entity}"
        );
        match base_trait {
            Some(name) => assert!(
                trait_names(entity).contains(&name.to_string()),
                "case {text:?}: expected the trait {name}, got {:?}",
                trait_names(entity)
            ),
            None => assert!(
                !trait_names(entity).contains(&"DIAGNOSIS".to_string()),
                "case {text:?}: this mention asserts nothing about the patient on its \
                 own, got {:?}",
                trait_names(entity)
            ),
        }
        assert_offsets_delimit_their_text(text, text, &answer);
    }
}

/// Every candidate an entity reports is a well-formed SNOMED CT concept id with its
/// description, the list is ordered by descending likelihood, and it is bounded.
#[tokio::test]
async fn concept_candidates_are_well_formed_ranked_and_bounded() {
    let router = router();
    let cases = [
        "Patient has diabetes, hypertension and asthma.",
        "Reports chest pain and shortness of breath.",
        "Assessment: congestive heart failure with chronic kidney disease.",
        "Plan: a chest X-ray and physical therapy.",
        "A biopsy of the skin was performed.",
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        assert!(
            !found.is_empty(),
            "case {text:?}: the text names clinical concepts, got {answer}"
        );
        for entity in found {
            let concepts = entity["SNOMEDCTConcepts"]
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
                    is_snomedct_concept_id(code),
                    "case {text:?}: {code:?} is not written as a SNOMED CT concept id"
                );
                assert!(
                    concept["Description"]
                        .as_str()
                        .is_some_and(|description| !description.is_empty()),
                    "case {text:?}: every candidate carries its description, got {concept}"
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

/// The response reports the number of CHARACTERS of the submitted text, the revision of
/// the ontology it linked against, and the version of the model it ran — whatever the
/// text contains.
///
/// The rows differ in length, in alphabet and in whether anything was found at all, and
/// they include both ends of the modelled 1..10000 length, so a count taken from
/// anything other than the submitted text (its bytes, its tokens, a recorded number)
/// fails here.
#[tokio::test]
async fn the_response_counts_the_characters_of_the_submitted_text() {
    let router = router();
    let cases: Vec<String> = vec![
        "a".to_string(),
        "Patient has diabetes.".to_string(),
        "Patiënt — naïve to treatment — reports fever.".to_string(),
        "\u{1f321} 39°C: fever and chills.".to_string(),
        "Follow up in two weeks with a repeat A1c.".to_string(),
        format!("{} diabetes", "a".repeat(10_000 - " diabetes".len())),
    ];
    for text in cases {
        let answer = infer(&router, &text).await;
        let case = &text[..text.len().min(40)];
        let expected = i64::try_from(text.chars().count()).expect("a character count fits an i64");
        assert_eq!(
            answer["Characters"]["OriginalTextCharacters"].as_i64(),
            Some(expected),
            "case {case:?}: expected {expected} characters, got {answer}"
        );
        for member in ["Edition", "Language", "VersionDate"] {
            assert!(
                answer["SNOMEDCTDetails"][member]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "case {case:?}: the SNOMED CT revision reports its {member}, got {answer}"
            );
        }
        let version = answer["ModelVersion"].as_str().unwrap_or_default();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty()
                        && part.chars().all(|digit| digit.is_ascii_digit())),
            "case {case:?}: the model version must be written n.n.n, got {version:?}"
        );
        assert!(
            answer["PaginationToken"].is_null(),
            "case {case:?}: nothing was truncated, so no page token is reported, got {answer}"
        );
    }
}

/// The words around a mention license its contextual traits, and a cue never reaches
/// past the clause it is written in.
///
/// The rows cover every trait the modelled `SNOMEDCTTraitName` enum holds beside the
/// three a bare mention licenses: negation, a family reference, a hypothetical, a hedge,
/// the patient's history and the future.
#[tokio::test]
async fn context_cues_license_the_traits_they_scope() {
    let router = router();
    // (text, the mention, the trait the context licenses)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("Patient denies chest pain.", "chest pain", "NEGATION"),
        ("No fever.", "fever", "NEGATION"),
        ("Negative for pneumonia.", "pneumonia", "NEGATION"),
        (
            "Family history of diabetes.",
            "diabetes",
            "PERTAINS_TO_FAMILY",
        ),
        ("Mother had a stroke.", "stroke", "PERTAINS_TO_FAMILY"),
        ("At risk of diabetes.", "diabetes", "HYPOTHETICAL"),
        ("Possible pneumonia.", "pneumonia", "LOW_CONFIDENCE"),
        ("Suspected influenza.", "influenza", "LOW_CONFIDENCE"),
        (
            "History of myocardial infarction.",
            "myocardial infarction",
            "PAST_HISTORY",
        ),
        ("Prior appendectomy.", "appendectomy", "PAST_HISTORY"),
        ("Will start dialysis.", "dialysis", "FUTURE"),
        ("Scheduled for a colonoscopy.", "colonoscopy", "FUTURE"),
    ];
    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = entity_of(text, &answer, &found, mention);
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
        let entity = entity_of(text, &answer, &found, mention);
        assert_eq!(
            trait_names(entity),
            vec!["DIAGNOSIS".to_string()],
            "case {text:?}: nothing in this clause hedges the mention, got {entity}"
        );
    }
}

/// One attribute row: the modelled attribute type, the stretch of text that carries it,
/// and the category that attribute is reported in.
type ExpectedAttribute = (&'static str, &'static str, &'static str);

/// One case of the attribute table: the text, the mention, and the attributes the
/// mention must carry.
type AttributeCase = (&'static str, &'static str, Vec<ExpectedAttribute>);

/// A word standing beside a mention is reported as an attribute of it: a modifier with
/// the attribute type its vocabulary carries, a body structure as the site of the
/// mention, and the number and unit after a test name as the value it measured.
///
/// The rows cover every member of the modelled `SNOMEDCTAttributeType` enum.
#[tokio::test]
async fn a_word_beside_a_mention_is_reported_as_its_attribute() {
    let router = router();
    let cases: Vec<AttributeCase> = vec![
        (
            "Severe left chest pain.",
            "chest pain",
            vec![
                ("QUALITY", "Severe", "MEDICAL_CONDITION"),
                ("DIRECTION", "left", "ANATOMY"),
            ],
        ),
        (
            "Chronic cough with wheezing.",
            "cough",
            vec![("ACUITY", "Chronic", "MEDICAL_CONDITION")],
        ),
        (
            "Left lower lung pneumonia.",
            "pneumonia",
            vec![
                ("DIRECTION", "Left", "ANATOMY"),
                ("DIRECTION", "lower", "ANATOMY"),
                ("SYSTEM_ORGAN_SITE", "lung", "ANATOMY"),
            ],
        ),
        (
            "A1c 7.2 percent today.",
            "A1c",
            vec![
                ("TEST_VALUE", "7.2", "TEST_TREATMENT_PROCEDURE"),
                ("TEST_UNIT", "percent", "TEST_TREATMENT_PROCEDURE"),
            ],
        ),
        (
            "Blood pressure 140/90 mmHg on arrival.",
            "Blood pressure",
            vec![
                ("TEST_VALUE", "140/90", "TEST_TREATMENT_PROCEDURE"),
                ("TEST_UNIT", "mmHg", "TEST_TREATMENT_PROCEDURE"),
            ],
        ),
        (
            "Heart rate 110 bpm.",
            "Heart rate",
            vec![
                ("TEST_VALUE", "110", "TEST_TREATMENT_PROCEDURE"),
                ("TEST_UNIT", "bpm", "TEST_TREATMENT_PROCEDURE"),
            ],
        ),
    ];

    for (text, mention, expected) in cases {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = entity_of(text, &answer, &found, mention);
        let attributes = entity["Attributes"].as_array().cloned().unwrap_or_default();
        for (attribute_type, written, category) in expected {
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
                attribute["Category"].as_str(),
                Some(category),
                "case {text:?}: wrong category for {attribute_type}, got {attribute}"
            );
            // Every member of the attribute-type enum is also a member of the
            // relationship-type enum, so an attribute relates to its entity as its own
            // type names.
            assert_eq!(
                attribute["RelationshipType"].as_str(),
                Some(attribute_type),
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

/// A body structure the text names as the site of a mention is reported as that
/// mention's attribute, linked to the SNOMED CT concept of the structure; one that
/// belongs to no mention is an entity of its own.
#[tokio::test]
async fn a_body_structure_is_the_site_of_the_mention_it_stands_beside() {
    let router = router();
    // (text, the mention the site belongs to, the site as written, its concept id)
    let attached: Vec<(&str, &str, &str, &str)> = vec![
        ("Left lung pneumonia.", "pneumonia", "lung", "39607008"),
        ("Rash on the skin of the arm.", "Rash", "skin", "39937001"),
        ("Biopsy of the liver.", "Biopsy", "liver", "10200004"),
    ];
    for (text, mention, site, code) in attached {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = entity_of(text, &answer, &found, mention);
        let attributes = entity["Attributes"].as_array().cloned().unwrap_or_default();
        let attribute = attributes
            .iter()
            .find(|attribute| {
                attribute["Type"].as_str() == Some("SYSTEM_ORGAN_SITE")
                    && attribute["Text"].as_str() == Some(site)
            })
            .unwrap_or_else(|| {
                panic!("case {text:?}: expected the site {site:?}, got {entity}")
            });
        assert_eq!(
            attribute["SNOMEDCTConcepts"][0]["Code"].as_str(),
            Some(code),
            "case {text:?}: the site is linked to its own concept, got {attribute}"
        );
    }

    // A structure no clinical mention stands beside is an entity in the ANATOMY
    // category, with no type: the modelled type enum names no anatomical type.
    for (text, mention) in [
        ("The abdomen is soft.", "abdomen"),
        ("Inspection of the throat.", "throat"),
    ] {
        let answer = infer(&router, text).await;
        let found = entities(&answer);
        let entity = entity_of(text, &answer, &found, mention);
        assert_eq!(
            entity["Category"].as_str(),
            Some("ANATOMY"),
            "case {text:?}: a lone body structure is an ANATOMY entity, got {entity}"
        );
        assert!(
            entity["Type"].is_null(),
            "case {text:?}: the type enum names no anatomical type, got {entity}"
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
            "case {text:?}: the text names a clinical concept, got {answer}"
        );
        assert_offsets_delimit_their_text(text, text, &answer);
    }
}

/// A text that names no clinical concept reports no entity.
#[tokio::test]
async fn a_text_with_no_clinical_concept_reports_no_entity() {
    let router = router();
    let cases = [
        "The weather is fine today.",
        "Patient is 44 years old and lives alone.",
        "a",
        &"a".repeat(10_000),
    ];
    for text in cases {
        let answer = infer(&router, text).await;
        assert_eq!(
            entities(&answer),
            Vec::<Value>::new(),
            "case {:?}: nothing in this text names a clinical concept, got {answer}",
            &text[..text.len().min(40)]
        );
    }
}

/// Several mentions in one text are reported in the order they are written, and every
/// `Id` in the response — entities and attributes alike — is unique and increasing, as
/// the modelled member documents.
#[tokio::test]
async fn mentions_are_ordered_and_every_id_is_unique_and_increasing() {
    let router = router();
    let text = "Patient with chronic asthma reports severe chest pain. \
                History of type 2 diabetes mellitus treated with dialysis, \
                and an EKG was obtained.";
    let answer = infer(&router, text).await;
    let found = entities(&answer);
    assert!(
        found.len() >= 5,
        "the text names five clinical concepts, got {} in {answer}",
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
    let text = "Patient denies chest pain but reports chronic cough; A1c 7.2 percent.";
    let first = infer(&router, text).await;
    let _ = infer(&router, "Known hypertension on lisinopril.").await;
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
        let (status, answer) = call(&router, "InferSNOMEDCT", &json!({"Text": text})).await;
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
