// SPDX-License-Identifier: CC0-1.0

//! How the MSK configuration family refuses an `Arn` it resolves to no
//! configuration, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! The RULE this suite asserts, over a table rather than one recorded request:
//! every operation that names a configuration by `Arn` refuses an `Arn` that
//! resolves to no configuration of the endpoint's region with HTTP 400
//! `BadRequestException`, naming `Arn` as the parameter at fault and carrying
//! the service's ONE fixed sentence for that condition — a sentence with no
//! variable parts, so no value from the request or the endpoint appears in it.
//! The rows span both dimensions the rule generalises over: which operation
//! answers (the two readers, the revision listing, the update, the delete) and
//! what makes the `Arn` resolve to nothing (never minted, deleted, held by
//! another region, another account, another service namespace, another resource
//! kind, or not an ARN at all).

use serde_json::{json, Value};

// The router harness, the signing region, the account id, the URI encoder and
// the configuration family's shared refusal assertion every suite of this crate
// shares — the assertion lives there so this suite and the deletion suite cannot
// drift apart on what that refusal is.
use super::kafka_harness::{
    assert_configuration_arn_resolves_to_nothing, pct, ConfigurationRequest, Emulator, Reply,
    ACCOUNT, REGION,
};

/// Every operation that takes a configuration `Arn`, as
/// `(name, method, uri suffix, body)` — the whole family the rule covers, each
/// bound as the model binds it under `/v1/configurations/{Arn}`.
fn operations() -> Vec<(&'static str, &'static str, &'static str, Option<Value>)> {
    vec![
        ("DescribeConfiguration", "GET", "", None),
        ("ListConfigurationRevisions", "GET", "/revisions", None),
        ("DescribeConfigurationRevision", "GET", "/revisions/1", None),
        // The same reader with a revision number that is illegal in itself: the
        // configuration is resolved first, so an `Arn` that names nothing is
        // what the refusal reports even when another member is wrong too.
        (
            "DescribeConfigurationRevision/illegal-revision",
            "GET",
            "/revisions/0",
            None,
        ),
        (
            "UpdateConfiguration",
            "PUT",
            "",
            Some(json!({
                "serverProperties": ::aws_smithy_types::base64::encode(b"num.partitions=7\n"),
            })),
        ),
        ("DeleteConfiguration", "DELETE", "", None),
    ]
}

/// Call `operation` on `arn` at `region`'s endpoint.
async fn call(
    emulator: &Emulator,
    region: &str,
    operation: &(&str, &str, &str, Option<Value>),
    arn: &str,
) -> Reply {
    let (_, method, suffix, body) = operation;
    emulator
        .call_in(
            region,
            method,
            &format!("/v1/configurations/{}{suffix}", pct(arn)),
            body.clone(),
        )
        .await
}

/// RULE: every operation of the configuration family refuses an `Arn` that
/// resolves to no configuration of the endpoint's region with HTTP 400
/// `BadRequestException` and the service's one fixed sentence for that
/// condition, naming `Arn` as the parameter at fault — however the `Arn` came to
/// resolve to nothing, and whichever operation answers.
#[tokio::test]
async fn every_configuration_operation_refuses_an_unresolvable_arn_alike() {
    let emulator = Emulator::new();

    // One configuration this endpoint holds (the control), one that lives in
    // another region, and one this endpoint held until a delete took it.
    let held = emulator
        .create_configuration_row_ok_in(REGION, &ConfigurationRequest::named("held-here"))
        .await;
    let elsewhere = emulator
        .create_configuration_row_ok_in("eu-central-1", &ConfigurationRequest::named("held-there"))
        .await;
    let deleted = emulator
        .create_configuration_row_ok_in(REGION, &ConfigurationRequest::named("to-delete"))
        .await;
    let gone = emulator
        .call_in(
            REGION,
            "DELETE",
            &format!("/v1/configurations/{}", pct(&deleted)),
            None,
        )
        .await;
    assert_eq!(
        gone.status, 200,
        "expected the delete that sets the already-deleted row up to succeed, got {} {}",
        gone.status, gone.body
    );

    // Each way an `Arn` can resolve to no configuration of this endpoint.
    let rows: Vec<(&str, String)> = vec![
        (
            "never-minted",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/never-minted/\
                 11111111-2222-4333-8444-555555555555-1"
            ),
        ),
        ("already-deleted", deleted.clone()),
        ("another-regions-configuration", elsewhere.clone()),
        (
            "this-regions-arn-rewritten-to-another-region",
            held.replace(REGION, "eu-central-1"),
        ),
        ("another-account", held.replace(ACCOUNT, "123456789012")),
        ("another-service-namespace", held.replace(":kafka:", ":kafka-cluster:")),
        (
            "another-resource-kind",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster/\
                 11111111-2222-4333-8444-555555555555-2"
            ),
        ),
        ("not-an-arn", "not-an-arn".to_string()),
        ("truncated-arn", "arn:aws:kafka".to_string()),
    ];

    for (case, arn) in &rows {
        for operation in operations() {
            let reply = call(&emulator, REGION, &operation, arn).await;
            assert_configuration_arn_resolves_to_nothing(
                &reply,
                &format!("{case}/{}", operation.0),
                arn,
                REGION,
            );
        }
    }

    // Nothing above touched the configuration this endpoint does hold: it is
    // still readable, still on its first revision, and the other region still
    // serves its own.
    let described = emulator.describe_configuration(&held).await;
    assert_eq!(
        (described.status, described.at("/latestRevision/revision").as_i64()),
        (200, Some(1)),
        "case control: the held configuration must be unchanged, got {} {}",
        described.status,
        described.body
    );
    let there = emulator
        .call_in(
            "eu-central-1",
            "GET",
            &format!("/v1/configurations/{}", pct(&elsewhere)),
            None,
        )
        .await;
    assert_eq!(
        there.status, 200,
        "case control: the other region must still serve its own configuration, got {} {}",
        there.status, there.body
    );
}

/// RULE: a configuration delete is reported through the state the registry is
/// left in, so the read that cannot resolve the ARN is the ONLY way to observe
/// that the configuration is gone — and it answers the same terminal refusal
/// however many times it is polled, whichever reader polls it.
#[tokio::test]
async fn the_read_after_a_delete_is_the_terminal_refusal_every_time() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_row_ok_in(REGION, &ConfigurationRequest::named("polled"))
        .await;

    // Readable before the delete, so what follows is a change the delete made
    // rather than a configuration that was never there.
    let before = emulator.describe_configuration(&arn).await;
    assert_eq!(
        before.status, 200,
        "case before-delete: the configuration must be readable, got {} {}",
        before.status, before.body
    );

    let deleted = emulator
        .call_in(
            REGION,
            "DELETE",
            &format!("/v1/configurations/{}", pct(&arn)),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected DeleteConfiguration to succeed, got {} {}",
        deleted.status, deleted.body
    );

    // A poll loop reads it again and again: every attempt, from every reader,
    // is the same refusal — no attempt resolves it and none changes the answer.
    for attempt in 1..=3 {
        for operation in operations() {
            let reply = call(&emulator, REGION, &operation, &arn).await;
            assert_configuration_arn_resolves_to_nothing(
                &reply,
                &format!("poll attempt {attempt}/{}", operation.0),
                &arn,
                REGION,
            );
        }
    }
}
