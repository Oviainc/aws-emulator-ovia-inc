// SPDX-License-Identifier: CC0-1.0

//! What a repeated `CreateConnection$clientToken` is judged against.
//!
//! RULE: a `@idempotencyToken` replay is answered with the connection the token
//! created only when the repeat IS that request — every member of the create the
//! caller owns compared, absence included — and a repeat that differs in any one of
//! them is refused with the operation's modelled `InterconnectValidationException`
//! (400) naming the token and the connection it already created, having provisioned
//! nothing.
//!
//! The table's rows differ in WHICH caller-owned member the repeat changes (and, for
//! the optional ones, in presence against absence), across every Environment of the
//! seeded catalog that takes connections — so the rule is checked on each kind of
//! identifying information an Environment asks for, not on one request.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text, Emulator,
};

/// The gateways the rows attach their connections to: an attach point IS a Direct
/// Connect gateway of the account, so the suite creates its own, and a row that
/// changes the attach point on a retry has a second one to change it to.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [
    ("ic-token-replay-a", None),
    ("ic-token-replay-b", Some(64_513)),
];

/// An account number that is not the emulator's own, so a retry can carry a
/// DIFFERENT `remoteAccount` that is still of the `account` kind an Environment may
/// declare — the refusal then has to come from the token comparison and from nothing
/// else.
const OTHER_ACCOUNT_IDENTIFIER: &str = "123456789012";

/// An email address that is not the one the harness picks, playing the same part for
/// an Environment declaring the `email` kind.
const OTHER_PARTNER_EMAIL: &str = "other-partner@example.test";

/// One `CreateConnection` request, as a map a row can change exactly one member of.
fn create_request(
    environment_id: &str,
    bandwidth: &str,
    attach_point: &str,
    description: &str,
    remote_account: Option<&Value>,
    tags: &Value,
    token: &str,
) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": attach_point }),
    );
    request.insert("description".to_string(), json!(description));
    request.insert("tags".to_string(), tags.clone());
    if let Some(remote_account) = remote_account {
        request.insert("remoteAccount".to_string(), remote_account.clone());
    }
    request.insert("clientToken".to_string(), json!(token));
    Value::Object(request)
}

/// A `remoteAccount` that differs from the one `environment` was created with while
/// still being what that Environment asks for — or, for an Environment that declares
/// no `remoteIdentifierType` and so was created without the member at all, a
/// `remoteAccount` where the first call sent none.
///
/// The two kinds are the values `RemoteAccountIdentifierType` models
/// (`smithy_query(query="shape",shape="RemoteAccountIdentifierType")` -> values:
/// ["account", "email"]).
fn other_remote_account(environment: &Value) -> Value {
    match environment["remoteIdentifierType"].as_str() {
        Some("email") => json!({ "identifier": OTHER_PARTNER_EMAIL }),
        _ => json!({ "identifier": OTHER_ACCOUNT_IDENTIFIER }),
    }
}

/// A repeat of `base` differing in exactly ONE caller-owned member, per row, labelled
/// by the member it changes.
///
/// `description` and `tags` are optional on the input, so each contributes a row that
/// REMOVES it as well as one that changes it: absence is a value of the member too.
/// `remoteAccount` changes to another value of the kind the Environment asks for, so
/// nothing but the token comparison can refuse it.
fn repeats_that_are_a_different_request(
    base: &Value,
    environment: &Value,
    other_bandwidth: Option<&String>,
    other_gateway: &str,
    other_environment_id: Option<&String>,
) -> Vec<(String, Value)> {
    let changed = |member: &str, value: Value| {
        let mut repeat = base.clone();
        repeat[member] = value;
        repeat
    };
    let removed = |member: &str| {
        let mut repeat = base.clone();
        repeat
            .as_object_mut()
            .expect("the request is a JSON object")
            .remove(member);
        repeat
    };

    let mut rows = vec![
        (
            "a different description".to_string(),
            changed("description", json!("a description the first call did not send")),
        ),
        ("no description at all".to_string(), removed("description")),
        (
            "different tags".to_string(),
            changed("tags", json!({ "env": "a-value-the-first-call-did-not-send" })),
        ),
        (
            "an extra tag".to_string(),
            changed("tags", json!({ "env": "test", "team": "network" })),
        ),
        ("no tags at all".to_string(), removed("tags")),
        (
            "a different attach point".to_string(),
            changed(
                "attachPoint",
                json!({ "directConnectGateway": other_gateway }),
            ),
        ),
        (
            "a different remoteAccount".to_string(),
            changed("remoteAccount", other_remote_account(environment)),
        ),
    ];
    if let Some(bandwidth) = other_bandwidth {
        rows.push((
            "a different bandwidth".to_string(),
            changed("bandwidth", json!(bandwidth)),
        ));
    }
    if let Some(environment_id) = other_environment_id {
        rows.push((
            "a different environment".to_string(),
            changed("environmentId", json!(environment_id)),
        ));
    }
    rows
}

/// Every connection the store holds, walked page by page, so a refused retry can be
/// shown to have provisioned nothing.
async fn stored_ids(emulator: &Emulator) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok("ListConnections", Value::Object(request))
            .await;
        for summary in listed["connections"].as_array().cloned().unwrap_or_default() {
            all.insert(text(&summary, "id"));
        }
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// RULE: a repeated `clientToken` replays the connection it created only for the SAME
/// request; a repeat differing in any caller-owned member — `remoteAccount` included —
/// is refused as a token/parameter mismatch and provisions nothing.
#[tokio::test]
async fn a_repeated_client_token_is_judged_on_every_caller_owned_member() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let environments = creatable_environments(&catalog);
    assert!(
        !environments.is_empty(),
        "the catalog must offer an environment that takes connections, got {}",
        environments.len()
    );
    // Another `environmentId` of the SAME endpoint, for the row that repeats the token
    // against a different environment. The published catalog offers one Environment per
    // Region, so that row has no subject here and is contributed only for a catalog that
    // offers a second one.
    let environment_ids: Vec<String> = environments
        .iter()
        .map(|environment| text(environment, "environmentId"))
        .collect();

    for (index, environment) in environments.iter().enumerate() {
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[0].clone();
        let other_bandwidth = bandwidths.get(1).cloned();
        let other_environment_id = environment_ids
            .iter()
            .find(|candidate| **candidate != environment_id)
            .cloned();
        let remote_account = remote_account_for(environment);
        let token = format!("token-{index}");
        let base = create_request(
            &environment_id,
            &bandwidth,
            &gateways[index % gateways.len()],
            &format!("replay row {index}"),
            remote_account.as_ref(),
            &json!({ "env": "test" }),
            &token,
        );

        // The call the token names.
        let created = emulator.ic_ok("CreateConnection", base.clone()).await;
        let id = text(&created["connection"], "id");
        let case = format!("{environment_id} / {bandwidth} / {token}");
        assert!(
            !id.is_empty(),
            "case {case:?}: expected the first call to allocate a connection, got {created:?}"
        );

        // The SAME request repeated is that same connection reported again.
        let replay = emulator.ic_ok("CreateConnection", base.clone()).await;
        assert_eq!(
            replay["connection"], created["connection"],
            "case {case:?}: expected the repeat of an identical request to replay the first connection, got {:?}",
            replay["connection"]
        );

        // A repeat that differs in ONE caller-owned member is a different request.
        let before = stored_ids(&emulator).await;
        for (label, repeat) in repeats_that_are_a_different_request(
            &base,
            environment,
            other_bandwidth.as_ref(),
            &gateways[(index + 1) % gateways.len()],
            other_environment_id.as_ref(),
        ) {
            let case = format!("{case} / {label}");
            let (status, body) = emulator.ic("CreateConnection", repeat).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the reused token to be refused, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains("InterconnectValidationException"),
                "case {case:?}: expected InterconnectValidationException, got {:?}",
                code(&body)
            );
            assert!(
                message(&body).contains(&token) && message(&body).contains(&id),
                "case {case:?}: expected the refusal to name the token and the connection it created, got {:?}",
                message(&body)
            );
        }
        let after = stored_ids(&emulator).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected every refused retry to have provisioned nothing, got {after:?} for {before:?} before"
        );

        // The member is part of the REQUEST, not part of the key: the same change
        // under a fresh token allocates a second connection.
        let mut fresh = base.clone();
        fresh["clientToken"] = json!(format!("{token}-fresh"));
        fresh["remoteAccount"] = other_remote_account(environment);
        let (status, body) = emulator.ic("CreateConnection", fresh).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a fresh token carrying another remoteAccount to be accepted, got {status} with {body:?}"
        );
        let fresh_id = text(&body["connection"], "id");
        assert_ne!(
            fresh_id, id,
            "case {case:?}: expected a distinct connection for a distinct client token, got {fresh_id:?}"
        );
    }
}
