// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the hyperparameter map a training job KEEPS.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! `HyperParameterValue` is modelled `len 0..2500`, so an empty value is a legal thing to
//! send and the create accepts it — but SageMaker does not keep such an entry, so a key
//! supplied with the empty string reads back exactly like a key that was never supplied,
//! while every non-empty value round-trips unchanged. The tests assert that RULE over a
//! TABLE spanning the key count, which entries are empty, and the value's shape and
//! length, rather than one recorded call.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::Emulator;
use crate::training_job_admission::{image_specification, training_body};

/// The `HyperParameters` a describe reports, as a map — absent and empty both read as no
/// entry at all, which is the whole point of the rule under test.
fn reported(described: &Value) -> BTreeMap<String, String> {
    described
        .get("HyperParameters")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One hyperparameter entry as a case sends or expects it.
type Entry = (String, String);

/// One row: the case's name, the entries the create supplies (`None` sends no
/// `HyperParameters` member at all) and the entries the describe must report.
type Case = (&'static str, Option<Vec<Entry>>, Vec<Entry>);

/// RULE: the hyperparameters a training job keeps are exactly the supplied entries whose
/// value is NOT the empty string — an entry supplied empty is indistinguishable from a key
/// never supplied, and every non-empty value round-trips unchanged.
#[tokio::test]
async fn an_empty_valued_hyperparameter_reads_back_exactly_like_an_absent_one() {
    let emulator = Emulator::new();

    // The modelled maximum of `HyperParameterValue` (len 0..2500), so a value at the
    // published bound is kept like any other non-empty one.
    let longest = "9".repeat(2500);
    // Rows differ along the entry count, which entries carry the empty string, and the
    // shape and length of the values that are kept: a single space, a zero, JSON text,
    // and the longest value the model admits.
    let cases: Vec<Case> = vec![
        ("no-hyperparameters-member", None, vec![]),
        (
            "one-non-empty-entry",
            Some(vec![("eta".to_string(), "0.2".to_string())]),
            vec![("eta".to_string(), "0.2".to_string())],
        ),
        (
            "one-empty-entry",
            Some(vec![("empty".to_string(), String::new())]),
            vec![],
        ),
        (
            "every-entry-empty",
            Some(vec![
                ("first".to_string(), String::new()),
                ("second".to_string(), String::new()),
                ("third".to_string(), String::new()),
            ]),
            vec![],
        ),
        (
            "empty-beside-non-empty",
            Some(vec![
                ("empty".to_string(), String::new()),
                ("present".to_string(), "42".to_string()),
            ]),
            vec![("present".to_string(), "42".to_string())],
        ),
        (
            "values-that-are-not-empty-are-kept-whatever-their-shape",
            Some(vec![
                ("blank".to_string(), " ".to_string()),
                ("zero".to_string(), "0".to_string()),
                (
                    "document".to_string(),
                    json!({ "nested": [1, 2] }).to_string(),
                ),
                ("longest".to_string(), longest.clone()),
                ("dropped".to_string(), String::new()),
            ]),
            vec![
                ("blank".to_string(), " ".to_string()),
                (
                    "document".to_string(),
                    json!({ "nested": [1, 2] }).to_string(),
                ),
                ("longest".to_string(), longest.clone()),
                ("zero".to_string(), "0".to_string()),
            ],
        ),
        (
            "many-entries-half-of-them-empty",
            Some(
                (0..10)
                    .map(|index| {
                        let value = if index % 2 == 0 {
                            String::new()
                        } else {
                            format!("v{index}")
                        };
                        (format!("k{index}"), value)
                    })
                    .collect(),
            ),
            (0..10)
                .filter(|index| index % 2 == 1)
                .map(|index| (format!("k{index}"), format!("v{index}")))
                .collect(),
        ),
    ];

    for (index, (case, supplied, expected)) in cases.into_iter().enumerate() {
        let name = format!("hyperparameters-{index}");
        let mut body = training_body(&name, image_specification(), "s3://demo/output/");
        if let Some(entries) = &supplied {
            let map: BTreeMap<&str, &str> = entries
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            body["HyperParameters"] = json!(map);
        }

        let (status, created) = emulator.call("CreateTrainingJob", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an empty hyperparameter value is modelled, so the create is \
             accepted, got {status} {created:?}"
        );

        let described = emulator
            .ok(
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": name.clone() }),
            )
            .await;
        let expected: BTreeMap<String, String> = expected.into_iter().collect();
        assert_eq!(
            reported(&described),
            expected,
            "case {case:?}: the kept hyperparameters, got {described}"
        );

        // A second read answers the same thing: the entry was dropped where the job was
        // STORED, not by the reader.
        let reread = emulator
            .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
            .await;
        assert_eq!(
            reported(&reread),
            reported(&described),
            "case {case:?}: a re-read reports the same key set, got {reread}"
        );
    }
}

/// RULE: because an empty-valued entry is not kept, two jobs whose requests differ only
/// in whether they supplied a key with the empty string are INDISTINGUISHABLE through the
/// describe — the key set is the same whether the key was sent empty or never sent.
#[tokio::test]
async fn a_key_sent_empty_is_indistinguishable_from_one_never_sent() {
    let emulator = Emulator::new();

    // Rows differ along the entries the pair shares, so the comparison is not about one
    // particular map: none, one, and several.
    let shared: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("shares-nothing", vec![]),
        ("shares-one-entry", vec![("eta", "0.2")]),
        (
            "shares-several-entries",
            vec![("eta", "0.2"), ("max_depth", "6"), ("objective", "reg")],
        ),
    ];

    for (index, (case, entries)) in shared.into_iter().enumerate() {
        let mut with_empty: BTreeMap<&str, &str> = entries.iter().copied().collect();
        with_empty.insert("knob", "");
        let without: BTreeMap<&str, &str> = entries.iter().copied().collect();

        let mut reported_maps = Vec::new();
        for (suffix, map) in [("empty", &with_empty), ("absent", &without)] {
            let name = format!("pair-{index}-{suffix}");
            let mut body = training_body(&name, image_specification(), "s3://demo/output/");
            body["HyperParameters"] = json!(map);
            emulator.ok("CreateTrainingJob", &body).await;
            let described = emulator
                .ok("DescribeTrainingJob", &json!({ "TrainingJobName": name }))
                .await;
            reported_maps.push(reported(&described));
        }

        assert_eq!(
            reported_maps[0], reported_maps[1],
            "case {case:?}: a key sent empty must leave the same key set as one never \
             sent, got {reported_maps:?}"
        );
        assert!(
            !reported_maps[0].contains_key("knob"),
            "case {case:?}: the key sent empty must be absent, got {:?}",
            reported_maps[0]
        );
    }
}
