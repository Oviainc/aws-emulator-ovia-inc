// SPDX-License-Identifier: CC0-1.0

//! One rule: a refusal of a document-shape constraint `Rescore`'s handler enforces is
//! a REQUEST-INDEPENDENT sentence.
//!
//! Every constraint this service enforces in a handler — the ones the model cannot
//! express — is refused with a sentence that names where the offending value was and
//! what it had to satisfy, and renders nothing the caller supplied (the crate's own
//! policy, stated once in `src/state.rs` and reached through `clause` /
//! `constraint_report` or a constant message). `Rescore` decides three such
//! constraints about ONE document inside one loop — its body must be there in exactly
//! one of `Body` / `TokenizedBody`, its title may be given in at most one of `Title` /
//! `TokenizedTitle`, and the batch has a per-request ceiling — so all three, plus the
//! batch rule around them, must word their refusal the same way.
//!
//! The table varies what the rule generalises over: WHICH constraint failed, and the
//! `Document$Id` the offending document carried (its length, its character set, and an
//! id that is itself shaped like a refusal sentence), the position of the offending
//! document in the batch, and the batch size. The id is caller-supplied text of up to
//! 2048 characters (`Document$Id`, `length 1..2048`, from
//! `smithy_query(query="shape",shape="Document")`), so an id is exactly the kind of
//! value a refusal must not reflect back — while a SERVED request still names it in
//! `RescoreResultItem$DocumentId`, which the last test pins.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::ViolationKind;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::scaffolded;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, create_active_plan, emulator, error_code, error_message};

/// A `Rescore` document carrying `id`, the required `OriginalScore`, and `extra` on top.
fn document(id: &str, extra: Value) -> Value {
    let mut doc = json!({ "Id": id, "OriginalScore": 0.5 });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            doc[key] = value.clone();
        }
    }
    doc
}

/// A `Rescore` request against `plan_id` carrying `documents`.
fn rescore(plan_id: &str, documents: Value) -> Value {
    json!({
        "RescoreExecutionPlanId": plan_id,
        "SearchQuery": "intelligent ranking",
        "Documents": documents,
    })
}

/// The ids this suite submits: the shortest the model admits, one at its published
/// 2048-character ceiling, one built from the punctuation a search service's own
/// identifier may carry, a purely numeric one, one carrying format and regex
/// metacharacters, and one shaped like a refusal sentence — the adversarial row, since
/// an id that LOOKS like a message is still not part of one.
fn document_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model admits", "d".to_string()),
        ("a plain id", "d1".to_string()),
        // `Document$Id` is `length 1..2048` (smithy_query query="shape" shape="Document").
        ("an id at the published 2048-character bound", "i".repeat(2048)),
        (
            "an id carrying a search service's punctuation",
            "doc:42/v1+final@source=opensearch".to_string(),
        ),
        ("a numeric id", "0123456789".to_string()),
        (
            "an id carrying format and regex metacharacters",
            "{}%s^$.*|(?)".to_string(),
        ),
        (
            "an id shaped like a refusal sentence",
            "Document d9 provides both Body and TokenizedBody; provide one.".to_string(),
        ),
    ]
}

/// The members that make one document fail one of the three per-document rules, and the
/// label of the rule it fails.
fn failing_shapes() -> Vec<(&'static str, Value)> {
    vec![
        // Neither form of the body text.
        ("body required", json!({ "Title": "a title" })),
        // Both forms of the body text.
        (
            "body exclusivity",
            json!({ "Body": "a body", "TokenizedBody": ["a", "body"] }),
        ),
        // Both forms of the title, with the body given once so the body rule passes.
        (
            "title exclusivity",
            json!({ "Body": "a body", "Title": "a title", "TokenizedTitle": ["a", "title"] }),
        ),
    ]
}

/// One row of the table.
struct Case {
    case: String,
    /// The rule the offending document fails, which is what fixes the expected sentence.
    rule: &'static str,
    documents: Value,
    expected_message: String,
}

#[tokio::test]
async fn a_document_shape_refusal_states_the_rule_and_never_the_documents_id() {
    let router = emulator();
    let plan = create_active_plan(&router, "document-shape-plan").await;

    let expected: BTreeMap<&str, String> = BTreeMap::from([
        (
            "body required",
            state::DOCUMENT_BODY_REQUIRED_MESSAGE.to_string(),
        ),
        (
            "body exclusivity",
            state::document_member_choice_message("Body", "TokenizedBody"),
        ),
        (
            "title exclusivity",
            state::document_member_choice_message("Title", "TokenizedTitle"),
        ),
        (
            "batch ceiling",
            state::constraint_report(
                "documents",
                ViolationKind::MaxLength(state::MAX_DOCUMENTS_PER_RESCORE as i64),
            ),
        ),
    ]);

    let mut cases: Vec<Case> = Vec::new();
    for (id_label, id) in document_ids() {
        for (rule, shape) in failing_shapes() {
            // The offending document alone in the batch.
            cases.push(Case {
                case: format!("{rule} on {id_label}"),
                rule,
                documents: json!([document(&id, shape.clone())]),
                expected_message: expected[rule].clone(),
            });
            // The same document behind a well-formed one: the position of the offending
            // document in the batch changes nothing about the sentence.
            cases.push(Case {
                case: format!("{rule} on {id_label}, second in the batch"),
                rule,
                documents: json!([
                    document("leading", json!({ "Body": "a leading body" })),
                    document(&id, shape.clone()),
                ]),
                expected_message: expected[rule].clone(),
            });
        }
    }
    // The fourth document-shape rule: the batch ceiling, one document past it, with
    // every document well formed so the ceiling is what refuses the request.
    let over_ceiling: Vec<Value> = (0..=state::MAX_DOCUMENTS_PER_RESCORE)
        .map(|n| document(&format!("over-{n}"), json!({ "Body": "a body" })))
        .collect();
    cases.push(Case {
        case: "the batch ceiling, one document past it".to_string(),
        rule: "batch ceiling",
        documents: Value::Array(over_ceiling),
        expected_message: expected["batch ceiling"].clone(),
    });

    let mut worded: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for case in &cases {
        let ids: Vec<String> = case
            .documents
            .as_array()
            .expect("the documents of a row are a list")
            .iter()
            .map(|document| document["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        let (status, answer) = call(&router, "Rescore", rescore(&plan, case.documents.clone())).await;
        assert_eq!(
            status,
            scaffolded::CLIENT_ERROR_STATUS,
            "case {:?}: expected the request to be refused with {}, got {status} {answer}",
            case.case,
            scaffolded::CLIENT_ERROR_STATUS
        );
        assert_eq!(
            error_code(&answer),
            state::VALIDATION_EXCEPTION,
            "case {:?}: expected {}, got {status} {answer}",
            case.case,
            state::VALIDATION_EXCEPTION
        );
        let message = error_message(&answer);
        assert_eq!(
            message, case.expected_message,
            "case {:?}: expected the refusal to state {:?}, got {message:?}",
            case.case, case.expected_message
        );
        for id in &ids {
            // A one- or two-character id can occur inside any English sentence, so the
            // no-echo assertion is made for the ids that could only have come from the
            // request.
            if id.chars().count() < 4 {
                continue;
            }
            assert!(
                !message.contains(id.as_str()),
                "case {:?}: the refusal must render no document Id, yet it carries {id:?}: \
                 got {message:?}",
                case.case
            );
        }
        worded.entry(case.rule).or_default().insert(message);
    }

    // The whole point of the rule: one sentence per constraint, whatever the request
    // carried.
    for (rule, sentences) in &worded {
        assert_eq!(
            sentences.len(),
            1,
            "rule {rule:?}: one request-independent sentence per constraint, got {sentences:?}"
        );
    }
    assert_eq!(
        worded.len(),
        expected.len(),
        "every document-shape constraint must be exercised, got {:?}",
        worded.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_served_document_still_names_its_id_in_the_result() {
    let router = emulator();
    let plan = create_active_plan(&router, "served-document-plan").await;

    // The rule bounds what a REFUSAL may render; a document whose shape is admitted is
    // still reported under the id it was submitted with, in either body form.
    for (id_label, id) in document_ids() {
        for (shape_label, shape) in [
            ("a text body", json!({ "Body": "intelligent ranking" })),
            (
                "a pre-tokenized body",
                json!({ "TokenizedBody": ["intelligent", "ranking"] }),
            ),
        ] {
            let case = format!("{shape_label} on {id_label}");
            let (status, answer) = call(
                &router,
                "Rescore",
                rescore(&plan, json!([document(&id, shape)])),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a document carrying exactly one body form must be served, \
                 got {status} {answer}"
            );
            let reported = answer["ResultItems"][0]["DocumentId"].as_str();
            assert_eq!(
                reported,
                Some(id.as_str()),
                "case {case:?}: the result item must name the submitted document, got {answer}"
            );
        }
    }
}
