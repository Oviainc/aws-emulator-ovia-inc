// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms protected-query slice
//! (`StartProtectedQuery` / `GetProtectedQuery`).
//!
//! The rule under test: a protected query is recorded against PERSISTED membership
//! state and every member of the response is computed from that state — the ids and
//! ARNs from the stored membership, the result configuration from the request else the
//! membership's stored default, the query-compute payer from the collaboration's payer
//! candidate, the worker compute from the request plus the model's declared defaults —
//! so a later `GetProtectedQuery` reports exactly what the start call returned, a
//! membership the store does not hold is a `ResourceNotFoundException`, and the
//! terminal status is the one the CLOCK owes rather than one a read invents.
//!
//! Each table varies what the rule generalises over (which SQL parameter carries the
//! query, which output configuration the results go to, where the result configuration
//! comes from, which worker compute members the caller filled in, which ability the
//! collaboration granted, who was configured to pay), so the recorded case the issue
//! cited — a start against a membership that does not exist — is one row among many.
//!
//! Every request goes over the wire through the same `MultiServiceRouter` the
//! production router uses.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, is_model_uuid, minimal_collaboration, Emulator, ALL_MEMBER_ABILITIES,
    REGION,
};

/// A well-formed `MembershipIdentifier`/`ProtectedQueryIdentifier` (model shape `UUID`:
/// 36 chars, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no
/// call ever created, so only state decides the answer.
const ABSENT_UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
/// A second one, for the rows that need two ids the store does not hold.
const OTHER_ABSENT_UUID: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

/// The published bounds of `WorkerComputeConfiguration$number` (model member
/// `number`, `range 2..1024`).
const WORKER_NUMBER_MIN: i64 = 2;
const WORKER_NUMBER_MAX: i64 = 1024;

/// Start a protected query against `membership_id`.
async fn start_query(emulator: &Emulator, membership_id: &str, body: Value) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/protectedQueries"),
            Some(body),
        )
        .await
}

/// Read a protected query back under the membership that started it.
async fn get_query(emulator: &Emulator, membership_id: &str, query_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership_id}/protectedQueries/{query_id}"),
            None,
        )
        .await
}

/// Create `collaboration`, join it with `membership_body` (its `collaborationIdentifier`
/// and `queryLogStatus` are filled in here) and return the stored `Membership`.
async fn join(emulator: &Emulator, collaboration: Value, membership_body: Value) -> Value {
    let (status, created) = emulator.create_collaboration(collaboration).await;
    assert_eq!(
        status, 200,
        "CreateCollaboration must succeed to set up the case, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no collaboration.id in {created}"))
        .to_string();

    let mut body = membership_body;
    body["collaborationIdentifier"] = json!(collaboration_id);
    body["queryLogStatus"] = json!("ENABLED");
    let (status, joined) = emulator.create_membership(body).await;
    assert_eq!(
        status, 200,
        "CreateMembership must succeed to set up the case, got {status} with {joined}"
    );
    joined["membership"].clone()
}

/// The membership of a collaboration whose creator holds every declared ability, with
/// no default result configuration.
async fn simple_membership(emulator: &Emulator, name: &str) -> Value {
    join(emulator, minimal_collaboration(name), json!({})).await
}

/// A minimal valid `StartProtectedQuery` body: the required `type` plus inline SQL.
fn minimal_query() -> Value {
    json!({
        "type": "SQL",
        "sqlParameters": { "queryString": "SELECT COUNT(*) FROM orders" },
    })
}

/// An `AnalysisTemplateArn` under `membership_id`, in the form the model's pattern
/// declares: `^arn:aws[-a-z]*:cleanrooms:[\w]{2}-[\w]{4,9}-[\d]:[\d]{12}:membership/[\d\w-]+/analysistemplate/[\d\w-]+$`.
fn analysis_template_arn(membership_id: &str) -> String {
    format!(
        "arn:aws:cleanrooms:{REGION}:{}:membership/{membership_id}/analysistemplate/{ABSENT_UUID}",
        account_id()
    )
}

/// The protected query without the two members the clock moves, so a round-trip
/// comparison is not a race against the owed transition.
fn without_settled_members(query: &Value) -> Value {
    let mut copy = query.clone();
    let members = copy
        .as_object_mut()
        .unwrap_or_else(|| panic!("a protected query is a JSON object, got {query}"));
    members.remove("status");
    members.remove("error");
    copy
}

/// Every state a stored protected query may legally be in: SUBMITTED with no error
/// while the owed transition is not yet due, else FAILED carrying the reason the run
/// was never performed. Nothing else, and never a SUCCESS with results nothing
/// computed.
fn assert_legal_state(case: &str, query: &Value) {
    match query["status"].as_str() {
        Some("SUBMITTED") => assert!(
            query.get("error").is_none(),
            "case {case:?}: a SUBMITTED query carries no error, got {:?}",
            query.get("error")
        ),
        Some("FAILED") => {
            assert_eq!(
                query["error"]["code"].as_str(),
                Some(cleanrooms_state::PROTECTED_QUERY_FAILURE_CODE),
                "case {case:?}: unexpected failure code, got {:?}",
                query["error"]
            );
            assert!(
                query.get("result").is_none() && query.get("statistics").is_none(),
                "case {case:?}: a query that never ran reports no result and no statistics, got {query}"
            );
        }
        other => panic!("case {case:?}: unexpected protected query status {other:?} in {query}"),
    }
}

/// What a row expects for the members the SERVICE decides. `Value::Null` means the
/// member must be ABSENT from the response.
struct Expected {
    result_configuration: Value,
    compute_configuration: Value,
    query_compute_payer_account_id: Value,
}

/// One row of the start/read round-trip table.
struct QueryCase {
    label: &'static str,
    /// Extra members for the `CreateMembership` body — a row that tests the fallback
    /// puts the membership's `defaultResultConfiguration` here.
    membership: fn() -> Value,
    /// The `StartProtectedQuery` body, built from the stored membership so a row can
    /// name that membership inside an ARN.
    request: fn(&Value) -> Value,
    expected: fn() -> Expected,
}

fn membership_default_result_configuration() -> Value {
    json!({
        "defaultResultConfiguration": {
            "outputConfiguration": {
                "s3": { "resultFormat": "PARQUET", "bucket": "membership-default-results" },
            },
            "roleArn": "arn:aws:iam::000000000000:role/clean-rooms-results",
        },
    })
}

fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            label: "inline queryString, no result or compute configuration anywhere",
            membership: || json!({}),
            request: |_| minimal_query(),
            expected: || Expected {
                result_configuration: Value::Null,
                compute_configuration: Value::Null,
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "parameterized queryString, s3 CSV results, CR.4X worker compute",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": {
                        "queryString": "SELECT region, COUNT(*) FROM orders WHERE region = :region GROUP BY region",
                        "parameters": { "region": "eu-central" },
                    },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "s3": {
                                "resultFormat": "CSV",
                                "bucket": "clean-rooms-results",
                                "keyPrefix": "queries/2026",
                                "singleFileOutput": true,
                            },
                        },
                    },
                    "computeConfiguration": { "worker": { "type": "CR.4X", "number": 32 } },
                })
            },
            expected: || Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": {
                            "resultFormat": "CSV",
                            "bucket": "clean-rooms-results",
                            "keyPrefix": "queries/2026",
                            "singleFileOutput": true,
                        },
                    },
                }),
                compute_configuration: json!({ "worker": { "type": "CR.4X", "number": 32 } }),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "analysisTemplateArn instead of inline SQL; the membership's default results apply",
            membership: membership_default_result_configuration,
            request: |membership| {
                json!({
                    "type": "SQL",
                    "sqlParameters": {
                        "analysisTemplateArn": analysis_template_arn(
                            membership["id"].as_str().unwrap_or_default(),
                        ),
                    },
                })
            },
            expected: || Expected {
                // The membership-side shape carries a `roleArn` the query-side shape
                // does not model, so only the output configuration carries over.
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": { "resultFormat": "PARQUET", "bucket": "membership-default-results" },
                    },
                }),
                compute_configuration: Value::Null,
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "member output configuration; worker compute with no members at all",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT 1" },
                    "resultConfiguration": {
                        "outputConfiguration": { "member": { "accountId": account_id() } },
                    },
                    "computeConfiguration": { "worker": {} },
                })
            },
            expected: || Expected {
                result_configuration: json!({
                    "outputConfiguration": { "member": { "accountId": account_id() } },
                }),
                // Both worker members carry a declared default, so the query reports
                // the compute it runs on.
                compute_configuration: json!({
                    "worker": {
                        "type": cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                        "number": cleanrooms_state::DEFAULT_WORKER_COMPUTE_NUMBER,
                    },
                }),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "distribute output configuration; worker number at the published minimum",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT * FROM impressions" },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "distribute": {
                                "locations": [
                                    { "s3": { "resultFormat": "PARQUET", "bucket": "distributed-results" } },
                                    { "member": { "accountId": account_id() } },
                                ],
                            },
                        },
                    },
                    "computeConfiguration": { "worker": { "number": WORKER_NUMBER_MIN } },
                })
            },
            expected: || Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "distribute": {
                            "locations": [
                                { "s3": { "resultFormat": "PARQUET", "bucket": "distributed-results" } },
                                { "member": { "accountId": account_id() } },
                            ],
                        },
                    },
                }),
                compute_configuration: json!({
                    "worker": {
                        "type": cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                        "number": WORKER_NUMBER_MIN,
                    },
                }),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "the request's result configuration overrides the membership default; worker number at the published maximum",
            membership: membership_default_result_configuration,
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT * FROM clicks" },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "s3": { "resultFormat": "CSV", "bucket": "per-query-results" },
                        },
                    },
                    "computeConfiguration": { "worker": { "type": "CR.8X", "number": WORKER_NUMBER_MAX } },
                    "queryComputePayerAccountId": account_id(),
                })
            },
            expected: || Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": { "resultFormat": "CSV", "bucket": "per-query-results" },
                    },
                }),
                compute_configuration: json!({
                    "worker": { "type": "CR.8X", "number": WORKER_NUMBER_MAX },
                }),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
    ]
}

#[tokio::test]
async fn start_protected_query_persists_and_get_protected_query_returns_it_unchanged() {
    let emulator = Emulator::new();

    for (row, case) in query_cases().into_iter().enumerate() {
        let label = case.label;
        let membership = join(
            &emulator,
            minimal_collaboration(&format!("queries-{row}")),
            (case.membership)(),
        )
        .await;
        let membership_id = membership["id"].as_str().expect("membership id").to_string();
        let request = (case.request)(&membership);
        let expected = (case.expected)();

        let (status, started) = start_query(&emulator, &membership_id, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        let query = started
            .get("protectedQuery")
            .unwrap_or_else(|| panic!("case {label:?}: no `protectedQuery` in {started}"))
            .clone();

        let id = query["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no `protectedQuery.id` in {started}"));
        assert!(
            is_model_uuid(id),
            "case {label:?}: expected a 36-char lowercase-hex UUID id, got {id:?}"
        );
        assert_eq!(
            query["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: the query belongs to the membership it was started under, got {:?}",
            query["membershipId"]
        );
        assert_eq!(
            query["membershipArn"], membership["arn"],
            "case {label:?}: the membership ARN comes from the stored membership, got {:?}",
            query["membershipArn"]
        );
        assert_eq!(
            query["status"].as_str(),
            Some("SUBMITTED"),
            "case {label:?}: a query the service has accepted but not finished is SUBMITTED, got {:?}",
            query["status"]
        );
        assert!(
            query["createTime"].is_number(),
            "case {label:?}: expected an epoch-seconds createTime, got {:?}",
            query["createTime"]
        );
        assert_eq!(
            query["sqlParameters"], request["sqlParameters"],
            "case {label:?}: the SQL parameters are stored as sent, got {:?}",
            query["sqlParameters"]
        );
        for absent in ["result", "statistics", "error", "differentialPrivacy"] {
            assert!(
                query.get(absent).is_none(),
                "case {label:?}: nothing has run, so {absent} must be absent, got {:?}",
                query.get(absent)
            );
        }
        for (member, expected) in [
            ("resultConfiguration", expected.result_configuration),
            ("computeConfiguration", expected.compute_configuration),
            (
                "queryComputePayerAccountId",
                expected.query_compute_payer_account_id,
            ),
        ] {
            match expected {
                Value::Null => assert!(
                    query.get(member).is_none(),
                    "case {label:?}: expected {member} to be absent, got {:?}",
                    query.get(member)
                ),
                expected => assert_eq!(
                    query[member], expected,
                    "case {label:?}: unexpected {member}, got {:?}",
                    query[member]
                ),
            }
        }

        // The round trip: the read reports the STORED query, member for member, apart
        // from the status the clock may already have moved on.
        let (status, fetched) = get_query(&emulator, &membership_id, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetProtectedQuery 200, got {status} with {fetched}"
        );
        let read_back = fetched["protectedQuery"].clone();
        assert_eq!(
            without_settled_members(&read_back),
            without_settled_members(&query),
            "case {label:?}: GetProtectedQuery must report the stored query, got {read_back}"
        );
        assert_legal_state(label, &read_back);
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_membership_the_store_does_not_hold() {
    let emulator = Emulator::new();

    // A membership that was never created, and one that was created and deleted: in
    // both cases the store holds no such membership, which is what decides the answer.
    let deleted = simple_membership(&emulator, "deleted-membership").await;
    let deleted_id = deleted["id"].as_str().expect("membership id").to_string();
    let (status, body) = emulator
        .call("DELETE", &format!("/memberships/{deleted_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "DeleteMembership must succeed to set up the case, got {status} with {body}"
    );

    for (label, membership_id) in [
        ("a membership id no call ever created", ABSENT_UUID.to_string()),
        ("the id of a deleted membership", deleted_id),
    ] {
        let (status, body) = start_query(&emulator, &membership_id, minimal_query()).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected StartProtectedQuery 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: the 404 names the membership the request addressed, got {:?}",
            body["resourceId"]
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("MEMBERSHIP"),
            "case {label:?}: unexpected resourceType, got {:?}",
            body["resourceType"]
        );

        // The refusal persisted nothing: no query is readable under that membership.
        let (status, body) = get_query(&emulator, &membership_id, OTHER_ABSENT_UUID).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetProtectedQuery 404, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_membership_whose_collaboration_is_gone() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("doomed"))
        .await;
    assert_eq!(status, 200, "CreateCollaboration failed: {created}");
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .expect("collaboration id")
        .to_string();
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(status, 200, "CreateMembership failed: {joined}");
    let membership_id = joined["membership"]["id"]
        .as_str()
        .expect("membership id")
        .to_string();

    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/collaborations/{collaboration_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "DeleteCollaboration must succeed to set up the case, got {status} with {body}"
    );

    let (status, body) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 404,
        "a query runs inside a collaboration, so a membership left by a deleted one has nothing to run against: got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unexpected error code, got {body}"
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(collaboration_id.as_str()),
        "the 404 names the collaboration that is gone, got {:?}",
        body["resourceId"]
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("COLLABORATION"),
        "unexpected resourceType, got {:?}",
        body["resourceType"]
    );
}

#[tokio::test]
async fn start_protected_query_requires_the_can_query_ability() {
    let emulator = Emulator::new();

    // The whole `MemberAbility` catalog, one collaboration per ability, plus a
    // collaboration that grants every ability at once: starting a query is the
    // CAN_QUERY ability and nothing else.
    let mut cases: Vec<(String, Vec<&str>)> = ALL_MEMBER_ABILITIES
        .iter()
        .map(|ability| (format!("only {ability}"), vec![*ability]))
        .collect();
    cases.push((
        "every declared ability".to_string(),
        ALL_MEMBER_ABILITIES.to_vec(),
    ));

    for (row, (label, abilities)) in cases.into_iter().enumerate() {
        let membership = join(
            &emulator,
            json!({
                "name": format!("abilities-{row}"),
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": abilities,
                "members": [],
                "queryLogStatus": "ENABLED",
            }),
            json!({}),
        )
        .await;
        let membership_id = membership["id"].as_str().expect("membership id");

        let (status, body) = start_query(&emulator, membership_id, minimal_query()).await;
        if abilities.contains(&"CAN_QUERY") {
            assert_eq!(
                status, 200,
                "case {label:?}: a member that can query may start one, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {label:?}: a member without CAN_QUERY cannot start a query, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {label:?}: unexpected error code, got {body}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("INSUFFICIENT_PERMISSIONS"),
                "case {label:?}: unexpected reason, got {:?}",
                body["reason"]
            );
        }
    }
}

#[tokio::test]
async fn start_protected_query_refuses_sql_parameters_that_name_no_single_query() {
    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "sql-parameters").await;
    let membership_id = membership["id"].as_str().expect("membership id");
    let arn = analysis_template_arn(membership_id);

    // `ProtectedQuerySQLParameters` models both members as optional, so the boundary
    // validator passes each of these: the SERVICE is what refuses a request that
    // identifies no single query to run.
    let cases: Vec<(&str, Value)> = vec![
        ("neither member", json!({})),
        ("an all-whitespace queryString", json!({ "queryString": "   " })),
        ("an empty queryString", json!({ "queryString": "" })),
        (
            "both members at once",
            json!({ "queryString": "SELECT 1", "analysisTemplateArn": arn }),
        ),
        (
            "both members, with query parameters",
            json!({
                "queryString": "SELECT :x",
                "analysisTemplateArn": analysis_template_arn(membership_id),
                "parameters": { "x": "1" },
            }),
        ),
    ];

    for (label, sql_parameters) in cases {
        let (status, body) = start_query(
            &emulator,
            membership_id,
            json!({ "type": "SQL", "sqlParameters": sql_parameters }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected StartProtectedQuery 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"].as_str(),
            Some("sqlParameters"),
            "case {label:?}: the refusal names the member at fault, got {body}"
        );
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_worker_count_outside_the_published_range() {
    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "worker-range").await;
    let membership_id = membership["id"].as_str().expect("membership id");

    // One past each published bound of `WorkerComputeConfiguration$number`
    // (`range 2..1024`); the bounds themselves are accepted by the table above.
    for number in [WORKER_NUMBER_MIN - 1, WORKER_NUMBER_MAX + 1] {
        let mut body = minimal_query();
        body["computeConfiguration"] = json!({ "worker": { "number": number } });
        let (status, answer) = start_query(&emulator, membership_id, body).await;
        assert_eq!(
            status, 400,
            "case {number:?} workers: outside the declared range 2..1024, got {status} with {answer}"
        );
    }
}

#[tokio::test]
async fn the_query_compute_payer_is_computed_from_the_collaboration() {
    let emulator = Emulator::new();

    let partner = "111111111111";
    // Rows: who the collaboration configured, and who must therefore pay. The payer
    // the request names wins; else the account the creator configured as responsible
    // for query compute; else, per `QueryComputePaymentConfig$isResponsible`, "the
    // member who can query".
    struct PayerCase {
        label: &'static str,
        collaboration: Value,
        requested_payer: Option<&'static str>,
        expected_payer: &'static str,
    }
    let cases = vec![
        PayerCase {
            label: "nobody configured: the member who can query is the default payer",
            collaboration: minimal_collaboration("payer-default"),
            requested_payer: None,
            expected_payer: account_id(),
        },
        PayerCase {
            label: "the creator configured as the query-compute payer",
            collaboration: json!({
                "name": "payer-creator",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "creatorPaymentConfiguration": { "queryCompute": { "isResponsible": true } },
                "members": [],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: None,
            expected_payer: account_id(),
        },
        PayerCase {
            label: "another member configured as the query-compute payer",
            collaboration: json!({
                "name": "payer-member",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "members": [{
                    "accountId": partner,
                    "displayName": "partner",
                    "memberAbilities": ["CAN_RECEIVE_RESULTS"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                }],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: None,
            expected_payer: partner,
        },
        PayerCase {
            label: "the request names a payer, overriding the configured candidate",
            collaboration: json!({
                "name": "payer-requested",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "members": [{
                    "accountId": partner,
                    "displayName": "partner",
                    "memberAbilities": ["CAN_RECEIVE_RESULTS"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                }],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: Some("000000000000"),
            expected_payer: account_id(),
        },
    ];

    for case in cases {
        let label = case.label;
        let membership = join(&emulator, case.collaboration, json!({})).await;
        let membership_id = membership["id"].as_str().expect("membership id");
        let mut body = minimal_query();
        if let Some(payer) = case.requested_payer {
            body["queryComputePayerAccountId"] = json!(payer);
        }

        let (status, started) = start_query(&emulator, membership_id, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        assert_eq!(
            started["protectedQuery"]["queryComputePayerAccountId"].as_str(),
            Some(case.expected_payer),
            "case {label:?}: unexpected query compute payer, got {:?}",
            started["protectedQuery"]["queryComputePayerAccountId"]
        );
    }

    // An account the collaboration does not hold cannot be made to pay for it.
    let membership = simple_membership(&emulator, "payer-stranger").await;
    let membership_id = membership["id"].as_str().expect("membership id");
    let mut body = minimal_query();
    body["queryComputePayerAccountId"] = json!("222222222222");
    let (status, answer) = start_query(&emulator, membership_id, body).await;
    assert_eq!(
        status, 400,
        "an account outside the collaboration cannot be the payer, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "unexpected error code, got {answer}"
    );
    assert_eq!(
        answer["fieldList"][0]["name"].as_str(),
        Some("queryComputePayerAccountId"),
        "the refusal names the member at fault, got {answer}"
    );
}

#[tokio::test]
async fn a_started_query_is_submitted_until_the_clock_settles_it_and_then_fails_with_a_reason() {
    // The span is the emulator's own choice, inside the band this project fixes for a
    // transition the clock owes, and it is never zero.
    let delay = cleanrooms_state::PROTECTED_QUERY_SETTLE_DELAY;
    assert!(
        (Duration::from_millis(250)..=Duration::from_millis(750)).contains(&delay),
        "the owed transition must be due 250-750 ms out, got {delay:?}"
    );

    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "settling").await;
    let membership_id = membership["id"].as_str().expect("membership id").to_string();

    let (status, started) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {started}"
    );
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();

    // Before the span has passed, a caller acting straight after the start call still
    // sees the transient state.
    let (status, fetched) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(status, 200, "expected GetProtectedQuery 200, got {fetched}");
    assert_eq!(
        fetched["protectedQuery"]["status"].as_str(),
        Some("SUBMITTED"),
        "inside the owed span the query is still SUBMITTED, got {fetched}"
    );
    assert!(
        fetched["protectedQuery"].get("error").is_none(),
        "a SUBMITTED query carries no error, got {fetched}"
    );

    tokio::time::sleep(delay + Duration::from_millis(250)).await;

    let (status, settled) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(status, 200, "expected GetProtectedQuery 200, got {settled}");
    let settled_query = settled["protectedQuery"].clone();
    assert_eq!(
        settled_query["status"].as_str(),
        Some("FAILED"),
        "once the span has passed the clock owes the terminal status, got {settled_query}"
    );
    assert_eq!(
        settled_query["error"]["code"].as_str(),
        Some(cleanrooms_state::PROTECTED_QUERY_FAILURE_CODE),
        "unexpected failure code, got {settled_query}"
    );
    assert_eq!(
        settled_query["error"]["message"].as_str(),
        Some(cleanrooms_state::PROTECTED_QUERY_FAILURE_MESSAGE),
        "the failure says why the run was never performed, got {settled_query}"
    );
    for absent in ["result", "statistics"] {
        assert!(
            settled_query.get(absent).is_none(),
            "nothing ran, so {absent} must be absent, got {settled_query}"
        );
    }
    // Everything the start call recorded is unchanged by the transition.
    assert_eq!(
        without_settled_members(&settled_query),
        without_settled_members(&started["protectedQuery"]),
        "the owed transition moves only the status and the error, got {settled_query}"
    );

    // Reading again changes nothing further: the transition is materialized once.
    let (_, again) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(
        again["protectedQuery"], settled_query,
        "a second read reports the same settled query, got {again}"
    );

    // Settling the queries already due leaves a query that is not due alone.
    let (status, fresh) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {fresh}"
    );
    let fresh_id = fresh["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();
    let (_, fresh_read) = get_query(&emulator, &membership_id, &fresh_id).await;
    assert_eq!(
        fresh_read["protectedQuery"]["status"].as_str(),
        Some("SUBMITTED"),
        "a query started after another settled is still SUBMITTED, got {fresh_read}"
    );
    let (_, settled_again) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(
        settled_again["protectedQuery"], settled_query,
        "the already settled query is untouched, got {settled_again}"
    );
}

#[tokio::test]
async fn get_protected_query_is_addressed_under_the_membership_that_started_it() {
    let emulator = Emulator::new();

    let first = simple_membership(&emulator, "owner").await;
    let second = simple_membership(&emulator, "bystander").await;
    let first_id = first["id"].as_str().expect("membership id").to_string();
    let second_id = second["id"].as_str().expect("membership id").to_string();

    let (status, started) = start_query(&emulator, &first_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {started}"
    );
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();

    let cases: Vec<(&str, &str, &str, u16)> = vec![
        ("its own membership", first_id.as_str(), query_id.as_str(), 200),
        (
            "another membership of the same account",
            second_id.as_str(),
            query_id.as_str(),
            404,
        ),
        (
            "a query id the membership does not hold",
            first_id.as_str(),
            ABSENT_UUID,
            404,
        ),
        (
            "a membership the store does not hold",
            OTHER_ABSENT_UUID,
            query_id.as_str(),
            404,
        ),
    ];

    for (label, membership_id, requested_query, expected_status) in cases {
        let (status, body) = get_query(&emulator, membership_id, requested_query).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: unexpected GetProtectedQuery status, got {status} with {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: unexpected error code, got {body}"
            );
        } else {
            assert_eq!(
                body["protectedQuery"]["id"].as_str(),
                Some(query_id.as_str()),
                "case {label:?}: unexpected query, got {body}"
            );
        }
    }
}
