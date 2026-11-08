// SPDX-License-Identifier: CC0-1.0

//! The planes one quota has, and the calls each plane serves.
//!
//! The rule under test, over a TABLE rather than one recorded call: the planes a quota
//! has are exactly the ones its `AdjustableAtLevel` publishes — `ACCOUNT` the
//! account-wide plane, `PER_RESOURCE` the per-resource plane, `ALL` both — so a call
//! that names a plane the quota does not have is refused with `IllegalArgumentException`
//! (HTTP 400), no record ever describes such a plane, and the read path and the write
//! path cannot disagree about which planes exist.
//!
//! The rows span every value `AdjustableAtLevelEnum` publishes
//! (`smithy_query(query="shape",shape="AdjustableAtLevelEnum")`: `ACCOUNT`,
//! `PER_RESOURCE`, `ALL`), both planes of each (a read or a write naming a resource
//! context, and one naming none), three services, and both handlers that ask the
//! question — `GetServiceQuota` and `RequestServiceQuotaIncrease` — plus the listing
//! that reports the account plane's records.
//!
//! The levels the seeded catalog publishes today are driven END TO END over the wire;
//! the one it does not (`PER_RESOURCE`, which no published row in this catalog carries)
//! is driven through the crate's single plane predicate and record builder, so the rule
//! is pinned for every value of the member rather than for the rows that happen to be
//! seeded. The harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    DefaultQuota, CONTEXT_ID_EVERY_RESOURCE, CONTEXT_SCOPE_ACCOUNT, CONTEXT_SCOPE_RESOURCE,
    LEVEL_ACCOUNT, LEVEL_ALL, LEVEL_PER_RESOURCE, LEVEL_RESOURCE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// Whether a quota published as adjustable at `level` has the account-wide plane and
/// the per-resource plane, from `AdjustableAtLevelEnum`'s own member documentation:
/// `ACCOUNT` — an increase only at the account level, `PER_RESOURCE` — only for an
/// individual resource, `ALL` — either.
fn planes_of(level: &str) -> (bool, bool) {
    match level {
        LEVEL_ACCOUNT => (true, false),
        LEVEL_PER_RESOURCE => (false, true),
        LEVEL_ALL => (true, true),
        other => panic!("case {other}: not a value AdjustableAtLevelEnum publishes"),
    }
}

/// A domain ARN of this account, as a `ContextId` naming one resource.
fn domain(name: &str) -> String {
    format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/{name}")
}

/// The body of a call about one quota, on the plane `context` names.
fn plane_body(service_code: &str, quota_code: &str, context: Option<&str>) -> Value {
    let mut body = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
    if let Some(context) = context {
        body["ContextId"] = json!(context);
    }
    body
}

/// Dispatch a call that must be refused as an illegal argument, and return its message.
///
/// The modelled `IllegalArgumentException` carries `Message` (its own modelled member);
/// the protocol's generic lowercase `message` is read as the fallback, as
/// `error_envelope.rs` does.
async fn refused_message(caller: &Client, operation: &str, body: Value) -> String {
    let (status, parsed) = caller.call(operation, &body).await;
    assert_eq!(
        status, 400,
        "case {operation} {body}: a plane the quota does not have must be refused with HTTP 400, \
         got {status} with {parsed}"
    );
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    assert!(
        code.contains("IllegalArgumentException"),
        "case {operation} {body}: expected IllegalArgumentException, got {code} with {parsed}"
    );
    parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every catalog row published at a level the seeded catalog carries: a read and a
/// write are served on exactly the planes that level publishes, and refused on the
/// others.
#[tokio::test]
async fn a_call_is_served_on_exactly_the_planes_the_published_level_has() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // (case, ServiceCode, QuotaName, the AdjustableAtLevel the catalog publishes it at).
    let rows = [
        (
            "a quota adjustable at either level",
            "es",
            "Instances per domain",
            LEVEL_ALL,
        ),
        (
            "a quota of the same service adjustable account-wide only",
            "es",
            "Domains per Region",
            LEVEL_ACCOUNT,
        ),
        (
            "a quota of another service adjustable account-wide only",
            "vpc",
            "VPCs per Region",
            LEVEL_ACCOUNT,
        ),
        (
            "a measured quota of a third service adjustable account-wide only",
            "lambda",
            "Concurrent executions",
            LEVEL_ACCOUNT,
        ),
    ];

    for (case, service_code, quota_name, level) in &rows {
        let quota_code = quota_code_for(&caller, service_code, quota_name).await;
        let (account_plane, resource_plane) = planes_of(level);

        // The catalog really does publish this row at the level the row claims: an
        // account-only quota carries no context at all, any other names its level.
        let default = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                plane_body(service_code, &quota_code, None),
            )
            .await["Quota"]
            .clone();
        let published = default["QuotaContext"]["AdjustableAtLevel"].clone();
        let wanted = if resource_plane {
            json!(level)
        } else {
            Value::Null
        };
        assert_eq!(
            published, wanted,
            "case {case}: expected AdjustableAtLevel {wanted}, got {default}"
        );

        // Each plane of the quota, read and written through the two handlers that ask
        // which planes it has.
        let probes = [
            ("no ContextId", None, account_plane),
            ("a resource ContextId", Some(domain("alpha")), resource_plane),
            (
                "the every-resource ContextId",
                Some(CONTEXT_ID_EVERY_RESOURCE.to_string()),
                resource_plane,
            ),
        ];
        for (plane, context, has_plane) in &probes {
            let body = plane_body(service_code, &quota_code, context.as_deref());
            if !has_plane {
                let message = refused_message(&caller, "GetServiceQuota", body.clone()).await;
                assert!(
                    message.contains(&quota_code),
                    "case {case} on {plane}: the refusal must name the quota, got {message:?}"
                );
                let write = refused_message(
                    &caller,
                    "RequestServiceQuotaIncrease",
                    json!({
                        "ServiceCode": service_code,
                        "QuotaCode": quota_code,
                        "DesiredValue": default["Value"].as_f64().unwrap_or_default() + 1.0,
                        "ContextId": context.clone().unwrap_or_default(),
                    }),
                )
                .await;
                assert!(
                    write.contains(&quota_code),
                    "case {case} on {plane}: the write must refuse the same plane and name the \
                     quota, got {write:?}"
                );
                continue;
            }

            let quota = caller.ok("GetServiceQuota", body).await["Quota"].clone();
            let expected_level = if context.is_some() {
                LEVEL_RESOURCE
            } else {
                LEVEL_ACCOUNT
            };
            assert_eq!(
                quota["QuotaAppliedAtLevel"],
                json!(expected_level),
                "case {case} on {plane}: expected QuotaAppliedAtLevel {expected_level}, got \
                 {quota}"
            );
            // The record describes the plane it answers for, and never one the quota
            // does not have: an account-only quota carries no context at all.
            let scope = quota["QuotaContext"]["ContextScope"].clone();
            let wanted_scope = match (resource_plane, context.is_some()) {
                (false, _) => Value::Null,
                (true, true) => json!(CONTEXT_SCOPE_RESOURCE),
                (true, false) => json!(CONTEXT_SCOPE_ACCOUNT),
            };
            assert_eq!(
                scope, wanted_scope,
                "case {case} on {plane}: expected ContextScope {wanted_scope}, got {quota}"
            );
            assert_eq!(
                quota["QuotaContext"]["ContextId"],
                context.clone().map_or(Value::Null, Value::String),
                "case {case} on {plane}: the record must name the context it answers for, got \
                 {quota}"
            );

            // The write path lands on the same plane the read is served on.
            if quota["Adjustable"] == json!(true) {
                let mut write = plane_body(service_code, &quota_code, context.as_deref());
                write["DesiredValue"] =
                    json!(quota["Value"].as_f64().unwrap_or_default() + 1.0);
                let requested = caller.ok("RequestServiceQuotaIncrease", write).await
                    ["RequestedQuota"]
                    .clone();
                assert_eq!(
                    requested["QuotaRequestedAtLevel"],
                    json!(expected_level),
                    "case {case} on {plane}: the request must be recorded at {expected_level}, \
                     got {requested}"
                );
            }
        }
    }
}

/// The account plane's listing reports exactly the records the single read serves on
/// that plane: no listing carries a record a read refuses.
#[tokio::test]
async fn the_account_listing_and_the_single_read_agree_on_the_account_plane() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for service_code in ["es", "vpc", "monitoring"] {
        let listed = walk_pages(
            &caller,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code, "QuotaAppliedAtLevel": LEVEL_ACCOUNT}),
            "Quotas",
            None,
        )
        .await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: the catalog must publish quotas for it, got {listed:?}"
        );
        for record in &listed {
            let quota_code = record["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a record names its quota"))
                .to_string();
            let read = caller
                .ok("GetServiceQuota", plane_body(service_code, &quota_code, None))
                .await["Quota"]
                .clone();
            assert_eq!(
                &read, record,
                "case {service_code} {quota_code}: the account-plane listing and the single read \
                 must answer the same record, got {read}"
            );
            assert_eq!(
                record["QuotaAppliedAtLevel"],
                json!(LEVEL_ACCOUNT),
                "case {service_code} {quota_code}: an account-plane record reports that level, \
                 got {record}"
            );
            // A record on the account plane never says the quota has none.
            assert_ne!(
                record["QuotaContext"]["AdjustableAtLevel"],
                json!(LEVEL_PER_RESOURCE),
                "case {service_code} {quota_code}: a quota adjustable per resource only has no \
                 account-plane record, got {record}"
            );
        }
    }
}

/// The plane rule for EVERY value `AdjustableAtLevelEnum` publishes, the one the seeded
/// catalog does not carry included: the planes a quota has, and the scope the record of
/// each plane describes.
#[tokio::test]
async fn the_published_level_decides_which_planes_a_quota_has() {
    let resource = domain("alpha");

    // (case, the published AdjustableAtLevel).
    let levels = [
        ("adjustable at the account level only", LEVEL_ACCOUNT),
        ("adjustable for an individual resource only", LEVEL_PER_RESOURCE),
        ("adjustable at either level", LEVEL_ALL),
    ];
    for (case, level) in &levels {
        let (account_plane, resource_plane) = planes_of(level);
        let quota = DefaultQuota {
            service_code: "es".to_string(),
            quota_code: "L-EXAMPLE".to_string(),
            quota_name: "Instances per domain".to_string(),
            value: 80.0,
            unit: "None".to_string(),
            adjustable: true,
            global_quota: false,
            adjustable_at_level: (*level).to_string(),
            context_scope_type: resource_plane.then(|| "AWS::Elasticsearch::Domain".to_string()),
            // The scope this quota publishes on its account-wide record: the account,
            // which is what a quota whose value is managed for the account reports (the
            // published-per-resource case is driven over the wire by
            // `recorded_default_catalog_rows.rs`).
            account_record_context_scope: CONTEXT_SCOPE_ACCOUNT.to_string(),
            usage_metric: None,
            period: None,
            description: None,
        };

        assert_eq!(
            quota.has_plane(None),
            account_plane,
            "case {case}: expected the account-wide plane to be {account_plane}, got {}",
            quota.has_plane(None)
        );
        assert_eq!(
            quota.has_plane(Some(&resource)),
            resource_plane,
            "case {case}: expected the per-resource plane to be {resource_plane}, got {}",
            quota.has_plane(Some(&resource))
        );
        assert_eq!(
            quota.has_plane(Some(CONTEXT_ID_EVERY_RESOURCE)),
            resource_plane,
            "case {case}: the every-resource context is the per-resource plane too, got {}",
            quota.has_plane(Some(CONTEXT_ID_EVERY_RESOURCE))
        );

        // The record of each plane describes THAT plane, and a quota with no
        // per-resource plane describes none at all.
        let contexts = [None, Some(resource.as_str())];
        for context in contexts {
            let info = quota.context_info(context);
            let scope = info
                .as_ref()
                .and_then(|info| info.context_scope.as_deref())
                .map(str::to_string);
            let wanted = if !resource_plane {
                None
            } else if context.is_some() || !account_plane {
                Some(CONTEXT_SCOPE_RESOURCE.to_string())
            } else {
                Some(CONTEXT_SCOPE_ACCOUNT.to_string())
            };
            assert_eq!(
                scope, wanted,
                "case {case} with ContextId {context:?}: expected ContextScope {wanted:?}, got \
                 {info:?}"
            );
            assert_eq!(
                info.as_ref()
                    .and_then(|info| info.context_id.as_deref())
                    .map(str::to_string),
                context.filter(|_| resource_plane).map(str::to_string),
                "case {case} with ContextId {context:?}: the context must name the resource asked \
                 for, got {info:?}"
            );
            assert_eq!(
                info.as_ref()
                    .and_then(|info| info.adjustable_at_level.as_deref()),
                resource_plane.then_some(*level),
                "case {case} with ContextId {context:?}: the context must publish the level, got \
                 {info:?}"
            );
        }
    }
}
