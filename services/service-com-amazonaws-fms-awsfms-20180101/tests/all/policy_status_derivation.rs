// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Policy$PolicyStatus`, the `CustomerPolicyStatus` the
//! SERVICE derives: "indicates whether the policy is in or out of an admin's
//! policy or Region scope" — `ACTIVE` while the administrative scope governing
//! the policy covers it ("the administrator can manage and delete the policy"),
//! `OUT_OF_ADMIN_SCOPE` otherwise ("the administrator can view the policy, but
//! they can't edit or delete the policy").
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.
//!
//! The rules asserted here, each over a TABLE rather than one request:
//!
//! * a `PolicyStatus` in the REQUEST is ignored, whichever of the modelled
//!   values it carries, exactly as a request's `PolicyId` and
//!   `PolicyUpdateToken` are: `PutPolicy`, `GetPolicy` and the `ListPolicies`
//!   summary all report the derived value instead;
//! * the derived value follows the administrative scope that governs the
//!   policy — the policy types and the Regions it covers — so it is `ACTIVE`
//!   exactly when that scope covers both the policy's type and the Region the
//!   policy was created in;
//! * it is derived on every read, so changing the scope moves a policy out of
//!   scope and back without the policy itself being touched, and a reader in
//!   one Region reports the same status as a reader in another.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::CUSTOMER_POLICY_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{
    call, call_in, emulator, emulator_with_administrator, ACCOUNT, REGION,
};

/// A second Region, so the Region a policy was created in is a dimension of the
/// table rather than a constant, and so a read can be made from a Region other
/// than the one the policy belongs to.
const OTHER_REGION: &str = "eu-central-1";

/// The account id associated as the organisation's DEFAULT administrator in the
/// scope tests. Deliberately NOT the calling account ([`ACCOUNT`]): the default
/// administrator's scope is the whole organisation and `PutAdminAccount`
/// refuses to configure a scope for it, so a test that needs the caller to hold
/// a NARROWED scope has to leave that role to another account.
const DEFAULT_ADMIN: &str = "111111111111";

/// One policy of the class: its `SecurityServicePolicyData.Type` and a
/// `ResourceType` that is valid for that type.
///
/// The pairs come from the model's own `Policy$ResourceType` documentation
/// ("The following are valid resource types for each Firewall Manager policy
/// type"), read with
///
/// ```text
/// smithy_query(query="doc",shape="com.amazonaws.fms#Policy",member="ResourceType",model_path="services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json")
/// ```
///
/// and the types are members of `SecurityServiceType`, asserted below against
/// the model's catalog — including `SECURITY_GROUPS_USAGE_AUDIT` and
/// `THIRD_PARTY_FIREWALL`, which the issue did not cite.
const POLICY_TYPES: &[(&str, &str)] = &[
    ("WAFV2", "AWS::ElasticLoadBalancingV2::LoadBalancer"),
    ("SHIELD_ADVANCED", "AWS::EC2::EIP"),
    ("NETWORK_FIREWALL", "AWS::EC2::VPC"),
    ("THIRD_PARTY_FIREWALL", "AWS::EC2::VPC"),
    ("SECURITY_GROUPS_USAGE_AUDIT", "AWS::EC2::SecurityGroup"),
];

/// The `SecurityServiceType` of a third-party firewall policy, which
/// `PolicyTypeScope$AllPolicyTypesEnabled` excepts: "third-party policy types
/// can only be managed by the Firewall Manager default administrator".
const THIRD_PARTY_FIREWALL: &str = "THIRD_PARTY_FIREWALL";

/// A minimal valid policy document of one type, optionally carrying the
/// `PolicyStatus` a caller is not entitled to set.
fn document(
    security_service_type: &str,
    resource_type: &str,
    requested_status: Option<&str>,
) -> Value {
    let mut policy = json!({
        "PolicyName": format!("{security_service_type}-policy"),
        "SecurityServicePolicyData": { "Type": security_service_type },
        "ResourceType": resource_type,
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    });
    if let Some(status) = requested_status {
        policy["PolicyStatus"] = json!(status);
    }
    policy
}

/// Create one policy in `region` and answer the `Policy` `PutPolicy` reports.
async fn put_new(
    router: &MultiServiceRouter,
    case: &str,
    document: &Value,
    region: &str,
) -> Value {
    let (status, body) = call_in(router, "PutPolicy", &json!({ "Policy": document }), region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"].clone()
}

/// The `PolicyStatus` `GetPolicy` reports for one policy, read from `region`.
async fn get_policy_status(
    router: &MultiServiceRouter,
    case: &str,
    policy_id: &str,
    region: &str,
) -> Value {
    let (status, body) =
        call_in(router, "GetPolicy", &json!({ "PolicyId": policy_id }), region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"]["PolicyStatus"].clone()
}

/// The `PolicyStatus` the `ListPolicies` summary of one policy carries, paged
/// through until that policy is found.
async fn summary_policy_status(
    router: &MultiServiceRouter,
    case: &str,
    policy_id: &str,
) -> Value {
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(next) = &token {
            request["NextToken"] = json!(next);
        }
        let (status, body) = call(router, "ListPolicies", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListPolicies to answer 200, got {status} with {body}"
        );
        let page = body["PolicyList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no PolicyList in {body}"))
            .clone();
        if let Some(summary) = page
            .iter()
            .find(|summary| summary["PolicyId"].as_str() == Some(policy_id))
        {
            return summary["PolicyStatus"].clone();
        }
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => panic!("case {case:?}: no ListPolicies summary for policy {policy_id}"),
        }
    }
}

/// Onboard the CALLING account as a delegated administrator holding `scope`,
/// which is the only way the scope governing its policies is narrowed.
async fn put_admin_scope(router: &MultiServiceRouter, case: &str, scope: &Value) {
    let (status, body) = call(
        router,
        "PutAdminAccount",
        &json!({ "AdminAccount": ACCOUNT, "AdminScope": scope }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutAdminAccount to answer 200, got {status} with {body}"
    );
}

/// An emulator whose DEFAULT administrator is another account, so the calling
/// account can hold a narrowed administrative scope of its own.
async fn emulator_with_another_default_administrator() -> MultiServiceRouter {
    let router = emulator();
    let (status, body) = call(
        &router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": DEFAULT_ADMIN }),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateAdminAccount must answer 200, got {status} with {body}"
    );
    router
}

/// An `AdminScope` with every member the service validates present: the account
/// scope a delegated administrator needs, plus the policy-type and Region
/// scopes under test.
fn admin_scope(policy_type_scope: Value, region_scope: Value) -> Value {
    json!({
        "AccountScope": { "AllAccountsEnabled": true },
        "PolicyTypeScope": policy_type_scope,
        "RegionScope": region_scope,
    })
}

/// One row of the scope table: the scope the calling account holds, and which
/// policy types and Regions that scope covers.
struct ScopeCase {
    name: &'static str,
    scope: Value,
    manages_policy_type: fn(&str) -> bool,
    manages_region: fn(&str) -> bool,
}

/// The administrative scopes the rule is asserted over: all-enabled and listed
/// forms of both the policy-type and the Region scope, and each of them absent.
fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase {
            name: "all-policy-types-and-all-regions",
            scope: admin_scope(
                json!({ "AllPolicyTypesEnabled": true }),
                json!({ "AllRegionsEnabled": true }),
            ),
            // "Allows the specified Firewall Manager administrator to manage all
            // Firewall Manager policy types, except for third-party policy
            // types" (`PolicyTypeScope$AllPolicyTypesEnabled`).
            manages_policy_type: |policy_type| policy_type != THIRD_PARTY_FIREWALL,
            manages_region: |_| true,
        },
        ScopeCase {
            name: "two-listed-policy-types-and-all-regions",
            scope: admin_scope(
                json!({ "PolicyTypes": ["WAFV2", "NETWORK_FIREWALL"] }),
                json!({ "AllRegionsEnabled": true }),
            ),
            manages_policy_type: |policy_type| {
                policy_type == "WAFV2" || policy_type == "NETWORK_FIREWALL"
            },
            manages_region: |_| true,
        },
        ScopeCase {
            name: "all-policy-types-and-one-listed-region",
            scope: admin_scope(
                json!({ "AllPolicyTypesEnabled": true }),
                json!({ "Regions": [REGION] }),
            ),
            manages_policy_type: |policy_type| policy_type != THIRD_PARTY_FIREWALL,
            manages_region: |region| region == REGION,
        },
        ScopeCase {
            name: "one-listed-policy-type-and-the-other-region",
            scope: admin_scope(
                json!({ "PolicyTypes": ["SHIELD_ADVANCED"] }),
                json!({ "Regions": [OTHER_REGION] }),
            ),
            manages_policy_type: |policy_type| policy_type == "SHIELD_ADVANCED",
            manages_region: |region| region == OTHER_REGION,
        },
        ScopeCase {
            name: "no-policy-type-scope",
            scope: json!({
                "AccountScope": { "AllAccountsEnabled": true },
                "RegionScope": { "AllRegionsEnabled": true },
            }),
            // A scope that configures no policy type has been granted none.
            manages_policy_type: |_| false,
            manages_region: |_| true,
        },
        ScopeCase {
            name: "no-region-scope",
            scope: json!({
                "AccountScope": { "AllAccountsEnabled": true },
                "PolicyTypeScope": { "AllPolicyTypesEnabled": true },
            }),
            manages_policy_type: |policy_type| policy_type != THIRD_PARTY_FIREWALL,
            manages_region: |_| false,
        },
    ]
}

/// The status the rule says a policy of `security_service_type` created in
/// `region` has under one scope row.
fn expected_status(case: &ScopeCase, security_service_type: &str, region: &str) -> &'static str {
    if (case.manages_policy_type)(security_service_type) && (case.manages_region)(region) {
        "ACTIVE"
    } else {
        "OUT_OF_ADMIN_SCOPE"
    }
}

/// A `PolicyStatus` in the REQUEST is ignored: for every modelled value, and
/// for a request that sends none, the policies of the organisation's default
/// administrator are reported `ACTIVE` by `PutPolicy`, `GetPolicy` and the
/// `ListPolicies` summary alike.
#[tokio::test]
async fn a_requested_policy_status_is_ignored_and_the_derived_one_is_reported() {
    let router = emulator_with_administrator().await;
    let catalog = service_com_amazonaws_fms_awsfms_20180101::ops::types::SECURITY_SERVICE_TYPE_VALUES;

    // Every modelled CustomerPolicyStatus a client could send, plus no member
    // at all. The values are the model's own catalog, not a hand-written list.
    let requested: Vec<Option<&str>> = std::iter::once(None)
        .chain(CUSTOMER_POLICY_STATUS_VALUES.iter().copied().map(Some))
        .collect();
    assert!(
        CUSTOMER_POLICY_STATUS_VALUES.contains(&"OUT_OF_ADMIN_SCOPE"),
        "the model's CustomerPolicyStatus catalog must carry the status a caller must not be \
         able to plant, got {CUSTOMER_POLICY_STATUS_VALUES:?}"
    );

    for (security_service_type, resource_type) in POLICY_TYPES {
        assert!(
            catalog.contains(security_service_type),
            "{security_service_type:?} is not in the model's SecurityServiceType catalog \
             {catalog:?}"
        );
        for requested_status in &requested {
            let case = format!("{security_service_type}/{requested_status:?}");
            let document = document(security_service_type, resource_type, *requested_status);
            let stored = put_new(&router, &case, &document, REGION).await;
            let policy_id = stored["PolicyId"].as_str().unwrap_or_default().to_string();

            assert_eq!(
                stored["PolicyStatus"],
                json!("ACTIVE"),
                "case {case:?}: PutPolicy must report the derived PolicyStatus ACTIVE, got {}",
                stored["PolicyStatus"]
            );
            let read = get_policy_status(&router, &case, &policy_id, REGION).await;
            assert_eq!(
                read,
                json!("ACTIVE"),
                "case {case:?}: GetPolicy must report the derived PolicyStatus ACTIVE, got {read}"
            );
            let summary = summary_policy_status(&router, &case, &policy_id).await;
            assert_eq!(
                summary,
                json!("ACTIVE"),
                "case {case:?}: the ListPolicies summary must report the derived PolicyStatus \
                 ACTIVE, got {summary}"
            );

            // The rest of the document still round-trips: only the member the
            // service owns is replaced.
            let (status, body) =
                call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetPolicy to answer 200, got {status} with {body}"
            );
            assert_eq!(
                body["Policy"]["PolicyName"], document["PolicyName"],
                "case {case:?}: the policy document must round-trip, got {}",
                body["Policy"]
            );
        }
    }
}

/// The derived status follows the administrative scope governing the policy: it
/// is `ACTIVE` exactly when that scope covers both the policy's type and the
/// Region the policy was created in, and every reader agrees — whichever Region
/// the read is made from.
#[tokio::test]
async fn the_policy_status_follows_the_scope_of_the_administrator_that_governs_it() {
    for case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope(&router, case.name, &case.scope).await;

        for (security_service_type, resource_type) in POLICY_TYPES {
            for region in [REGION, OTHER_REGION] {
                let row = format!("{}/{security_service_type}/{region}", case.name);
                let want = expected_status(&case, security_service_type, region);
                let document = document(security_service_type, resource_type, None);
                let stored = put_new(&router, &row, &document, region).await;
                let policy_id = stored["PolicyId"].as_str().unwrap_or_default().to_string();

                assert_eq!(
                    stored["PolicyStatus"],
                    json!(want),
                    "case {row:?}: expected PutPolicy to report PolicyStatus {want}, got {}",
                    stored["PolicyStatus"]
                );
                // The policy's own Region decides, not the reader's: both reads
                // report the same status.
                for reader_region in [REGION, OTHER_REGION] {
                    let read =
                        get_policy_status(&router, &row, &policy_id, reader_region).await;
                    assert_eq!(
                        read,
                        json!(want),
                        "case {row:?}: expected GetPolicy in {reader_region} to report \
                         PolicyStatus {want}, got {read}"
                    );
                }
                let summary = summary_policy_status(&router, &row, &policy_id).await;
                assert_eq!(
                    summary,
                    json!(want),
                    "case {row:?}: expected the ListPolicies summary to report PolicyStatus \
                     {want}, got {summary}"
                );
            }
        }
    }
}

/// The status is derived on EVERY read, so narrowing the administrator's scope
/// moves its policies out of scope and widening it moves them back — without
/// the policies themselves being written at all.
#[tokio::test]
async fn changing_the_administrative_scope_moves_the_policies_in_and_out_of_scope() {
    let router = emulator_with_another_default_administrator().await;

    // Created while the calling account holds no scope of its own, so every
    // policy starts in scope.
    let mut policies: Vec<(&str, String)> = Vec::new();
    for (security_service_type, resource_type) in POLICY_TYPES {
        let document = document(security_service_type, resource_type, None);
        let stored = put_new(&router, security_service_type, &document, REGION).await;
        policies.push((
            security_service_type,
            stored["PolicyId"].as_str().unwrap_or_default().to_string(),
        ));
        assert_eq!(
            stored["PolicyStatus"],
            json!("ACTIVE"),
            "case {security_service_type:?}: the default administrator's policies are ACTIVE, \
             got {}",
            stored["PolicyStatus"]
        );
    }

    // Narrow, widen, then narrow again: the same policies, read three times.
    let narrow = admin_scope(
        json!({ "PolicyTypes": ["WAFV2"] }),
        json!({ "Regions": [OTHER_REGION] }),
    );
    let wide = admin_scope(
        json!({ "AllPolicyTypesEnabled": true }),
        json!({ "AllRegionsEnabled": true }),
    );
    for (step, scope) in [("narrowed", &narrow), ("widened", &wide), ("re-narrowed", &narrow)] {
        put_admin_scope(&router, step, scope).await;
        for (security_service_type, policy_id) in &policies {
            let row = format!("{step}/{security_service_type}");
            // Under `narrow` the scope covers only WAFV2 and only the other
            // Region, so no policy created in REGION is in it; under `wide` all
            // of them are, except the third-party type the documented exception
            // reserves for the default administrator.
            let want = if step == "widened" && *security_service_type != THIRD_PARTY_FIREWALL {
                "ACTIVE"
            } else {
                "OUT_OF_ADMIN_SCOPE"
            };
            let read = get_policy_status(&router, &row, policy_id, REGION).await;
            assert_eq!(
                read,
                json!(want),
                "case {row:?}: expected GetPolicy to report PolicyStatus {want}, got {read}"
            );
            let summary = summary_policy_status(&router, &row, policy_id).await;
            assert_eq!(
                summary,
                json!(want),
                "case {row:?}: expected the ListPolicies summary to report PolicyStatus {want}, \
                 got {summary}"
            );
        }
    }
}
