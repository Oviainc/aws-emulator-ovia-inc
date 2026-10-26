// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE account rule every account-scoped read of this service
//! holds: an account id a request supplies selects this store's records when it names the
//! account this store holds, and nothing when it names any other account; an absent account
//! id names no account and selects this store's records.
//!
//! Each reader here takes that decision through the same place in `crate::state`, so this
//! suite drives all of them over one table of account ids: `ListServices` and
//! `ListServiceStates` (their own `AwsAccountId`), `ListServiceLevelObjectives`
//! (`SloOwnerAwsAccountId`), `ListGroupingAttributeDefinitions` (`AwsAccountId`) and
//! `ListAuditFindings` (the account an audited `ServiceEntity` names). Every request goes
//! over the real `restJson1` surface the router mounts in production, so the statuses and
//! JSON bodies are the ones a client sees.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

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

/// Seconds since the epoch, for a window around the instant the stored records are written.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The entity the seeded objectives monitor, and the entity the audit target names.
fn audited_service() -> Value {
    json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" })
}

/// Store the records every reader below projects: two objectives on one service entity (so
/// `ListServiceLevelObjectives`, `ListServices`, `ListServiceStates` and `ListAuditFindings`
/// all hold something for this account) and the account's grouping configuration (so
/// `ListGroupingAttributeDefinitions` does too).
async fn seed(router: &MultiServiceRouter) {
    for (name, operation) in [
        ("checkout-latency", "POST /checkout"),
        ("cart-availability", "GET /cart"),
    ] {
        let (status, answer) = call(
            router,
            "POST",
            "/slo",
            Some(json!({
                "Name": name,
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": audited_service(),
                        "OperationName": operation,
                        "MetricType": "LATENCY"
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the SLO create to answer 200, got {status} with {answer:?}"
        );
    }

    let (status, answer) = call(
        router,
        "PUT",
        "/grouping-configuration",
        Some(json!({
            "GroupingAttributeDefinitions": [
                { "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["business_unit"] }
            ]
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"seed the grouping configuration\": expected 200, got {status} with {answer:?}"
    );
}

/// One account-scoped reader of this store.
#[derive(Clone, Copy, Debug)]
enum Reader {
    Services,
    ServiceStates,
    ServiceLevelObjectives,
    GroupingAttributeDefinitions,
    AuditFindings,
}

/// Every reader that takes an account id, one per place the rule used to be written out.
const READERS: [Reader; 5] = [
    Reader::Services,
    Reader::ServiceStates,
    Reader::ServiceLevelObjectives,
    Reader::GroupingAttributeDefinitions,
    Reader::AuditFindings,
];

impl Reader {
    /// The output member carrying what the request selected, as the operation's output shape
    /// names it (`smithy_query(query="operation", operation="...")`).
    fn selection(self) -> &'static str {
        match self {
            Self::Services => "ServiceSummaries",
            Self::ServiceStates => "ServiceStates",
            Self::ServiceLevelObjectives => "SloSummaries",
            Self::GroupingAttributeDefinitions => "GroupingAttributeDefinitions",
            Self::AuditFindings => "AuditFindings",
        }
    }

    /// Drive this reader over the window `[start, end]`, naming `account` where the operation
    /// takes one (`None` leaves the member absent) and sending `linked` as
    /// `IncludeLinkedAccounts` where the operation declares it.
    async fn read(
        self,
        router: &MultiServiceRouter,
        account: Option<&str>,
        linked: bool,
        start: i64,
        end: i64,
    ) -> (u16, Value) {
        match self {
            Self::Services => {
                let mut query =
                    format!("StartTime={start}&EndTime={end}&IncludeLinkedAccounts={linked}");
                if let Some(account) = account {
                    query.push_str(&format!("&AwsAccountId={account}"));
                }
                call(router, "GET", &format!("/services?{query}"), None).await
            }
            Self::ServiceStates => {
                let mut body = json!({
                    "StartTime": start,
                    "EndTime": end,
                    "IncludeLinkedAccounts": linked
                });
                if let Some(account) = account {
                    body["AwsAccountId"] = json!(account);
                }
                call(router, "POST", "/service/states", Some(body)).await
            }
            Self::ServiceLevelObjectives => {
                let mut query = format!("IncludeLinkedAccounts={linked}");
                if let Some(account) = account {
                    query.push_str(&format!("&SloOwnerAwsAccountId={account}"));
                }
                call(router, "POST", &format!("/slos?{query}"), Some(json!({}))).await
            }
            Self::GroupingAttributeDefinitions => {
                let mut query = format!("IncludeLinkedAccounts={linked}");
                if let Some(account) = account {
                    query.push_str(&format!("&AwsAccountId={account}"));
                }
                call(
                    router,
                    "POST",
                    &format!("/grouping-attribute-definitions?{query}"),
                    None,
                )
                .await
            }
            Self::AuditFindings => {
                // The account a target names is a member of the audited entity itself, not a
                // selector of the request, so it is set inside the target's `ServiceEntity`.
                let mut entity = audited_service();
                if let Some(account) = account {
                    entity["AwsAccountId"] = json!(account);
                }
                call(
                    router,
                    "POST",
                    &format!("/auditFindings?StartTime={start}&EndTime={end}"),
                    Some(json!({
                        "AuditTargets": [{ "Type": "service", "Data": { "Service": entity } }]
                    })),
                )
                .await
            }
        }
    }
}

/// One row of the account-id table: the id a request names, and whether it selects this
/// store's records.
struct Case {
    label: &'static str,
    /// The account id the request carries; `None` leaves the member absent.
    account: Option<String>,
    /// Whether naming it selects what this store holds.
    selects: bool,
}

/// The account ids the rule is asserted over.
///
/// The rows differ along what the rule generalises over: whether the member is supplied at
/// all, whether the id is this store's own, and — for the ids that are not — how far from it
/// they are, from one digit away to an id sharing no digit with it. Every id here matches the
/// modelled `AwsAccountId` pattern `^[0-9]{12}$`
/// (`smithy_query(query="shape", shape="AwsAccountId")`), so each one reaches the handler
/// instead of being refused by the boundary.
fn cases() -> Vec<Case> {
    let this_account = engine::emulator_account_id().to_string();
    vec![
        Case {
            label: "the account member absent",
            account: None,
            selects: true,
        },
        Case {
            label: "this store's own account",
            account: Some(this_account),
            selects: true,
        },
        Case {
            label: "an account one digit from this store's",
            account: Some("000000000001".to_string()),
            selects: false,
        },
        Case {
            label: "the documented example account",
            account: Some("123456789012".to_string()),
            selects: false,
        },
        Case {
            label: "an account sharing no digit with this store's",
            account: Some("999999999999".to_string()),
            selects: false,
        },
    ]
}

/// Every account-scoped reader decides "this account or nothing" the same way: naming this
/// store's account answers exactly what naming no account answers, naming any other account
/// answers an empty selection, and `IncludeLinkedAccounts` changes neither — there is no
/// second account whose records it could add.
#[tokio::test]
async fn every_account_scoped_reader_selects_this_account_or_nothing() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let (start, end) = (now - 3_600, now + 3_600);

    for reader in READERS {
        // What the reader answers when the request names no account at all: the baseline
        // every selecting row must match.
        let (status, baseline) = reader.read(&router, None, false, start, end).await;
        let case = format!("{reader:?}, no account named");
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {baseline:?}"
        );
        let selected = baseline[reader.selection()].as_array().cloned();
        assert!(
            selected.is_some_and(|items| !items.is_empty()),
            "case {case:?}: expected the seeded records to be selected, got {:?}",
            baseline[reader.selection()]
        );

        for row in cases() {
            for linked in [false, true] {
                let case = format!(
                    "{reader:?}, {}, IncludeLinkedAccounts={linked}",
                    row.label
                );
                let (status, answer) = reader
                    .read(&router, row.account.as_deref(), linked, start, end)
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
                );
                if row.selects {
                    assert_eq!(
                        answer[reader.selection()],
                        baseline[reader.selection()],
                        "case {case:?}: expected the same selection as naming no account \
                         ({:?}), got {:?}",
                        baseline[reader.selection()],
                        answer[reader.selection()]
                    );
                } else {
                    assert_eq!(
                        answer[reader.selection()],
                        json!([]),
                        "case {case:?}: expected another account to select nothing, got {:?}",
                        answer[reader.selection()]
                    );
                }
            }
        }
    }
}

/// The grouping reader reports no `UpdatedAt` for an account it holds no configuration for:
/// the instant belongs to the configuration that was selected, so nothing selected is no
/// instant.
#[tokio::test]
async fn the_grouping_reader_reports_no_update_instant_for_another_account() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    let (status, mine) = Reader::GroupingAttributeDefinitions
        .read(&router, None, false, now - 3_600, now + 3_600)
        .await;
    assert_eq!(
        status, 200,
        "case \"no account named\": expected HTTP 200, got {status} with {mine:?}"
    );
    assert!(
        mine["UpdatedAt"].is_number(),
        "case \"no account named\": expected the stored configuration's UpdatedAt, got {:?}",
        mine["UpdatedAt"]
    );

    for row in cases().into_iter().filter(|row| !row.selects) {
        let (status, answer) = Reader::GroupingAttributeDefinitions
            .read(&router, row.account.as_deref(), false, now - 3_600, now + 3_600)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            row.label
        );
        assert_eq!(
            answer["UpdatedAt"],
            Value::Null,
            "case {:?}: expected no UpdatedAt for another account, got {:?}",
            row.label,
            answer["UpdatedAt"]
        );
    }
}
