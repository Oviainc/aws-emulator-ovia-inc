// SPDX-License-Identifier: CC0-1.0

//! What `CreateApi` can ANSWER: the outcomes its own model declares, and nothing else.
//!
//! The rule, one level above the request that filed the issue (one Event API carrying an
//! `API_KEY` provider):
//!
//! > A create of an Event API is served from this service's own state — every request the
//! > model admits is answered `200` with the `api` the call persisted, and every request it
//! > refuses carries one of the error shapes `CreateApi` declares, at that shape's modelled
//! > status. The emulator's unimplemented channel (`501`, `NotImplemented`) is not among
//! > them, so no request of either class can be answered with it.
//!
//! Three suites assert it, each over a TABLE that spans what the rule generalises over:
//!
//! 1. the admitted creates — the character classes and both length bounds of `ApiName`'s
//!    modelled pattern, both bounds of `TagMap`'s modelled size, and an `ownerContact`
//!    present or absent — each answered `200` and reported identically by BOTH readers of
//!    the record (`GetApi` and `ListApis`);
//! 2. the refused creates — an absent required member at the top level and nested, a name
//!    outside the modelled length or pattern, a tag map past its modelled size, and the two
//!    authorization-configuration refusals — each at a modelled status, carrying a
//!    non-empty error code that is not the scaffold's, and persisting nothing;
//! 3. the service quota — creates up to the per-Region cap are admitted, the one past it is
//!    the modelled `ServiceQuotaExceededException` (402), and the store still holds exactly
//!    the APIs that were created.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers and
//! the shared engine together.
//!
//! The harness (`call`, `router`, `Answer`) is the error-contract suite's; the `eventConfig`
//! builder is the Event-API suite's.

use serde_json::{json, Map, Value};

use crate::error_contract::{call, router, Answer};
use crate::event_api::{create_event_api, event_config};

/// The number of characters an AppSync API id has (`state::API_ID_LEN`).
const API_ID_LEN: usize = 26;

/// `ApiName`'s published length bounds (`@length` 1..50,
/// `smithy_query(query="operation", operation="CreateApi")`).
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 50;

/// `TagMap`'s published size bounds (`@length` 1..50,
/// `smithy_query(query="shape", shape="TagMap")`).
const TAGS_MIN: usize = 1;
const TAGS_MAX: usize = 50;

/// The APIs an account holds in one Region — AppSync's published per-Region API quota, and
/// the bound `CreateApi` reports `ServiceQuotaExceededException` past
/// (`ops::create_api::APIS_PER_REGION`).
const APIS_PER_REGION: usize = 25;

/// Every status `CreateApi` declares an error at, in the order the model lists them:
/// `BadRequestException` 400, `UnauthorizedException` 401, `ServiceQuotaExceededException`
/// 402, `ConcurrentModificationException` 409, `InternalFailureException` 500
/// (`smithy_query(query="operation", operation="CreateApi")`).
const MODELLED_ERROR_STATUSES: &[u16] = &[400, 401, 402, 409, 500];

/// The error code and status of the emulator's unimplemented channel
/// (`crate::scaffolded`). `CreateApi` models neither, so neither may appear.
const SCAFFOLD_CODE: &str = "NotImplemented";
const SCAFFOLD_STATUS: u16 = 501;

/// AppSync's own refusal for a member the model rules out outright, as this service words
/// it (`service::member_refusal`).
fn invalid_member_message(member: &str) -> String {
    format!("Invalid {member} given.")
}

/// An `eventConfig` set up through one `API_KEY` provider, which every authorization mode
/// list then draws on.
fn api_key_config() -> Value {
    event_config(&["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None)
}

/// A well-formed `CreateApi` body for `name`.
fn create_body(name: &str) -> Value {
    json!({"name": name, "eventConfig": api_key_config()})
}

/// A tag map of `count` entries, keyed and valued inside `TagKey`'s and `TagValue`'s
/// modelled patterns (`smithy_query(query="shape", shape="TagMap")`).
fn tag_map(count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("team{index}"), json!(format!("events{index}")));
    }
    Value::Object(tags)
}

/// The `apiId` of every `Api` the service holds, in listing order.
async fn listed_api_ids(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let listed = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(
        listed.status, 200,
        "case list the stored APIs: expected 200, got {listed:?}"
    );
    assert!(
        listed.body["nextToken"].is_null(),
        "case list the stored APIs: expected one whole page, got {listed:?}"
    );
    listed.body["apis"]
        .as_array()
        .map(|apis| {
            apis.iter()
                .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `Api` the listing reports under `api_id`, or `Value::Null` when it reports none.
async fn listed_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
) -> Value {
    let listed = call(router, "GET", "/v2/apis", None).await;
    listed.body["apis"]
        .as_array()
        .and_then(|apis| {
            apis.iter()
                .find(|api| api["apiId"].as_str() == Some(api_id))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert an answer is not the emulator's unimplemented channel.
fn assert_not_scaffolded(case: &str, answer: &Answer) {
    assert_ne!(
        answer.status, SCAFFOLD_STATUS,
        "case {case:?}: CreateApi models no {SCAFFOLD_STATUS}, got {answer:?}"
    );
    assert_ne!(
        answer.error_type, SCAFFOLD_CODE,
        "case {case:?}: CreateApi models no {SCAFFOLD_CODE} error, got {answer:?}"
    );
}

/// One row of the admitted-create table.
struct AdmittedRow {
    case: &'static str,
    name: String,
    owner_contact: Option<&'static str>,
    tags: Option<Value>,
}

impl AdmittedRow {
    /// This row as a `CreateApi` request body.
    fn body(&self) -> Value {
        let mut body = create_body(&self.name);
        if let Some(contact) = self.owner_contact {
            body["ownerContact"] = json!(contact);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        body
    }
}

/// The admitted creates: the rows differ along the character classes `ApiName`'s pattern
/// admits (`^[A-Za-z0-9_\-\ ]+$`), both of its length bounds, both of `TagMap`'s size
/// bounds, and whether the request carries an `ownerContact`.
fn admitted_rows() -> Vec<AdmittedRow> {
    vec![
        AdmittedRow {
            case: "letters only, carrying neither tags nor an ownerContact",
            name: "Events".to_string(),
            owner_contact: None,
            tags: None,
        },
        AdmittedRow {
            case: "every character class the modelled name pattern admits",
            name: "Ev3nt_api-name with spaces".to_string(),
            owner_contact: Some("platform.team"),
            tags: None,
        },
        AdmittedRow {
            case: "a name of the modelled minimum length",
            name: "E".repeat(NAME_MIN_LEN),
            owner_contact: None,
            tags: Some(tag_map(TAGS_MIN)),
        },
        AdmittedRow {
            case: "a name of the modelled maximum length",
            name: "E".repeat(NAME_MAX_LEN),
            owner_contact: None,
            tags: None,
        },
        AdmittedRow {
            case: "a tag map of the modelled maximum size",
            name: "Events_tagged".to_string(),
            owner_contact: Some("owner.one"),
            tags: Some(tag_map(TAGS_MAX)),
        },
    ]
}

/// Suite 1: every admitted create is answered `200` from the record it persisted, and both
/// readers of that record report it.
#[tokio::test]
async fn an_admitted_create_is_answered_from_the_record_both_readers_report() {
    let router = router();

    for row in admitted_rows() {
        let case = row.case;
        let created = create_event_api(&router, row.body()).await;
        assert_not_scaffolded(case, &created);
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the modelled success status 200, got {created:?}"
        );
        let api = created.body["api"].clone();
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            api_id.chars().count(),
            API_ID_LEN,
            "case {case:?}: expected a minted apiId of {API_ID_LEN} characters, got {api:?}"
        );
        assert_eq!(
            api["name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the requested name back, got {api:?}"
        );
        assert_eq!(
            api["tags"],
            row.tags.clone().unwrap_or_else(|| json!({})),
            "case {case:?}: expected the requested tags back, got {api:?}"
        );

        // The two readers of the stored record answer the body the create answered.
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetApi 200 for the created API, got {read:?}"
        );
        assert_eq!(
            read.body["api"], api,
            "case {case:?}: expected GetApi to answer the created API {api:?}, got {read:?}"
        );
        assert_eq!(
            listed_api(&router, &api_id).await,
            api,
            "case {case:?}: expected ListApis to report the created API {api:?}"
        );
    }

    // Every row created one API, and the listing holds exactly those.
    assert_eq!(
        listed_api_ids(&router).await.len(),
        admitted_rows().len(),
        "case the whole table: expected one stored API per admitted create"
    );
}

/// One row of the refused-create table.
struct RefusedRow {
    case: String,
    body: Value,
    status: u16,
    /// The modelled error shape, where the refusal is the SERVICE's own; `None` where the
    /// framework's constraint report words it, whose code is asserted non-empty only.
    error_type: Option<&'static str>,
    message: Option<String>,
}

/// The refused creates: an absent required member at the top level and nested inside
/// `eventConfig`, a name outside the modelled length bounds and outside the modelled
/// pattern, a tag map one past its modelled size, and the two authorization-configuration
/// refusals (a mode no provider sets up, a provider without the configuration its type
/// needs).
fn refused_rows() -> Vec<RefusedRow> {
    let mut rows = Vec::new();

    // An absent required member, at the top level and nested one level down.
    for member in ["name", "eventConfig"] {
        let mut body = create_body("Events_absent");
        body.as_object_mut()
            .expect("the request body is an object")
            .remove(member);
        rows.push(RefusedRow {
            case: format!("the required member {member} absent"),
            body,
            status: 400,
            error_type: Some("BadRequestException"),
            message: Some(invalid_member_message(member)),
        });
    }
    for member in [
        "authProviders",
        "connectionAuthModes",
        "defaultPublishAuthModes",
        "defaultSubscribeAuthModes",
    ] {
        let mut body = create_body("Events_absent_nested");
        body["eventConfig"]
            .as_object_mut()
            .expect("the event configuration is an object")
            .remove(member);
        rows.push(RefusedRow {
            case: format!("the required eventConfig member {member} absent"),
            body,
            status: 400,
            error_type: Some("BadRequestException"),
            message: Some(invalid_member_message(member)),
        });
    }

    // A name outside the modelled length bounds, on either side, and one holding a
    // character the modelled pattern does not admit.
    for (case, name) in [
        ("a name one short of the modelled minimum length", String::new()),
        (
            "a name one past the modelled maximum length",
            "E".repeat(NAME_MAX_LEN + 1),
        ),
        (
            "a name holding a character the modelled pattern excludes",
            "Events!".to_string(),
        ),
    ] {
        rows.push(RefusedRow {
            case: case.to_string(),
            body: create_body(&name),
            status: 400,
            error_type: None,
            message: None,
        });
    }

    // A tag map one past the modelled maximum size.
    let mut over_tagged = create_body("Events_over_tagged");
    over_tagged["tags"] = tag_map(TAGS_MAX + 1);
    rows.push(RefusedRow {
        case: format!("a tag map of {} entries, one past the modelled maximum", TAGS_MAX + 1),
        body: over_tagged,
        status: 400,
        error_type: None,
        message: None,
    });

    // An authorization mode whose type the API's own providers do not set up.
    let mut unconfigured_mode = create_body("Events_unconfigured_mode");
    unconfigured_mode["eventConfig"]["defaultPublishAuthModes"] = json!([{"authType": "AWS_IAM"}]);
    rows.push(RefusedRow {
        case: "an authorization mode no configured provider sets up".to_string(),
        body: unconfigured_mode,
        status: 400,
        error_type: Some("BadRequestException"),
        message: None,
    });

    // A provider whose authorization type is set up through a member of its own, without
    // that member.
    let mut bare_provider = create_body("Events_bare_provider");
    bare_provider["eventConfig"] = event_config(
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        None,
    );
    bare_provider["eventConfig"]["authProviders"] = json!([{"authType": "OPENID_CONNECT"}]);
    rows.push(RefusedRow {
        case: "a provider without the configuration its authorization type needs".to_string(),
        body: bare_provider,
        status: 400,
        error_type: Some("BadRequestException"),
        message: None,
    });

    rows
}

/// Suite 2: a refused create answers a MODELLED error — never the scaffold's — and leaves
/// the store as it found it.
#[tokio::test]
async fn a_refused_create_answers_a_modelled_error_and_persists_nothing() {
    let router = router();

    // One stored API to refuse against, so "persists nothing" is asserted against a store
    // that is not empty either side of the refusal.
    let seeded = create_event_api(&router, create_body("Events_seed")).await;
    assert_eq!(
        seeded.status, 200,
        "case seed one API: expected 200, got {seeded:?}"
    );
    let before = listed_api_ids(&router).await;

    for row in refused_rows() {
        let case = row.case.as_str();
        let answer = create_event_api(&router, row.body.clone()).await;
        assert_not_scaffolded(case, &answer);
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected status {}, got {answer:?}",
            row.status
        );
        assert!(
            MODELLED_ERROR_STATUSES.contains(&answer.status),
            "case {case:?}: expected one of the modelled error statuses \
             {MODELLED_ERROR_STATUSES:?}, got {answer:?}"
        );
        assert!(
            !answer.error_type.is_empty(),
            "case {case:?}: expected the refusal to name an error shape, got {answer:?}"
        );
        if let Some(expected) = row.error_type {
            assert_eq!(
                answer.error_type, expected,
                "case {case:?}: expected error type {expected}, got {answer:?}"
            );
        }
        if let Some(expected) = &row.message {
            assert_eq!(
                &answer.message, expected,
                "case {case:?}: expected the refusal text {expected:?}, got {answer:?}"
            );
        }
        assert!(
            answer.body["api"].is_null(),
            "case {case:?}: a refused create answers no API, got {answer:?}"
        );
        assert_eq!(
            listed_api_ids(&router).await,
            before,
            "case {case:?}: a refused create persists nothing"
        );
    }

    // The refusals changed nothing, so an admitted create still succeeds afterwards.
    let accepted = create_event_api(&router, create_body("Events_after_refusals")).await;
    assert_eq!(
        accepted.status, 200,
        "case an admitted create after the refusals: expected 200, got {accepted:?}"
    );
}

/// Suite 3: the per-Region API quota — the create past it is the modelled quota refusal,
/// and the store still holds exactly the APIs that were created.
#[tokio::test]
async fn a_create_past_the_api_quota_is_the_modelled_quota_refusal() {
    let router = router();

    let mut created = Vec::new();
    for index in 0..APIS_PER_REGION {
        let case = format!("create {} of {APIS_PER_REGION}", index + 1);
        let answer = create_event_api(&router, create_body(&format!("Events_{index}"))).await;
        assert_not_scaffolded(&case, &answer);
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a create inside the quota is admitted, got {answer:?}"
        );
        created.push(
            answer.body["api"]["apiId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Every create past the bound is refused the same way, whatever it is named.
    for name in ["Events_past_the_bound", "Events_past_the_bound_again"] {
        let answer = create_event_api(&router, create_body(name)).await;
        assert_not_scaffolded(name, &answer);
        assert_eq!(
            answer.status, 402,
            "case {name:?}: expected the modelled quota status 402, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "ServiceQuotaExceededException",
            "case {name:?}: expected ServiceQuotaExceededException, got {answer:?}"
        );
        assert!(
            answer.message.contains(&APIS_PER_REGION.to_string()),
            "case {name:?}: expected the refusal to state the quota {APIS_PER_REGION}, \
             got {answer:?}"
        );
        assert!(
            answer.body["api"].is_null(),
            "case {name:?}: a refused create answers no API, got {answer:?}"
        );
    }

    // The refusals wrote nothing: the store holds exactly the admitted creates, each still
    // readable under the id it was minted with.
    assert_eq!(
        listed_api_ids(&router).await,
        created,
        "case the whole quota: expected the store to hold exactly the admitted creates"
    );
    for api_id in &created {
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case read {api_id:?} back: expected 200, got {read:?}"
        );
    }
}
