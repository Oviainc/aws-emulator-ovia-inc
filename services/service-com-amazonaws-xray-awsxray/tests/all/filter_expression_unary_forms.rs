// SPDX-License-Identifier: CC0-1.0

//! The UNARY forms of the filter-expression language, end to end over the wire
//! (the harness in `xray_wire`).
//!
//! Two rules, each asserted over a table whose rows differ along what the rule
//! generalises over rather than along the one expression a client happened to
//! send:
//!
//! * an annotation key is written either after a dot or as a SUBSCRIPT
//!   (`annotation[<key>]`), both naming the one field, and a subscript with no
//!   comparison after it is the unary predicate "this annotation is SET" — true
//!   for a trace that recorded the key whatever value it holds, false for one
//!   that never recorded it;
//! * `!` is the language's negation operator, the symbol spelling of `NOT`, and
//!   it applies to EVERY term the grammar admits — a boolean keyword, a
//!   comparison, a complex keyword with or without a block, a parenthesised
//!   group and an annotation predicate — so `!X` selects exactly the traces `X`
//!   does not, and `!=` is still the inequality comparison and not a negated
//!   `=`.
//!
//! Both rules are the grammar's, so each form is asserted on the surface that
//! EVALUATES an expression (`GetTraceSummaries`) and on the one that only STORES
//! it (`CreateGroup`), which read the language through the one parser.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::xray_wire::{array, Xray};

/// The window every query below covers, chosen to hold every seeded trace.
const WINDOW: (f64, f64) = (900.0, 2_000.0);

/// One seeded trace: the label a row names it by, its id, and its documents.
struct Seeded {
    label: &'static str,
    id: String,
    documents: Vec<String>,
}

/// A 35-character X-Ray trace id, distinct per `n`.
fn id(n: u32) -> String {
    format!("1-{n:08x}-{n:024x}")
}

/// The corpus every row runs against.
///
/// The traces differ along what the two rules generalise over: which annotation
/// keys they carry and of which JSON value type (string, number, boolean —
/// `false` among them, so presence is not read as truth), whether a key sits on
/// the segment or deep inside its subsegments, whether they carry annotations at
/// all, which services and edges they ran through, and whether they faulted.
fn corpus() -> Vec<Seeded> {
    let beta = id(41);
    let prod = id(42);
    let deep = id(43);
    let bare = id(44);
    let silent = id(45);
    vec![
        Seeded {
            label: "beta",
            id: beta.clone(),
            documents: vec![json!({
                "trace_id": beta,
                "id": "0000000000000041",
                "name": "frontend",
                "start_time": 1_000.0_f64,
                "end_time": 1_000.5_f64,
                "annotations": {"probe_run": "m1", "stage": "beta", "attempt": 3, "cached": false},
                "subsegments": [{
                    "id": "0000000000000141",
                    "name": "ledger",
                    "namespace": "remote",
                    "start_time": 1_000.1_f64,
                    "end_time": 1_000.4_f64,
                }],
            })
            .to_string()],
        },
        Seeded {
            label: "prod",
            id: prod.clone(),
            documents: vec![json!({
                "trace_id": prod,
                "id": "0000000000000042",
                "name": "shop",
                "start_time": 1_100.0_f64,
                "end_time": 1_100.5_f64,
                "fault": true,
                "annotations": {"probe_run": "m1", "stage": "prod"},
            })
            .to_string()],
        },
        Seeded {
            label: "deep",
            id: deep.clone(),
            documents: vec![json!({
                "trace_id": deep,
                "id": "0000000000000043",
                "name": "worker",
                "start_time": 1_200.0_f64,
                "end_time": 1_200.5_f64,
                "annotations": {"probe_run": "m1"},
                "subsegments": [{
                    "id": "0000000000000143",
                    "name": "inner",
                    "start_time": 1_200.1_f64,
                    "end_time": 1_200.4_f64,
                    "subsegments": [{
                        "id": "0000000000000243",
                        "name": "innermost",
                        "start_time": 1_200.2_f64,
                        "end_time": 1_200.3_f64,
                        "annotations": {"deep_marker": "yes"},
                    }],
                }],
            })
            .to_string()],
        },
        Seeded {
            label: "bare",
            id: bare.clone(),
            documents: vec![json!({
                "trace_id": bare,
                "id": "0000000000000044",
                "name": "lonely",
                "start_time": 1_300.0_f64,
                "end_time": 1_300.5_f64,
                "annotations": {"probe_run": "m1", "and": "kept", "true": "kept", "my key": "spaced"},
            })
            .to_string()],
        },
        Seeded {
            label: "silent",
            id: silent.clone(),
            documents: vec![json!({
                "trace_id": silent,
                "id": "0000000000000045",
                "name": "quiet",
                "start_time": 1_400.0_f64,
                "end_time": 1_400.5_f64,
            })
            .to_string()],
        },
    ]
}

/// Ingest the corpus through `PutTraceSegments`, so every row below is answered
/// from stored segment documents.
async fn seeded() -> (Xray, Vec<Seeded>) {
    let xray = Xray::new();
    let corpus = corpus();
    for trace in &corpus {
        let put = xray.put_segments(&trace.documents).await;
        assert_eq!(
            put.status, 200,
            "seeding {:?} must succeed, got {put:?}",
            trace.label
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "seeding {:?} must store every document, got {:?}",
            trace.label,
            put.get("UnprocessedTraceSegments")
        );
    }
    (xray, corpus)
}

/// Which corpus traces `expression` selects, walking every page of the
/// selection: the rule is about WHICH traces an expression selects, not about
/// where a page breaks.
async fn selects(xray: &Xray, corpus: &[Seeded], expression: &str) -> BTreeSet<&'static str> {
    let mut selected: BTreeSet<&'static str> = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let got = xray
            .query_summaries(WINDOW.0, WINDOW.1, Some(expression), token.as_deref())
            .await;
        assert_eq!(
            got.status, 200,
            "case {expression:?}: an expression the language admits is served, got {got:?}"
        );
        selected.extend(
            array(&got.body, "TraceSummaries")
                .iter()
                .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                .map(|found| {
                    corpus
                        .iter()
                        .find(|trace| trace.id == found)
                        .map_or("unknown", |trace| trace.label)
                }),
        );
        match got.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    selected
}

/// The labels in `labels`, as a selection to compare against.
fn set(labels: &[&'static str]) -> BTreeSet<&'static str> {
    labels.iter().copied().collect()
}

// =============================================================================
// 1. The subscripted annotation predicate
// =============================================================================

#[tokio::test]
async fn a_subscripted_annotation_predicate_asks_whether_the_annotation_is_set() {
    let (xray, corpus) = seeded().await;

    // Rows differ along the JSON type of the value behind the key (string,
    // number, boolean `false`), where the annotation was recorded (on the
    // segment, deep inside its subsegments), whether any trace carries the key
    // at all, whether the subscript stands alone or carries a comparison, and
    // how it composes with the rest of an expression.
    let rows: Vec<(&str, Vec<&'static str>)> = vec![
        // A key every annotated trace recorded: set for each of them, and not
        // for the trace that recorded no annotations.
        (
            "annotation[probe_run]",
            vec!["beta", "prod", "deep", "bare"],
        ),
        ("annotation[stage]", vec!["beta", "prod"]),
        // A numeric value: the predicate is about the KEY, not the value.
        ("annotation[attempt]", vec!["beta"]),
        // A `false` value is still a value the trace recorded.
        ("annotation[cached]", vec!["beta"]),
        // Indexed wherever it was recorded, subsegments included.
        ("annotation[deep_marker]", vec!["deep"]),
        ("annotation[absent_key]", vec![]),
        // The key is the caller's data, not a name of this language, so a key
        // that spells a word of the language is read as the key it is.
        ("annotation[and]", vec!["bare"]),
        ("annotation[true]", vec!["bare"]),
        // Quoted, the subscript holds whatever the quotes do — which is how a
        // key no bare word can spell is written, and it names the same field a
        // bare word does.
        ("annotation[\"my key\"]", vec!["bare"]),
        ("annotation[\"stage\"]", vec!["beta", "prod"]),
        ("!annotation[\"my key\"]", vec!["beta", "prod", "deep", "silent"]),
        // Negated, the predicate asks whether the annotation is NOT set.
        ("!annotation[stage]", vec!["deep", "bare", "silent"]),
        ("NOT annotation[stage]", vec!["deep", "bare", "silent"]),
        ("! annotation[stage]", vec!["deep", "bare", "silent"]),
        ("!annotation[absent_key]", vec!["beta", "prod", "deep", "bare", "silent"]),
        // Composed with the other predicates, as a scoping conjunction.
        ("annotation[probe_run] AND annotation[stage]", vec!["beta", "prod"]),
        (
            "annotation.probe_run = \"m1\" AND annotation[attempt]",
            vec!["beta"],
        ),
        (
            "annotation.probe_run = \"m1\" AND !annotation[stage]",
            vec!["deep", "bare"],
        ),
        (
            "annotation[stage] OR annotation[deep_marker]",
            vec!["beta", "prod", "deep"],
        ),
        (
            "!(annotation[stage] OR annotation[deep_marker])",
            vec!["bare", "silent"],
        ),
        // A subscript that DOES carry a comparison names the same field the
        // dotted form does, so it reads exactly as the dotted form.
        ("annotation[stage] = \"prod\"", vec!["prod"]),
        ("annotation.stage = \"prod\"", vec!["prod"]),
        ("annotation[attempt] > 2", vec!["beta"]),
        ("annotation[attempt] > 3", vec![]),
        ("annotation[cached] = false", vec!["beta"]),
        // `!=` is the inequality comparison, not a negated `=`: no trace holds
        // an `attempt` other than 3, so nothing is selected — where the negated
        // predicate `!annotation[attempt]` selects every trace without the key.
        ("annotation[attempt] != 3", vec![]),
        (
            "!annotation[attempt]",
            vec!["prod", "deep", "bare", "silent"],
        ),
    ];

    for (expression, expected) in rows {
        let selected = selects(&xray, &corpus, expression).await;
        assert_eq!(
            selected,
            set(&expected),
            "case {expression:?}: expected {:?}, got {selected:?}",
            set(&expected)
        );
    }
}

// =============================================================================
// 2. The `!` negation operator over every term the grammar admits
// =============================================================================

#[tokio::test]
async fn the_negation_operator_applies_to_every_term_and_selects_the_complement() {
    let (xray, corpus) = seeded().await;
    let every: BTreeSet<&'static str> = corpus.iter().map(|trace| trace.label).collect();

    // Rows: one per class of term the grammar admits, because the operator
    // applies to the TERM and not to a subset of the keywords — a boolean
    // keyword, a numeric and a string comparison, a complex keyword with
    // arguments, one with a block, an edge, a parenthesised group (which is how
    // a whole composition is negated, since `!` binds to one term) and an
    // annotation predicate in both of its forms.
    let terms = [
        "fault",
        "fault = true",
        "duration > 0.4",
        "http.status = 200",
        "service(\"shop\")",
        "service(\"ledger\")",
        "service(\"nobody\")",
        "service(\"shop\") { fault }",
        "edge(\"frontend\", \"ledger\")",
        "(annotation[probe_run] AND annotation[stage])",
        "(fault OR annotation[stage])",
        "annotation.stage = \"prod\"",
        "annotation[probe_run]",
    ];

    for term in terms {
        let plain = selects(&xray, &corpus, term).await;
        let negated = selects(&xray, &corpus, &format!("!{term}")).await;
        let worded = selects(&xray, &corpus, &format!("NOT {term}")).await;
        let complement: BTreeSet<&'static str> = every.difference(&plain).copied().collect();
        assert_eq!(
            negated, complement,
            "case {term:?}: `!` must select the complement {complement:?} of {plain:?}, got {negated:?}"
        );
        assert_eq!(
            negated, worded,
            "case {term:?}: `!` and `NOT` are the one operator, got {negated:?} and {worded:?}"
        );
    }

    // The composition the issue's query is: a scoping predicate AND a negated
    // complex keyword, which excludes the traces that ran through that service.
    let composed = selects(
        &xray,
        &corpus,
        "annotation.probe_run = \"m1\" AND !service(\"shop\")",
    )
    .await;
    assert_eq!(
        composed,
        set(&["beta", "deep", "bare"]),
        "a negated complex keyword excludes the traces through that service, got {composed:?}"
    );
    let composed_edge = selects(
        &xray,
        &corpus,
        "annotation.probe_run = \"m1\" AND !edge(\"frontend\", \"ledger\")",
    )
    .await;
    assert_eq!(
        composed_edge,
        set(&["prod", "deep", "bare"]),
        "a negated edge excludes the traces that carried it, got {composed_edge:?}"
    );
}

// =============================================================================
// 3. The same grammar on the surface that only STORES an expression
// =============================================================================

#[tokio::test]
async fn the_unary_forms_are_read_by_the_storing_surface_too() {
    let xray = Xray::new();

    // Rows: the forms this language admits, each stored and read back through
    // the implemented reader.
    let accepted = [
        "annotation[probe_run]",
        "!annotation[probe_run]",
        "NOT annotation[probe_run]",
        "annotation[stage] = \"prod\"",
        "annotation.probe_run = \"m1\" AND annotation[stage]",
        "annotation.probe_run = \"m1\" AND !service(\"shop\")",
        "!edge(\"frontend\", \"ledger\")",
        "!(fault OR annotation[stage])",
        "!fault AND !annotation[stage]",
        "annotation[attempt] != 3",
    ];
    for (index, expression) in accepted.iter().enumerate() {
        let name = format!("unary-{index:02}");
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": name, "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {expression:?}: a form the language admits is stored, got {created:?}"
        );
        let read = xray
            .call("/GetGroup", &json!({"GroupName": name}))
            .await;
        assert_eq!(
            read.get("Group")
                .get("FilterExpression")
                .and_then(Value::as_str),
            Some(*expression),
            "case {expression:?}: the stored expression must read back, got {read:?}"
        );
    }

    // What the subscript does NOT admit, each refused with the parser's location
    // template: the rows differ along where the parse gives up — an empty
    // subscript and an empty quoted key (neither names a field), a subscript
    // nothing closes, a subscript on a keyword that takes no caller-chosen key,
    // and a negation with no term after it.
    let refused: [(&str, &str, usize, &str); 5] = [
        ("an empty subscript", "annotation[]", 12, "]"),
        (
            "a subscript whose quoted key is empty",
            "annotation[\"\"]",
            1,
            "annotation[",
        ),
        (
            "a subscript nothing closes",
            "annotation[stage",
            17,
            "annotation[stage",
        ),
        ("a subscript on a boolean keyword", "fault[x]", 6, "["),
        ("a negation with nothing to negate", "!", 2, "!"),
    ];
    for (case, expression, position, symbol) in refused {
        let expected =
            format!("Error Location: line 1, position {position} : Invalid input symbol: '{symbol}'");
        for (surface, path, body) in [
            (
                "CreateGroup",
                "/CreateGroup",
                json!({"GroupName": "refused", "FilterExpression": expression}),
            ),
            (
                "GetTraceSummaries",
                "/TraceSummaries",
                json!({
                    "StartTime": WINDOW.0,
                    "EndTime": WINDOW.1,
                    "FilterExpression": expression,
                }),
            ),
        ] {
            let answer = xray.call(path, &body).await;
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {surface}: expected 400 InvalidRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                Some(expected.as_str()),
                "case {case:?} on {surface}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}
