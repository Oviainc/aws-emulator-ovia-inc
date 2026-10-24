// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE plane of an applied quota: the `ContextId` that selects it, the
//! `QuotaContext` that describes it, and the level every record reports it at.
//!
//! The rule under test, over a TABLE rather than one recorded call: a quota the
//! catalog publishes as adjustable per resource carries a `QuotaContext` on every
//! record and honours a `ContextId`, which selects the value in force for THAT
//! resource (the value applied to it, else the one applied to every resource with none
//! of its own, else the account-wide value, else the AWS default) and is reported at
//! the `RESOURCE` level; a quota adjustable at the account level only carries no
//! context and refuses a `ContextId`.
//!
//! The rows span both kinds of quota, two services, several distinct `ContextId`s (one
//! per resource, the every-resource `*`, and a resource no value was ever applied to),
//! and both request levels of the same quota. The harness (`Client`, `walk_pages`) is
//! the one `quota_workflow.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    CONTEXT_ID_EVERY_RESOURCE, LEVEL_ACCOUNT, LEVEL_ALL, LEVEL_RESOURCE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};

/// Comfortably past `crate::state::REQUEST_DECISION_DELAY` (500 ms), so a request made
/// before this wait is decided afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_200);

/// The history listing, whose name is too long to repeat in every call.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The quota the catalog publishes as adjustable per resource AND account-wide, and
/// the resource type its per-resource plane applies to (`src/state.rs`, the `es` rows
/// — `QuotaContextInfo`'s own worked example).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";
const RESOURCE_SCOPE_TYPE: &str = "AWS::Elasticsearch::Domain";

/// A quota of the SAME service that is adjustable at the account level only, so the
/// contrast is not between two services.
const ACCOUNT_QUOTA_NAME: &str = "Domains per Region";

/// A quota of ANOTHER service that is adjustable at the account level only, so the
/// contrast is not confined to one service's rows either.
const OTHER_ACCOUNT_QUOTA_NAME: &str = "VPCs per Region";

/// The `QuotaCode` the catalog gives the quota named `quota_name` of `service_code`.
///
/// Looked up through the listing rather than written down, so the table does not pin a
/// code the catalog derives.
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

/// A domain ARN of this account, as a `ContextId` naming one resource.
fn domain(name: &str) -> String {
    format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/{name}")
}

/// Ask for an increase and return the minted request.
async fn request_increase(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    desired: f64,
    context: Option<&str>,
) -> Value {
    let mut body = json!({
        "ServiceCode": service_code,
        "QuotaCode": quota_code,
        "DesiredValue": desired,
    });
    if let Some(context) = context {
        body["ContextId"] = json!(context);
    }
    caller.ok("RequestServiceQuotaIncrease", body).await["RequestedQuota"].clone()
}

/// Read one applied quota, optionally for a resource context.
async fn get_quota(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    context: Option<&str>,
) -> Value {
    let mut body = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
    if let Some(context) = context {
        body["ContextId"] = json!(context);
    }
    caller.ok("GetServiceQuota", body).await["Quota"].clone()
}

/// A quota adjustable per resource describes its plane on every record; one adjustable
/// at the account level only carries no context and refuses a `ContextId`.
#[tokio::test]
async fn the_quota_context_is_reported_exactly_for_the_quotas_adjustable_per_resource() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let resource_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let account_code = quota_code_for(&caller, "es", ACCOUNT_QUOTA_NAME).await;
    let other_account_code = quota_code_for(&caller, "vpc", OTHER_ACCOUNT_QUOTA_NAME).await;

    // (case, service code, quota code, the QuotaContext every record must carry).
    let rows = [
        (
            "a quota adjustable per resource and account-wide",
            "es",
            resource_code.clone(),
            Some(json!({
                "ContextScope": LEVEL_ACCOUNT,
                "ContextScopeType": RESOURCE_SCOPE_TYPE,
                "AdjustableAtLevel": LEVEL_ALL,
            })),
        ),
        (
            "a quota of the same service adjustable at the account level only",
            "es",
            account_code.clone(),
            None,
        ),
        (
            "a quota of another service adjustable at the account level only",
            "vpc",
            other_account_code.clone(),
            None,
        ),
    ];

    for (case, service_code, quota_code, expected) in &rows {
        let applied = get_quota(&caller, service_code, quota_code, None).await;
        let listed = walk_pages(
            &caller,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        let default = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await["Quota"]
            .clone();

        let wanted = expected.clone().unwrap_or(Value::Null);
        assert_eq!(
            applied["QuotaContext"], wanted,
            "case {case}: GetServiceQuota must report {wanted}, got {applied}"
        );
        assert_eq!(
            default["QuotaContext"], wanted,
            "case {case}: GetAWSDefaultServiceQuota must report {wanted}, got {default}"
        );
        assert!(
            listed.contains(&applied),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
        // Whatever the quota's adjustable level, a read that names no context answers
        // for the account.
        assert_eq!(
            applied["QuotaAppliedAtLevel"],
            json!(LEVEL_ACCOUNT),
            "case {case}: a read naming no ContextId answers at the ACCOUNT level, got {applied}"
        );

        // A ContextId is honoured exactly where there is a resource plane for it.
        let context = domain("probe");
        let body = json!({
            "ServiceCode": service_code,
            "QuotaCode": quota_code,
            "ContextId": context,
        });
        if expected.is_some() {
            let scoped = caller.ok("GetServiceQuota", body).await["Quota"].clone();
            assert_eq!(
                scoped["QuotaContext"]["ContextId"],
                json!(context),
                "case {case}: the record must name the resource asked for, got {scoped}"
            );
        } else {
            let (status, code) = caller.refused("GetServiceQuota", body).await;
            assert_eq!(
                status, 400,
                "case {case}: a ContextId must be refused with HTTP 400, got {status} and {code}"
            );
            assert!(
                code.contains("IllegalArgumentException"),
                "case {case}: expected IllegalArgumentException, got {code}"
            );
        }
    }
}

/// A `ContextId` selects the value in force for that resource: its own, else the one
/// applied to every resource, else the account-wide value, else the AWS default.
#[tokio::test]
async fn a_context_id_selects_the_value_applied_to_that_resource() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let default_value = caller
        .ok(
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "es", "QuotaCode": quota_code}),
        )
        .await["Quota"]["Value"]
        .as_f64()
        .expect("the default quota reports a Value");

    let alpha = domain("alpha");
    let beta = domain("beta");
    let untouched = domain("gamma");

    // Each row is an increase on a DIFFERENT plane of the one quota: two named
    // resources, and the every-resource `*`. They are asked for before any of them is
    // decided, which is also what pins that one plane's open request does not block
    // another's.
    let increases = [
        (alpha.clone(), 120.0_f64),
        (beta.clone(), 200.0),
        (CONTEXT_ID_EVERY_RESOURCE.to_string(), 90.0),
    ];
    for (context, desired) in &increases {
        let requested = request_increase(&caller, "es", &quota_code, *desired, Some(context)).await;
        assert_eq!(
            requested["QuotaRequestedAtLevel"],
            json!(LEVEL_RESOURCE),
            "case {context}: a request naming a ContextId is made at the RESOURCE level, got \
             {requested}"
        );
        assert_eq!(
            requested["QuotaContext"],
            json!({
                "ContextScope": LEVEL_RESOURCE,
                "ContextScopeType": RESOURCE_SCOPE_TYPE,
                "ContextId": context,
                "AdjustableAtLevel": LEVEL_ALL,
            }),
            "case {context}: the stored request must describe the resource plane, got {requested}"
        );
    }

    // Nothing is in force yet: the decisions are not due.
    for (context, _) in &increases {
        let scoped = get_quota(&caller, "es", &quota_code, Some(context)).await;
        assert_eq!(
            scoped["Value"].as_f64(),
            Some(default_value),
            "case {context} before the decision: the value in force must not move, got {scoped}"
        );
    }

    tokio::time::sleep(PAST_DECISION).await;

    // (case, the ContextId read, the value it must now report).
    let expectations = [
        (
            "the resource its own value was applied to",
            Some(alpha.clone()),
            120.0_f64,
        ),
        ("another resource with its own value", Some(beta.clone()), 200.0),
        (
            "the every-resource context",
            Some(CONTEXT_ID_EVERY_RESOURCE.to_string()),
            90.0,
        ),
        (
            "a resource no value of its own was applied to",
            Some(untouched.clone()),
            90.0,
        ),
        ("the account-wide record", None, default_value),
    ];
    for (case, context, expected) in &expectations {
        let quota = get_quota(&caller, "es", &quota_code, context.as_deref()).await;
        assert_eq!(
            quota["Value"].as_f64(),
            Some(*expected),
            "case {case}: expected Value {expected}, got {quota}"
        );
        let level = if context.is_some() {
            LEVEL_RESOURCE
        } else {
            LEVEL_ACCOUNT
        };
        assert_eq!(
            quota["QuotaAppliedAtLevel"],
            json!(level),
            "case {case}: expected QuotaAppliedAtLevel {level}, got {quota}"
        );
        assert_eq!(
            quota["QuotaContext"]["ContextId"],
            context.clone().map_or(Value::Null, Value::String),
            "case {case}: the record must name the context it answers for, got {quota}"
        );
        // Reading twice answers the same: a read applies nothing.
        let again = get_quota(&caller, "es", &quota_code, context.as_deref()).await;
        assert_eq!(
            again, quota,
            "case {case}: a second read must answer the same record, got {again}"
        );
    }

    // The AWS default never moved, and nor did another resource-level quota's plane.
    let default_now = caller
        .ok(
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "es", "QuotaCode": quota_code}),
        )
        .await["Quota"]["Value"]
        .as_f64();
    assert_eq!(
        default_now,
        Some(default_value),
        "case the AWS default: a resource-level increase must never move it, got {default_now:?}"
    );

    // An account-wide increase on the same quota lands on the account plane only: a
    // resource with a value of its own keeps it.
    let account_wide = request_increase(&caller, "es", &quota_code, 500.0, None).await;
    assert_eq!(
        account_wide["QuotaRequestedAtLevel"],
        json!(LEVEL_ACCOUNT),
        "case an account-wide request: it must be recorded at the ACCOUNT level, got \
         {account_wide}"
    );
    tokio::time::sleep(PAST_DECISION).await;
    let after = [
        ("the account-wide record", None, 500.0_f64),
        ("a resource with its own value", Some(alpha.clone()), 120.0),
        (
            "a resource covered by the every-resource value",
            Some(untouched.clone()),
            90.0,
        ),
    ];
    for (case, context, expected) in &after {
        let quota = get_quota(&caller, "es", &quota_code, context.as_deref()).await;
        assert_eq!(
            quota["Value"].as_f64(),
            Some(*expected),
            "case {case} after an account-wide increase: expected Value {expected}, got {quota}"
        );
    }
}

/// The listing reports the plane the caller asks for: `RESOURCE` the records a value
/// was applied to a resource for, `ACCOUNT` (the default) the account-wide ones, `ALL`
/// both.
#[tokio::test]
async fn the_applied_level_filter_picks_the_plane_the_listing_reports() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let account_only = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "es"}),
        "Quotas",
        None,
    )
    .await;

    // With nothing applied per resource, the RESOURCE plane is empty.
    let planes = [
        (LEVEL_ACCOUNT, account_only.len()),
        (LEVEL_ALL, account_only.len()),
        (LEVEL_RESOURCE, 0),
    ];
    for (level, expected) in planes {
        let listed = walk_pages(
            &caller,
            "ListServiceQuotas",
            &json!({"ServiceCode": "es", "QuotaAppliedAtLevel": level}),
            "Quotas",
            None,
        )
        .await;
        assert_eq!(
            listed.len(),
            expected,
            "case QuotaAppliedAtLevel={level} with nothing applied per resource: expected \
             {expected} records, got {listed:?}"
        );
    }

    let contexts = [domain("alpha"), CONTEXT_ID_EVERY_RESOURCE.to_string()];
    for (index, context) in contexts.iter().enumerate() {
        request_increase(
            &caller,
            "es",
            &quota_code,
            150.0 + index as f64,
            Some(context),
        )
        .await;
    }
    tokio::time::sleep(PAST_DECISION).await;

    let resource_records = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "es", "QuotaAppliedAtLevel": LEVEL_RESOURCE}),
        "Quotas",
        None,
    )
    .await;
    let reported: Vec<&str> = resource_records
        .iter()
        .map(|quota| {
            quota["QuotaContext"]["ContextId"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    let mut wanted: Vec<&str> = contexts.iter().map(String::as_str).collect();
    wanted.sort_unstable();
    assert_eq!(
        reported, wanted,
        "case QuotaAppliedAtLevel=RESOURCE: expected one record per applied context, got \
         {resource_records:?}"
    );
    for record in &resource_records {
        assert_eq!(
            record["QuotaAppliedAtLevel"],
            json!(LEVEL_RESOURCE),
            "case a resource record: it must report the RESOURCE level, got {record}"
        );
        assert_eq!(
            record["QuotaCode"],
            json!(quota_code),
            "case a resource record: it must name its quota, got {record}"
        );
    }

    // ACCOUNT, and the unfiltered listing, keep reporting one record per quota; ALL
    // reports both planes.
    let unfiltered = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "es"}),
        "Quotas",
        None,
    )
    .await;
    assert_eq!(
        unfiltered, account_only,
        "case no QuotaAppliedAtLevel: the listing defaults to the ACCOUNT plane, got \
         {unfiltered:?}"
    );
    let every = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "es", "QuotaAppliedAtLevel": LEVEL_ALL}),
        "Quotas",
        None,
    )
    .await;
    assert_eq!(
        every.len(),
        account_only.len() + resource_records.len(),
        "case QuotaAppliedAtLevel=ALL: expected both planes, got {every:?}"
    );
}

/// A resource-level request is stored, read back and listed at the `RESOURCE` level,
/// and one open request per plane is the ceiling — not one per quota.
#[tokio::test]
async fn a_resource_level_request_is_stored_and_listed_at_its_own_level() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let alpha = domain("alpha");
    let beta = domain("beta");

    let first = request_increase(&caller, "es", &quota_code, 120.0, Some(&alpha)).await;
    let id = first["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case a resource-level request: expected an Id, got {first}"))
        .to_string();
    let stored = caller
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await["RequestedQuota"]
        .clone();
    assert_eq!(
        stored, first,
        "case a resource-level request: it must read back unchanged, got {stored}"
    );

    // Another resource's plane accepts its own open request; the SAME plane does not.
    let second = request_increase(&caller, "es", &quota_code, 130.0, Some(&beta)).await;
    assert_eq!(
        second["Status"],
        json!("PENDING"),
        "case a second resource's request: it must be accepted, got {second}"
    );
    let (status, code) = caller
        .refused(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "es",
                "QuotaCode": quota_code,
                "DesiredValue": 140.0,
                "ContextId": alpha,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a second request for one resource: expected HTTP 400, got {status} and {code}"
    );
    assert!(
        code.contains("ResourceAlreadyExistsException"),
        "case a second request for one resource: expected ResourceAlreadyExistsException, got \
         {code}"
    );

    // An account-wide request on the same quota is its own plane, recorded at ACCOUNT.
    let account_wide = request_increase(&caller, "es", &quota_code, 400.0, None).await;
    assert_eq!(
        account_wide["QuotaRequestedAtLevel"],
        json!(LEVEL_ACCOUNT),
        "case an account-wide request: expected the ACCOUNT level, got {account_wide}"
    );
    assert_eq!(
        account_wide["QuotaContext"],
        json!({
            "ContextScope": LEVEL_ACCOUNT,
            "ContextScopeType": RESOURCE_SCOPE_TYPE,
            "AdjustableAtLevel": LEVEL_ALL,
        }),
        "case an account-wide request: it must describe the account plane, got {account_wide}"
    );

    // The history filter keeps the requests made at the level asked for.
    let expectations = [
        (LEVEL_RESOURCE, 2_usize),
        (LEVEL_ACCOUNT, 1),
        (LEVEL_ALL, 3),
    ];
    for (level, expected) in expectations {
        let history = walk_pages(
            &caller,
            HISTORY,
            &json!({
                "ServiceCode": "es",
                "QuotaCode": quota_code,
                "QuotaRequestedAtLevel": level,
            }),
            "RequestedQuotas",
            None,
        )
        .await;
        assert_eq!(
            history.len(),
            expected,
            "case QuotaRequestedAtLevel={level}: expected {expected} requests, got {history:?}"
        );
    }
}
