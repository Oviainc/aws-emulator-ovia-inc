// SPDX-License-Identifier: CC0-1.0

//! The identity of a `StartPatientInsightsJob` REQUEST: how the token that identifies a
//! request is told from the same token presented with DIFFERENT job parameters.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router's own dispatch — and asserts the RULE rather than one recorded
//! body: two parameter sets the model admits as different are different requests, so a
//! `clientToken` already bound to a job started with one of them answers the modelled
//! `ConflictException` when it is presented with the other, and only a request carrying
//! the very same parameters replays the first job.
//!
//! The rows generalise over what makes two parameter sets different where the digest
//! previously could not see it: whether an optional member was supplied at all, whether
//! the list it carries is empty, how many elements it carries, in which order, and which
//! section of `inputDataConfig` a value sits in. The empty list is one of the values the
//! member admits: `S3Sources` declares `@length {max: 10}` and no minimum
//! (`smithy_query(query="shape",shape="InputDataConfig",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{get_job, minimal_body, start_path};
use crate::patient_insights_job_terminal_output::domain_of;

/// `S3Sources`: `@length {max: 10}`, with no minimum.
const MAX_S3_SOURCES: usize = 10;

/// An `s3Sources` list of `n` distinct sources, each a valid `S3Uri`.
fn sources(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "uri": format!("s3://clinic-input/chart-{index}.json") }))
            .collect(),
    )
}

/// A `StartPatientInsightsJob` body whose `inputDataConfig` is `config` and whose
/// `clientToken` is `token`; every other member is the minimal required set, so a pair of
/// bodies differs in exactly the parameters the row varies.
fn body_with(config: &Value, token: &str) -> Value {
    let mut body = minimal_body();
    body["inputDataConfig"] = config.clone();
    body["clientToken"] = json!(token);
    body
}

/// The pairs of `inputDataConfig` the model admits as DIFFERENT: one row per way two of
/// them can differ without differing in any other member. Neither spelling of a row may
/// be read as the other, in either order.
fn differing_pairs() -> Vec<(String, Value, Value)> {
    let endpoint = "https://fhir.example.org/r4";
    // A value that is a valid `S3Uri` and a valid `oauthToken` at once, so the row below
    // can present it in each section in turn.
    let shared = "s3://clinic-input/chart-0.json";
    vec![
        (
            "an absent s3Sources against an empty one".to_string(),
            json!({}),
            json!({ "s3Sources": [] }),
        ),
        (
            "an empty s3Sources against a single source".to_string(),
            json!({ "s3Sources": [] }),
            json!({ "s3Sources": sources(1) }),
        ),
        (
            "one source against two".to_string(),
            json!({ "s3Sources": sources(1) }),
            json!({ "s3Sources": sources(2) }),
        ),
        (
            format!("{} sources against the modelled maximum of {MAX_S3_SOURCES}", MAX_S3_SOURCES - 1),
            json!({ "s3Sources": sources(MAX_S3_SOURCES - 1) }),
            json!({ "s3Sources": sources(MAX_S3_SOURCES) }),
        ),
        (
            "the same two sources in the other order".to_string(),
            json!({ "s3Sources": [
                { "uri": "s3://clinic-input/chart-0.json" },
                { "uri": "s3://clinic-input/chart-1.json" },
            ] }),
            json!({ "s3Sources": [
                { "uri": "s3://clinic-input/chart-1.json" },
                { "uri": "s3://clinic-input/chart-0.json" },
            ] }),
        ),
        (
            "one source against a different source".to_string(),
            json!({ "s3Sources": [{ "uri": "s3://clinic-input/chart-0.json" }] }),
            json!({ "s3Sources": [{ "uri": "s3://clinic-input/chart-9.json" }] }),
        ),
        (
            "an absent s3Sources against an empty one, beside a configured FHIR server"
                .to_string(),
            json!({ "fhirServer": { "fhirEndpoint": endpoint } }),
            json!({ "fhirServer": { "fhirEndpoint": endpoint }, "s3Sources": [] }),
        ),
        (
            "an absent fhirServer against a configured one".to_string(),
            json!({ "s3Sources": sources(1) }),
            json!({ "fhirServer": { "fhirEndpoint": endpoint }, "s3Sources": sources(1) }),
        ),
        (
            "one value carried by the FHIR server against the same value as a source"
                .to_string(),
            json!({ "fhirServer": { "fhirEndpoint": endpoint, "oauthToken": shared } }),
            json!({
                "fhirServer": { "fhirEndpoint": endpoint },
                "s3Sources": [{ "uri": shared }],
            }),
        ),
        (
            "an input config with neither source against one with both".to_string(),
            json!({}),
            json!({
                "fhirServer": { "fhirEndpoint": endpoint, "oauthToken": "token-value" },
                "s3Sources": sources(2),
            }),
        ),
    ]
}

#[tokio::test]
async fn a_token_bound_to_one_parameter_set_refuses_every_other_one() {
    let emulator = Emulator::new("us-west-2");
    let domain_id = domain_of(&emulator, "clinic-request-identity").await;

    for (index, (case, left, right)) in differing_pairs().into_iter().enumerate() {
        // Each ORDER on its own token, so neither spelling is privileged: whichever of
        // the two started the job, the other one is a conflict against it.
        for (order, first, second) in [
            (0, left.clone(), right.clone()),
            (1, right.clone(), left.clone()),
        ] {
            let case = format!(
                "{case}, {} spelling first",
                if order == 0 { "first" } else { "second" }
            );
            let case = case.as_str();
            let token = format!("token-{index}-{order}");
            let (status, started) = emulator
                .send(
                    "POST",
                    &start_path(&domain_id),
                    Some(&body_with(&first, &token)),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first start must answer 200, got {status} with {started}"
            );
            let job_id = string_at(&started, "jobId");

            let (status, refused) = emulator
                .send(
                    "POST",
                    &start_path(&domain_id),
                    Some(&body_with(&second, &token)),
                )
                .await;
            assert_eq!(
                status, 409,
                "case {case:?}: the same token with different parameters must answer 409, \
                 got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {case:?}: got {refused}"
            );
            let detail = string_at(&refused, "Message");
            assert!(
                detail.contains(&token) && detail.contains(&job_id),
                "case {case:?}: the detail must name the token and the job it is bound to \
                 ({token}, {job_id}), got {detail:?}"
            );

            // The refused request changed nothing: the job the first call created still
            // reports the parameters IT was started with.
            let fetched = get_job(&emulator, case, &domain_id, &job_id).await;
            assert_eq!(
                fetched.get("inputDataConfig").cloned().unwrap_or(Value::Null),
                first,
                "case {case:?}: the stored job must still carry the parameters it was \
                 started with, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn a_retry_of_the_very_same_parameters_replays_the_first_job() {
    let emulator = Emulator::new("eu-west-1");
    let domain_id = domain_of(&emulator, "clinic-request-replay").await;

    // Every spelling the table names, each presented three times under one token of its
    // own: a repeat is a repeat, so the second and third calls answer the first job
    // rather than minting another or refusing it.
    let mut configs: Vec<(String, Value)> = Vec::new();
    for (case, left, right) in differing_pairs() {
        configs.push((format!("{case} (first spelling)"), left));
        configs.push((format!("{case} (second spelling)"), right));
    }

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, (case, config)) in configs.into_iter().enumerate() {
        let case = case.as_str();
        let token = format!("replay-{index}");
        let body = body_with(&config, &token);
        let (status, first) = emulator
            .send("POST", &start_path(&domain_id), Some(&body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start must answer 200, got {status} with {first}"
        );
        let job_id = string_at(&first, "jobId");
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: every distinct token must start its own job, got the reused \
             {job_id:?}"
        );

        for attempt in ["second call", "third call"] {
            let (status, retried) = emulator
                .send("POST", &start_path(&domain_id), Some(&body))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {attempt}: a retry of the same request must answer 200, \
                 got {status} with {retried}"
            );
            assert_eq!(
                retried, first,
                "case {case:?} / {attempt}: a retry must answer the first job, got {retried}"
            );
        }

        // What the start persisted is what the reader projects: an absent member stays
        // absent and a supplied empty list reads back as an empty list, which is what
        // makes the two spellings observably different records.
        let fetched = get_job(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            fetched.get("inputDataConfig").cloned().unwrap_or(Value::Null),
            config,
            "case {case:?}: inputDataConfig must read back exactly as it was persisted, \
             got {fetched}"
        );
    }
}
