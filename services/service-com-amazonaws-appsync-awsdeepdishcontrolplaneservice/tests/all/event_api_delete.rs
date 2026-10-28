// SPDX-License-Identifier: CC0-1.0

//! `DeleteApi` ("Deletes an `Api` object"), end to end over HTTP.
//!
//! Two rules, each asserted over a TABLE that spans the class it generalises over
//! rather than the one request that exposed it:
//!
//! 1. deleting a resource removes the stored record and everything held under it, so
//!    every later read of that resource — or of one of its children — answers
//!    `NotFoundException` (404), while every OTHER stored resource is untouched;
//! 2. a delete addresses one resource family: the id's SHAPE decides a bad request
//!    (400, naming the rejected id) from a not-found, and an id that resolves to no
//!    `Api` — one nothing created, or one naming a GraphQL API — is a 404 that deletes
//!    nothing.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handler and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract suite's;
//! the Event API builders (`create_event_api`, `event_config`) are the `event_api`
//! suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::event_api::{create_event_api, event_config, AUTHENTICATION_TYPES};

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the
/// service's own copy, and an id of exactly this length is well formed.
const API_ID_LEN: usize = 26;

/// The fixed text AppSync answers for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// One row of the delete table: the Event API to create and the children to create
/// under it before it is deleted.
struct DeleteRow {
    label: &'static str,
    name: &'static str,
    /// The authorization types the API's own providers set up.
    auth_types: &'static [&'static str],
    /// The channel namespaces to create under the API, by name.
    namespaces: &'static [&'static str],
    /// How many API keys to create under the API.
    keys: usize,
}

/// The delete table: the rows differ along what the rule generalises over — how many
/// resources the API holds under it (none, one, several), of which kinds (channel
/// namespaces, API keys, both), and how its authorization providers are configured.
fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            label: "an API holding nothing",
            name: "Events_delete_bare",
            auth_types: &["API_KEY"],
            namespaces: &[],
            keys: 0,
        },
        DeleteRow {
            label: "an API holding one channel namespace",
            name: "Events_delete_one_namespace",
            auth_types: &["AWS_IAM"],
            namespaces: &["orders"],
            keys: 0,
        },
        DeleteRow {
            label: "an API holding API keys only",
            name: "Events_delete_keys",
            auth_types: &["API_KEY"],
            namespaces: &[],
            keys: 2,
        },
        DeleteRow {
            label: "an API holding several namespaces and a key",
            name: "Events_delete_many",
            auth_types: &["API_KEY", "AWS_IAM"],
            namespaces: &["orders", "payments", "shipping"],
            keys: 1,
        },
        DeleteRow {
            label: "an API setting up every modelled authorization type",
            name: "Events_delete_all_providers",
            auth_types: AUTHENTICATION_TYPES,
            namespaces: &["telemetry"],
            keys: 1,
        },
    ]
}

/// Create one Event API whose providers set up `auth_types` and whose three auth-mode
/// lists draw on all of them, and return the `apiId` the service minted for it.
async fn seed_event_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    auth_types: &[&str],
) -> String {
    let body = json!({
        "name": name,
        "eventConfig": event_config(auth_types, auth_types, auth_types, auth_types, None),
    });
    let answer = create_event_api(router, body).await;
    assert_eq!(
        answer.status, 200,
        "case seed Event API {name:?}: expected CreateApi 200, got {answer:?}"
    );
    answer.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CreateChannelNamespace` under `api_id`, leaving the API's default authorization
/// configuration in place.
async fn create_namespace(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        Some(json!({"name": name})),
    )
    .await
}

/// `CreateApiKey` under `api_id` — an API of either family is given its keys through
/// this one path.
async fn create_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    description: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": description})),
    )
    .await
}

/// `DeleteApi` on `api_id`.
async fn delete_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
) -> Answer {
    call(router, "DELETE", &format!("/v2/apis/{api_id}"), None).await
}

/// The `apiId`s `ListApis` reports, in the order it reports them.
async fn listed_api_ids(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(
        answer.status, 200,
        "case list the APIs: expected ListApis 200, got {answer:?}"
    );
    answer.body["apis"]
        .as_array()
        .map(|apis| {
            apis.iter()
                .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an answer carries the empty `DeleteApiResponse`: no members, which on the
/// wire is either no body at all or an object with nothing in it.
fn is_empty_response(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(|members| members.is_empty())
}

/// Rule 1: a delete removes the API and every resource held under it, and touches
/// nothing else.
#[tokio::test]
async fn deleting_an_api_removes_it_and_everything_held_under_it() {
    let router = router();

    for row in delete_rows() {
        let case = row.label;

        // The API to delete, and a second API that must survive it: nothing about one
        // stored resource may depend on another being deleted.
        let doomed = seed_event_api(&router, row.name, row.auth_types).await;
        let survivor = seed_event_api(&router, "Events_delete_survivor", row.auth_types).await;

        for name in row.namespaces {
            for api_id in [&doomed, &survivor] {
                let created = create_namespace(&router, api_id, name).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected CreateChannelNamespace {name:?} 200, \
                     got {created:?}"
                );
            }
        }
        let mut doomed_keys: Vec<String> = Vec::new();
        for index in 0..row.keys {
            let created = create_key(&router, &doomed, &format!("key {index}")).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected CreateApiKey 200, got {created:?}"
            );
            doomed_keys.push(
                created.body["apiKey"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
            let survivor_key = create_key(&router, &survivor, &format!("key {index}")).await;
            assert_eq!(
                survivor_key.status, 200,
                "case {case:?}: expected the survivor's CreateApiKey 200, got {survivor_key:?}"
            );
        }

        let deleted = delete_api(&router, &doomed).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteApi 200, got {deleted:?}"
        );
        assert!(
            is_empty_response(&deleted.body),
            "case {case:?}: expected the empty DeleteApiResponse, got {deleted:?}"
        );

        // Every later read of the deleted API, of its children, and a second delete of
        // it, answers the absent API it now is.
        let mut absent: Vec<(String, &str, String)> = vec![
            ("GetApi".to_string(), "GET", format!("/v2/apis/{doomed}")),
            (
                "DeleteApi again".to_string(),
                "DELETE",
                format!("/v2/apis/{doomed}"),
            ),
            (
                "ListChannelNamespaces".to_string(),
                "GET",
                format!("/v2/apis/{doomed}/channelNamespaces"),
            ),
            (
                "ListApiKeys".to_string(),
                "GET",
                format!("/v1/apis/{doomed}/apikeys"),
            ),
        ];
        for name in row.namespaces {
            absent.push((
                format!("GetChannelNamespace {name}"),
                "GET",
                format!("/v2/apis/{doomed}/channelNamespaces/{name}"),
            ));
        }
        for key_id in &doomed_keys {
            absent.push((
                format!("DeleteApiKey {key_id}"),
                "DELETE",
                format!("/v1/apis/{doomed}/apikeys/{key_id}"),
            ));
        }
        for (read, method, path) in absent {
            let answer = call(&router, method, &path, None).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected {read} after the delete to answer 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "NotFoundException",
                "case {case:?}: expected {read} to answer NotFoundException, got {answer:?}"
            );
        }

        // The absent API is reported by AppSync's own fixed text on the reporting path.
        let read = call(&router, "GET", &format!("/v2/apis/{doomed}"), None).await;
        assert_eq!(
            read.message, API_NOT_FOUND,
            "case {case:?}: expected AppSync's absent-API text, got {read:?}"
        );

        // The survivor kept everything it held.
        let survived = call(&router, "GET", &format!("/v2/apis/{survivor}"), None).await;
        assert_eq!(
            survived.status, 200,
            "case {case:?}: expected the second API to survive the delete, got {survived:?}"
        );
        for name in row.namespaces {
            let held = call(
                &router,
                "GET",
                &format!("/v2/apis/{survivor}/channelNamespaces/{name}"),
                None,
            )
            .await;
            assert_eq!(
                held.status, 200,
                "case {case:?}: expected the survivor to keep namespace {name:?}, got {held:?}"
            );
        }
        let survivor_keys = call(
            &router,
            "GET",
            &format!("/v1/apis/{survivor}/apikeys"),
            None,
        )
        .await;
        assert_eq!(
            survivor_keys.status, 200,
            "case {case:?}: expected the survivor to keep its keys, got {survivor_keys:?}"
        );
        assert_eq!(
            survivor_keys.body["apiKeys"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            row.keys,
            "case {case:?}: expected the survivor's {} keys, got {survivor_keys:?}",
            row.keys
        );

        // The listing pages over the stored records, so the deleted API is gone from it
        // and the survivor is still there.
        let listed = listed_api_ids(&router).await;
        assert!(
            !listed.contains(&doomed),
            "case {case:?}: expected the deleted API gone from ListApis, got {listed:?}"
        );
        assert!(
            listed.contains(&survivor),
            "case {case:?}: expected the surviving API still listed, got {listed:?}"
        );

        // Clean the survivor up, so each row starts from the store its own creates
        // filled: the delete that empties it is itself the rule under test.
        let cleaned = delete_api(&router, &survivor).await;
        assert_eq!(
            cleaned.status, 200,
            "case {case:?}: expected the survivor's delete to answer 200, got {cleaned:?}"
        );
        let remaining = listed_api_ids(&router).await;
        assert!(
            remaining.is_empty(),
            "case {case:?}: expected no API left after both deletes, got {remaining:?}"
        );
    }
}

/// Rule 2: the delete addresses an `Api` and nothing else — the id's shape decides
/// bad-request from not-found, and an id naming a GraphQL API deletes nothing.
#[tokio::test]
async fn the_delete_resolves_an_event_api_id_only() {
    let router = router();

    let event_api_id = seed_event_api(&router, "Events_delete_reader", &["API_KEY"]).await;
    // A GraphQL API is deleted by the other delete operation, so its id names no `Api`.
    let graphql_api_id = create_api(&router, "Api_graphql_delete").await;

    // (label, id, expected status). The rows span the id's length from one character to
    // past the API-id length, plus the two ids this engine really holds.
    let rows: Vec<(String, String, u16)> = vec![
        ("a GraphQL API's id".to_string(), graphql_api_id.clone(), 404),
        ("one character".to_string(), "a".to_string(), 404),
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
        (
            "the Event API's own id".to_string(),
            event_api_id.clone(),
            200,
        ),
    ];

    for (case, id, expected) in rows {
        let answer = delete_api(&router, &id).await;
        assert_eq!(
            answer.status, expected,
            "case {case}: expected status {expected}, got {answer:?}"
        );
        match expected {
            200 => assert!(
                is_empty_response(&answer.body),
                "case {case}: expected the empty DeleteApiResponse, got {answer:?}"
            ),
            404 => {
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
                    answer.error_type, "BadRequestException",
                    "case {case}: expected error type BadRequestException, got {answer:?}"
                );
                assert!(
                    answer.message.contains(&id),
                    "case {case}: expected the refusal to name the rejected id, got {answer:?}"
                );
            }
        }
    }

    // The refused and not-found calls deleted nothing: the GraphQL API is still there,
    // and only the Event API this test deleted is gone.
    let graphql = call(&router, "GET", &format!("/v1/apis/{graphql_api_id}"), None).await;
    assert_eq!(
        graphql.status, 200,
        "case the GraphQL API after a DeleteApi naming it: expected it untouched, got {graphql:?}"
    );
    let listed = listed_api_ids(&router).await;
    assert!(
        listed.is_empty(),
        "case after deleting the only Event API: expected an empty ListApis, got {listed:?}"
    );
}
