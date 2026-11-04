// SPDX-License-Identifier: CC0-1.0

//! What `SupportCaseAllowed` decides about a quota increase request, and the gate every
//! quota request TEMPLATE operation stands behind.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour and the state behind it.
//!
//! Two rules, each asserted over a TABLE of inputs rather than one recorded case:
//!
//! * a quota increase request settles by what its submitter asked for. The model
//!   documents `RequestServiceQuotaIncreaseRequest$SupportCaseAllowed` as "By default,
//!   this flag is set to True … If you set the flag to False, Amazon Web Services does
//!   not open a support case and updates the request status to Not approved", so a
//!   request that opted out of its support case ends `NOT_APPROVED` with no `CaseId`
//!   and puts NOTHING in force, while one that allowed it (explicitly or by omission)
//!   is processed under a support case and ends `APPROVED`, keeping that case's
//!   `CaseId`, with its `DesiredValue` in force (the steps that get it there are
//!   `support_case_lifecycle.rs`);
//! * every template operation is refused by the template gate while the account is in no
//!   AWS Organization: outside the plane's home Region with
//!   `TemplatesNotAvailableInRegionException`, and inside it — for an account that belongs
//!   to no organization, which is the account an emulator serves until one is created
//!   through AWS Organizations — with `NoAvailableOrganizationException`. No template
//!   entry becomes observable either way, and unrelated Service Quotas calls keep working.
//!   What the plane serves once the account IS in an all-features organization is
//!   `quota_request_template.rs`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The routing header from this crate's `OPERATIONS.md`: `POST /` with
/// `X-Amz-Target: ServiceQuotasV20190624.<OperationName>`.
const TARGET_PREFIX: &str = "ServiceQuotasV20190624.";

/// Comfortably past both steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY` then
/// `crate::state::REQUEST_DECISION_DELAY`), so a request made before this wait has
/// settled afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// A caller pointed at one regional endpoint of one emulator.
struct Client {
    router: MultiServiceRouter,
    region: String,
}

/// The ONE shared engine, exactly as the router builds it.
fn emulator() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// A caller for `region`, serving out of `engine`.
///
/// AWS Organizations is mounted beside Service Quotas because the template gate reads
/// the caller's organization membership from it — and mounting it establishes the
/// organization the emulated account belongs to, so that store is cleared again right
/// after: what these cases pin is the gate's answer for an account that is in none.
fn client(engine: &Arc<engine::InMemoryEngine>, region: &str) -> Client {
    let mut router = MultiServiceRouter::new();
    // These tests sign nothing, so they must not be refused for it.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    *engine
        .state::<service_com_amazonaws_organizations_awsorganizationsv20161128::state::ComAmazonawsOrganizationsAwsorganizationsv20161128State>()
        .lock() = Default::default();
    Client {
        router,
        region: region.to_string(),
    }
}

impl Client {
    /// Dispatch one awsJson1_1 call and return its status and parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("servicequotas.{}.amazonaws.com", self.region))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            // The credential scope is what routes the request to this service and
            // names the region it is addressed to.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLIENT/20260101/{}/servicequotas/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=notchecked",
                    self.region
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Dispatch a call that must succeed, and return its body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Dispatch a call that must be refused, and return its status and error code.
    async fn refused(&self, operation: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, &body).await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string();
        (status, code)
    }
}

/// One row of the `SupportCaseAllowed` table: a request to make, and what must be true
/// once its decision has come due.
struct SupportCaseRow {
    /// Why this row is in the table.
    why: &'static str,
    /// How the request gives `SupportCaseAllowed`; `None` is a request that carries the
    /// member at all.
    flag: Option<bool>,
    service_code: &'static str,
    quota_code: &'static str,
    desired: f64,
    /// The `RequestStatus` member the request must settle to.
    terminal_status: &'static str,
    /// The value in force for the quota once it has settled.
    in_force: f64,
}

/// One accepted request, waiting for its decision to come due.
struct Submitted {
    case: String,
    id: String,
    /// The `(ServiceCode, QuotaCode)` body a quota read takes.
    pair: Value,
    terminal_status: &'static str,
    in_force: f64,
}

/// `SupportCaseAllowed` decides the terminal status of a quota increase request, and
/// whether its `DesiredValue` is ever put in force.
#[tokio::test]
async fn support_case_allowed_decides_the_terminal_status_and_the_applied_value() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // The rows differ in the FORM of the flag (absent, explicitly true, explicitly
    // false), in the service and quota they name, and in whether the quota is counted
    // per Region or account-wide — the seeded defaults are
    // `src/state.rs::seed_default_quotas` (vpc/L-F678F1CE = 5, ec2/L-0263D0A3 = 5,
    // vpc/L-A4707A72 = 5, budgets/Budgets = 20000 and global, dynamodb/L-F98FE922 =
    // 2500).
    let rows = vec![
        SupportCaseRow {
            why: "absent: the model's default is True",
            flag: None,
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            desired: 20.0,
            terminal_status: "APPROVED",
            in_force: 20.0,
        },
        SupportCaseRow {
            why: "explicitly allowed",
            flag: Some(true),
            service_code: "ec2",
            quota_code: "L-0263D0A3",
            desired: 100.0,
            terminal_status: "APPROVED",
            in_force: 100.0,
        },
        SupportCaseRow {
            why: "explicitly allowed on an account-wide quota",
            flag: Some(true),
            service_code: "budgets",
            quota_code: "Budgets",
            desired: 30_000.0,
            terminal_status: "APPROVED",
            in_force: 30_000.0,
        },
        SupportCaseRow {
            why: "opted out: no support case is opened",
            flag: Some(false),
            service_code: "vpc",
            quota_code: "L-A4707A72",
            desired: 50.0,
            terminal_status: "NOT_APPROVED",
            // Nothing is put in force, so the quota keeps its AWS default.
            in_force: 5.0,
        },
        SupportCaseRow {
            why: "opted out on another service's quota",
            flag: Some(false),
            service_code: "dynamodb",
            quota_code: "L-F98FE922",
            desired: 100_000.0,
            terminal_status: "NOT_APPROVED",
            in_force: 2500.0,
        },
    ];

    let mut submitted: Vec<Submitted> = Vec::new();
    for row in rows {
        let case = format!("{} ({}/{})", row.why, row.service_code, row.quota_code);
        let mut body = json!({
            "ServiceCode": row.service_code,
            "QuotaCode": row.quota_code,
            "DesiredValue": row.desired,
        });
        if let Some(flag) = row.flag {
            body["SupportCaseAllowed"] = json!(flag);
        }
        let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
        let requested = &accepted["RequestedQuota"];
        // Whatever the flag, the request is accepted PENDING: the decision is owed
        // later, so a caller reading straight after the call still sees it.
        assert_eq!(
            requested["Status"].as_str(),
            Some("PENDING"),
            "case {case}: expected a PENDING request, got {accepted}"
        );
        assert!(
            requested["CaseId"].is_null(),
            "case {case}: a request carries no CaseId when it is made, got {accepted}"
        );
        let id = requested["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the accepted request reports an Id"))
            .to_string();
        submitted.push(Submitted {
            case,
            id,
            pair: json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            terminal_status: row.terminal_status,
            in_force: row.in_force,
        });
    }

    tokio::time::sleep(PAST_DECISION).await;

    for Submitted {
        case,
        id,
        pair,
        terminal_status: expected_status,
        in_force: expected_value,
    } in submitted
    {
        let settled = caller
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await;
        let requested = &settled["RequestedQuota"];
        let status = requested["Status"].as_str();
        assert_eq!(
            status,
            Some(expected_status),
            "case {case}: expected the terminal status {expected_status:?}, got {status:?} \
             in {settled}"
        );
        // A support case is what processes a request Service Quotas may open one for,
        // so an approved request reports the `CaseId` of the case it was processed
        // under; a request whose submitter opted out of that case has none.
        let case_id = requested["CaseId"].as_str();
        if expected_status == "APPROVED" {
            assert!(
                case_id.is_some_and(|id| !id.is_empty()),
                "case {case}: expected the CaseId of the case that processed it, got {settled}"
            );
        } else {
            assert!(
                case_id.is_none(),
                "case {case}: expected no CaseId, got {settled}"
            );
        }
        // Reading twice does not move the status: the decision was the clock's, not
        // the reader's.
        let again = caller
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await;
        assert_eq!(
            again["RequestedQuota"]["Status"].as_str(),
            Some(expected_status),
            "case {case}: a second read must report the same status, got {again}"
        );

        let applied = caller.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            applied["Quota"]["Value"].as_f64(),
            Some(expected_value),
            "case {case}: expected {expected_value} in force, got {applied}"
        );
        // Whatever was decided, the AWS DEFAULT is untouched.
        let default = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await;
        assert!(
            default["Quota"]["Value"].as_f64().is_some(),
            "case {case}: the default read must still answer, got {default}"
        );
    }
}

/// A request that opted out of its support case is terminal: it blocks no later
/// request, and the history reports it alongside the approved ones.
#[tokio::test]
async fn a_not_approved_request_is_terminal_and_blocks_no_later_request() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    let opted_out = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "DesiredValue": 40.0,
                "SupportCaseAllowed": false,
            }),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the first request reports an Id")
        .to_string();

    // While it is still awaiting its decision it is the one open request for the quota.
    let (status, code) = caller
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 41.0}),
        )
        .await;
    assert_eq!(
        (status, code.contains("ResourceAlreadyExistsException")),
        (400, true),
        "case a second open request: got {status} {code}"
    );

    tokio::time::sleep(PAST_DECISION).await;

    // NOT_APPROVED is terminal, so a further request is accepted — and it is measured
    // against the value still in force (the default 5), not against the desired value
    // of the request that was refused.
    let (status, code) = caller
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 5.0}),
        )
        .await;
    assert_eq!(
        (status, code.contains("IllegalArgumentException")),
        (400, true),
        "case a desired value equal to the value still in force: got {status} {code}"
    );
    let approved = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "DesiredValue": 10.0,
                "SupportCaseAllowed": true,
            }),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the second request reports an Id")
        .to_string();

    tokio::time::sleep(PAST_DECISION).await;

    let applied = caller.ok("GetServiceQuota", pair.clone()).await;
    assert_eq!(
        applied["Quota"]["Value"].as_f64(),
        Some(10.0),
        "case after one refused and one approved request: expected the approved value in \
         force, got {applied}"
    );

    // Both requests are in the history, newest first, each with the status it settled
    // to: the refused one is kept, not dropped.
    let history = caller
        .ok("ListRequestedServiceQuotaChangeHistoryByQuota", pair)
        .await;
    let entries = history["RequestedQuotas"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reported: Vec<(Option<&str>, Option<&str>)> = entries
        .iter()
        .map(|entry| (entry["Id"].as_str(), entry["Status"].as_str()))
        .collect();
    assert_eq!(
        reported,
        vec![
            (Some(approved.as_str()), Some("APPROVED")),
            (Some(opted_out.as_str()), Some("NOT_APPROVED")),
        ],
        "case the history of both requests: expected both ids newest first, each with the \
         status it settled to, got {history}"
    );
}

/// Every quota request template operation is refused by the template gate while the
/// calling account belongs to no organization, and the refusal says WHICH condition
/// failed.
#[tokio::test]
async fn every_template_operation_is_refused_by_the_template_gate() {
    let engine = emulator();

    // Every template operation this service models, with a request the model accepts
    // (`smithy_query(query="operation", operation="AssociateServiceQuotaTemplate,\
    // DisassociateServiceQuotaTemplate,GetAssociationForServiceQuotaTemplate,\
    // PutServiceQuotaIncreaseRequestIntoTemplate,\
    // GetServiceQuotaIncreaseRequestFromTemplate,\
    // DeleteServiceQuotaIncreaseRequestFromTemplate,\
    // ListServiceQuotaIncreaseRequestsInTemplate")`) — the three association
    // operations take no members at all, the entry operations take a quota and a
    // Region, and the listing takes optional filters.
    let template_calls: Vec<(&str, Value)> = vec![
        ("AssociateServiceQuotaTemplate", json!({})),
        ("DisassociateServiceQuotaTemplate", json!({})),
        ("GetAssociationForServiceQuotaTemplate", json!({})),
        (
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 1_000_000.0,
            }),
        ),
        (
            "GetServiceQuotaIncreaseRequestFromTemplate",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "AwsRegion": "us-east-1"}),
        ),
        (
            "DeleteServiceQuotaIncreaseRequestFromTemplate",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "AwsRegion": "us-east-1"}),
        ),
        ("ListServiceQuotaIncreaseRequestsInTemplate", json!({})),
        (
            "ListServiceQuotaIncreaseRequestsInTemplate",
            json!({"ServiceCode": "vpc", "AwsRegion": "us-east-1", "MaxResults": 10}),
        ),
    ];

    // The caller's Region decides WHICH gate refuses: the template plane is served
    // from `state::TEMPLATE_HOME_REGION` only. The rows differ in partition and in
    // distance from the home Region, and include the home Region itself.
    let regions: Vec<(&str, &str)> = vec![
        ("us-east-1", "NoAvailableOrganizationException"),
        ("us-west-2", "TemplatesNotAvailableInRegionException"),
        ("eu-west-1", "TemplatesNotAvailableInRegionException"),
        ("ap-southeast-2", "TemplatesNotAvailableInRegionException"),
        ("us-gov-west-1", "TemplatesNotAvailableInRegionException"),
    ];

    for (region, expected_code) in regions {
        let caller = client(&engine, region);
        for (operation, body) in &template_calls {
            let case = format!("{operation} from {region}");
            let (status, code) = caller.refused(operation, body.clone()).await;
            // Both refusals are client faults, which this awsJson front end serves on
            // 400 whatever `@httpError` the shape declares.
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {code}"
            );
            assert!(
                code.contains(expected_code),
                "case {case}: expected {expected_code}, got {code}"
            );
            // Never the scaffold's 501: the gate IS the behaviour, not a missing
            // implementation.
            assert!(
                !code.contains("NotImplemented"),
                "case {case}: the template gate must be evaluated, got {code}"
            );
        }
    }
}

/// The template gate changes nothing else: no template entry becomes observable, and
/// the rest of Service Quotas keeps working around it.
#[tokio::test]
async fn a_refused_template_call_leaves_every_other_call_working() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    let before = caller.ok("GetServiceQuota", pair.clone()).await;

    for (operation, body) in [
        (
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 1_000_000.0,
            }),
        ),
        ("AssociateServiceQuotaTemplate", json!({})),
    ] {
        let (status, code) = caller.refused(operation, body).await;
        assert_eq!(
            (status, code.contains("NoAvailableOrganizationException")),
            (400, true),
            "case {operation}: got {status} {code}"
        );
    }

    // The quota the refused Put named is unchanged: a refused template write puts
    // nothing in force.
    let after = caller.ok("GetServiceQuota", pair.clone()).await;
    assert_eq!(
        after, before,
        "case the quota a refused template write named: expected it unchanged, got {after}"
    );

    // And the template read still reports nothing but the gate — no entry was stored.
    let (status, code) = caller
        .refused(
            "GetServiceQuotaIncreaseRequestFromTemplate",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "AwsRegion": "us-east-1"}),
        )
        .await;
    assert_eq!(
        (status, code.contains("NoAvailableOrganizationException")),
        (400, true),
        "case reading the template after a refused write: got {status} {code}"
    );

    // Unrelated calls are unaffected.
    let services = caller.ok("ListServices", json!({})).await;
    assert!(
        services["Services"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "case ListServices after a refused template call: got {services}"
    );
    let increase = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10.0}),
        )
        .await;
    assert_eq!(
        increase["RequestedQuota"]["Status"].as_str(),
        Some("PENDING"),
        "case RequestServiceQuotaIncrease after a refused template call: got {increase}"
    );
}
