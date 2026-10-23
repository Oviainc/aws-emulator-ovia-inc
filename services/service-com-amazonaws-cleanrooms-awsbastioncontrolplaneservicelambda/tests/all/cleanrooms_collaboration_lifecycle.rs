// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms collaboration/membership lifecycle.
//!
//! Every request goes over the wire: an `http::Request` is dispatched through the
//! `MultiServiceRouter` the production router uses, so the restJson1 routing,
//! deserialization, constraint validation, handler and serialization all run.
//!
//! The rule under test is that each operation computes its response from persisted
//! state, so the tables below vary the inputs the rule generalises over (value, count,
//! type, ordering, page size) rather than re-testing one recorded shape.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents; the collaboration creator in these
/// tests. Read from the vendored runtime so the test cannot drift from the emulator.
pub(crate) fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// The region the test signs for, so the ARNs the service mints are predictable.
pub(crate) const REGION: &str = "us-east-1";

/// Every `MemberAbility` the model declares (shape `MemberAbility`), so the tables
/// below cover the whole catalog rather than only the ability the issue cited.
pub(crate) const ALL_MEMBER_ABILITIES: [&str; 4] = [
    "CAN_QUERY",
    "CAN_RECEIVE_RESULTS",
    "CAN_RUN_JOB",
    "CAN_EXPORT_QUERY_ANALYSIS_LOG",
];

/// Every `AnalyticsEngine` the model declares (shape `AnalyticsEngine`).
pub(crate) const ALL_ANALYTICS_ENGINES: [&str; 2] = ["SPARK", "CLEAN_ROOMS_SQL"];

/// Every `CollaborationQueryLogStatus` / `CollaborationJobLogStatus` the model declares.
pub(crate) const ALL_LOG_STATUSES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The published bounds of `ListCollaborations$maxResults` / `ListMemberships$maxResults`
/// (model `MaxResults`, range 1..100).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// One emulator: the single shared engine behind the router, as the production router
/// builds it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the mounted service holds, so a test can seed the store the
    /// handlers read — used only to fill a store up to its cap.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// This service's store, as the handlers reach it.
    pub(crate) fn store(&self) -> engine::ServiceState<cleanrooms_state::CleanRoomsState> {
        cleanrooms_state::service_state(&*self.engine).expect("the cleanrooms store")
    }

    /// Dispatch one request and return its status and JSON body (`Value::Null` for an
    /// empty body, which is what a 204 carries).
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let host = format!("cleanrooms.{REGION}.amazonaws.com");
        let payload = body
            .map(|b| serde_json::to_vec(&b).expect("serialize request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            // The SigV4 credential scope is the routing key AND the region the ARNs
            // are minted in; signatures are not checked (authentication is off).
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLEANROOM/20260101/{REGION}/cleanrooms/aws4_request, \
                     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} -> {status}: body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, json)
    }

    pub(crate) async fn create_collaboration(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/collaborations", Some(body)).await
    }

    pub(crate) async fn get_collaboration(&self, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/collaborations/{id}"), None).await
    }

    pub(crate) async fn delete_collaboration(&self, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/collaborations/{id}"), None)
            .await
    }

    pub(crate) async fn create_membership(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/memberships", Some(body)).await
    }

    pub(crate) async fn get_membership(&self, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/memberships/{id}"), None).await
    }

    pub(crate) async fn delete_membership(&self, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/memberships/{id}"), None).await
    }
}

/// The wire error code a failure response carries (`__type` on restJson1).
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a failure response carries. restJson1 serializes the modelled `message`
/// member; an SDK surfaces it as `Message`, so both spellings are accepted here and the
/// test asserts on the composed text rather than on which one the wire used.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Whether `id` is in the form the model's `UUID` shape declares:
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
pub(crate) fn is_model_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// A minimal valid CreateCollaboration body.
pub(crate) fn minimal_collaboration(name: &str) -> Value {
    json!({
        "name": name,
        "creatorDisplayName": "creator",
        "creatorMemberAbilities": ["CAN_QUERY"],
        "members": [],
        "queryLogStatus": "ENABLED",
    })
}

/// One row of the create/read round-trip table.
struct CollaborationCase {
    label: &'static str,
    body: Value,
}

fn collaboration_cases() -> Vec<CollaborationCase> {
    let member_a = json!({
        "accountId": "111111111111",
        "memberAbilities": ["CAN_RECEIVE_RESULTS"],
        "displayName": "partner-a",
    });
    let member_b = json!({
        "accountId": "222222222222",
        "memberAbilities": ["CAN_QUERY", "CAN_RUN_JOB"],
        "displayName": "partner-b",
        "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
    });
    vec![
        CollaborationCase {
            label: "minimal: no description, no members, one ability",
            body: minimal_collaboration("demo-collab"),
        },
        CollaborationCase {
            label: "every declared member ability, DISABLED query log",
            body: json!({
                "name": "all-abilities",
                "description": "a collaboration that exercises every ability",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ALL_MEMBER_ABILITIES,
                "members": [],
                "queryLogStatus": ALL_LOG_STATUSES[1],
                "jobLogStatus": ALL_LOG_STATUSES[0],
            }),
        },
        CollaborationCase {
            label: "one initial member, SPARK engine, tags, metrics on",
            body: json!({
                "name": "one-member",
                "description": "",
                "creatorDisplayName": "creator-display",
                "creatorMemberAbilities": ["CAN_QUERY", "CAN_RECEIVE_RESULTS"],
                "members": [member_a.clone()],
                "queryLogStatus": "ENABLED",
                "analyticsEngine": ALL_ANALYTICS_ENGINES[0],
                "tags": { "env": "test", "owner": "fixer" },
                "isMetricsEnabled": true,
            }),
        },
        CollaborationCase {
            label: "two initial members, CLEAN_ROOMS_SQL engine, encryption metadata",
            body: json!({
                "name": "two-members",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_RUN_JOB"],
                "creatorMLMemberAbilities": {
                    // Catalog: model shape `CustomMLMemberAbility`.
                    "customMLMemberAbilities": ["CAN_RECEIVE_MODEL_OUTPUT"],
                },
                "members": [member_a, member_b],
                "queryLogStatus": "DISABLED",
                "jobLogStatus": "DISABLED",
                "analyticsEngine": ALL_ANALYTICS_ENGINES[1],
                "dataEncryptionMetadata": {
                    "allowCleartext": true,
                    "allowDuplicates": false,
                    "allowJoinsOnColumnsWithDifferentNames": true,
                    "preserveNulls": false,
                },
                // Catalog: model shape `AutoApprovedChangeType`.
                "autoApprovedChangeRequestTypes": ["ADD_MEMBER"],
                "creatorPaymentConfiguration": {
                    "queryCompute": { "isResponsible": true },
                    "jobCompute": { "isResponsible": true },
                },
                "isMetricsEnabled": false,
            }),
        },
        CollaborationCase {
            label: "name at the declared maximum length (100)",
            body: json!({
                "name": "x".repeat(100),
                "creatorDisplayName": "y".repeat(100),
                "creatorMemberAbilities": ["CAN_EXPORT_QUERY_ANALYSIS_LOG"],
                "members": [],
                "queryLogStatus": "ENABLED",
            }),
        },
    ]
}

#[tokio::test]
async fn create_collaboration_persists_and_get_collaboration_returns_it_unchanged() {
    let emulator = Emulator::new();

    for case in collaboration_cases() {
        let (status, created) = emulator.create_collaboration(case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateCollaboration 200, got {status} with {created}",
            case.label
        );

        let collaboration = created
            .get("collaboration")
            .unwrap_or_else(|| panic!("case {:?}: no `collaboration` in {created}", case.label));
        let id = collaboration["id"].as_str().unwrap_or_else(|| {
            panic!("case {:?}: no `collaboration.id` in {created}", case.label)
        });

        assert!(
            is_model_uuid(id),
            "case {:?}: expected a 36-char lowercase-hex UUID id, got {id:?}",
            case.label
        );
        assert_eq!(
            collaboration["arn"].as_str(),
            Some(format!("arn:aws:cleanrooms:{REGION}:{}:collaboration/{id}", account_id()).as_str()),
            "case {:?}: unexpected arn, got {:?}",
            case.label,
            collaboration["arn"]
        );
        assert_eq!(
            collaboration["creatorAccountId"].as_str(),
            Some(account_id()),
            "case {:?}: expected the emulator account as creator, got {:?}",
            case.label,
            collaboration["creatorAccountId"]
        );
        assert_eq!(
            collaboration["memberStatus"].as_str(),
            Some("INVITED"),
            "case {:?}: the creator holds no membership yet, so it is INVITED, got {:?}",
            case.label,
            collaboration["memberStatus"]
        );
        assert_eq!(
            collaboration["createTime"], collaboration["updateTime"],
            "case {:?}: a freshly created collaboration has createTime == updateTime, got {:?} and {:?}",
            case.label, collaboration["createTime"], collaboration["updateTime"]
        );
        assert!(
            collaboration.get("membershipId").is_none(),
            "case {:?}: no membership exists yet, so membershipId must be absent, got {:?}",
            case.label,
            collaboration.get("membershipId")
        );

        // Every member the request supplied is echoed from STATE, not from the request:
        // read it back through GetCollaboration and require the identical body.
        let (status, fetched) = emulator.get_collaboration(id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetCollaboration 200, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            fetched["collaboration"], *collaboration,
            "case {:?}: GetCollaboration must return what CreateCollaboration returned, got {:?}",
            case.label, fetched["collaboration"]
        );

        // Declared request members must survive the round trip with their own values.
        for (member, expected) in [
            ("name", case.body.get("name")),
            ("creatorDisplayName", case.body.get("creatorDisplayName")),
            ("queryLogStatus", case.body.get("queryLogStatus")),
            ("jobLogStatus", case.body.get("jobLogStatus")),
            ("analyticsEngine", case.body.get("analyticsEngine")),
            ("isMetricsEnabled", case.body.get("isMetricsEnabled")),
            (
                "dataEncryptionMetadata",
                case.body.get("dataEncryptionMetadata"),
            ),
        ] {
            if let Some(expected) = expected {
                assert_eq!(
                    collaboration.get(member),
                    Some(expected),
                    "case {:?}: member {member} must be the stored request value, got {:?}",
                    case.label,
                    collaboration.get(member)
                );
            }
        }
        // The input member `autoApprovedChangeRequestTypes` is reported as
        // `autoApprovedChangeTypes` on the Collaboration shape.
        if let Some(expected) = case.body.get("autoApprovedChangeRequestTypes") {
            assert_eq!(
                collaboration.get("autoApprovedChangeTypes"),
                Some(expected),
                "case {:?}: autoApprovedChangeTypes must mirror the request, got {:?}",
                case.label,
                collaboration.get("autoApprovedChangeTypes")
            );
        }
        // `description` carries the model's `default ""`.
        assert_eq!(
            collaboration["description"].as_str(),
            Some(
                case.body
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
            "case {:?}: unexpected description, got {:?}",
            case.label,
            collaboration["description"]
        );
    }
}

#[tokio::test]
async fn get_collaboration_refuses_an_id_that_is_not_stored() {
    let emulator = Emulator::new();
    let (_, created) = emulator
        .create_collaboration(minimal_collaboration("present"))
        .await;
    let stored = created["collaboration"]["id"].as_str().unwrap().to_string();

    // Syntactically valid ids (36-char lowercase hex) that no CreateCollaboration
    // minted. GetCollaboration declares no 404, and a caller's authorization to a
    // collaboration lives in the collaboration, so an id it cannot reach is refused as
    // an authorization failure: AccessDeniedException (403).
    let absent = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0123456f-89ab-cdef-0123-456789abcdef",
    ];
    for id in absent {
        let (status, body) = emulator.get_collaboration(id).await;
        assert_eq!(
            status, 403,
            "case {id:?}: expected 403 for an unknown collaboration, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {id:?}: expected AccessDeniedException, got {body}"
        );
    }

    // And the stored one is still readable: a refused read changed nothing.
    let (status, _) = emulator.get_collaboration(&stored).await;
    assert_eq!(
        status, 200,
        "case {stored:?}: the stored collaboration must still be readable, got {status}"
    );
}

#[tokio::test]
async fn list_collaborations_pages_every_collaboration_exactly_once() {
    let emulator = Emulator::new();

    // Seven collaborations, so the page sizes below divide the set evenly and unevenly.
    let mut created: Vec<String> = Vec::new();
    for n in 0..7 {
        let (status, body) = emulator
            .create_collaboration(minimal_collaboration(&format!("collab-{n}")))
            .await;
        assert_eq!(status, 200, "case {n}: CreateCollaboration failed: {body}");
        created.push(body["collaboration"]["id"].as_str().unwrap().to_string());
    }

    // Rows differ in page size, including both published bounds of `maxResults`.
    for max_results in [MAX_RESULTS_MIN, 2, 3, 7, 8, MAX_RESULTS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &token {
                Some(t) => format!("/collaborations?maxResults={max_results}&nextToken={t}"),
                None => format!("/collaborations?maxResults={max_results}"),
            };
            let (status, body) = emulator.call("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: expected 200, got {status} with {body}"
            );
            let page = body["collaborationList"].as_array().unwrap_or_else(|| {
                panic!("case maxResults={max_results}: no collaborationList in {body}")
            });
            assert!(
                page.len() as i64 <= max_results,
                "case maxResults={max_results}: page carried {} rows, more than asked",
                page.len()
            );
            for summary in page {
                seen.push(summary["id"].as_str().unwrap().to_string());
            }
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= 20,
                "case maxResults={max_results}: pagination did not terminate"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case maxResults={max_results}: paging must yield every collaboration once, in creation order"
        );
    }

    // Unpaged: no maxResults returns the whole set and no token.
    let (status, body) = emulator.call("GET", "/collaborations", None).await;
    assert_eq!(status, 200, "unpaged list failed: {body}");
    let all: Vec<String> = body["collaborationList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        all, created,
        "case unpaged: expected every collaboration in creation order, got {all:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case unpaged: a complete page must carry no nextToken, got {:?}",
        body.get("nextToken")
    );
}

#[tokio::test]
async fn list_collaborations_refuses_a_max_results_outside_the_declared_range() {
    let emulator = Emulator::new();
    // One past each published bound of `MaxResults` (range 1..100).
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = emulator
            .call("GET", &format!("/collaborations?maxResults={max_results}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: expected a 400 refusal, got {status} with {body}"
        );
    }
    // And each bound itself is accepted.
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) = emulator
            .call("GET", &format!("/collaborations?maxResults={max_results}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results}: a published bound must be accepted, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn list_collaborations_refuses_a_next_token_it_never_handed_out() {
    let emulator = Emulator::new();
    emulator
        .create_collaboration(minimal_collaboration("only"))
        .await;

    for token in ["not-hex", "6e6f742d612d6b6e6f776e2d6964", ""] {
        let (status, body) = emulator
            .call("GET", &format!("/collaborations?nextToken={token}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: expected a 400 refusal, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={token:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn list_collaborations_filters_on_member_status() {
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    for n in 0..3 {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration(&format!("filtered-{n}")))
            .await;
        ids.push(created["collaboration"]["id"].as_str().unwrap().to_string());
    }
    // The caller joins exactly one of the three, so the filter has rows on both sides:
    // ACTIVE is the joined one, INVITED the two it still owes a membership.
    let joined = ids[1].clone();
    let (status, response) = emulator
        .create_membership(json!({
            "collaborationIdentifier": joined,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(status, 200, "CreateMembership failed: {response}");

    // Catalog: model shape `FilterableMemberStatus` (INVITED | ACTIVE).
    for (member_status, expected) in [("ACTIVE", vec![joined.clone()]), ("INVITED", vec![ids[0].clone(), ids[2].clone()])] {
        let (status, body) = emulator
            .call(
                "GET",
                &format!("/collaborations?memberStatus={member_status}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case memberStatus={member_status}: expected 200, got {status} with {body}"
        );
        let rows = body["collaborationList"].as_array().unwrap();
        let seen: Vec<String> = rows
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            seen, expected,
            "case memberStatus={member_status}: expected {expected:?}, got {seen:?}"
        );
        for row in rows {
            assert_eq!(
                row["memberStatus"].as_str(),
                Some(member_status),
                "case memberStatus={member_status}: a filtered row carried {:?}",
                row["memberStatus"]
            );
        }
    }
}

#[tokio::test]
async fn create_collaboration_refuses_a_members_list_that_cannot_be_represented() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, Value)> = vec![
        (
            "members includes the creator's own account",
            json!([{ "accountId": account_id(), "memberAbilities": ["CAN_QUERY"], "displayName": "me" }]),
        ),
        (
            "members lists one account twice",
            json!([
                { "accountId": "111111111111", "memberAbilities": ["CAN_QUERY"], "displayName": "a" },
                { "accountId": "111111111111", "memberAbilities": ["CAN_RUN_JOB"], "displayName": "a-again" },
            ]),
        ),
    ];
    for (label, members) in cases {
        let mut body = minimal_collaboration("bad-members");
        body["members"] = members;
        let (status, response) = emulator.create_collaboration(body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a 400 refusal, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {response}"
        );
    }

    // The accepted counterpart: two DISTINCT partner accounts, neither the creator.
    let mut body = minimal_collaboration("good-members");
    body["members"] = json!([
        { "accountId": "111111111111", "memberAbilities": ["CAN_QUERY"], "displayName": "a" },
        { "accountId": "222222222222", "memberAbilities": ["CAN_RUN_JOB"], "displayName": "b" },
    ]);
    let (status, response) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "case \"two distinct partner accounts\": expected 200, got {status} with {response}"
    );
}

#[tokio::test]
async fn create_collaboration_caps_the_members_list_it_will_process() {
    let emulator = Emulator::new();

    // `MemberList` is declared `len 0..` with no maximum, so the service enforces its
    // own bound on the work one request can ask for and reports it through the
    // `ServiceQuotaExceededException` (402) the operation declares. Rows straddle the
    // bound: at it (accepted) and one past it (refused).
    let cap = 1000usize;
    let member = |n: usize| {
        json!({
            // AccountId: len 12..12, pattern ^\d+$.
            "accountId": format!("{:012}", n + 1),
            "memberAbilities": ["CAN_QUERY"],
            "displayName": format!("partner-{n}"),
        })
    };
    for (count, expected) in [(cap, 200u16), (cap + 1, 402)] {
        let mut body = minimal_collaboration("capped");
        body["members"] = Value::Array((0..count).map(member).collect());
        let (status, response) = emulator.create_collaboration(body).await;
        assert_eq!(
            status, expected,
            "case members={count}: expected {expected}, got {status} with {response}"
        );
        if expected != 200 {
            assert_eq!(
                error_code(&response),
                "ServiceQuotaExceededException",
                "case members={count}: expected ServiceQuotaExceededException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn create_membership_copies_the_collaboration_from_state_and_refuses_a_second_join() {
    let emulator = Emulator::new();

    // Rows differ in the collaboration being joined and in the membership-only
    // settings the request carries.
    let collaborations: Vec<(&str, Value, Value)> = vec![
        (
            "minimal collaboration, minimal membership",
            minimal_collaboration("join-minimal"),
            json!({ "queryLogStatus": "ENABLED" }),
        ),
        (
            "every ability, membership with job log + metrics",
            json!({
                "name": "join-rich",
                "description": "joined",
                "creatorDisplayName": "rich-creator",
                "creatorMemberAbilities": ALL_MEMBER_ABILITIES,
                "members": [],
                "queryLogStatus": "DISABLED",
                "analyticsEngine": ALL_ANALYTICS_ENGINES[0],
            }),
            json!({
                "queryLogStatus": "DISABLED",
                "jobLogStatus": "ENABLED",
                "isMetricsEnabled": true,
                "tags": { "team": "data" },
                "paymentConfiguration": {
                    "queryCompute": { "isResponsible": true },
                    "jobCompute": { "isResponsible": false },
                },
            }),
        ),
    ];

    for (label, collaboration_body, membership_extra) in collaborations {
        let (_, created) = emulator.create_collaboration(collaboration_body.clone()).await;
        let collaboration = created["collaboration"].clone();
        let collaboration_id = collaboration["id"].as_str().unwrap().to_string();

        let mut body = membership_extra.clone();
        body["collaborationIdentifier"] = json!(collaboration_id);
        let (status, response) = emulator.create_membership(body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateMembership 200, got {status} with {response}"
        );
        let membership = response["membership"].clone();
        let membership_id = membership["id"].as_str().unwrap().to_string();

        assert!(
            is_model_uuid(&membership_id),
            "case {label:?}: expected a 36-char lowercase-hex UUID id, got {membership_id:?}"
        );
        assert_eq!(
            membership["arn"].as_str(),
            Some(
                format!("arn:aws:cleanrooms:{REGION}:{}:membership/{membership_id}", account_id())
                    .as_str()
            ),
            "case {label:?}: unexpected membership arn, got {:?}",
            membership["arn"]
        );
        // Copied FROM the stored collaboration, not from the request.
        for (member, expected) in [
            ("collaborationId", &collaboration["id"]),
            ("collaborationArn", &collaboration["arn"]),
            ("collaborationName", &collaboration["name"]),
            (
                "collaborationCreatorAccountId",
                &collaboration["creatorAccountId"],
            ),
            (
                "collaborationCreatorDisplayName",
                &collaboration["creatorDisplayName"],
            ),
        ] {
            assert_eq!(
                &membership[member], expected,
                "case {label:?}: {member} must come from the stored collaboration, got {:?}",
                membership[member]
            );
        }
        assert_eq!(
            membership["status"].as_str(),
            Some("ACTIVE"),
            "case {label:?}: a new membership is ACTIVE, got {:?}",
            membership["status"]
        );
        assert_eq!(
            membership["memberAbilities"], collaboration_body["creatorMemberAbilities"],
            "case {label:?}: memberAbilities must come from the collaboration's creator spec, got {:?}",
            membership["memberAbilities"]
        );
        assert_eq!(
            membership["queryLogStatus"], membership_extra["queryLogStatus"],
            "case {label:?}: queryLogStatus is the membership's own request value, got {:?}",
            membership["queryLogStatus"]
        );

        // Round trip through the reader.
        let (status, fetched) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetMembership 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["membership"], membership,
            "case {label:?}: GetMembership must return what CreateMembership returned, got {:?}",
            fetched["membership"]
        );

        // The collaboration now reports the caller's membership, computed from the index.
        let (_, reread) = emulator.get_collaboration(&collaboration_id).await;
        assert_eq!(
            reread["collaboration"]["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: the collaboration must report the new membershipId, got {:?}",
            reread["collaboration"]["membershipId"]
        );
        assert_eq!(
            reread["collaboration"]["membershipArn"], membership["arn"],
            "case {label:?}: the collaboration must report the new membershipArn, got {:?}",
            reread["collaboration"]["membershipArn"]
        );

        // The identical call again: one account holds at most one membership per
        // collaboration, and the identifier it sent is what fails validation.
        let (status, refusal) = emulator.create_membership(body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a second join must be refused, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {refusal}"
        );
    }
}

#[tokio::test]
async fn create_membership_refuses_a_collaboration_it_cannot_reach() {
    let emulator = Emulator::new();
    // Syntactically valid collaboration ids that no CreateCollaboration minted.
    for collaboration_id in [
        "00000000-0000-0000-0000-000000000000",
        "deadbeef-dead-beef-dead-beefdeadbeef",
    ] {
        let (status, body) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(
            status, 403,
            "case {collaboration_id:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {collaboration_id:?}: expected AccessDeniedException, got {body}"
        );
    }
}

#[tokio::test]
async fn membership_payment_responsibility_defaults_to_the_paying_ability() {
    let emulator = Emulator::new();

    // The documented default, from `QueryComputePaymentConfig$isResponsible`: "If the
    // collaboration creator hasn't specified anyone as the member paying for query
    // compute costs, then the member who can query is the default payer." The same
    // rule applies to job compute, whose paying ability is CAN_RUN_JOB. Rows span the
    // whole `MemberAbility` catalog.
    let cases: Vec<(&str, Vec<&str>, bool, bool)> = vec![
        ("can query only", vec![ALL_MEMBER_ABILITIES[0]], true, false),
        (
            "can receive results only",
            vec![ALL_MEMBER_ABILITIES[1]],
            false,
            false,
        ),
        ("can run job only", vec![ALL_MEMBER_ABILITIES[2]], false, true),
        (
            "can export query analysis log only",
            vec![ALL_MEMBER_ABILITIES[3]],
            false,
            false,
        ),
        (
            "query and job",
            vec![ALL_MEMBER_ABILITIES[0], ALL_MEMBER_ABILITIES[2]],
            true,
            true,
        ),
        (
            "every ability",
            ALL_MEMBER_ABILITIES.to_vec(),
            true,
            true,
        ),
    ];

    for (label, abilities, query_payer, job_payer) in cases {
        let mut collaboration = minimal_collaboration("payer");
        collaboration["creatorMemberAbilities"] = json!(abilities);
        let (status, created) = emulator.create_collaboration(collaboration).await;
        assert_eq!(status, 200, "case {label:?}: CreateCollaboration: {created}");
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();

        let (status, response) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(status, 200, "case {label:?}: CreateMembership: {response}");
        let payment = &response["membership"]["paymentConfiguration"];
        assert_eq!(
            payment["queryCompute"]["isResponsible"].as_bool(),
            Some(query_payer),
            "case {label:?}: expected queryCompute.isResponsible={query_payer}, got {payment}"
        );
        assert_eq!(
            payment["jobCompute"]["isResponsible"].as_bool(),
            Some(job_payer),
            "case {label:?}: expected jobCompute.isResponsible={job_payer}, got {payment}"
        );
    }

    // And an explicit request value wins over the default.
    let (_, created) = emulator
        .create_collaboration(minimal_collaboration("explicit-payer"))
        .await;
    let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
    let (status, response) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
            "paymentConfiguration": { "queryCompute": { "isResponsible": false } },
        }))
        .await;
    assert_eq!(status, 200, "case \"explicit payment\": {response}");
    assert_eq!(
        response["membership"]["paymentConfiguration"]["queryCompute"]["isResponsible"].as_bool(),
        Some(false),
        "case \"explicit payment\": the request value must win, got {:?}",
        response["membership"]["paymentConfiguration"]
    );
}

#[tokio::test]
async fn list_memberships_pages_and_filters_on_status() {
    let emulator = Emulator::new();

    let mut memberships: Vec<String> = Vec::new();
    for n in 0..5 {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration(&format!("ml-{n}")))
            .await;
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
        let (status, response) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(status, 200, "case {n}: CreateMembership: {response}");
        memberships.push(response["membership"]["id"].as_str().unwrap().to_string());
    }

    for max_results in [MAX_RESULTS_MIN, 2, 5, MAX_RESULTS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let path = match &token {
                Some(t) => format!("/memberships?maxResults={max_results}&nextToken={t}"),
                None => format!("/memberships?maxResults={max_results}"),
            };
            let (status, body) = emulator.call("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: expected 200, got {status} with {body}"
            );
            for summary in body["membershipSummaries"].as_array().unwrap() {
                seen.push(summary["id"].as_str().unwrap().to_string());
            }
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, memberships,
            "case maxResults={max_results}: paging must yield every membership once, in creation order"
        );
    }

    // Catalog: model shape `MembershipStatus` (ACTIVE | REMOVED | COLLABORATION_DELETED).
    for (status_filter, expected) in [
        ("ACTIVE", 5usize),
        ("REMOVED", 0),
        ("COLLABORATION_DELETED", 0),
    ] {
        let (status, body) = emulator
            .call("GET", &format!("/memberships?status={status_filter}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case status={status_filter}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["membershipSummaries"].as_array().unwrap().len(),
            expected,
            "case status={status_filter}: expected {expected} rows, got {body}"
        );
    }
}

#[tokio::test]
async fn get_membership_and_delete_membership_refuse_an_id_that_is_not_stored() {
    let emulator = Emulator::new();
    for membership_id in [
        "00000000-0000-0000-0000-000000000000",
        "abcdef01-2345-6789-abcd-ef0123456789",
    ] {
        let (status, body) = emulator.get_membership(membership_id).await;
        assert_eq!(
            status, 404,
            "case GetMembership {membership_id:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case GetMembership {membership_id:?}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = emulator.delete_membership(membership_id).await;
        assert_eq!(
            status, 404,
            "case DeleteMembership {membership_id:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case DeleteMembership {membership_id:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_collaboration_hides_it_and_marks_its_memberships() {
    let emulator = Emulator::new();

    // Three collaborations, each joined, so deleting one must leave the others whole.
    let mut pairs: Vec<(String, String)> = Vec::new();
    for n in 0..3 {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration(&format!("del-{n}")))
            .await;
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
        let (_, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        pairs.push((
            collaboration_id,
            joined["membership"]["id"].as_str().unwrap().to_string(),
        ));
    }

    for (index, (collaboration_id, membership_id)) in pairs.clone().into_iter().enumerate() {
        let (status, body) = emulator.delete_collaboration(&collaboration_id).await;
        assert_eq!(
            status, 204,
            "case {index}: expected DeleteCollaboration 204, got {status} with {body}"
        );

        let (status, body) = emulator.get_collaboration(&collaboration_id).await;
        assert_eq!(
            status, 403,
            "case {index}: a deleted collaboration must not be readable, got {status} with {body}"
        );

        let (_, list) = emulator.call("GET", "/collaborations", None).await;
        let remaining: Vec<&str> = list["collaborationList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert!(
            !remaining.contains(&collaboration_id.as_str()),
            "case {index}: a deleted collaboration must not be listed, got {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            pairs.len() - index - 1,
            "case {index}: expected {} remaining collaborations, got {remaining:?}",
            pairs.len() - index - 1
        );

        // The membership survives, with the status that says why.
        let (status, body) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            status, 200,
            "case {index}: the membership must survive its collaboration, got {status} with {body}"
        );
        assert_eq!(
            body["membership"]["status"].as_str(),
            Some("COLLABORATION_DELETED"),
            "case {index}: expected status COLLABORATION_DELETED, got {:?}",
            body["membership"]["status"]
        );

        let (_, list) = emulator
            .call("GET", "/memberships?status=COLLABORATION_DELETED", None)
            .await;
        let marked: Vec<&str> = list["membershipSummaries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert!(
            marked.contains(&membership_id.as_str()),
            "case {index}: ListMemberships must show {membership_id} as COLLABORATION_DELETED, got {marked:?}"
        );

        // @idempotent with no 404 declared: deleting it again is the same 204.
        let (status, body) = emulator.delete_collaboration(&collaboration_id).await;
        assert_eq!(
            status, 204,
            "case {index}: a repeated DeleteCollaboration must also be 204, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_membership_frees_the_account_to_join_again() {
    let emulator = Emulator::new();
    let (_, created) = emulator
        .create_collaboration(minimal_collaboration("rejoin"))
        .await;
    let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
    let join = json!({
        "collaborationIdentifier": collaboration_id,
        "queryLogStatus": "ENABLED",
    });

    // Three join/leave cycles: each mints a NEW membership id, and the collaboration
    // reports whichever one currently exists.
    let mut ids: Vec<String> = Vec::new();
    for cycle in 0..3 {
        let (status, response) = emulator.create_membership(join.clone()).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected CreateMembership 200, got {status} with {response}"
        );
        let membership_id = response["membership"]["id"].as_str().unwrap().to_string();
        assert!(
            !ids.contains(&membership_id),
            "case cycle {cycle}: a rejoin must mint a fresh id, got {membership_id} again"
        );

        let (_, reread) = emulator.get_collaboration(&collaboration_id).await;
        assert_eq!(
            reread["collaboration"]["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case cycle {cycle}: the collaboration must report the current membership, got {:?}",
            reread["collaboration"]["membershipId"]
        );

        let (status, body) = emulator.delete_membership(&membership_id).await;
        assert_eq!(
            status, 204,
            "case cycle {cycle}: expected DeleteMembership 204, got {status} with {body}"
        );
        let (status, body) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            status, 404,
            "case cycle {cycle}: a deleted membership must not be readable, got {status} with {body}"
        );
        let (_, reread) = emulator.get_collaboration(&collaboration_id).await;
        assert!(
            reread["collaboration"].get("membershipId").is_none(),
            "case cycle {cycle}: the collaboration must report no membership, got {:?}",
            reread["collaboration"].get("membershipId")
        );
        ids.push(membership_id);
    }
}

#[tokio::test]
async fn the_resource_stores_refuse_to_grow_past_their_caps() {
    // A generated emulator keeps its resources for the life of the process, so each
    // store carries an emulator-enforced maximum entry count and the create refuses
    // past it with the `ServiceQuotaExceededException` (402) the model declares. The
    // caps are read from the crate, not retyped, so the test cannot drift from them.
    let collaboration_cap = cleanrooms_state::MAX_COLLABORATIONS;
    let membership_cap = cleanrooms_state::MAX_MEMBERSHIPS;

    // --- collaborations ---
    let emulator = Emulator::new();
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("seed"))
        .await;
    assert_eq!(status, 200, "case seed collaboration: {created}");
    let seed_id = created["collaboration"]["id"].as_str().unwrap().to_string();
    {
        // Fill the store to the cap with copies of the stored record, each under its
        // own id, so the next create meets a full store.
        let store = emulator.store();
        let mut store = store.lock();
        let seed = store.collaborations.get(&seed_id).cloned().unwrap();
        while store.collaborations.len() < collaboration_cap {
            let id = format!("{:08x}-0000-4000-8000-{:012x}", store.collaborations.len(), 1);
            let mut record = seed.clone();
            record.id = id.clone();
            store.collaborations.insert(id.clone(), record);
            store.collaboration_order.push(id);
        }
    }
    let (status, body) = emulator
        .create_collaboration(minimal_collaboration("over-cap"))
        .await;
    assert_eq!(
        status, 402,
        "case collaborations at the cap of {collaboration_cap}: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case collaborations at the cap: expected ServiceQuotaExceededException, got {body}"
    );
    // A full store still serves reads: the cap refuses growth, not access.
    let (status, body) = emulator.get_collaboration(&seed_id).await;
    assert_eq!(
        status, 200,
        "case read at the cap: a full store must still answer, got {status} with {body}"
    );

    // --- memberships ---
    let emulator = Emulator::new();
    let (_, created) = emulator
        .create_collaboration(minimal_collaboration("seed-join"))
        .await;
    let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(status, 200, "case seed membership: {joined}");
    let seed_membership = joined["membership"]["id"].as_str().unwrap().to_string();
    {
        let store = emulator.store();
        let mut store = store.lock();
        let seed = store.memberships.get(&seed_membership).cloned().unwrap();
        while store.memberships.len() < membership_cap {
            let id = format!("{:08x}-0000-4000-8000-{:012x}", store.memberships.len(), 2);
            let mut membership = seed.clone();
            membership.id = id.clone();
            store.memberships.insert(id.clone(), membership);
            store.membership_order.push(id);
        }
    }
    // A DIFFERENT collaboration, so the refusal is the store cap and not the
    // duplicate-membership conflict.
    let (_, other) = emulator
        .create_collaboration(minimal_collaboration("another"))
        .await;
    let other_id = other["collaboration"]["id"].as_str().unwrap().to_string();
    let (status, body) = emulator
        .create_membership(json!({
            "collaborationIdentifier": other_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 402,
        "case memberships at the cap of {membership_cap}: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case memberships at the cap: expected ServiceQuotaExceededException, got {body}"
    );
    // At the cap the MORE SPECIFIC modelled errors still win: the cap check runs after
    // the resource-identity checks.
    let (status, body) = emulator
        .create_membership(json!({
            "collaborationIdentifier": "00000000-0000-0000-0000-000000000000",
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 403,
        "case unknown collaboration at the membership cap: expected 403, got {status} with {body}"
    );
    let (status, body) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 400,
        "case repeat join at the membership cap: expected 400, got {status} with {body}"
    );
    let (status, body) = emulator.get_membership(&seed_membership).await;
    assert_eq!(
        status, 200,
        "case read at the cap: a full store must still answer, got {status} with {body}"
    );
}

#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    for n in 0..3 {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration(&format!("stable-{n}")))
            .await;
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
        emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        ids.push(collaboration_id);
    }

    // Each reporting operation, called twice, answers identically: no status moves, no
    // counter advances, nothing is consumed by being read.
    let reads: Vec<String> = ids
        .iter()
        .map(|id| format!("/collaborations/{id}"))
        .chain([
            "/collaborations".to_string(),
            "/collaborations?maxResults=2".to_string(),
            "/memberships".to_string(),
            "/memberships?maxResults=1".to_string(),
        ])
        .collect();
    for path in reads {
        let (first_status, first) = emulator.call("GET", &path, None).await;
        let (second_status, second) = emulator.call("GET", &path, None).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {path:?}: a read must not change what the next read sees"
        );
    }
}
