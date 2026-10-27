// SPDX-License-Identifier: CC0-1.0

//! The Event API (`Api`) update path, end to end over HTTP.
//!
//! Three rules, each asserted over a TABLE that spans the class it generalises over
//! rather than the one request that exposed the defect:
//!
//! * an update REPLACES the stored API's settable configuration with what the request
//!   carries (`name`, `ownerContact` and the whole `eventConfig`) and leaves its minted
//!   identity alone (the `apiId`, the ARN, the endpoint hostnames, the creation instant,
//!   the tags, the X-Ray flag and the associated web ACL), answering from the record it
//!   wrote — so the reader and the listing report the same body afterwards;
//! * the configuration an update leaves behind has to be well formed on the same
//!   grounds as the one the create was accepted with, and a refused update persists
//!   nothing;
//! * an update resolves an `Api` id only: an id that names a GraphQL API — or nothing
//!   at all — is a not-found, while an id longer than an AppSync API id is a bad
//!   request.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handler and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract suite's,
//! and `event_config` / `create_event_api` / `AUTHENTICATION_TYPES` the Event API
//! create suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::event_api::{create_event_api, event_config, AUTHENTICATION_TYPES};
use crate::graphql_api_lifecycle::REGION;

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The number of characters an AppSync API id has; an id of exactly this length is
/// well formed, and `state::API_ID_LEN` is the service's own copy.
const API_ID_LEN: usize = 26;

/// The fixed text AppSync answers for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// One row of the update table: the `UpdateApi` request to send to the seeded API.
struct UpdateRow {
    label: &'static str,
    name: String,
    owner_contact: Option<&'static str>,
    event_config: Value,
}

impl UpdateRow {
    /// This row as an `UpdateApi` request body.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name, "eventConfig": self.event_config});
        if let Some(contact) = self.owner_contact {
            body["ownerContact"] = json!(contact);
        }
        body
    }
}

/// The update table: the rows differ along what the rule generalises over — the name
/// (including each published bound of `ApiName`'s `@length` 1..50,
/// `smithy_query(query="operation", operation="UpdateApi")`), whether `ownerContact` is
/// carried at all (so the stored one is replaced on one row and cleared on the next),
/// how many authorization providers the configuration sets up and which types, which of
/// them each of the three mode lists draws on, and whether the configuration carries a
/// `logConfig`. Applied in order to ONE API, so each row also updates an API a previous
/// update already rewrote.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            label: "a new name and a new ownerContact, one provider",
            name: "Events_renamed".to_string(),
            owner_contact: Some("platform.team"),
            event_config: event_config(&["AWS_IAM"], &["AWS_IAM"], &["AWS_IAM"], &["AWS_IAM"], None),
        },
        UpdateRow {
            label: "no ownerContact member, so the stored contact is cleared",
            name: "Events renamed again".to_string(),
            owner_contact: None,
            event_config: event_config(
                &["API_KEY", "AWS_IAM"],
                &["API_KEY"],
                &["AWS_IAM"],
                &["API_KEY", "AWS_IAM"],
                None,
            ),
        },
        UpdateRow {
            label: "every modelled authorization type, each mode list on a different one",
            name: "Events-all-providers".to_string(),
            owner_contact: Some("owner.two"),
            event_config: event_config(
                AUTHENTICATION_TYPES,
                &["API_KEY", "AWS_IAM"],
                &["AMAZON_COGNITO_USER_POOLS"],
                &["OPENID_CONNECT", "AWS_LAMBDA"],
                // `EventLogConfig` requires both members
                // (`smithy_query(query="shape", shape="EventLogConfig")`); `ERROR` is
                // one of `EventLogLevel`'s declared values.
                Some(json!({
                    "logLevel": "ERROR",
                    "cloudWatchLogsRoleArn":
                        format!("arn:aws:iam::{ACCOUNT}:role/service-role/appsync-events"),
                })),
            ),
        },
        UpdateRow {
            label: "a configuration that carries no logConfig, so the stored one is gone",
            name: "E".to_string(),
            owner_contact: None,
            event_config: event_config(
                &["OPENID_CONNECT"],
                &["OPENID_CONNECT"],
                &["OPENID_CONNECT"],
                &["OPENID_CONNECT"],
                None,
            ),
        },
        UpdateRow {
            // `ApiName`'s published maximum length.
            label: "a name of exactly the modelled maximum length",
            name: "N".repeat(50),
            owner_contact: Some("owner.three"),
            event_config: event_config(
                &["AWS_LAMBDA", "AMAZON_COGNITO_USER_POOLS"],
                &["AWS_LAMBDA"],
                &["AMAZON_COGNITO_USER_POOLS"],
                &["AWS_LAMBDA", "AMAZON_COGNITO_USER_POOLS"],
                None,
            ),
        },
    ]
}

/// Seed one Event API, with tags so the update can be shown to leave them alone, and
/// return the `Api` the create answered with.
async fn seed(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Value {
    let created = create_event_api(
        router,
        json!({
            "name": name,
            "ownerContact": "seed.owner",
            "tags": {"team": "events", "tier": "gold"},
            "eventConfig": event_config(
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                Some(json!({
                    "logLevel": "ALL",
                    "cloudWatchLogsRoleArn":
                        format!("arn:aws:iam::{ACCOUNT}:role/service-role/appsync-seed"),
                })),
            ),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed {name:?}: expected CreateApi 200, got {created:?}"
    );
    created.body["api"].clone()
}

/// Send one `UpdateApi` for `api_id`.
async fn update_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    body: Value,
) -> Answer {
    call(router, "POST", &format!("/v2/apis/{api_id}"), Some(body)).await
}

/// The `authType` of every entry of an `authProviders` or auth-mode list, in order.
fn auth_types(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["authType"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 1: the update replaces the settable configuration, keeps the minted identity,
/// and answers from the record it wrote — which the reader and the listing report.
#[tokio::test]
async fn an_update_replaces_the_configuration_and_keeps_the_minted_identity() {
    let router = router();
    let created = seed(&router, "Events_updated").await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    for row in update_rows() {
        let case = row.label;
        let answer = update_api(&router, &api_id, row.body()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateApi 200, got {answer:?}"
        );
        let api = answer.body["api"].clone();

        // The settable members are now what the request carried — and `ownerContact`,
        // which the request may omit, is ABSENT on a request that omitted it rather
        // than carried over.
        assert_eq!(
            api["name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the requested name, got {api:?}"
        );
        assert_eq!(
            api["ownerContact"].as_str(),
            row.owner_contact,
            "case {case:?}: expected the requested ownerContact, got {api:?}"
        );
        let requested = &row.event_config;
        let reported = &api["eventConfig"];
        assert_eq!(
            auth_types(&reported["authProviders"]),
            auth_types(&requested["authProviders"]),
            "case {case:?}: expected the submitted providers back in order, got {api:?}"
        );
        for member in [
            "connectionAuthModes",
            "defaultPublishAuthModes",
            "defaultSubscribeAuthModes",
        ] {
            assert_eq!(
                auth_types(&reported[member]),
                auth_types(&requested[member]),
                "case {case:?}: expected {member} back in order, got {api:?}"
            );
        }
        assert_eq!(
            reported["logConfig"], requested["logConfig"],
            "case {case:?}: expected the submitted logConfig, got {api:?}"
        );

        // The identity the create minted is untouched by the update.
        for member in ["apiId", "apiArn", "dns", "created", "tags", "xrayEnabled"] {
            assert_eq!(
                api[member], created[member],
                "case {case:?}: expected {member} to survive the update as {:?}, got {api:?}",
                created[member]
            );
        }
        assert_eq!(
            api["apiArn"].as_str(),
            Some(format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}").as_str()),
            "case {case:?}: expected the API's own ARN, got {api:?}"
        );
        assert!(
            api["wafWebAclArn"].is_null(),
            "case {case:?}: an update associates no web ACL, got {api:?}"
        );

        // The reader answers the record the update stored, so it reports this body.
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetApi 200, got {read:?}"
        );
        assert_eq!(
            read.body["api"], api,
            "case {case:?}: expected GetApi to answer the updated API {api:?}, got {read:?}"
        );

        // So does the listing, which projects the same stored record.
        let listed = call(&router, "GET", "/v2/apis", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListApis 200, got {listed:?}"
        );
        let entry = listed.body["apis"]
            .as_array()
            .and_then(|apis| apis.iter().find(|entry| entry["apiId"] == api["apiId"]))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            entry, api,
            "case {case:?}: expected ListApis to report the updated API {api:?}, got {listed:?}"
        );
    }
}

/// An update touches ONE API: a second API created alongside keeps its own
/// configuration, so the write is addressed by the `apiId` in the URI and nothing else.
#[tokio::test]
async fn an_update_leaves_every_other_api_alone() {
    let router = router();
    let first = seed(&router, "Events_first").await;
    let second = seed(&router, "Events_second").await;
    let first_id = first["apiId"].as_str().unwrap_or_default().to_string();
    let second_id = second["apiId"].as_str().unwrap_or_default().to_string();

    let answer = update_api(
        &router,
        &first_id,
        json!({
            "name": "Events_only_the_first",
            "eventConfig": event_config(&["AWS_IAM"], &["AWS_IAM"], &["AWS_IAM"], &["AWS_IAM"], None),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case update the first API: expected 200, got {answer:?}"
    );

    let read = call(&router, "GET", &format!("/v2/apis/{second_id}"), None).await;
    assert_eq!(
        read.body["api"], second,
        "case the untouched API: expected it unchanged as {second:?}, got {read:?}"
    );
}

/// Rule 2: the configuration an update leaves behind is refused on the same grounds as
/// the create's, and a refused update persists nothing.
#[tokio::test]
async fn an_update_to_a_malformed_configuration_is_refused_and_persists_nothing() {
    let router = router();
    let created = seed(&router, "Events_refusals").await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    // (label, the eventConfig to submit). The rows span the refusal classes the shared
    // rule states: a mode naming a type no provider sets up, on each of the three mode
    // lists and over every modelled type the one provider does not set up; one type set
    // up twice; a provider missing the configuration member its type is set up through;
    // and an OIDC issuer that is not an absolute URL.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for member in [
        "connectionAuthModes",
        "defaultPublishAuthModes",
        "defaultSubscribeAuthModes",
    ] {
        for unconfigured in AUTHENTICATION_TYPES.iter().filter(|ty| **ty != "API_KEY") {
            let mut config = event_config(&["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None);
            config[member] = json!([{"authType": "API_KEY"}, {"authType": unconfigured}]);
            rows.push((format!("{member} naming {unconfigured}"), config));
        }
    }
    let mut twice = event_config(&["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None);
    twice["authProviders"] = json!([{"authType": "API_KEY"}, {"authType": "API_KEY"}]);
    rows.push(("one type set up twice".to_string(), twice));
    for auth_type in AUTHENTICATION_TYPES
        .iter()
        .filter(|ty| !matches!(**ty, "API_KEY" | "AWS_IAM"))
    {
        let mut bare = event_config(&[auth_type], &[auth_type], &[auth_type], &[auth_type], None);
        bare["authProviders"] = json!([{"authType": auth_type}]);
        rows.push((format!("{auth_type} without its configuration"), bare));
    }
    let mut issuer = event_config(
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        None,
    );
    issuer["authProviders"] =
        json!([{"authType": "OPENID_CONNECT", "openIDConnectConfig": {"issuer": "issuer"}}]);
    rows.push(("an issuer that is not a URL".to_string(), issuer));

    for (case, config) in rows {
        let answer = update_api(
            &router,
            &api_id,
            json!({"name": "Events_refused", "eventConfig": config}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {case}: expected error type BadRequestException, got {answer:?}"
        );
        assert!(
            answer.body["api"].is_null(),
            "case {case}: a refused update answers no API, got {answer:?}"
        );

        // Nothing was written: the API still reads back exactly as it was created.
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.body["api"], created,
            "case {case}: expected the stored API untouched as {created:?}, got {read:?}"
        );
    }
}

/// Rule 3: the update resolves an `Api` id only — the id's SHAPE decides bad-request
/// from not-found, and an id that names a GraphQL API names no `Api`.
#[tokio::test]
async fn the_update_resolves_an_event_api_id_only() {
    let router = router();
    let created = seed(&router, "Events_resolution").await;
    let event_api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    // A GraphQL API is created by the other create operation, so its id names no `Api`.
    let graphql_api_id = create_api(&router, "Api_graphql_update").await;

    // (label, id, expected status). The rows span the id's length from one character to
    // past the API-id length, plus the two ids this engine really holds.
    let rows: Vec<(String, String, u16)> = vec![
        (
            "the Event API's own id".to_string(),
            event_api_id.clone(),
            200,
        ),
        ("a GraphQL API's id".to_string(), graphql_api_id, 404),
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
    ];

    for (case, id, expected) in rows {
        let answer = update_api(
            &router,
            &id,
            json!({
                "name": "Events_resolved",
                "eventConfig": event_config(
                    &["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None,
                ),
            }),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case}: expected status {expected}, got {answer:?}"
        );
        match expected {
            200 => assert_eq!(
                answer.body["api"]["apiId"].as_str(),
                Some(event_api_id.as_str()),
                "case {case}: expected the stored API, got {answer:?}"
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
}
