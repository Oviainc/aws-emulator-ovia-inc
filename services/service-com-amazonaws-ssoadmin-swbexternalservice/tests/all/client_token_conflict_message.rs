// SPDX-License-Identifier: CC0-1.0

//! What a create answers when an `@idempotencyToken` is reused for a DIFFERENT request,
//! end to end over the `awsJson1_1` wire.
//!
//! The rule under test, from the token member's own documentation ("Passing the same value
//! to a later call to an operation requires that you also pass the same value for all
//! other parameters... If you retry the operation with the same ClientToken, but with
//! different parameters, the retry fails"): the refusal is the modelled
//! `ConflictException`, and the sentence it carries is FIXED for that error class — the
//! same words whichever create was called and whichever parameter differed, with no
//! variable part, so it names neither the token, nor the resource the earlier request
//! created, nor the account. A refusal detail built out of the emulator's own state would
//! differ per run and could not be matched by a client at all.
//!
//! The rows span the class the rule generalises over: every create of this service that
//! carries an `@idempotencyToken` (`CreateInstance`, `CreateApplication`,
//! `CreateTrustedTokenIssuer`) and, per create, WHICH parameter the second request differs
//! in (a `Name`, a tag value, a nested configuration member, an optional member the first
//! request did not send at all). Whether a reused token with the SAME parameters replays,
//! and what set of parameters a token is scoped to, are owned by
//! `instance_create_client_token_fold.rs`, `trusted_token_issuers.rs`,
//! `application_assignments.rs` and `create_tag_fingerprint.rs`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, empty_account_router, router, seeded_instance_arn,
};

/// The generic customer-managed OAuth 2.0 application provider, the one provider
/// `CreateApplication` accepts — resolved over the wire by
/// `application_provider_resolution.rs`, and named here so a row is one request.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// One row: a create, the request a `ClientToken` is established with, and a request that
/// differs from it in the member the row's label names.
///
/// `InstanceArn` is filled in from the router the row runs against; `CreateInstance`
/// creates the instance itself, so its builders ignore the argument.
struct Row {
    operation: &'static str,
    differs_in: &'static str,
    first: fn(&str) -> Value,
    second: fn(&str) -> Value,
}

/// The member of the create's OUTPUT that carries the resource the first call minted: the
/// per-run value a refusal must NOT quote.
fn minted_arn(operation: &str, created: &Value) -> String {
    let member = match operation {
        "CreateInstance" => "InstanceArn",
        "CreateApplication" => "ApplicationArn",
        "CreateTrustedTokenIssuer" => "TrustedTokenIssuerArn",
        other => panic!("no minted-ARN member is known for {other}"),
    };
    created[member]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} minted no {member}: {created}"))
        .to_string()
}

/// A `CreateTrustedTokenIssuer` configuration for one issuer URL. Every other member is
/// held fixed, so the row differs only in the URL
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
fn oidc_configuration(issuer_url: &str) -> Value {
    json!({
        "OidcJwtConfiguration": {
            "IssuerUrl": issuer_url,
            "ClaimAttributePath": "email",
            "IdentityStoreAttributePath": "emails.value",
            "JwksRetrievalOption": "OPEN_ID_DISCOVERY",
        }
    })
}

/// The rows: each create that carries an `@idempotencyToken`, and per create a second
/// request differing in one member.
fn rows() -> Vec<Row> {
    vec![
        Row {
            operation: "CreateInstance",
            differs_in: "the Name",
            first: |_| json!({"Name": "token-conflict-first"}),
            second: |_| json!({"Name": "token-conflict-second"}),
        },
        Row {
            operation: "CreateInstance",
            differs_in: "a tag value",
            first: |_| {
                json!({
                    "Name": "token-conflict-tagged",
                    "Tags": [{"Key": "Owner", "Value": "platform"}],
                })
            },
            second: |_| {
                json!({
                    "Name": "token-conflict-tagged",
                    "Tags": [{"Key": "Owner", "Value": "identity"}],
                })
            },
        },
        Row {
            operation: "CreateApplication",
            differs_in: "the Name",
            first: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                    "Name": "token-conflict-first",
                })
            },
            second: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                    "Name": "token-conflict-second",
                })
            },
        },
        Row {
            operation: "CreateApplication",
            differs_in: "a Description the first request did not carry",
            first: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                    "Name": "token-conflict-described",
                })
            },
            second: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                    "Name": "token-conflict-described",
                    "Description": "a member the first request omitted",
                })
            },
        },
        Row {
            operation: "CreateTrustedTokenIssuer",
            differs_in: "a nested configuration member (the IssuerUrl)",
            first: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "token-conflict-issuer",
                    "TrustedTokenIssuerType": "OIDC_JWT",
                    "TrustedTokenIssuerConfiguration":
                        oidc_configuration("https://example.com/oidc-first"),
                })
            },
            second: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "token-conflict-issuer",
                    "TrustedTokenIssuerType": "OIDC_JWT",
                    "TrustedTokenIssuerConfiguration":
                        oidc_configuration("https://example.com/oidc-second"),
                })
            },
        },
        Row {
            operation: "CreateTrustedTokenIssuer",
            differs_in: "the Name",
            first: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "token-conflict-issuer-a",
                    "TrustedTokenIssuerType": "OIDC_JWT",
                    "TrustedTokenIssuerConfiguration":
                        oidc_configuration("https://example.com/oidc"),
                })
            },
            second: |instance_arn| {
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "token-conflict-issuer-b",
                    "TrustedTokenIssuerType": "OIDC_JWT",
                    "TrustedTokenIssuerConfiguration":
                        oidc_configuration("https://example.com/oidc"),
                })
            },
        },
    ]
}

/// A router for one row, plus the `InstanceArn` its requests name.
///
/// `CreateInstance` is documented for an account that holds NO instance, so its rows run
/// against such an account; every other create names the instance the account already
/// holds.
async fn row_router(operation: &str) -> (MultiServiceRouter, String) {
    if operation == "CreateInstance" {
        return (empty_account_router(), String::new());
    }
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    (router, instance_arn)
}

/// RULE — a reused `@idempotencyToken` carrying a different request is refused
/// `ConflictException` with AWS's FIXED sentence for that error class: the same words for
/// every create and every differing member, carrying no per-run value.
#[tokio::test]
async fn a_reused_token_with_different_parameters_is_refused_with_the_fixed_sentence() {
    let mut messages: Vec<String> = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = format!("{} differing in {}", row.operation, row.differs_in);
        let (router, instance_arn) = row_router(row.operation).await;
        // `ClientToken` is `length 1..64` of `^[!-~]+$`.
        let token = format!("conflict-token-{index}");

        let mut first = (row.first)(&instance_arn);
        first["ClientToken"] = json!(token);
        let (status, created) = call(&router, row.operation, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must answer 200, got {status} {created}"
        );
        let arn = minted_arn(row.operation, &created);

        let mut second = (row.second)(&instance_arn);
        second["ClientToken"] = json!(token);
        let (status, refused) = call(&router, row.operation, &second).await;
        assert_refused(
            &case,
            "the reused ClientToken carrying another request",
            "ConflictException",
            status,
            &refused,
        );

        // The model names the member `ConflictException$Message`, so that is where the
        // detail text is read from.
        let message = refused["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {refused}"))
            .to_string();
        assert_eq!(
            message,
            state::CLIENT_TOKEN_MISMATCH_MESSAGE,
            "case {case:?}: expected the fixed conflict sentence, got {message:?}"
        );
        // No part of it may come from the request or from the store: a detail that quotes
        // either differs per run.
        for (what, value) in [("the minted ARN", arn.as_str()), ("the ClientToken", &token)] {
            assert!(
                !message.contains(value),
                "case {case:?}: the fixed sentence must not quote {what} ({value:?}), got \
                 {message:?}"
            );
        }
        assert!(
            !message.contains("arn:") && !message.contains(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: the fixed sentence carries no ARN and no account id, got \
             {message:?}"
        );
        messages.push(message);
    }

    let first = messages
        .first()
        .expect("the table has rows")
        .clone();
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(
            message, &first,
            "row {index}: one condition reads ONE way across the creates, got {messages:?}"
        );
    }
}
