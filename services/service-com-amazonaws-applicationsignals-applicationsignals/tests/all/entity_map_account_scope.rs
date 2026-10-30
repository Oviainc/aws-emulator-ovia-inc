// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ACCOUNT an entity map states, as `ListEntityEvents` takes it.
//!
//! `Entity` is a string-to-string map whose own doc lists an account id among its fields —
//! "`AwsAccountId` specifies the account where this object is in"
//! (`smithy_query(query="doc", shape="ListEntityEventsInput", member="Entity")`) — beside the
//! attributes that say WHICH object it is (`Type`, `Name`, `Environment`, `ResourceType`,
//! `Identifier`).
//!
//! The rule under test: an account id a request supplies is judged against the one account
//! this store holds and is never matched as part of the entity's identity, wherever the model
//! admits it — so an entity map naming this store's own account reports exactly the history
//! naming no account reports, one naming any other account reports nothing, and the identity
//! the records are matched and the page cursor is tagged on is the map WITHOUT that entry.
//!
//! Every request goes over the real `restJson1` surface the router mounts in production, so
//! the routes, statuses and JSON bodies asserted here are the ones a client sees, and the
//! records the events are read out of are created over that same surface. (The harness below
//! is this file's own, as in every other suite in this crate: the suites are separate modules
//! of one test target and none of them exports its helpers.)

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The `Attributes` entry an entity map states its account in, as the member's doc names it.
const ACCOUNT_ATTRIBUTE: &str = "AwsAccountId";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Whole seconds since the epoch: an instant goes on the wire as seconds with a fraction, so
/// a window named in whole seconds round-trips exactly while the records' own sub-second
/// instants fall strictly inside it.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as f64
}

/// The identity of the service entity the seeded objectives monitor.
fn service_entity() -> Value {
    json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" })
}

/// The identity of the resource entity one seeded objective monitors: the form the `Entity`
/// member's doc spells for a resource.
fn resource_entity() -> Value {
    json!({
        "Type": "AWS::Resource",
        "ResourceType": "AWS::DynamoDB::Table",
        "Identifier": "orders-table"
    })
}

/// Create one objective monitoring `key_attributes`, so the store holds a dated record
/// against that entity.
async fn create_slo(router: &MultiServiceRouter, name: &str, key_attributes: &Value) {
    let (status, answer) = call(
        router,
        "POST",
        "/slo",
        Some(json!({
            "Name": name,
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": key_attributes,
                "MetricType": "LATENCY"
            }}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the SLO create to answer 200, got {status} with {answer}"
    );
}

/// A store holding three dated records: two objectives on one service entity and one on a
/// resource entity, so a history is held against entities of two different `Type`s.
async fn seeded_router() -> MultiServiceRouter {
    let router = router();
    create_slo(&router, "checkout-latency", &service_entity()).await;
    create_slo(&router, "checkout-availability", &service_entity()).await;
    create_slo(&router, "orders-table-latency", &resource_entity()).await;
    router
}

/// `POST /events` for `entity` over `[start, end]`, with the page controls `query` carries.
async fn list_events(
    router: &MultiServiceRouter,
    entity: &Value,
    start: f64,
    end: f64,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/events{query}"),
        Some(json!({ "Entity": entity, "StartTime": start, "EndTime": end })),
    )
    .await
}

/// `entity` with the account entry added, or as it stands when no account is named.
fn in_account(entity: &Value, account: Option<&str>) -> Value {
    let mut entity = entity.clone();
    if let Some(account) = account {
        entity[ACCOUNT_ATTRIBUTE] = json!(account);
    }
    entity
}

/// The `ChangeEvents` of one answer, as `(EventName, Entity)` pairs in the order reported.
fn reported(answer: &Value) -> Vec<(String, Value)> {
    answer["ChangeEvents"]
        .as_array()
        .expect("ChangeEvents is a required output member")
        .iter()
        .map(|event| {
            (
                event["EventName"].as_str().unwrap_or_default().to_string(),
                event["Entity"].clone(),
            )
        })
        .collect()
}

/// One queried entity map, without the account entry.
struct Entity {
    label: &'static str,
    attributes: Value,
}

/// The entity maps the rule is asserted over.
///
/// The rows differ along what the rule generalises over: the entity `Type` (`Service`,
/// `AWS::Resource`), which attributes the identity names (the full service identity, the
/// resource pair, a partial identity, the type alone — which selects every service entity at
/// once) and therefore how many records one answer spans. Each row has room for the account
/// entry within the modelled `Attributes` bound (`length 1..4`,
/// `smithy_query(query="shape", shape="Attributes")`).
fn entities() -> Vec<Entity> {
    vec![
        Entity {
            label: "the full service identity",
            attributes: service_entity(),
        },
        Entity {
            label: "a partial service identity",
            attributes: json!({ "Type": "Service", "Name": "checkout" }),
        },
        Entity {
            label: "the entity type alone",
            attributes: json!({ "Type": "Service" }),
        },
        Entity {
            label: "a resource entity",
            attributes: json!({
                "Type": "AWS::Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table"
            }),
        },
    ]
}

/// One account id an entity map may state, and whether stating it selects this store's
/// records.
struct Account {
    label: &'static str,
    /// `None` leaves the entry out of the map entirely.
    id: Option<String>,
    selects: bool,
}

/// The account ids the rule is asserted over.
///
/// The rows differ along whether the entry is present at all, whether the id is this store's
/// own, and — for the ids that are not — how far from it they are, from one digit away to an
/// id sharing no digit with it.
fn accounts() -> Vec<Account> {
    vec![
        Account {
            label: "the account entry absent",
            id: None,
            selects: true,
        },
        Account {
            label: "this store's own account",
            id: Some(engine::emulator_account_id().to_string()),
            selects: true,
        },
        Account {
            label: "an account one digit from this store's",
            id: Some("000000000001".to_string()),
            selects: false,
        },
        Account {
            label: "the documented example account",
            id: Some("123456789012".to_string()),
            selects: false,
        },
        Account {
            label: "an account sharing no digit with this store's",
            id: Some("999999999999".to_string()),
            selects: false,
        },
    ]
}

/// The account an entity map states is judged as an account, not matched as an identifying
/// attribute: this store's own account reports exactly what naming no account reports, and
/// any other account reports nothing.
#[tokio::test]
async fn an_account_an_entity_map_states_selects_this_store_or_nothing() {
    let router = seeded_router().await;
    let now = now_seconds();
    let (start, end) = (now - 3_600.0, now + 3_600.0);
    let this_account = engine::emulator_account_id();

    for entity in entities() {
        let (status, baseline) =
            list_events(&router, &entity.attributes, start, end, "").await;
        let case = format!("{}, no account entry", entity.label);
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {baseline}"
        );
        let events = reported(&baseline);
        assert!(
            !events.is_empty(),
            "case {case:?}: expected the seeded records to be reported, got {}",
            baseline["ChangeEvents"]
        );

        for account in accounts() {
            let case = format!("{}, {}", entity.label, account.label);
            let (status, answer) = list_events(
                &router,
                &in_account(&entity.attributes, account.id.as_deref()),
                start,
                end,
                "",
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer}"
            );
            if account.selects {
                assert_eq!(
                    reported(&answer),
                    events,
                    "case {case:?}: expected the same history as naming no account \
                     ({events:?}), got {:?}",
                    reported(&answer)
                );
                for (index, event) in answer["ChangeEvents"]
                    .as_array()
                    .expect("ChangeEvents is a required output member")
                    .iter()
                    .enumerate()
                {
                    // The reported entity is the identity the record is held under, which
                    // carries no account entry; the account the record is in is the event's
                    // own `AccountId`.
                    assert!(
                        event["Entity"].get(ACCOUNT_ATTRIBUTE).is_none(),
                        "case {case:?}: event {index} reports the stored identity, got {:?}",
                        event["Entity"]
                    );
                    assert_eq!(
                        event["AccountId"].as_str(),
                        Some(this_account),
                        "case {case:?}: event {index} is in this store's account, got {:?}",
                        event["AccountId"]
                    );
                }
            } else {
                assert_eq!(
                    answer["ChangeEvents"],
                    json!([]),
                    "case {case:?}: expected another account to report nothing, got {:?}",
                    answer["ChangeEvents"]
                );
            }
        }
    }
}

/// The page cursor is a boundary in the history the RESOLVED identity selects, so a token
/// issued to a read that named this store's own account resumes the read that names none, and
/// the pages together are the single-page history either spelling reports.
#[tokio::test]
async fn a_page_cursor_resumes_a_read_naming_the_same_entity_in_this_account() {
    let router = seeded_router().await;
    let now = now_seconds();
    let (start, end) = (now - 3_600.0, now + 3_600.0);
    let entity = service_entity();
    let this_account = engine::emulator_account_id().to_string();

    let (status, whole) = list_events(&router, &entity, start, end, "?MaxResults=250").await;
    let expected = reported(&whole);
    assert_eq!(
        status, 200,
        "case \"the whole history\": expected HTTP 200, got {status} with {whole}"
    );
    assert_eq!(
        expected.len(),
        2,
        "case \"the whole history\": expected the two seeded records' events, got {expected:?}"
    );

    // Each row names the account on one side of the page boundary only, so the token has to
    // resolve across both spellings of one selection.
    let rows = [
        (
            "the account named on the first page only",
            Some(this_account.clone()),
            None,
        ),
        (
            "the account named on the second page only",
            None,
            Some(this_account.clone()),
        ),
        (
            "the account named on both pages",
            Some(this_account.clone()),
            Some(this_account),
        ),
    ];

    for (label, first, second) in rows {
        let (status, page) = list_events(
            &router,
            &in_account(&entity, first.as_deref()),
            start,
            end,
            "?MaxResults=1",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for the first page, got {status} with {page}"
        );
        let mut collected = reported(&page);
        let token = page["NextToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {label:?}: expected a token for the page after the first, got {:?}",
            page["NextToken"]
        );

        let (status, next) = list_events(
            &router,
            &in_account(&entity, second.as_deref()),
            start,
            end,
            &format!("?MaxResults=1&NextToken={token}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for the second page, got {status} with {next}"
        );
        collected.extend(reported(&next));
        assert_eq!(
            collected, expected,
            "case {label:?}: expected the paged history to be the whole one ({expected:?}), \
             got {collected:?}"
        );
    }
}
