// SPDX-License-Identifier: CC0-1.0

//! The Automatic Management exclusion list as a set of RESOURCE names, end to end over
//! the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the HTTP
//! behaviour and the persisted state behind it. The harness (`emulator`, `client`,
//! `Client`) is `quota_workflow.rs`'s.
//!
//! The rule asserted, over a TABLE of exclusion lists rather than one case: an
//! exclusion names the quota it excludes by `(ServiceCode, QuotaCode)`, so a write that
//! carries a name no quota of this service answers to is refused and records nothing —
//! a key naming no service at all with the modelled `NoSuchResourceException`, in the
//! same words the catalog states that fact in, and a code a published service holds no
//! quota under with the modelled `IllegalArgumentException`, naming the service code and
//! the limit code of the entry that resolved to nothing — while a list whose every entry
//! resolves is stored and reported with the `QuotaName` the catalog holds for each code.
//!
//! Both writers of the member are driven: the FULL writer (`StartAutoManagement`) and
//! the PARTIAL one (`UpdateAutoManagement`) over a running configuration, whose stored
//! list a refused update must leave exactly as it was. `auto_management.rs` owns the
//! accepting side of the writers; `auto_management_configuration.rs` owns how a stored
//! configuration is reported.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    OPT_IN_LEVEL_VALUES, OPT_IN_TYPE_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::excluded_quota_absent_message;
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    OPT_IN_STATUS_DISABLED, OPT_IN_STATUS_ENABLED,
};

use crate::quota_workflow::{client, emulator, Client};

/// The only `OptInLevel` member the model declares, taken from the model's own value
/// list rather than spelled again here.
const ACCOUNT_LEVEL: &str = OPT_IN_LEVEL_VALUES[0];

/// An `OptInType` member a write records, taken from the model's own value list: this
/// suite is about the exclusion list rather than about that member, which
/// `auto_management.rs` spans over every member the enum declares.
const OPT_IN_TYPE: &str = OPT_IN_TYPE_VALUES[0];

/// The operation that reads a configuration back.
const GET_CONFIG: &str = "GetAutoManagementConfiguration";

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// An exclusion list as the rows below spell one: service code -> quota codes.
type Exclusions = &'static [(&'static str, &'static [&'static str])];

/// What an exclusion list names that no quota of this service answers to — and the
/// catalog call that states the SAME fact over the wire, so the refusal is compared
/// against the service's own wording for it instead of a second copy of it.
#[derive(Debug, Clone, Copy)]
enum Unresolvable {
    /// A `ServiceCode` the service catalog does not hold. `ListAWSDefaultServiceQuotas`
    /// states it for the same code.
    Service(&'static str),
    /// A `(ServiceCode, QuotaCode)` pair the quota catalog does not hold.
    /// `GetAWSDefaultServiceQuota` states it for the same pair.
    Quota(&'static str, &'static str),
}

/// The `ExclusionList` member of a request, from a row's exclusions.
fn exclusion_request(exclusions: &[(&str, &[&str])]) -> Value {
    let mut map = serde_json::Map::new();
    for (service_code, codes) in exclusions {
        map.insert((*service_code).to_string(), json!(codes));
    }
    Value::Object(map)
}

/// Dispatch a call and return its status, error code and `Message`.
async fn answer(client: &Client, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, parsed) = client.call(operation, body).await;
    let member = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    (status, member("__type"), member("Message"))
}

/// How the service states that `unresolvable` names nothing it holds: the error code and
/// the `Message` an exclusion write must answer with.
///
/// A key naming no service is the SAME fact a catalog listing scoped to that code states,
/// so it is read back off the catalog instead of being spelled a second time here. A code
/// under a service the catalog does publish is a bad argument in the map the call carried,
/// and names the entry the caller has to correct.
async fn expected_refusal(client: &Client, unresolvable: Unresolvable) -> (&'static str, String) {
    match unresolvable {
        Unresolvable::Service(service_code) => {
            let request = json!({ "ServiceCode": service_code });
            let (status, code, message) =
                answer(client, "ListAWSDefaultServiceQuotas", &request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "NoSuchResourceException"),
                "case catalog {unresolvable:?}: expected the catalog listing to refuse a \
                 service code it does not hold with HTTP {CLIENT_FAULT} \
                 NoSuchResourceException, got {status} {code} {message:?}"
            );
            ("NoSuchResourceException", message)
        }
        Unresolvable::Quota(service_code, quota_code) => (
            "IllegalArgumentException",
            excluded_quota_absent_message(service_code, quota_code),
        ),
    }
}

/// One reported exclusion: the `QuotaCode` and the `QuotaName` beside it — each `None`
/// when the entry carries that member at all, so a half-populated entry is visible
/// rather than papered over.
type ReportedEntry = (Option<String>, Option<String>);

/// The reported exclusion list, keyed by service code, each service's entries in
/// reported order. Empty when the read reported no `ExclusionList` at all.
fn reported_entries(body: &Value) -> BTreeMap<String, Vec<ReportedEntry>> {
    let Some(reported) = body.get("ExclusionList").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    reported
        .iter()
        .map(|(service_code, listed)| {
            let entries = listed
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|entry| {
                    let member =
                        |name: &str| entry.get(name).and_then(Value::as_str).map(str::to_string);
                    (member("QuotaCode"), member("QuotaName"))
                })
                .collect();
            (service_code.clone(), entries)
        })
        .collect()
}

/// Read the configuration back, which must succeed whatever is stored.
async fn configuration(client: &Client, case: &str) -> Value {
    let (status, body) = client.call(GET_CONFIG, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {GET_CONFIG} to answer HTTP 200, got {status} with {body}"
    );
    body
}

/// Assert that `operation` refused `request` with `expected_code` on this front end's
/// client-fault status, stating exactly `expected`.
async fn refused_with(
    client: &Client,
    case: &str,
    operation: &str,
    request: &Value,
    expected_code: &str,
    expected: &str,
) {
    let (status, code, message) = answer(client, operation, request).await;
    assert_eq!(
        status, CLIENT_FAULT,
        "case {case}: expected {operation} to answer HTTP {CLIENT_FAULT}, got {status} \
         with {message:?}"
    );
    assert_eq!(
        code, expected_code,
        "case {case}: expected {expected_code}, got {code} with {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case}: expected the refusal to state {expected:?}, got {message:?}"
    );
}

/// The exclusion lists that name something this service publishes no quota for, as
/// `(case, list, what it gets wrong)`.
///
/// The rows differ along what the list gets wrong and where: an unknown `QuotaCode`
/// alone and beside one the catalog holds, a code of ANOTHER service, an unknown
/// `ServiceCode` carrying codes and carrying none, a known service code in the wrong
/// case, and an unresolvable entry beside an entry that resolves (which must not be
/// recorded either). Shared by every test below, so each one spans the whole class.
fn unresolvable_rows() -> [(&'static str, Exclusions, Unresolvable); 7] {
    [
        (
            "a quota code the catalog does not hold",
            &[("lambda", &["L-00000000"])],
            Unresolvable::Quota("lambda", "L-00000000"),
        ),
        (
            "an unknown quota code beside one the catalog holds",
            &[("vpc", &["L-F678F1CE", "L-00000000"])],
            Unresolvable::Quota("vpc", "L-00000000"),
        ),
        (
            "a real quota code of another service",
            &[("lambda", &["L-F678F1CE"])],
            Unresolvable::Quota("lambda", "L-F678F1CE"),
        ),
        (
            "a service code the catalog does not hold",
            &[("not-a-service", &["L-12345678"])],
            Unresolvable::Service("not-a-service"),
        ),
        (
            "a service code the catalog does not hold, with no quota codes",
            &[("not-a-service", &[])],
            Unresolvable::Service("not-a-service"),
        ),
        (
            "a known service code in the wrong case",
            &[("LAMBDA", &["L-B99A9384"])],
            Unresolvable::Service("LAMBDA"),
        ),
        (
            "an unresolvable entry beside one that resolves",
            &[("ec2", &["L-1216C47A"]), ("not-a-service", &["L-12345678"])],
            Unresolvable::Service("not-a-service"),
        ),
    ]
}

/// An exclusion list that names something this service publishes no quota for is
/// refused by BOTH writers of the member, and records nothing.
///
/// The rows are [`unresolvable_rows`], and each one is driven through the FULL writer
/// (`StartAutoManagement`) and the PARTIAL one (`UpdateAutoManagement`) over a running
/// configuration, whose stored list the refusal must leave exactly as it was.
#[tokio::test]
async fn an_exclusion_that_names_no_quota_is_refused_and_records_nothing() {
    // The exclusion list the running configuration of the update half carries: codes
    // the catalog holds, so the only thing a row changes is the list under test.
    const RUNNING: Exclusions = &[("lambda", &["L-B99A9384"])];

    for (case, exclusions, unresolvable) in unresolvable_rows() {
        // The FULL writer, on an account that never opted in: the refusal leaves it
        // with no configuration at all.
        let start_engine = emulator();
        let start_client = client(&start_engine, "us-east-1");
        let (expected_code, expected) = expected_refusal(&start_client, unresolvable).await;

        let start_case = format!("StartAutoManagement {case}");
        let request = json!({
            "OptInLevel": ACCOUNT_LEVEL,
            "OptInType": OPT_IN_TYPE,
            "ExclusionList": exclusion_request(exclusions),
        });
        refused_with(
            &start_client,
            &start_case,
            "StartAutoManagement",
            &request,
            expected_code,
            &expected,
        )
        .await;

        let read = configuration(&start_client, &start_case).await;
        assert_eq!(
            read.get("OptInStatus").and_then(Value::as_str),
            Some(OPT_IN_STATUS_DISABLED),
            "case {start_case}: a refused start must record nothing, got {read}"
        );
        assert_eq!(
            reported_entries(&read),
            BTreeMap::new(),
            "case {start_case}: a refused start must record no exclusion, got {read}"
        );

        // The PARTIAL writer, over a RUNNING configuration: the refusal leaves the
        // stored list exactly as it was.
        let update_engine = emulator();
        let update_client = client(&update_engine, "eu-west-1");
        let update_case = format!("UpdateAutoManagement {case}");
        update_client
            .ok(
                "StartAutoManagement",
                json!({
                    "OptInLevel": ACCOUNT_LEVEL,
                    "OptInType": OPT_IN_TYPE,
                    "ExclusionList": exclusion_request(RUNNING),
                }),
            )
            .await;
        let running = configuration(&update_client, &update_case).await;

        let update = json!({ "ExclusionList": exclusion_request(exclusions) });
        refused_with(
            &update_client,
            &update_case,
            "UpdateAutoManagement",
            &update,
            expected_code,
            &expected,
        )
        .await;

        let read = configuration(&update_client, &update_case).await;
        assert_eq!(
            read, running,
            "case {update_case}: a refused update must leave the running configuration \
             {running} as it was, got {read}"
        );
        assert_eq!(
            read.get("OptInStatus").and_then(Value::as_str),
            Some(OPT_IN_STATUS_ENABLED),
            "case {update_case}: the running configuration must still be ENABLED, got {read}"
        );
    }
}

/// The configuration an update changes is looked up BEFORE the names the call carries:
/// an account with nothing running is told that it has no configuration to update,
/// whatever the exclusion list it sent names, and nothing is recorded either way.
///
/// The rows are the same unresolvable lists the refusal table spans, so the precedence
/// is pinned for every one of them rather than for one.
#[tokio::test]
async fn an_update_with_nothing_running_states_that_before_resolving_names() {
    for (case, exclusions, _) in unresolvable_rows() {
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let case = format!("UpdateAutoManagement with nothing running, {case}");

        // What this operation answers an account with no running configuration, read
        // from the service itself rather than spelled here: an update that carries
        // nothing to resolve at all.
        let (status, code, expected) = answer(&client, "UpdateAutoManagement", &json!({})).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_FAULT, "NoSuchResourceException"),
            "case {case}: expected an update with nothing running to be refused with HTTP \
             {CLIENT_FAULT} NoSuchResourceException, got {status} {code} {expected:?}"
        );

        let update = json!({ "ExclusionList": exclusion_request(exclusions) });
        refused_with(
            &client,
            &case,
            "UpdateAutoManagement",
            &update,
            "NoSuchResourceException",
            &expected,
        )
        .await;

        let read = configuration(&client, &case).await;
        assert_eq!(
            read.get("OptInStatus").and_then(Value::as_str),
            Some(OPT_IN_STATUS_DISABLED),
            "case {case}: a refused update must record nothing, got {read}"
        );
        assert_eq!(
            reported_entries(&read),
            BTreeMap::new(),
            "case {case}: a refused update must record no exclusion, got {read}"
        );
    }
}

/// Every entry of a list that DOES resolve is stored and reported with the catalog's
/// own `QuotaName` for its code — the other half of the rule, driven over quotas read
/// out of the catalog rather than named here, so the suite spans entries no issue cited.
#[tokio::test]
async fn an_exclusion_naming_quotas_the_catalog_holds_is_stored_and_reported_with_their_names() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    // Two services, each with the first two quotas the catalog publishes for it, read
    // over the wire: whatever the catalog holds is what is excluded.
    let picked = services_with_quotas(&client, 2, 2).await;
    assert_eq!(
        picked.len(),
        2,
        "case setup: expected two catalog services with at least two quotas each, got \
         {picked:?}"
    );

    let mut request = serde_json::Map::new();
    let mut expected: BTreeMap<String, Vec<ReportedEntry>> = BTreeMap::new();
    for (service_code, quotas) in &picked {
        request.insert(
            service_code.clone(),
            json!(quotas.iter().map(|(code, _)| code).collect::<Vec<_>>()),
        );
        expected.insert(
            service_code.clone(),
            quotas
                .iter()
                .map(|(code, name)| (Some(code.clone()), Some(name.clone())))
                .collect(),
        );
    }

    client
        .ok(
            "StartAutoManagement",
            json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": OPT_IN_TYPE,
                "ExclusionList": Value::Object(request.clone()),
            }),
        )
        .await;

    let read = configuration(&client, "catalog exclusions").await;
    assert_eq!(
        reported_entries(&read),
        expected,
        "case catalog exclusions: excluding {request:?} must be reported with the \
         catalog's own QuotaNames, got {read}"
    );
}

/// `wanted` services the catalog publishes at least `quotas_each` quotas for, each with
/// its first `quotas_each` `(QuotaCode, QuotaName)` pairs — read over the wire through
/// `ListServices` and `ListAWSDefaultServiceQuotas`, so this suite carries no copy of
/// the catalog.
async fn services_with_quotas(
    client: &Client,
    wanted: usize,
    quotas_each: usize,
) -> Vec<(String, Vec<(String, String)>)> {
    let listed = client
        .ok("ListServices", json!({ "MaxResults": 20 }))
        .await;
    let services = listed
        .get("Services")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut picked: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for service in services {
        if picked.len() == wanted {
            break;
        }
        let Some(service_code) = service.get("ServiceCode").and_then(Value::as_str) else {
            continue;
        };
        let (status, body) = client
            .call(
                "ListAWSDefaultServiceQuotas",
                &json!({ "ServiceCode": service_code, "MaxResults": quotas_each }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case setup {service_code}: expected the catalog listing to answer HTTP 200, \
             got {status} with {body}"
        );
        let quotas: Vec<(String, String)> = body
            .get("Quotas")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|quota| {
                let member = |name: &str| quota.get(name).and_then(Value::as_str);
                Some((member("QuotaCode")?.to_string(), member("QuotaName")?.to_string()))
            })
            .collect();
        if quotas.len() == quotas_each {
            picked.push((service_code.to_string(), quotas));
        }
    }
    picked
}
