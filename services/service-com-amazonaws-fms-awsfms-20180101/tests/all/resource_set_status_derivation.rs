// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ResourceSet$ResourceSetStatus`, the `ResourceSetStatus`
//! the SERVICE derives: "indicates whether the resource set is in or out of an
//! admin's Region scope" — `ACTIVE` while the administrative scope governing the
//! resource set covers the Region it lives in ("the administrator can manage and
//! delete the resource set"), `OUT_OF_ADMIN_SCOPE` otherwise ("the administrator
//! can view the resource set, but they can't edit or delete the resource set").
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.
//!
//! The rules asserted here, each over a TABLE rather than one request:
//!
//! * a `ResourceSetStatus` in the REQUEST is ignored, whichever of the modelled
//!   values it carries, exactly as a request's `Id` and `UpdateToken` are:
//!   `PutResourceSet`, `GetResourceSet` and the `ListResourceSets` summary all
//!   report the derived value instead;
//! * the derived value follows the REGION SCOPE of the administrative scope that
//!   governs the resource set — and only the Region scope, because a resource set
//!   carries no policy type — so it is `ACTIVE` exactly when that scope covers
//!   the Region the resource set was created in, whichever Region the read is
//!   made from;
//! * it is derived on every read, so changing the scope moves a resource set out
//!   of scope and back without the resource set itself being touched.
//!
//! The administrative scopes are the sibling suite's table
//! ([`crate::policy_status_derivation::scope_cases`]: all-enabled and listed
//! forms of the Region scope, and an absent Region scope), reused rather than
//! copied, and the resource-set rows below vary the dimensions the rule
//! generalises over that the scope rows do not: name length, presence and length
//! of `Description`, and how many resource types the set holds.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::RESOURCE_SET_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{call, call_in, emulator_with_administrator, REGION};
use crate::policy_status_derivation::{
    admin_scope, emulator_with_another_default_administrator, put_admin_scope, scope_cases,
    OTHER_REGION,
};

/// One resource set of the class: a name for the assertion messages and the
/// `ResourceSet` document the create sends.
///
/// Member bounds are the model's own (`smithy_query(query="shape",
/// shape="com.amazonaws.fms#ResourceSet")`): `Name` is `com.amazonaws.fms#Name`,
/// 1..128 characters; `Description` is `com.amazonaws.fms#Description`, 0..256.
/// The `ResourceTypeList` entries are resource types the emulator's own
/// `DISCOVERABLE_RESOURCE_TYPES` catalog carries.
struct Case {
    name: &'static str,
    document: fn() -> Value,
}

/// The resource-set rows every rule below is asserted on.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The minimum a resource set can be: a one-character name, one
            // resource type, no description.
            name: "shortest-name-no-description",
            document: || {
                json!({
                    "Name": "a",
                    "ResourceTypeList": ["AWS::EC2::VPC"],
                })
            },
        },
        Case {
            // An empty description is inside the modelled 0..256, and is not the
            // same thing as an absent one.
            name: "empty-description",
            document: || {
                json!({
                    "Name": "empty-description",
                    "Description": "",
                    "ResourceTypeList": ["AWS::NetworkFirewall::Firewall"],
                })
            },
        },
        Case {
            // The upper published bounds, and three resource types.
            name: "longest-name-and-description",
            document: || {
                json!({
                    "Name": "n".repeat(128),
                    "Description": "d".repeat(256),
                    "ResourceTypeList": [
                        "AWS::EC2::Instance",
                        "AWS::EC2::Subnet",
                        "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    ],
                })
            },
        },
    ]
}

/// The `ResourceSet` document of one row, optionally carrying the
/// `ResourceSetStatus` a caller is not entitled to set.
fn document(case: &Case, requested_status: Option<&str>) -> Value {
    let mut document = (case.document)();
    if let Some(status) = requested_status {
        document["ResourceSetStatus"] = json!(status);
    }
    document
}

/// Create one resource set in `region` and answer the `ResourceSet`
/// `PutResourceSet` reports.
async fn put_new(
    router: &MultiServiceRouter,
    case: &str,
    document: &Value,
    region: &str,
) -> Value {
    let (status, body) =
        call_in(router, "PutResourceSet", &json!({ "ResourceSet": document }), region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutResourceSet to answer 200, got {status} with {body}"
    );
    body["ResourceSet"].clone()
}

/// The `ResourceSet` `GetResourceSet` reports for one id, read from `region`.
async fn get_resource_set(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    region: &str,
) -> Value {
    let (status, body) =
        call_in(router, "GetResourceSet", &json!({ "Identifier": id }), region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetResourceSet to answer 200, got {status} with {body}"
    );
    body["ResourceSet"].clone()
}

/// The `ResourceSetSummary` the `ListResourceSets` enumeration carries for one
/// id, paged through until that resource set is found.
async fn summary(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(next) = &token {
            request["NextToken"] = json!(next);
        }
        let (status, body) = call(router, "ListResourceSets", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListResourceSets to answer 200, got {status} with {body}"
        );
        let page = body["ResourceSets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ResourceSets in {body}"))
            .clone();
        if let Some(found) = page
            .iter()
            .find(|summary| summary["Id"].as_str() == Some(id))
        {
            return found.clone();
        }
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => panic!("case {case:?}: no ListResourceSets summary for resource set {id}"),
        }
    }
}

/// Assert that every reader of one resource set reports `want` as its
/// `ResourceSetStatus`: `GetResourceSet` from both Regions the suite uses, and
/// the `ListResourceSets` summary.
async fn assert_every_reader_reports(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    want: &str,
) {
    for reader_region in [REGION, OTHER_REGION] {
        let read = get_resource_set(router, case, id, reader_region).await;
        assert_eq!(
            read["ResourceSetStatus"],
            json!(want),
            "case {case:?}: expected GetResourceSet in {reader_region} to report \
             ResourceSetStatus {want}, got {}",
            read["ResourceSetStatus"]
        );
    }
    let summary = summary(router, case, id).await;
    assert_eq!(
        summary["ResourceSetStatus"],
        json!(want),
        "case {case:?}: expected the ListResourceSets summary to report ResourceSetStatus \
         {want}, got {}",
        summary["ResourceSetStatus"]
    );
}

/// A `ResourceSetStatus` in the REQUEST is ignored: for every modelled value,
/// and for a request that sends none, the resource sets of the organisation's
/// default administrator are reported `ACTIVE` by `PutResourceSet`,
/// `GetResourceSet` and the `ListResourceSets` summary alike.
#[tokio::test]
async fn a_requested_resource_set_status_is_ignored_and_the_derived_one_is_reported() {
    let router = emulator_with_administrator().await;

    // Every modelled ResourceSetStatus a client could send, plus no member at
    // all. The values are the model's own catalog, not a hand-written list.
    let requested: Vec<Option<&str>> = std::iter::once(None)
        .chain(RESOURCE_SET_STATUS_VALUES.iter().copied().map(Some))
        .collect();
    assert!(
        RESOURCE_SET_STATUS_VALUES.contains(&"OUT_OF_ADMIN_SCOPE"),
        "the model's ResourceSetStatus catalog must carry the status a caller must not be able \
         to plant, got {RESOURCE_SET_STATUS_VALUES:?}"
    );

    for case in cases() {
        for requested_status in &requested {
            let row = format!("{}/{requested_status:?}", case.name);
            let sent = document(&case, *requested_status);
            let stored = put_new(&router, &row, &sent, REGION).await;
            let id = stored["Id"].as_str().unwrap_or_default().to_string();

            assert_eq!(
                stored["ResourceSetStatus"],
                json!("ACTIVE"),
                "case {row:?}: PutResourceSet must report the derived ResourceSetStatus ACTIVE, \
                 got {}",
                stored["ResourceSetStatus"]
            );
            assert_every_reader_reports(&router, &row, &id, "ACTIVE").await;

            // The rest of the document still round-trips: only the members the
            // service owns are replaced.
            let read = get_resource_set(&router, &row, &id, REGION).await;
            assert_eq!(
                read["Name"], sent["Name"],
                "case {row:?}: the resource set document must round-trip, got {read}"
            );
            assert_eq!(
                read["ResourceTypeList"], sent["ResourceTypeList"],
                "case {row:?}: the resource set document must round-trip, got {read}"
            );
        }
    }
}

/// The derived status follows the Region scope of the administrative scope
/// governing the resource set: it is `ACTIVE` exactly when that scope covers the
/// Region the resource set was created in, and every reader agrees — whichever
/// Region the read is made from.
#[tokio::test]
async fn the_resource_set_status_follows_the_region_scope_of_the_governing_administrator() {
    for scope_case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope(&router, scope_case.name, &scope_case.scope).await;

        for case in cases() {
            for region in [REGION, OTHER_REGION] {
                let row = format!("{}/{}/{region}", scope_case.name, case.name);
                // Only the Region scope decides a resource set's status, so the
                // policy-type dimension of the shared scope table does not
                // enter: a row whose Region scope covers `region` is ACTIVE even
                // when it manages no policy type at all.
                let want = if (scope_case.manages_region)(region) {
                    "ACTIVE"
                } else {
                    "OUT_OF_ADMIN_SCOPE"
                };
                let sent = document(&case, None);
                let stored = put_new(&router, &row, &sent, region).await;
                let id = stored["Id"].as_str().unwrap_or_default().to_string();

                assert_eq!(
                    stored["ResourceSetStatus"],
                    json!(want),
                    "case {row:?}: expected PutResourceSet to report ResourceSetStatus {want}, \
                     got {}",
                    stored["ResourceSetStatus"]
                );
                assert_every_reader_reports(&router, &row, &id, want).await;
            }
        }
    }
}

/// The status is derived on EVERY read, so narrowing the administrator's scope
/// moves its resource sets out of scope and widening it moves them back —
/// without the resource sets themselves being written at all.
#[tokio::test]
async fn changing_the_administrative_scope_moves_the_resource_sets_in_and_out_of_scope() {
    let router = emulator_with_another_default_administrator().await;

    // Created while the calling account holds no scope of its own, so every
    // resource set starts in the default administrator's full scope.
    let mut sets: Vec<(&'static str, String, Value)> = Vec::new();
    for case in cases() {
        let stored = put_new(&router, case.name, &document(&case, None), REGION).await;
        assert_eq!(
            stored["ResourceSetStatus"],
            json!("ACTIVE"),
            "case {:?}: the default administrator's resource sets are ACTIVE, got {}",
            case.name,
            stored["ResourceSetStatus"]
        );
        sets.push((
            case.name,
            stored["Id"].as_str().unwrap_or_default().to_string(),
            stored,
        ));
    }

    // Narrow to a Region no set was created in, widen to every Region, then
    // narrow again: the same resource sets, read three times.
    let narrow = admin_scope(
        json!({ "AllPolicyTypesEnabled": true }),
        json!({ "Regions": [OTHER_REGION] }),
    );
    let wide = admin_scope(
        json!({ "AllPolicyTypesEnabled": true }),
        json!({ "AllRegionsEnabled": true }),
    );
    for (step, scope) in [
        ("narrowed", &narrow),
        ("widened", &wide),
        ("re-narrowed", &narrow),
    ] {
        put_admin_scope(&router, step, scope).await;
        for (name, id, created) in &sets {
            let row = format!("{step}/{name}");
            let want = if step == "widened" {
                "ACTIVE"
            } else {
                "OUT_OF_ADMIN_SCOPE"
            };
            assert_every_reader_reports(&router, &row, id, want).await;
            // Nothing but the derived status moved: the stored document, its
            // minted identifiers and its LastUpdateTime are as they were.
            let read = get_resource_set(&router, &row, id, REGION).await;
            for member in ["Id", "Name", "ResourceTypeList", "UpdateToken", "LastUpdateTime"] {
                assert_eq!(
                    read[member], created[member],
                    "case {row:?}: member {member} must be untouched by a scope change, got \
                     {read}"
                );
            }
        }
    }
}
