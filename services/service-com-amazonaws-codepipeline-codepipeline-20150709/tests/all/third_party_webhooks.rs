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
//! * the connection either operation makes or breaks is the GitHub HMAC one the
//!   webhook's shared secret authenticates, so a webhook whose stored
//!   `WebhookDefinition$authentication` is any other `WebhookAuthenticationType` is
//!   refused with `ValidationException` stating the requirement and the authentication
//!   type the store holds for that webhook;
//! * that connection starts the webhook's `targetPipeline`, so a webhook whose target
//!   pipeline the account does not hold is refused with `ValidationException` naming the
//!   webhook and the pipeline its definition references;
//! * the webhook itself is left where it is: deregistering is not deleting.
//!
//! The rows span what the rules generalise over: the operation, names at both ends of
//! `WebhookName`'s modelled 1..100 length and over its pattern's character classes,
//! names that were never stored, every authentication type the model declares, and the
//! provider the target action declares (which this rule does NOT turn on).
//!
//! The harness comes from `tests/all/pipeline_lifecycle.rs`, so every assertion goes
//! through the same router the production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    webhook_authentication_unsupported_message, webhook_dangling_target_pipeline_message,
    AUTHENTICATION_GITHUB_HMAC, AUTHENTICATION_IP, AUTHENTICATION_UNAUTHENTICATED,
};

use crate::pipeline_lifecycle::{build_stage, Emulator, ROLE_ARN};

/// The operations under test: the pair that connects a webhook to the third party.
const REGISTER: &str = "RegisterWebhookWithThirdParty";
const DEREGISTER: &str = "DeregisterWebhookWithThirdParty";

/// The error the operation models for a name no stored webhook carries, with the
/// status the model defaults it to.
const NOT_FOUND: &str = "WebhookNotFoundException";
const REFUSAL_STATUS: u16 = 400;

/// A `SecretToken` satisfying `WebhookAuthConfigurationSecretToken`'s modelled 1..100
/// length — what a GITHUB_HMAC webhook is configured with.
const SECRET_TOKEN: &str = "s3cr3t-token";

/// The pipeline whose source action a webhook targets, per provider under test.
fn pipeline_name(provider: &str) -> String {
    format!("hooked-{}", provider.to_lowercase())
}

/// A pipeline whose source action declares `provider`, with the non-`Source` stage
/// every declaration has to carry (`state::validate_declaration`).
///
/// The owners are the ones the catalog carries for each provider
/// (`state::BUILT_IN_ACTION_TYPES`): `GitHub` is a `ThirdParty` source and `S3` an
/// `AWS` one.
fn pipeline(provider: &str, owner: &str) -> Value {
    json!({
        "name": pipeline_name(provider),
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [
            {
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
            },
            build_stage("Build", 1),
        ],
    })
}

/// The `authenticationConfiguration` each authentication type is configured with, as
/// `WebhookAuthConfiguration` documents it: "For GITHUB_HMAC, only the SecretToken
/// property must be set. For IP, only the AllowedIPRange property must be set to a
/// valid CIDR range. For UNAUTHENTICATED, no properties can be set."
fn authentication_configuration(authentication: &str) -> Value {
    match authentication {
        AUTHENTICATION_GITHUB_HMAC => json!({"SecretToken": SECRET_TOKEN}),
        AUTHENTICATION_IP => json!({"AllowedIPRange": "10.0.0.0/8"}),
        _ => json!({}),
    }
}

/// A `WebhookDefinition` named `name`, targeting `provider`'s pipeline and configured
/// with `authentication`.
fn definition(name: &str, provider: &str, authentication: &str) -> Value {
    json!({
        "name": name,
        "targetPipeline": pipeline_name(provider),
        "targetAction": "Src",
        "filters": [{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}],
        "authentication": authentication,
        "authenticationConfiguration": authentication_configuration(authentication),
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

    // The same names, once the webhook IS stored and configured with the
    // authentication the connection is made with, are accepted — so what the refusal
    // reports is the store and not the shape of the name.
    for (case, name) in &cases {
        emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": definition(name, "GitHub", AUTHENTICATION_GITHUB_HMAC)}),
            )
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

/// RULE: both operations serve only a webhook configured with the GitHub HMAC
/// authentication, and a webhook carrying any other `WebhookAuthenticationType` is
/// refused with `ValidationException` naming the webhook and the authentication type
/// the store holds for it.
///
/// The rows are every value `WebhookAuthenticationType` declares, each against both
/// providers a target action can declare here — so the rule is shown to turn on the
/// stored AUTHENTICATION and not on what the webhook targets.
#[tokio::test]
async fn a_webhook_not_configured_with_github_hmac_is_not_supported() {
    let emulator = emulator().await;
    // (authentication, whether the operations serve a webhook carrying it)
    let authentications = [
        (AUTHENTICATION_GITHUB_HMAC, true),
        (AUTHENTICATION_IP, false),
        (AUTHENTICATION_UNAUTHENTICATED, false),
    ];

    for (authentication, served) in authentications {
        for provider in ["GitHub", "S3"] {
            let name = format!("hook-{}-{authentication}", provider.to_lowercase());
            let case = format!("{authentication} targeting a {provider} action");
            emulator
                .ok(
                    "PutWebhook",
                    &json!({"webhook": definition(&name, provider, authentication)}),
                )
                .await;

            for operation in [REGISTER, DEREGISTER] {
                let (status, response) = emulator
                    .call(operation, &json!({"webhookName": name}))
                    .await;
                if served {
                    assert_eq!(
                        status, 200,
                        "case {case:?} through {operation}: a webhook configured with \
                         {AUTHENTICATION_GITHUB_HMAC} is served, got {status} with {response}"
                    );
                    continue;
                }
                assert_eq!(
                    (
                        status,
                        response["__type"].as_str().unwrap_or_default().to_string()
                    ),
                    (REFUSAL_STATUS, "ValidationException".to_string()),
                    "case {case:?} through {operation}: expected the refusal the model \
                     declares, got {status} with {response}"
                );
                // The requirement and the authentication that blocked the call are what
                // the message states, built from the request's own name and the stored
                // webhook's authentication.
                let expected = webhook_authentication_unsupported_message(&name, authentication);
                assert_eq!(
                    response["message"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?} through {operation}: expected {expected:?}, got {response}"
                );
                // The wording itself, written out here so a reword of the handler's
                // text fails this suite rather than following it.
                assert_eq!(
                    expected,
                    format!(
                        "Webhook {name} must be configured with GITHUB_HMAC authentication \
                         type. Type is {authentication}."
                    ),
                    "case {case:?}: the refusal states the requirement AWS states"
                );
            }
        }
    }

    // A webhook whose target pipeline is GONE is a request neither operation serves:
    // the connection they make and break starts that pipeline, so the dangling
    // reference is refused with `ValidationException` naming both ends of it.
    let github_hook = format!("hook-github-{AUTHENTICATION_GITHUB_HMAC}");
    emulator
        .ok("DeletePipeline", &json!({"name": pipeline_name("GitHub")}))
        .await;
    for operation in [REGISTER, DEREGISTER] {
        let (status, response) = emulator
            .call(operation, &json!({"webhookName": github_hook}))
            .await;
        assert_eq!(
            (
                status,
                response["__type"].as_str().unwrap_or_default().to_string()
            ),
            (REFUSAL_STATUS, "ValidationException".to_string()),
            "{operation}: a webhook referencing a pipeline the account no longer holds is \
             refused, got {status} with {response}"
        );
        let expected =
            webhook_dangling_target_pipeline_message(&github_hook, &pipeline_name("GitHub"));
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "{operation}: expected {expected:?}, got {response}"
        );
        // The wording itself, written out here so a reword of the handler's text fails
        // this suite rather than following it.
        assert_eq!(
            expected,
            format!(
                "Webhook {github_hook} references a pipeline {} which does not exist",
                pipeline_name("GitHub")
            ),
            "{operation}: the refusal states the reference that could not be resolved"
        );
    }

    // The webhook itself is untouched by either refusal, and the same calls are served
    // again once a pipeline of that name is held: what blocked them was the reference,
    // not the webhook.
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline("GitHub", "ThirdParty")}),
        )
        .await;
    for operation in [REGISTER, DEREGISTER] {
        let (status, answer) = emulator
            .call(operation, &json!({"webhookName": github_hook}))
            .await;
        assert_eq!(
            status, 200,
            "{operation}: a stored GITHUB_HMAC webhook whose target pipeline is held is \
             served, got {status} with {answer}"
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
