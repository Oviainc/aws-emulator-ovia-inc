// SPDX-License-Identifier: CC0-1.0

//! The API-key operations against an API of EITHER resource family, end to end over
//! HTTP.
//!
//! An API key belongs to the API its `apiId` path label names, and that label is ONE
//! identifier space over both families AppSync creates APIs in — a `GraphqlApi`
//! (`CreateGraphqlApi`) and an `Api` (`CreateApi`). Three rules follow from that, each
//! asserted over a TABLE spanning the class it generalises over rather than one
//! recorded request:
//!
//! 1. every API-key operation (`CreateApiKey`, `ListApiKeys`, `UpdateApiKey`,
//!    `DeleteApiKey`) resolves the `apiId` of an API of either family, so a key can be
//!    minted, listed, updated and deleted under any stored API;
//! 2. `NotFoundException` (404, "API not found.") is the answer for an `apiId` that
//!    names no API of either family, while an id too long to be an AppSync API id is a
//!    bad request (400) — the shape rule the rest of this service's `apiId` label
//!    follows;
//! 3. a key belongs to ONE API: an API lists only the keys minted under it, and a key
//!    held by another API — of either family — is not found under this one.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`) is the error-contract suite's.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, router, Answer};

/// One hour in seconds — the granularity `ApiKey$expires` and `ApiKey$deletes` are
/// documented with: both are "seconds since the epoch, rounded down to the nearest
/// hour". Source: `smithy_query(query="shape", shape="ApiKey")`.
const HOUR: i64 = 3_600;

/// One day in seconds — the unit the API-key validity window is stated in.
const DAY: i64 = 86_400;

/// `CreateApiKey`'s documented default validity: "The default value for this parameter
/// is 7 days from creation time."
const DEFAULT_VALIDITY: i64 = 7 * DAY;

/// How long an expired API key is kept before the service reclaims it — the span
/// between a key's `expires` and its `deletes`. Source: the `ApiKey` shape's doc for
/// the `da2` key version, "Expired API keys are kept for 60 days after the expiration
/// time" (`smithy_query(query="doc", shape="ApiKey")`).
const RETENTION: i64 = 60 * DAY;

/// How far inside a validity bound a row sits, so the seconds that elapse between this
/// test's clock read and the handler's cannot push the row across the bound it
/// exercises. The bounds themselves come from `ApiKeyValidityOutOfBoundsException`:
/// "must be set to a value between 1 and 365 days from creation (for CreateApiKey) or
/// from update (for UpdateApiKey)".
const BOUND_SLACK: i64 = 5;

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the service's
/// own copy, and an id of exactly this length is well formed.
const API_ID_LEN: usize = 26;

/// AppSync's fixed text for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// The key version this service mints: `ApiKey`'s own documentation describes `da2`
/// keys as the ones reporting `expires`/`deletes` in seconds.
const KEY_VERSION_PREFIX: &str = "da2-";

/// `AuthenticationType`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="AuthenticationType")`.
const AUTHENTICATION_TYPES: &[&str] = &[
    "API_KEY",
    "AWS_IAM",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_LAMBDA",
];

/// Now, as whole seconds since the Unix epoch — the form `ApiKey$expires` takes.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// `seconds` rounded DOWN to the nearest hour, the form the two time members take.
fn floor_to_hour(seconds: i64) -> i64 {
    seconds - seconds.rem_euclid(HOUR)
}

/// Which resource family an API was created in — the two families whose ids share the
/// API-key operations' `apiId` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// A `GraphqlApi`, created by `CreateGraphqlApi` (POST /v1/apis).
    Graphql,
    /// An `Api` (an Event API), created by `CreateApi` (POST /v2/apis).
    Event,
}

/// One `authProviders` entry for `auth_type`, carrying the configuration member that
/// type is set up through.
///
/// Source: the `AuthProvider` shape and the members each configuration shape requires
/// (`smithy_query(query="closure", operation="CreateApi")`). `API_KEY` and `AWS_IAM`
/// are configured by nothing, so their provider carries only the type.
fn provider(auth_type: &str) -> Value {
    match auth_type {
        "AMAZON_COGNITO_USER_POOLS" => json!({
            "authType": auth_type,
            "cognitoConfig": {"userPoolId": "pool_keys", "awsRegion": "eu-west-1"},
        }),
        "OPENID_CONNECT" => json!({
            "authType": auth_type,
            "openIDConnectConfig": {"issuer": "https://issuer.example.com/"},
        }),
        "AWS_LAMBDA" => json!({
            "authType": auth_type,
            "lambdaAuthorizerConfig": {
                "authorizerUri":
                    "arn:aws:lambda:eu-west-1:000000000000:function:keys_authorizer",
            },
        }),
        _ => json!({"authType": auth_type}),
    }
}

/// A `CreateApi` body whose providers set up `auth_types` and whose three auth-mode
/// lists each name every one of them.
fn event_api_body(name: &str, auth_types: &[&str]) -> Value {
    let providers: Vec<Value> = auth_types.iter().map(|ty| provider(ty)).collect();
    let modes: Vec<Value> = auth_types
        .iter()
        .map(|ty| json!({"authType": ty}))
        .collect();
    json!({
        "name": name,
        "eventConfig": {
            "authProviders": providers,
            "connectionAuthModes": modes,
            "defaultPublishAuthModes": modes,
            "defaultSubscribeAuthModes": modes,
        },
    })
}

/// A `CreateGraphqlApi` body with the required members set.
fn graphql_api_body(name: &str, authentication_type: &str) -> Value {
    let mut body = json!({
        "name": name,
        "authenticationType": authentication_type,
        "queryDepthLimit": 0,
        "resolverCountLimit": 0,
        "xrayEnabled": false,
    });
    // Each authentication type that is set up through a member of its own needs that
    // member (`CreateGraphqlApi`'s own refusals).
    match authentication_type {
        "AMAZON_COGNITO_USER_POOLS" => {
            body["userPoolConfig"] = json!({
                "userPoolId": "pool_keys",
                "awsRegion": "eu-west-1",
                "defaultAction": "ALLOW",
            });
        }
        "OPENID_CONNECT" => {
            body["openIDConnectConfig"] = json!({"issuer": "https://issuer.example.com/"});
        }
        "AWS_LAMBDA" => {
            body["lambdaAuthorizerConfig"] = json!({
                "authorizerUri":
                    "arn:aws:lambda:eu-west-1:000000000000:function:keys_authorizer",
            });
        }
        _ => {}
    }
    body
}

/// One row of the parent table: an API to mint keys under, in one of the two families.
struct ParentRow {
    label: &'static str,
    family: Family,
    body: Value,
}

/// The parent table: the rows differ along the family the API is created in and along
/// how it authenticates — one row per modelled `AuthenticationType` on each family,
/// plus an Event API that sets up every one of them at once.
fn parent_rows() -> Vec<ParentRow> {
    let mut rows = Vec::new();
    for auth_type in AUTHENTICATION_TYPES {
        rows.push(ParentRow {
            label: "a GraphQL API",
            family: Family::Graphql,
            body: graphql_api_body(&format!("Api_keys_{auth_type}"), auth_type),
        });
        rows.push(ParentRow {
            label: "an Event API",
            family: Family::Event,
            body: event_api_body(&format!("Events_keys_{auth_type}"), &[auth_type]),
        });
    }
    rows.push(ParentRow {
        label: "an Event API setting up every authorization type",
        family: Family::Event,
        body: event_api_body("Events_keys_all", AUTHENTICATION_TYPES),
    });
    rows
}

/// Create the API `row` describes and return the `apiId` the service minted for it.
async fn create_parent(router: &MultiServiceRouter, row: &ParentRow) -> String {
    let (path, member) = match row.family {
        Family::Graphql => ("/v1/apis", "graphqlApi"),
        Family::Event => ("/v2/apis", "api"),
    };
    let case = row.label;
    let answer = call(router, "POST", path, Some(row.body.clone())).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the parent create to answer 200, got {answer:?}"
    );
    let api_id = answer.body[member]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        api_id.chars().count(),
        API_ID_LEN,
        "case {case:?}: expected a minted apiId of {API_ID_LEN} characters, got {answer:?}"
    );
    api_id
}

/// `CreateApiKey` under `api_id`.
async fn create_key(router: &MultiServiceRouter, api_id: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(body),
    )
    .await
}

/// `UpdateApiKey` on one key of `api_id`.
async fn update_key(
    router: &MultiServiceRouter,
    api_id: &str,
    key_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys/{key_id}"),
        Some(body),
    )
    .await
}

/// `DeleteApiKey` on one key of `api_id`.
async fn delete_key(router: &MultiServiceRouter, api_id: &str, key_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/apikeys/{key_id}"),
        None,
    )
    .await
}

/// The keys `ListApiKeys` reports for `api_id`, in the order it reports them.
async fn listed_keys(router: &MultiServiceRouter, api_id: &str) -> Vec<Value> {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        answer.status, 200,
        "case list the keys of {api_id}: expected ListApiKeys 200, got {answer:?}"
    );
    answer.body["apiKeys"].as_array().cloned().unwrap_or_default()
}

/// The ids `ListApiKeys` reports for `api_id`.
async fn listed_ids(router: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    listed_keys(router, api_id)
        .await
        .iter()
        .map(|key| key["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the key table: the `CreateApiKey` request to send under each parent.
struct KeyRow {
    label: &'static str,
    description: Option<&'static str>,
    /// The validity the request asks for, in seconds from the call, or `None` for a
    /// request that carries no `expires` at all.
    validity: Option<i64>,
}

/// The key table: the rows differ along what the create reads — whether the request
/// carries a description, and how far off the expiration it asks for is, spanning the
/// documented validity window from just inside its lower bound to just inside its
/// upper one, plus the request that names no expiration at all.
fn key_rows() -> Vec<KeyRow> {
    vec![
        KeyRow {
            label: "no description and no expiration",
            description: None,
            validity: None,
        },
        KeyRow {
            label: "a description and the default expiration",
            description: Some("the events key"),
            validity: None,
        },
        KeyRow {
            label: "just inside the lower validity bound",
            description: Some("a day"),
            validity: Some(DAY + BOUND_SLACK),
        },
        KeyRow {
            label: "an expiration that is not on an hour boundary",
            description: Some("an odd instant"),
            validity: Some(30 * DAY + 11 * HOUR + 59),
        },
        KeyRow {
            label: "just inside the upper validity bound",
            description: None,
            validity: Some(365 * DAY - BOUND_SLACK),
        },
    ]
}

/// Rule 1: a key is minted, listed, updated and deleted under an API of either
/// resource family, and every answer comes from the stored key.
#[tokio::test]
async fn an_api_key_lifecycle_runs_on_an_api_of_either_family() {
    let router = router();

    for parent in parent_rows() {
        let api_id = create_parent(&router, &parent).await;

        for row in key_rows() {
            let case = format!("{} with {}", parent.label, row.label);

            let mut body = json!({});
            if let Some(description) = row.description {
                body["description"] = json!(description);
            }
            let before = unix_now();
            if let Some(validity) = row.validity {
                body["expires"] = json!(before + validity);
            }
            let created = create_key(&router, &api_id, body).await;
            let after = unix_now();
            assert_eq!(
                created.status, 200,
                "case {case}: expected CreateApiKey 200, got {created:?}"
            );
            let key = created.body["apiKey"].clone();
            let key_id = key["id"].as_str().unwrap_or_default().to_string();
            assert!(
                key_id.starts_with(KEY_VERSION_PREFIX),
                "case {case}: expected a minted {KEY_VERSION_PREFIX} key id, got {key:?}"
            );
            assert_eq!(
                key["description"].as_str(),
                row.description,
                "case {case}: expected the requested description back, got {key:?}"
            );

            // The expiration is the requested instant floored to the hour; a request
            // that names none takes the documented 7-day default, measured from the
            // handler's own clock read, which sits between this test's two.
            let expires = key["expires"].as_i64().unwrap_or_default();
            match row.validity {
                Some(validity) => assert_eq!(
                    expires,
                    floor_to_hour(before + validity),
                    "case {case}: expected the requested expiration floored to the hour, \
                     got {key:?}"
                ),
                None => assert!(
                    (floor_to_hour(before + DEFAULT_VALIDITY)
                        ..=floor_to_hour(after + DEFAULT_VALIDITY))
                        .contains(&expires),
                    "case {case}: expected the 7-day default expiration floored to the hour, \
                     got {key:?}"
                ),
            }
            assert_eq!(
                expires.rem_euclid(HOUR),
                0,
                "case {case}: expected an expiration on an hour boundary, got {key:?}"
            );
            assert_eq!(
                key["deletes"].as_i64(),
                Some(floor_to_hour(expires + RETENTION)),
                "case {case}: expected the deletion time one retention span past the \
                 expiration, got {key:?}"
            );

            // The listing reports the key the create stored, so the create answered
            // from state rather than from the request.
            let listed = listed_keys(&router, &api_id).await;
            assert!(
                listed.contains(&key),
                "case {case}: expected ListApiKeys to report the created key {key:?}, \
                 got {listed:?}"
            );

            // The update writes onto that same stored key, under the same apiId.
            let submitted = unix_now() + 45 * DAY;
            let updated = update_key(
                &router,
                &api_id,
                &key_id,
                json!({"description": "updated", "expires": submitted}),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: expected UpdateApiKey 200, got {updated:?}"
            );
            let updated_key = updated.body["apiKey"].clone();
            assert_eq!(
                updated_key["id"].as_str(),
                Some(key_id.as_str()),
                "case {case}: an update keeps the key's id, got {updated_key:?}"
            );
            assert_eq!(
                updated_key["description"].as_str(),
                Some("updated"),
                "case {case}: expected the submitted description, got {updated_key:?}"
            );
            assert_eq!(
                updated_key["expires"].as_i64(),
                Some(floor_to_hour(submitted)),
                "case {case}: expected the submitted expiration floored to the hour, \
                 got {updated_key:?}"
            );
            assert_eq!(
                updated_key["deletes"].as_i64(),
                Some(floor_to_hour(floor_to_hour(submitted) + RETENTION)),
                "case {case}: expected the deletion time one retention span past the \
                 updated expiration, got {updated_key:?}"
            );
            let listed = listed_keys(&router, &api_id).await;
            assert!(
                listed.contains(&updated_key),
                "case {case}: expected ListApiKeys to report the updated key \
                 {updated_key:?}, got {listed:?}"
            );

            // The delete removes it: the listing stops reporting it, and a second
            // delete finds no key to remove.
            let deleted = delete_key(&router, &api_id, &key_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case}: expected DeleteApiKey 200, got {deleted:?}"
            );
            assert!(
                !listed_ids(&router, &api_id).await.contains(&key_id),
                "case {case}: expected the deleted key to be gone, got {:?}",
                listed_ids(&router, &api_id).await
            );
            let again = delete_key(&router, &api_id, &key_id).await;
            assert_eq!(
                again.status, 404,
                "case {case}: expected a second DeleteApiKey to answer 404, got {again:?}"
            );
            assert_eq!(
                again.error_type, "NotFoundException",
                "case {case}: expected error type NotFoundException, got {again:?}"
            );
            assert_ne!(
                again.message, API_NOT_FOUND,
                "case {case}: the API is still there, so the absent key is what is \
                 reported, got {again:?}"
            );
        }

        // The API ends the row holding no keys, since every key minted on it was
        // deleted again.
        assert!(
            listed_ids(&router, &api_id).await.is_empty(),
            "case {}: expected no keys left, got {:?}",
            parent.label,
            listed_ids(&router, &api_id).await
        );
    }
}

/// Rule 2: an `apiId` that names no API of either family is a not-found on every
/// API-key operation, while an id longer than an AppSync API id is a bad request — and
/// an id either family really minted is served.
#[tokio::test]
async fn the_key_operations_refuse_only_an_api_id_that_names_no_api() {
    let router = router();

    let graphql_api_id = create_parent(
        &router,
        &ParentRow {
            label: "a GraphQL API",
            family: Family::Graphql,
            body: graphql_api_body("Api_keys_resolution", "API_KEY"),
        },
    )
    .await;
    let event_api_id = create_parent(
        &router,
        &ParentRow {
            label: "an Event API",
            family: Family::Event,
            body: event_api_body("Events_keys_resolution", &["API_KEY"]),
        },
    )
    .await;

    // (case, apiId, expected status). The rows span the id's length from one character
    // to well past the API-id length, plus the two ids this engine really holds — one
    // per family.
    let rows: Vec<(String, String, u16)> = vec![
        ("a GraphQL API's own id".to_string(), graphql_api_id.clone(), 200),
        ("an Event API's own id".to_string(), event_api_id.clone(), 200),
        ("one character".to_string(), "a".to_string(), 404),
        (
            "21 characters outside the id alphabet".to_string(),
            "nonexistent0000000000".to_string(),
            404,
        ),
        (
            "an id of the full length nothing created".to_string(),
            "b".repeat(API_ID_LEN),
            404,
        ),
        (
            "one past the API-id length".to_string(),
            "c".repeat(API_ID_LEN + 1),
            400,
        ),
        ("64 characters".to_string(), "d".repeat(64), 400),
    ];

    for (label, api_id, expected) in rows {
        // Every operation that takes the `apiId` label of an API key, each addressing
        // a key id of the form this service mints.
        let key_id = format!("{KEY_VERSION_PREFIX}{}", "e".repeat(API_ID_LEN));
        let calls: Vec<(&str, Answer)> = vec![
            (
                "CreateApiKey",
                create_key(&router, &api_id, json!({"description": label.clone()})).await,
            ),
            (
                "ListApiKeys",
                call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await,
            ),
            (
                "UpdateApiKey",
                update_key(
                    &router,
                    &api_id,
                    &key_id,
                    json!({"description": label.clone()}),
                )
                .await,
            ),
            ("DeleteApiKey", delete_key(&router, &api_id, &key_id).await),
        ];

        for (operation, answer) in calls {
            let case = format!("{operation} with an apiId of {label}");
            match expected {
                200 => {
                    // The API exists, so the only refusal left is about the KEY the
                    // request addresses: the two operations that create or list keys
                    // are served, the two that address one key by id report the
                    // absent key rather than an absent API.
                    if matches!(operation, "CreateApiKey" | "ListApiKeys") {
                        assert_eq!(
                            answer.status, 200,
                            "case {case}: expected status 200, got {answer:?}"
                        );
                    } else {
                        assert_eq!(
                            answer.status, 404,
                            "case {case}: expected the absent key to be reported, \
                             got {answer:?}"
                        );
                        assert_ne!(
                            answer.message, API_NOT_FOUND,
                            "case {case}: the API exists, so its absence must not be \
                             reported, got {answer:?}"
                        );
                    }
                }
                404 => {
                    assert_eq!(
                        answer.status, 404,
                        "case {case}: expected status 404, got {answer:?}"
                    );
                    assert_eq!(
                        answer.error_type, "NotFoundException",
                        "case {case}: expected error type NotFoundException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.message, API_NOT_FOUND,
                        "case {case}: expected AppSync's absent-API text, got {answer:?}"
                    );
                }
                _ => {
                    assert_eq!(
                        answer.status, 400,
                        "case {case}: expected status 400, got {answer:?}"
                    );
                    assert_eq!(
                        answer.error_type, "BadRequestException",
                        "case {case}: expected error type BadRequestException, got {answer:?}"
                    );
                    assert!(
                        answer.message.contains(&api_id),
                        "case {case}: expected the refusal to name the rejected id, \
                         got {answer:?}"
                    );
                }
            }
        }
    }

    // A deleted API holds no keys either: once the GraphQL API is gone, the same
    // `apiId` is the not-found case again.
    let deleted = call(&router, "DELETE", &format!("/v1/apis/{graphql_api_id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the GraphQL API: expected 200, got {deleted:?}"
    );
    let answer = create_key(&router, &graphql_api_id, json!({})).await;
    assert_eq!(
        answer.status, 404,
        "case CreateApiKey after the API was deleted: expected 404, got {answer:?}"
    );
    assert_eq!(
        answer.message, API_NOT_FOUND,
        "case CreateApiKey after the API was deleted: expected AppSync's absent-API text, \
         got {answer:?}"
    );
}

/// Rule 3: a key belongs to ONE API — an API lists only its own keys, and a key held
/// by another API, of either family, is not found under this one.
#[tokio::test]
async fn a_key_is_held_by_one_api_of_one_family() {
    let router = router();

    // Two APIs per family, so a key is checked against both another API of its own
    // family and one of the other.
    let mut apis: Vec<(&str, String)> = Vec::new();
    for (label, row) in [
        (
            "the first GraphQL API",
            ParentRow {
                label: "a GraphQL API",
                family: Family::Graphql,
                body: graphql_api_body("Api_keys_owner_one", "API_KEY"),
            },
        ),
        (
            "the second GraphQL API",
            ParentRow {
                label: "a GraphQL API",
                family: Family::Graphql,
                body: graphql_api_body("Api_keys_owner_two", "AWS_IAM"),
            },
        ),
        (
            "the first Event API",
            ParentRow {
                label: "an Event API",
                family: Family::Event,
                body: event_api_body("Events_keys_owner_one", &["API_KEY"]),
            },
        ),
        (
            "the second Event API",
            ParentRow {
                label: "an Event API",
                family: Family::Event,
                body: event_api_body("Events_keys_owner_two", &["AWS_IAM"]),
            },
        ),
    ] {
        apis.push((label, create_parent(&router, &row).await));
    }

    // Each API is given a different number of keys, so a listing that mixed two APIs'
    // keys up could not match the count either.
    let mut owned: Vec<(&str, String, Vec<String>)> = Vec::new();
    for (index, (label, api_id)) in apis.iter().enumerate() {
        let mut ids = Vec::new();
        for key_index in 0..=index {
            let created = create_key(
                &router,
                api_id,
                json!({"description": format!("{label} key {key_index}")}),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case seed a key under {label}: expected CreateApiKey 200, got {created:?}"
            );
            ids.push(
                created.body["apiKey"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        owned.push((label, api_id.clone(), ids));
    }

    // Every key id is minted independently, so no two APIs can share one.
    let all: Vec<&String> = owned.iter().flat_map(|(_, _, ids)| ids.iter()).collect();
    let unique: std::collections::BTreeSet<&&String> = all.iter().collect();
    assert_eq!(
        unique.len(),
        all.len(),
        "case every minted key: expected distinct ids, got {all:?}"
    );

    for (label, api_id, ids) in &owned {
        assert_eq!(
            &listed_ids(&router, api_id).await,
            ids,
            "case {label}: expected exactly its own keys {ids:?}, got {:?}",
            listed_ids(&router, api_id).await
        );

        // Another API's key is not addressable under this one, however its own
        // family compares.
        for (other_label, _, other_ids) in &owned {
            if other_label == label {
                continue;
            }
            for other_id in other_ids {
                let case = format!("{other_label}'s key {other_id} addressed under {label}");
                let updated = update_key(
                    &router,
                    api_id,
                    other_id,
                    json!({"description": "written across APIs"}),
                )
                .await;
                assert_eq!(
                    updated.status, 404,
                    "case {case}: expected UpdateApiKey 404, got {updated:?}"
                );
                let deleted = delete_key(&router, api_id, other_id).await;
                assert_eq!(
                    deleted.status, 404,
                    "case {case}: expected DeleteApiKey 404, got {deleted:?}"
                );
            }
        }
    }

    // Those refusals wrote nothing and removed nothing: every API still holds exactly
    // the keys it was given, with the descriptions it was given.
    for (label, api_id, ids) in &owned {
        let keys = listed_keys(&router, api_id).await;
        assert_eq!(
            keys.len(),
            ids.len(),
            "case {label}: expected {} keys left, got {keys:?}",
            ids.len()
        );
        for (key_index, key) in keys.iter().enumerate() {
            assert_eq!(
                key["description"].as_str(),
                Some(format!("{label} key {key_index}").as_str()),
                "case {label}: expected the stored description back, got {key:?}"
            );
        }
    }
}
