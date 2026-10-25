// SPDX-License-Identifier: CC0-1.0

//! The support case a quota increase request is processed under, from the request that
//! allows one to the decision that closes the review.
//!
//! The rule under test, over a TABLE of requests rather than one recorded call: a quota
//! increase request that Service Quotas may open a support case for (the model's
//! default for `RequestServiceQuotaIncreaseRequest$SupportCaseAllowed`) is received
//! `PENDING` and then passes through the `CASE_OPENED` status its `Status` member
//! documents — "Service Quotas opened a support case to process the quota increase
//! request" — carrying the `CaseId` of that case, before any terminal status; only a
//! request whose submitter opted out of the case goes straight from `PENDING` to its
//! terminal status with no case id. The case id is state, so every read path reports
//! the same one and a decided request keeps it.
//!
//! Each request is WATCHED rather than read once after a sleep: the suite polls it
//! until it is terminal and asserts over the whole sequence of statuses it reported, so
//! what is pinned is the order of the steps rather than one instant. Reading is a
//! report, so polling cannot be what moves the request along.
//!
//! The harness (`Client`, `client`, `emulator`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    LEVEL_ALL, LEVEL_RESOURCE, REQUEST_DECISION_DELAY, STATUS_APPROVED, STATUS_CASE_OPENED,
    STATUS_NOT_APPROVED, STATUS_PENDING, SUPPORT_CASE_OPEN_DELAY,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};

/// The per-quota history listing, whose name is too long to repeat in every row.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The account-wide history listing.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// How often a watched request is read while it walks the workflow: well inside the
/// shortest step the clock owes, so no step passes unobserved.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One row of the table: a quota increase request to make, and whether its submitter
/// lets Service Quotas open the support case that processes it.
struct Row {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    /// The quota to raise, by the `QuotaName` the catalog publishes — the code is
    /// looked up, so no row pins a code the catalog derives.
    quota_name: &'static str,
    desired: f64,
    /// `SupportCaseAllowed` as the request carries it; `None` omits the member, which
    /// the model documents as True.
    support_case_allowed: Option<bool>,
    /// A `ContextId` naming one resource, for a quota that is adjustable per resource.
    context_id: Option<String>,
}

impl Row {
    /// Whether Service Quotas opens a support case for this request itself: the model
    /// documents the flag's default as True, and only False as "does not open a support
    /// case".
    fn expects_case(&self) -> bool {
        self.support_case_allowed.unwrap_or(true)
    }

    /// The status the request must settle to: an increase a case reviewed is approved,
    /// one whose submitter opted out of the case is not approved.
    fn terminal_status(&self) -> &'static str {
        if self.expects_case() {
            STATUS_APPROVED
        } else {
            STATUS_NOT_APPROVED
        }
    }

    /// The whole status sequence the request must report, in order.
    fn expected_statuses(&self) -> Vec<&'static str> {
        if self.expects_case() {
            vec![STATUS_PENDING, STATUS_CASE_OPENED, self.terminal_status()]
        } else {
            vec![STATUS_PENDING, self.terminal_status()]
        }
    }
}

/// What a read reported about a request at one poll.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    status: String,
    case_id: Option<String>,
}

impl Observed {
    /// The `RequestedServiceQuotaChange` a read answered with, as an observation.
    fn of(record: &Value) -> Self {
        Self {
            status: record["Status"].as_str().unwrap_or_default().to_string(),
            case_id: record["CaseId"].as_str().map(str::to_string),
        }
    }
}

/// The stored request `id`, as `GetRequestedServiceQuotaChange` reports it.
async fn read_request(caller: &Client, id: &str) -> Value {
    caller
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await["RequestedQuota"]
        .clone()
}

/// The `QuotaCode` the catalog gives the quota named `quota_name` of `service_code`.
async fn quota_code_for(caller: &Client, service_code: &str, quota_name: &str) -> String {
    let listed = walk_pages(
        caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await;
    listed
        .iter()
        .find(|quota| quota["QuotaName"] == json!(quota_name))
        .and_then(|quota| quota["QuotaCode"].as_str())
        .unwrap_or_else(|| {
            panic!("case {service_code}: the catalog must publish a quota named {quota_name:?}")
        })
        .to_string()
}

/// Read the value in force for one quota, on the plane `context_id` names.
async fn value_in_force(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    context_id: Option<&str>,
) -> f64 {
    let mut body = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
    if let Some(context_id) = context_id {
        body["ContextId"] = json!(context_id);
    }
    let quota = caller.ok("GetServiceQuota", body).await;
    quota["Quota"]["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {service_code}/{quota_code}: expected a value, got {quota}"))
}

/// A request under observation, with everything the table expects of it.
struct Watched {
    case: String,
    id: String,
    quota_code: String,
    service_code: &'static str,
    desired: f64,
    context_id: Option<String>,
    /// The value in force before the request was made, which an undecided — and a
    /// never-approved — request leaves alone.
    before: f64,
    expects_case: bool,
    expected_statuses: Vec<&'static str>,
    /// Every DISTINCT observation, in the order it was reported.
    seen: Vec<Observed>,
    /// Whether the mid-flight checks have been made for this request yet.
    checked_open: bool,
}

impl Watched {
    /// The statuses reported so far, in order.
    fn statuses(&self) -> Vec<&str> {
        self.seen
            .iter()
            .map(|observed| observed.status.as_str())
            .collect()
    }

    /// Whether the request has reached a status the workflow does not leave.
    fn settled(&self) -> bool {
        self.seen
            .last()
            .is_some_and(|observed| observed.status == STATUS_APPROVED || observed.status == STATUS_NOT_APPROVED)
    }

    /// The `CaseId` the request reported while its case was open, if it ever did.
    fn case_id_while_open(&self) -> Option<&str> {
        self.seen
            .iter()
            .find(|observed| observed.status == STATUS_CASE_OPENED)
            .and_then(|observed| observed.case_id.as_deref())
    }
}

/// Make the request of one row and start watching it.
async fn submit(caller: &Client, row: &Row, quota_code: String) -> Watched {
    let case = format!("{} ({}/{})", row.why, row.service_code, row.quota_name);
    let before = value_in_force(
        caller,
        row.service_code,
        &quota_code,
        row.context_id.as_deref(),
    )
    .await;

    let mut body = json!({
        "ServiceCode": row.service_code,
        "QuotaCode": quota_code,
        "DesiredValue": row.desired,
    });
    if let Some(flag) = row.support_case_allowed {
        body["SupportCaseAllowed"] = json!(flag);
    }
    if let Some(context_id) = &row.context_id {
        body["ContextId"] = json!(context_id);
    }
    let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
    let minted = &accepted["RequestedQuota"];
    let observed = Observed::of(minted);
    // However the request will be processed, it is RECEIVED pending and with no case:
    // a case is opened on it afterwards, not by the call that makes it.
    assert_eq!(
        (observed.status.as_str(), observed.case_id.as_deref()),
        (STATUS_PENDING, None),
        "case {case}: expected a PENDING request carrying no CaseId, got {accepted}"
    );
    let id = minted["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the accepted request reports an Id"))
        .to_string();
    if row.context_id.is_some() {
        assert_eq!(
            minted["QuotaRequestedAtLevel"].as_str(),
            Some(LEVEL_RESOURCE),
            "case {case}: a request naming a resource is made at the RESOURCE level, got \
             {accepted}"
        );
    }

    Watched {
        case,
        id,
        quota_code,
        service_code: row.service_code,
        desired: row.desired,
        context_id: row.context_id.clone(),
        before,
        expects_case: row.expects_case(),
        expected_statuses: row.expected_statuses(),
        seen: vec![observed],
        checked_open: false,
    }
}

/// While a support case is open on a request, the request is still under review: it is
/// reported as `CASE_OPENED` with its `CaseId` by both history listings, selected by the
/// `CASE_OPENED` status filter, blocks a second request for the same plane, and has put
/// NOTHING in force yet.
async fn check_while_the_case_is_open(caller: &Client, watched: &Watched) {
    let case = &watched.case;
    let case_id = watched
        .case_id_while_open()
        .unwrap_or_else(|| panic!("case {case}: a CASE_OPENED request must report a CaseId"));
    assert!(
        !case_id.is_empty(),
        "case {case}: expected a non-empty CaseId while the case is open"
    );

    // `QuotaRequestedAtLevel` is asked for as ALL because the model documents ACCOUNT
    // as its default, and one row of the table asks for a resource's own increase.
    for (listing, body) in [
        (
            HISTORY_BY_QUOTA,
            json!({
                "ServiceCode": watched.service_code,
                "QuotaCode": watched.quota_code,
                "Status": STATUS_CASE_OPENED,
                "QuotaRequestedAtLevel": LEVEL_ALL,
            }),
        ),
        (
            HISTORY,
            json!({"Status": STATUS_CASE_OPENED, "QuotaRequestedAtLevel": LEVEL_ALL}),
        ),
    ] {
        let reported = caller.ok(listing, body.clone()).await;
        let matching: Vec<&Value> = reported["RequestedQuotas"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry["Id"].as_str() == Some(watched.id.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            matching.len(),
            1,
            "case {case}: the {listing} {body} filter must select the request exactly once, \
             got {reported}"
        );
        assert_eq!(
            matching[0]["CaseId"].as_str(),
            Some(case_id),
            "case {case}: the {listing} filter must report the same CaseId, got {reported}"
        );
    }

    // An open case is an open request, so the quota plane takes no second one.
    let mut second = json!({
        "ServiceCode": watched.service_code,
        "QuotaCode": watched.quota_code,
        "DesiredValue": watched.desired + 1.0,
    });
    if let Some(context_id) = &watched.context_id {
        second["ContextId"] = json!(context_id);
    }
    let (status, code) = caller.refused("RequestServiceQuotaIncrease", second).await;
    assert_eq!(
        (status, code.contains("ResourceAlreadyExistsException")),
        (400, true),
        "case {case}: a request whose case is open blocks a second one, got {status} {code}"
    );

    // The increase is under review, not in force: nothing is applied before the
    // decision.
    let in_force = value_in_force(
        caller,
        watched.service_code,
        &watched.quota_code,
        watched.context_id.as_deref(),
    )
    .await;
    assert_eq!(
        in_force, watched.before,
        "case {case}: expected the quota to keep {} while the case is open, got {in_force}",
        watched.before
    );
}

/// A request Service Quotas may open a support case for passes through `CASE_OPENED`
/// with a `CaseId` before it is decided; one whose submitter opted out of the case does
/// not.
#[tokio::test]
async fn a_request_that_allows_a_support_case_passes_through_case_opened_with_a_case_id() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // The rows differ in the FORM of `SupportCaseAllowed` (absent, explicitly true,
    // explicitly false), in the service and quota they name, in whether the quota is
    // counted per Region or account-wide, and in the plane the increase is asked for
    // (account-wide, or one resource of a quota the catalog publishes as adjustable per
    // resource).
    let rows = vec![
        Row {
            why: "absent: the model documents the default as True",
            service_code: "vpc",
            quota_name: "VPCs per Region",
            desired: 20.0,
            support_case_allowed: None,
            context_id: None,
        },
        Row {
            why: "explicitly allowed",
            service_code: "ec2",
            quota_name: "EC2-VPC Elastic IPs",
            desired: 100.0,
            support_case_allowed: Some(true),
            context_id: None,
        },
        Row {
            why: "explicitly allowed on an account-wide quota",
            service_code: "s3",
            quota_name: "General purpose buckets",
            desired: 20_000.0,
            support_case_allowed: Some(true),
            context_id: None,
        },
        Row {
            why: "allowed for ONE resource of a quota adjustable per resource",
            service_code: "es",
            quota_name: "Instances per domain",
            desired: 200.0,
            support_case_allowed: None,
            context_id: Some(format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/search-index")),
        },
        Row {
            why: "opted out: Amazon Web Services opens no support case",
            service_code: "dynamodb",
            quota_name: "Maximum number of tables",
            desired: 100_000.0,
            support_case_allowed: Some(false),
            context_id: None,
        },
        Row {
            why: "opted out on another service's quota",
            service_code: "lambda",
            quota_name: "Concurrent executions",
            desired: 2_000.0,
            support_case_allowed: Some(false),
            context_id: None,
        },
    ];

    let mut watched: Vec<Watched> = Vec::new();
    for row in &rows {
        let quota_code = quota_code_for(&caller, row.service_code, row.quota_name).await;
        watched.push(submit(&caller, row, quota_code).await);
    }

    // Watch every request at once, so each one's steps are observed as the clock makes
    // them due. The deadline clears both steps of the workflow many times over, so it
    // only ever fires on a request that is genuinely stuck.
    let deadline = Instant::now() + 20 * (SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY);
    while watched.iter().any(|entry| !entry.settled()) {
        assert!(
            Instant::now() < deadline,
            "case the watched requests: expected every request to settle, got {:?}",
            watched
                .iter()
                .map(|entry| (entry.case.clone(), entry.statuses().join(" -> ")))
                .collect::<Vec<_>>()
        );
        for entry in watched.iter_mut() {
            if entry.settled() {
                continue;
            }
            let observed = Observed::of(&read_request(&caller, &entry.id).await);
            if entry.seen.last() != Some(&observed) {
                entry.seen.push(observed);
            }
            // Everything a request's own open case implies, checked while it is open.
            let case_is_open = !entry.checked_open
                && entry
                    .seen
                    .last()
                    .is_some_and(|observed| observed.status == STATUS_CASE_OPENED);
            if case_is_open {
                check_while_the_case_is_open(&caller, entry).await;
                entry.checked_open = true;
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let mut case_ids: BTreeSet<String> = BTreeSet::new();
    for entry in &watched {
        let case = &entry.case;
        assert_eq!(
            entry.statuses(),
            entry.expected_statuses,
            "case {case}: expected the status sequence {:?}, got {:?}",
            entry.expected_statuses,
            entry.statuses()
        );

        let settled = read_request(&caller, &entry.id).await;
        let final_observation = Observed::of(&settled);
        if entry.expects_case {
            let open_case_id = entry
                .case_id_while_open()
                .unwrap_or_else(|| panic!("case {case}: expected a CaseId, got {settled}"));
            // The case id is state: the decided record keeps the case the request was
            // processed under, so a client can still follow it up.
            assert_eq!(
                final_observation.case_id.as_deref(),
                Some(open_case_id),
                "case {case}: a decided request keeps its CaseId, got {settled}"
            );
            assert!(
                case_ids.insert(open_case_id.to_string()),
                "case {case}: two requests must not share the CaseId {open_case_id}"
            );
            // Before the case was opened the request carried no case id at all.
            assert_eq!(
                entry.seen.first().and_then(|first| first.case_id.as_deref()),
                None,
                "case {case}: expected no CaseId before the case was opened, got {:?}",
                entry.seen
            );
        } else {
            assert!(
                entry
                    .seen
                    .iter()
                    .all(|observed| observed.case_id.is_none()),
                "case {case}: a request that opted out of its case must never report a \
                 CaseId, got {:?}",
                entry.seen
            );
        }

        // What the decision did to the quota: an approved increase is in force on the
        // plane it was asked for, and a request that was never approved leaves it alone.
        let expected = if entry.expects_case {
            entry.desired
        } else {
            entry.before
        };
        let in_force = value_in_force(
            &caller,
            entry.service_code,
            &entry.quota_code,
            entry.context_id.as_deref(),
        )
        .await;
        assert_eq!(
            in_force, expected,
            "case {case}: expected {expected} in force once the request settled, got {in_force}"
        );
        // Reading again reports the same record: the steps were the clock's, not the
        // reader's.
        let again = Observed::of(&read_request(&caller, &entry.id).await);
        assert_eq!(
            again, final_observation,
            "case {case}: a second read must report the same record, got {again:?}"
        );
    }
}

/// Every read path reports the one stored record of a request whose case Service Quotas
/// opened itself, and the status filters read the status it carries now.
#[tokio::test]
async fn every_read_path_reports_the_case_service_quotas_opened_itself() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");
    let quota_code = quota_code_for(&caller, "vpc", "VPCs per Region").await;
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": quota_code});

    let id = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": quota_code,
                "DesiredValue": 30.0,
            }),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the accepted request reports an Id")
        .to_string();

    // Poll until Service Quotas has taken the request up and opened the case that
    // processes it.
    let deadline = Instant::now() + 20 * (SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY);
    let mut record = read_request(&caller, &id).await;
    while Observed::of(&record).status != STATUS_CASE_OPENED {
        assert!(
            Instant::now() < deadline,
            "case a request left to Service Quotas: expected it to reach {STATUS_CASE_OPENED}, \
             got {record}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
        record = read_request(&caller, &id).await;
    }
    let case_id = record["CaseId"]
        .as_str()
        .unwrap_or_else(|| panic!("case an opened case: expected a CaseId, got {record}"))
        .to_string();

    // Both listings read the SAME record as the single read, unfiltered and filtered on
    // the status the request carries now.
    for (listing, body) in [
        (HISTORY_BY_QUOTA, pair.clone()),
        (
            HISTORY_BY_QUOTA,
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": pair["QuotaCode"].clone(),
                "Status": STATUS_CASE_OPENED,
            }),
        ),
        (HISTORY, json!({})),
        (HISTORY, json!({"Status": STATUS_CASE_OPENED})),
    ] {
        let reported = caller.ok(listing, body.clone()).await;
        let listed: Vec<&Value> = reported["RequestedQuotas"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry["Id"].as_str() == Some(id.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {listing} {body}: expected the request listed exactly once, got {reported}"
        );
        assert_eq!(
            *listed[0], record,
            "case {listing} {body}: the listed record must equal the single read, got {reported}"
        );
        assert_eq!(
            listed[0]["CaseId"].as_str(),
            Some(case_id.as_str()),
            "case {listing} {body}: expected the stored CaseId, got {reported}"
        );
    }

    // A status the request does NOT carry selects nothing, so the filter reads the
    // stored status rather than ignoring it.
    for status in [STATUS_PENDING, STATUS_APPROVED, STATUS_NOT_APPROVED, "DENIED"] {
        let reported = caller
            .ok(
                HISTORY_BY_QUOTA,
                json!({
                    "ServiceCode": "vpc",
                    "QuotaCode": pair["QuotaCode"].clone(),
                    "Status": status,
                }),
            )
            .await;
        let selected: Vec<&str> = reported["RequestedQuotas"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["Id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !selected.contains(&id.as_str()),
            "case the {status} filter: a request whose case is open must not be selected, got \
             {reported}"
        );
    }
}
