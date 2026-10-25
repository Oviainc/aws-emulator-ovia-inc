// SPDX-License-Identifier: CC0-1.0

//! `RegisterWebhookWithThirdParty` and `DeregisterWebhookWithThirdParty`, over the wire
//! the service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test, each asserted over a set of inputs:
//!
//! * the call names a webhook of this account by the name `PutWebhook` stored it under,
//!   so the name is RESOLVED against the stored webhooks — a name no stored webhook
//!   carries is refused with `WebhookNotFoundException`, the only outcome besides
//!   `ValidationException` either operation models, and never with an error code they do
//!   not declare;
//! * both are supported only for a webhook of the third party they connect it to ("an
//!   action type of GitHub"), so a webhook whose target action declares any other
//!   provider is refused with `ValidationException`;
//! * the webhook itself is left where it is: deregistering is not deleting.
//!
//! The rows span what the rules generalise over: the operation, names at both ends of
//! `WebhookName`'s modelled 1..100 length and over its pattern's character classes,
//! names that were never stored, and the provider the target action declares.
//!
//! The harness comes from `tests/all/pipeline_lifecycle.rs`, so every assertion goes
//! through the same router the production emulator builds.

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{Emulator, ROLE_ARN};

/// The operations under test: the pair that connects a webhook to the third party.
const REGISTER: &str = "RegisterWebhookWithThirdParty";
const DEREGISTER: &str = "DeregisterWebhookWithThirdParty";

/// The error the operation models for a name no stored webhook carries, with the
/// status the model defaults it to.
const NOT_FOUND: &str = "WebhookNotFoundException";
const REFUSAL_STATUS: u16 = 400;

/// The pipeline whose source action a webhook targets, per provider under test.
fn pipeline_name(provider: &str) -> String {
    format!("hooked-{}", provider.to_lowercase())
}

/// A one-stage pipeline whose source action declares `provider`.
///
/// The owners are the ones the catalog carries for each provider
/// (`state::BUILT_IN_ACTION_TYPES`): `GitHub` is a `ThirdParty` source and `S3` an
/// `AWS` one.
fn pipeline(provider: &str, owner: &str) -> Value {
    json!({
        "name": pipeline_name(provider),
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [{
            "name": "Source",
            "actions": [{
                "name": "Src",
                "actionTypeId": {
                    "category": "Source",
                    "owner": owner,
                    "provider": provider,
                    "version": "1",
                },
                "configuration": {"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"},
                "outputArtifacts": [{"name": "SourceOut"}],
            }],
        }],
    })
}

/// A `WebhookDefinition` named `name`, targeting `provider`'s pipeline.
fn definition(name: &str, provider: &str) -> Value {
    json!({
        "name": name,
        "targetPipeline": pipeline_name(provider),
        "targetAction": "Src",
        "filters": [{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}],
        "authentication": "UNAUTHENTICATED",
        "authenticationConfiguration": {},
    })
}

/// An emulator holding a pipeline for each provider under test.
async fn emulator() -> Emulator {
    let emulator = Emulator::new();
    for (provider, owner) in [("GitHub", "ThirdParty"), ("S3", "AWS")] {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": pipeline(provider, owner)}))
            .await;
    }
    emulator
}

#[tokio::test]
async fn a_name_no_stored_webhook_carries_is_refused_as_not_found() {
    let emulator = emulator().await;
    // Every row satisfies `WebhookName`'s modelled length (1..100) and pattern
    // (^[A-Za-z0-9.@\-_]+$), so the boundary validator passes each to the handler,
    // which then resolves it against the store.
    let cases = [
        ("a name never stored", "sim2real-cp-nowebhook".to_string()),
        ("WebhookName's minimum length", "a".to_string()),
        ("WebhookName's maximum length", "w".repeat(100)),
        (
            "every character class the pattern allows",
            "Hook.name@9-_x".to_string(),
        ),
    ];

    for (case, name) in &cases {
        for operation in [REGISTER, DEREGISTER] {
            let (status, code) = emulator
                .expect_error(operation, &json!({"webhookName": name}))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (REFUSAL_STATUS, NOT_FOUND),
                "case {case:?} through {operation}: an unregistered name is a handleable \
                 condition the model states, not an emulator limit"
            );
        }
    }

    // The same names, once the webhook IS stored against a GitHub-targeted action, are
    // accepted — so what the refusal reports is the store and not the shape of the
    // name.
    for (case, name) in &cases {
        emulator
            .ok("PutWebhook", &json!({"webhook": definition(name, "GitHub")}))
            .await;
        for operation in [REGISTER, DEREGISTER] {
            let (status, answer) = emulator
                .call(operation, &json!({"webhookName": name}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} is served for a stored webhook, got {status} \
                 with {answer}"
            );
        }

        // Deregistering does not delete the webhook: `DeleteWebhook` does that, and
        // the listing still reports it.
        let listed = emulator.ok("ListWebhooks", &json!({})).await;
        let names: Vec<&str> = listed["webhooks"]
            .as_array()
            .unwrap_or_else(|| panic!("ListWebhooks reports a list, got {listed}"))
            .iter()
            .filter_map(|item| item["definition"]["name"].as_str())
            .collect();
        assert!(
            names.contains(&name.as_str()),
            "case {case:?}: the webhook itself is left where it is, got {listed}"
        );

        // And the call is idempotent for a webhook that is stored: the same name is
        // accepted again rather than refused.
        let (status, answer) = emulator
            .call(DEREGISTER, &json!({"webhookName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deregistering twice is accepted, got {status} with {answer}"
        );
    }
}

#[tokio::test]
async fn a_webhook_that_targets_another_action_type_is_not_supported() {
    let emulator = emulator().await;
    // The same name, the same pipeline shape, the same filters: only the provider the
    // target action declares differs, which is what the rule turns on.
    emulator
        .ok("PutWebhook", &json!({"webhook": definition("s3-hook", "S3")}))
        .await;
    for operation in [REGISTER, DEREGISTER] {
        let (status, code) = emulator
            .expect_error(operation, &json!({"webhookName": "s3-hook"}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "ValidationException"),
            "{operation}: a webhook targeting an action type it does not serve is \
             refused as the model declares"
        );
    }

    emulator
        .ok(
            "PutWebhook",
            &json!({"webhook": definition("github-hook", "GitHub")}),
        )
        .await;
    let (status, answer) = emulator
        .call(DEREGISTER, &json!({"webhookName": "github-hook"}))
        .await;
    assert_eq!(
        status, 200,
        "the same call for a GitHub-targeted webhook is supported, got {status} with \
         {answer}"
    );

    // A webhook whose target pipeline is gone targets no action type at all, so it is
    // refused the same way rather than accepted.
    emulator
        .ok("DeletePipeline", &json!({"name": pipeline_name("GitHub")}))
        .await;
    for operation in [REGISTER, DEREGISTER] {
        let (status, code) = emulator
            .expect_error(operation, &json!({"webhookName": "github-hook"}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "ValidationException"),
            "{operation}: a webhook whose target action cannot be read targets no \
             supported action type"
        );
    }
}

#[tokio::test]
async fn a_malformed_name_and_a_missing_one_are_refused_as_validation() {
    let emulator = emulator().await;
    // One past either published length bound, and a name the pattern rules out: each
    // is refused at the boundary, before the store is consulted.
    for (case, body) in [
        (
            "a name with characters the pattern rules out",
            json!({"webhookName": "bad name!"}),
        ),
        ("one below WebhookName's minimum length", json!({"webhookName": ""})),
        (
            "one above WebhookName's maximum length",
            json!({"webhookName": "w".repeat(101)}),
        ),
        ("no webhookName at all", json!({})),
    ] {
        for operation in [REGISTER, DEREGISTER] {
            let (status, code) = emulator.expect_error(operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (REFUSAL_STATUS, "ValidationException"),
                "case {case:?} through {operation}: a request that names no webhook is \
                 refused as invalid"
            );
        }
    }
}
