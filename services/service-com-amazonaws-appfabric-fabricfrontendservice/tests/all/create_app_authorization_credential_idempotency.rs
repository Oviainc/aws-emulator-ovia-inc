// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the part `credential` plays in `CreateAppAuthorization`'s
//! idempotency judgement, driven over the wire (restJson1) through the same
//! router the emulator boots.
//!
//! `create_app_authorization_contract` already states the rule over the members
//! that identify an authorization (`app`, `tenant`, `authType`, `tags`). What
//! this suite adds is the credential's own VALUES: a replay carrying another API
//! key, client id or client secret is a different request, not a retry — while
//! nothing the emulator stores or reports keeps the credential material those
//! values were compared through.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::service_state;

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// Every credential value any case below sends, so one assertion can check that
/// no response and no stored byte carries any of them. They are recognisable
/// strings of differing length, because the rule has to hold for values that
/// differ in size as well as in content.
const SENT_VALUES: &[&str] = &[
    "key-one-9f3a",
    "key-two-9f3a",
    "key-one-9f3a-longer",
    "clientid-one-7c4a",
    "clientid-two-7c4a",
    "secret-one-91fd",
    "secret-two-91fd",
    "alpha5d2ebeta5d2e",
    "alpha5d2e",
    "beta5d2egamma",
    "gamma",
];

/// An `apiKeyCredential` credential body.
fn api_key(key: &str) -> Value {
    json!({"apiKeyCredential": {"apiKey": key}})
}

/// An `oauth2Credential` credential body.
fn oauth2(client_id: &str, client_secret: &str) -> Value {
    json!({"oauth2Credential": {"clientId": client_id, "clientSecret": client_secret}})
}

/// A client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
const CLIENT_TOKEN: &str = "11111111-0000-4000-8000-000000000000";

/// A create body for one auth type and credential, carrying the shared token.
fn request(auth_type: &str, credential: &Value) -> Value {
    json!({
        "clientToken": CLIENT_TOKEN,
        "app": "SLACK",
        "authType": auth_type,
        "credential": credential.clone(),
        "tenant": {
            "tenantIdentifier": "tenant-k",
            "tenantDisplayName": "tenant-k display",
        },
    })
}

/// `POST /appbundles/{appBundleIdentifier}/appauthorizations`.
async fn create_authorization(emulator: &Emulator, bundle: &str, body: &Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(body.clone()),
        )
        .await
}

/// The `appAuthorizationArn` of a success body.
fn authorization_arn(answer: &Answer, case: &str) -> String {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorization.appAuthorizationArn, got {}",
                answer.body
            )
        })
        .to_string()
}

/// Every `appAuthorizationArn` `ListAppAuthorizations` reports for one bundle.
async fn listed_authorization_arns(emulator: &Emulator, bundle: &str, case: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorizationSummaryList, got {}",
                answer.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("appAuthorizationArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorizationArn in {summary}"))
                .to_string()
        })
        .collect()
}

/// What a second create carrying the already-spent `clientToken` must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replay {
    /// The same parameters, so the first call's authorization is replayed.
    SameAuthorization,
    /// A parameter differs, so the token is the conflict AWS documents as
    /// `IdempotentParameterMismatch`.
    Mismatch,
}

/// RULE: a replayed `@idempotencyToken` is a true retry only when EVERY
/// parameter of the request carries the same value, so a second
/// `CreateAppAuthorization` with a spent `clientToken` and a credential whose
/// values differ in any way is refused with the modelled `ConflictException`
/// (HTTP 409) naming the token, while one whose credential is identical replays
/// the authorization the first call minted (HTTP 201, same body). Either way the
/// bundle holds exactly the first authorization.
///
/// The table spans how a credential can differ: the value of each member of each
/// union arm (`apiKey`, `clientId`, `clientSecret`), the SIZE of a value, the
/// same values moved between members, and the same characters split between
/// members differently — plus, as the control rows, byte-identical credentials of
/// both arms.
#[tokio::test]
async fn a_replayed_client_token_with_another_credential_is_a_parameter_mismatch() {
    // (case, authType, first credential, second credential, what the second call must do)
    let cases: Vec<(&str, &str, Value, Value, Replay)> = vec![
        (
            "the same apiKey",
            "apiKey",
            api_key("key-one-9f3a"),
            api_key("key-one-9f3a"),
            Replay::SameAuthorization,
        ),
        (
            "the same oauth2 credential",
            "oauth2",
            oauth2("clientid-one-7c4a", "secret-one-91fd"),
            oauth2("clientid-one-7c4a", "secret-one-91fd"),
            Replay::SameAuthorization,
        ),
        (
            "another apiKey value",
            "apiKey",
            api_key("key-one-9f3a"),
            api_key("key-two-9f3a"),
            Replay::Mismatch,
        ),
        (
            "a longer apiKey with the first as its prefix",
            "apiKey",
            api_key("key-one-9f3a"),
            api_key("key-one-9f3a-longer"),
            Replay::Mismatch,
        ),
        (
            "another clientId",
            "oauth2",
            oauth2("clientid-one-7c4a", "secret-one-91fd"),
            oauth2("clientid-two-7c4a", "secret-one-91fd"),
            Replay::Mismatch,
        ),
        (
            "another clientSecret",
            "oauth2",
            oauth2("clientid-one-7c4a", "secret-one-91fd"),
            oauth2("clientid-one-7c4a", "secret-two-91fd"),
            Replay::Mismatch,
        ),
        (
            "the same two values swapped between the oauth2 members",
            "oauth2",
            oauth2("clientid-one-7c4a", "clientid-two-7c4a"),
            oauth2("clientid-two-7c4a", "clientid-one-7c4a"),
            Replay::Mismatch,
        ),
        (
            "the same characters split between the oauth2 members differently",
            "oauth2",
            oauth2("alpha5d2ebeta5d2e", "gamma"),
            oauth2("alpha5d2e", "beta5d2egamma"),
            Replay::Mismatch,
        ),
    ];

    for (case, auth_type, first_credential, second_credential, expected) in cases {
        // A fresh emulator per row: a spent token is state, so one row must not
        // see the tokens another row spent. The engine handle is kept so the row
        // can also read the store the handlers write.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let emulator = Emulator::with_engine(&engine);
        let created_bundle = emulator.create_app_bundle(json!({})).await;
        assert_eq!(
            created_bundle.status, 201,
            "case {case:?}: the set-up app bundle must be created, got HTTP {} with body {}",
            created_bundle.status, created_bundle.body
        );
        let bundle = arn_of(&created_bundle.body, "appBundle");

        let first = create_authorization(&emulator, &bundle, &request(auth_type, &first_credential)).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: the first create must answer HTTP 201, got {} with body {}",
            first.status, first.body
        );
        let first_arn = authorization_arn(&first, case);

        let second = create_authorization(&emulator, &bundle, &request(auth_type, &second_credential)).await;
        match expected {
            Replay::SameAuthorization => {
                assert_eq!(
                    second.status, 201,
                    "case {case:?}: a retry must answer HTTP 201, got {} with body {}",
                    second.status, second.body
                );
                assert_eq!(
                    second.body, first.body,
                    "case {case:?}: a retry must replay the first authorization {}, got {}",
                    first.body, second.body
                );
            }
            Replay::Mismatch => {
                assert_eq!(
                    second.status, 409,
                    "case {case:?}: a token replayed with another credential must answer HTTP 409, got {} with body {}",
                    second.status, second.body
                );
                assert!(
                    second.error_code().contains("ConflictException"),
                    "case {case:?}: expected a ConflictException, got {:?} with body {}",
                    second.error_code(),
                    second.body
                );
                assert_eq!(
                    second
                        .body
                        .get("resourceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    CLIENT_TOKEN,
                    "case {case:?}: the refusal must name the replayed token, got body {}",
                    second.body
                );
            }
        }

        // Either way the bundle holds exactly the first authorization: a retry
        // minted nothing, and a refused call wrote nothing.
        let stored = listed_authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            stored,
            vec![first_arn.clone()],
            "case {case:?}: expected the bundle to hold only {first_arn:?}, got {stored:?}"
        );

        // The judgement above is made WITHOUT keeping the credential: neither
        // response, nor the whole store the handlers wrote, carries any value
        // either call sent.
        let store = service_state(&*engine)
            .unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
        let persisted = format!("{:?}", *store.lock());
        for (where_, rendered) in [
            ("the first response", first.body.to_string()),
            ("the second response", second.body.to_string()),
            ("the store", persisted),
        ] {
            for value in SENT_VALUES {
                assert!(
                    !rendered.contains(value),
                    "case {case:?}: {where_} must not carry the credential value {value:?}, got {rendered}"
                );
            }
        }
    }
}
