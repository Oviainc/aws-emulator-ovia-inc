// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the semantic validation `PutPolicy` applies to a policy
//! document, over the class of documents each rule ranges over.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`] with the `X-Amz-Target` header and a
//! SigV4 credential scope (the harness in [`crate::policy_lifecycle`]), so the
//! whole deserialize → handle → serialize walk runs and the assertions are about
//! the WIRE response.
//!
//! The rules asserted here are the ones the model states but cannot express as a
//! constraint, so the boundary validator passes a document that breaks them:
//!
//! 1. Every resource type a policy puts in its scope — `ResourceType`, or each
//!    entry of `ResourceTypeList` when `ResourceType` is the sentinel
//!    `ResourceTypeList` — must be one the model lists as valid for the policy's
//!    `SecurityServicePolicyData.Type`; otherwise the call is refused with HTTP
//!    400 `InvalidTypeException`. The sentinel itself needs a non-empty
//!    `ResourceTypeList` behind it, or the policy is scoped to nothing: HTTP 400
//!    `InvalidInputException`.
//! 2. Every id in `ResourceSetIds` must name a resource set that exists;
//!    otherwise the call is refused with HTTP 400 `InvalidInputException`.
//! 3. `IncludeMap` and `ExcludeMap` cannot both scope one policy — "You can
//!    specify inclusions or exclusions, but not both" — otherwise the call is
//!    refused with HTTP 400 `InvalidInputException`.
//!
//! And for all three: a refused call writes NOTHING, on an update as much as on
//! a create.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, emulator, error_code};

/// The `ResourceType` value that says "the scope is the resource types in
/// `ResourceTypeList`", per `Policy$ResourceType`: "To apply this policy to
/// multiple resource types, specify a resource type of `ResourceTypeList` and
/// then specify the resource types in a `ResourceTypeList`."
const RESOURCE_TYPE_LIST: &str = "ResourceTypeList";

/// One row of the resource-type table: a `SecurityServicePolicyData.Type`, the
/// resource types the model lists as valid for it, and resource types that are
/// not.
struct TypeCase {
    security_service_type: &'static str,
    valid: &'static [&'static str],
    invalid: &'static [&'static str],
}

/// The class rule 1 ranges over: every policy type the model enumerates valid
/// resource types for.
///
/// `valid` is the model's own catalog, from the `Policy$ResourceType`
/// documentation ("The following are valid resource types for each Firewall
/// Manager policy type"), read with
///
/// ```text
/// smithy_query(query="doc",shape="com.amazonaws.fms#Policy",member="ResourceType",model_path="services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json")
/// ```
///
/// mapping each policy type the doc names onto its `SecurityServiceType` value:
/// "Amazon Web Services WAF Classic" is `WAF`, "WAF" is `WAFV2`, "Network ACL"
/// is `NETWORK_ACL_COMMON`, and "DNS Firewall, Network Firewall, and third-party
/// firewall" are `DNS_FIREWALL`, `NETWORK_FIREWALL` and `THIRD_PARTY_FIREWALL`.
///
/// Each row's `invalid` entries are mostly resource types the doc lists for a
/// DIFFERENT policy type — so the row cannot pass by accepting any
/// well-known-looking string — plus, once, a type no policy type lists at all.
/// Rows the issue did not cite (`WAF`, `NETWORK_ACL_COMMON`, both security-group
/// audit policies, `DNS_FIREWALL`, `THIRD_PARTY_FIREWALL`) are asserted the same
/// way as the two it did.
const TYPE_CASES: &[TypeCase] = &[
    TypeCase {
        // "Amazon Web Services WAF Classic"
        security_service_type: "WAF",
        valid: &[
            "AWS::ApiGateway::Stage",
            "AWS::CloudFront::Distribution",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
        ],
        invalid: &["AWS::EC2::VPC", "AWS::EC2::Subnet", "AWS::EC2::SecurityGroup"],
    },
    TypeCase {
        // "WAF"
        security_service_type: "WAFV2",
        valid: &[
            "AWS::ApiGateway::Stage",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            "AWS::CloudFront::Distribution",
        ],
        invalid: &[
            "AWS::EC2::VPC",
            // The CLASSIC load balancer: listed for Shield Advanced only.
            "AWS::ElasticLoadBalancing::LoadBalancer",
            "AWS::EC2::Instance",
        ],
    },
    TypeCase {
        security_service_type: "SHIELD_ADVANCED",
        valid: &[
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            "AWS::ElasticLoadBalancing::LoadBalancer",
            "AWS::EC2::EIP",
            "AWS::CloudFront::Distribution",
        ],
        invalid: &[
            "AWS::EC2::Subnet",
            "AWS::ApiGateway::Stage",
            "AWS::EC2::VPC",
        ],
    },
    TypeCase {
        // "Network ACL"
        security_service_type: "NETWORK_ACL_COMMON",
        valid: &["AWS::EC2::Subnet"],
        invalid: &["AWS::EC2::VPC", "AWS::EC2::SecurityGroup"],
    },
    TypeCase {
        // "Security group usage audit"
        security_service_type: "SECURITY_GROUPS_USAGE_AUDIT",
        valid: &["AWS::EC2::SecurityGroup"],
        // Listed for the CONTENT audit policy, not for the usage audit.
        invalid: &["AWS::EC2::NetworkInterface", "AWS::EC2::Instance"],
    },
    TypeCase {
        // "Security group content audit"
        security_service_type: "SECURITY_GROUPS_CONTENT_AUDIT",
        valid: &[
            "AWS::EC2::SecurityGroup",
            "AWS::EC2::NetworkInterface",
            "AWS::EC2::Instance",
        ],
        invalid: &["AWS::EC2::VPC", "AWS::EC2::Subnet"],
    },
    // "DNS Firewall, Network Firewall, and third-party firewall"
    TypeCase {
        security_service_type: "DNS_FIREWALL",
        valid: &["AWS::EC2::VPC"],
        invalid: &["AWS::EC2::Subnet", "AWS::EC2::Instance"],
    },
    TypeCase {
        security_service_type: "NETWORK_FIREWALL",
        valid: &["AWS::EC2::VPC"],
        invalid: &["AWS::EC2::Instance", "AWS::CloudFront::Distribution"],
    },
    TypeCase {
        security_service_type: "THIRD_PARTY_FIREWALL",
        valid: &["AWS::EC2::VPC"],
        // The second is a resource type NO policy type lists.
        invalid: &["AWS::EC2::Subnet", "AWS::Bogus::NotARealType"],
    },
];

/// The `SecurityServiceType` values the `Policy$ResourceType` documentation
/// enumerates NO resource types for. Their resource type is left unconstrained
/// rather than guessed at, so every value the boundary validator accepts stands.
const UNCONSTRAINED_TYPES: &[&str] = &["SECURITY_GROUPS_COMMON", "IMPORT_NETWORK_FIREWALL"];

/// Well-formed `Base62Id`s (the model's `com.amazonaws.fms#Base62Id`: 22
/// characters matching `^[a-z0-9A-Z]{22}$`) that name no resource set. The rows
/// differ in character class: all digits, all lower case, all upper case, mixed.
const UNKNOWN_RESOURCE_SET_IDS: &[&str] = &[
    "0000000000000000000001",
    "abcdefghijklmnopqrstuv",
    "ABCDEFGHIJKLMNOPQRSTUV",
    "aB3dE6gH9jK2mN5pQ8sT1v",
];

/// A policy document: every required `Policy` member, with the scope under test.
fn policy_document(
    name: &str,
    security_service_type: &str,
    resource_type: &str,
    resource_type_list: Option<&[&str]>,
) -> Value {
    let mut policy = json!({
        "PolicyName": name,
        "SecurityServicePolicyData": { "Type": security_service_type },
        "ResourceType": resource_type,
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    });
    if let Some(list) = resource_type_list {
        policy["ResourceTypeList"] = json!(list);
    }
    policy
}

/// Every stored policy's `(PolicyId, PolicyName)`, read through `ListPolicies` —
/// the evidence that a refused call wrote nothing.
async fn stored_policies(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, "ListPolicies", &json!({ "MaxResults": 100 })).await;
    assert_eq!(
        status, 200,
        "case {case}: ListPolicies must answer 200, got {status} with {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case {case}: these suites store fewer policies than one page holds, got {body}"
    );
    body["PolicyList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListPolicies carries no PolicyList: {body}"))
        .iter()
        .map(|summary| {
            (
                summary["PolicyId"].as_str().unwrap_or_default().to_string(),
                summary["PolicyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Rule 1, the accepting side: every resource type the model lists for a policy
/// type is accepted, as the single `ResourceType` and as an entry of a
/// `ResourceTypeList`, and the stored policy reads back through `GetPolicy`.
#[tokio::test]
async fn every_resource_type_the_model_lists_for_a_policy_type_is_accepted() {
    // The table covers the model's whole `SecurityServiceType` catalog: a value
    // is either constrained by a row above or deliberately unconstrained.
    let catalog = service_com_amazonaws_fms_awsfms_20180101::ops::types::SECURITY_SERVICE_TYPE_VALUES;
    for declared in catalog {
        assert!(
            TYPE_CASES
                .iter()
                .any(|row| row.security_service_type == *declared)
                || UNCONSTRAINED_TYPES.contains(declared),
            "case {declared:?}: the model declares a SecurityServiceType this table does not \
             cover; catalog is {catalog:?}"
        );
    }

    // Each row's catalog must be non-empty and free of duplicates, so a
    // transcription slip when the model snapshot is refreshed fails here.
    for row in TYPE_CASES {
        assert!(
            !row.valid.is_empty(),
            "case {:?}: a constrained policy type must list at least one valid resource type",
            row.security_service_type
        );
        for (index, resource_type) in row.valid.iter().enumerate() {
            assert!(
                !row.valid[..index].contains(resource_type),
                "case {:?}: {resource_type:?} is listed twice in {:?}",
                row.security_service_type,
                row.valid
            );
            assert!(
                !row.invalid.contains(resource_type),
                "case {:?}: {resource_type:?} is listed as both valid and invalid",
                row.security_service_type
            );
        }
    }

    let router = emulator();
    for row in TYPE_CASES {
        for resource_type in row.valid {
            let case = format!("{}/{resource_type}", row.security_service_type);
            let document = policy_document(
                "valid-single-resource-type",
                row.security_service_type,
                resource_type,
                None,
            );
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                status, 200,
                "case {case}: a resource type the model lists for this policy type must be \
                 accepted, got {status} with {body}"
            );
            let policy_id = body["Policy"]["PolicyId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: PutPolicy minted no PolicyId: {body}"))
                .to_string();
            let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
            assert_eq!(
                (status, got["Policy"]["ResourceType"].as_str()),
                (200, Some(*resource_type)),
                "case {case}: GetPolicy must report the accepted resource type, got {status} \
                 with {got}"
            );
        }

        // The `ResourceTypeList` form: the sentinel `ResourceType` plus every
        // valid resource type of the row at once.
        let case = format!("{}/ResourceTypeList", row.security_service_type);
        let document = policy_document(
            "valid-resource-type-list",
            row.security_service_type,
            RESOURCE_TYPE_LIST,
            Some(row.valid),
        );
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
        assert_eq!(
            status, 200,
            "case {case}: a ResourceTypeList of resource types the model lists must be \
             accepted, got {status} with {body}"
        );
        assert_eq!(
            body["Policy"]["ResourceTypeList"],
            json!(row.valid),
            "case {case}: the accepted ResourceTypeList must be stored as sent, got {}",
            body["Policy"]["ResourceTypeList"]
        );
    }

    // A policy type the model enumerates no resource types for is not
    // constrained here: any resource type the boundary validator accepted stands.
    for security_service_type in UNCONSTRAINED_TYPES {
        for resource_type in ["AWS::EC2::SecurityGroup", "AWS::EC2::VPC", "AWS::EC2::Subnet"] {
            let case = format!("{security_service_type}/{resource_type}");
            let document = policy_document(
                "unconstrained-policy-type",
                security_service_type,
                resource_type,
                None,
            );
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                status, 200,
                "case {case}: the model lists no resource types for this policy type, so the \
                 call must be answered, got {status} with {body}"
            );
        }
    }
}

/// Rule 1, the refusing side: a resource type the model does not list for the
/// policy type is refused with `InvalidTypeException` — whether it is the single
/// `ResourceType` or one entry of a `ResourceTypeList` — and nothing is stored.
#[tokio::test]
async fn a_resource_type_the_policy_type_does_not_allow_is_refused_and_stores_nothing() {
    let router = emulator();
    for row in TYPE_CASES {
        for resource_type in row.invalid {
            let case = format!("{}/{resource_type}", row.security_service_type);

            // As the single `ResourceType`.
            let document = policy_document(
                "invalid-single-resource-type",
                row.security_service_type,
                resource_type,
                None,
            );
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidTypeException"),
                "case {case}: a resource type the model does not list for this policy type \
                 must be refused, got {status} with {body}"
            );

            // As ONE entry of a `ResourceTypeList` whose other entries are all
            // valid: every entry of the scope is judged, not just the first.
            let mut list: Vec<&str> = row.valid.to_vec();
            list.push(*resource_type);
            let document = policy_document(
                "invalid-resource-type-list-entry",
                row.security_service_type,
                RESOURCE_TYPE_LIST,
                Some(&list),
            );
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidTypeException"),
                "case {case}: a ResourceTypeList entry the model does not list for this policy \
                 type must be refused, got {status} with {body}"
            );

            // The refusal names the resource type and the policy type, so the
            // caller can fix the request from the message alone.
            let message = body
                .get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains(*resource_type) && message.contains(row.security_service_type),
                "case {case}: the refusal must name the resource type and the policy type, \
                 got {message:?}"
            );
        }
    }

    // Not one of those calls created a policy.
    let stored = stored_policies(&router, "after-every-refusal").await;
    assert!(
        stored.is_empty(),
        "every PutPolicy above was refused, so nothing may be stored, got {stored:?}"
    );
}

/// Rule 1, the degenerate scope: a `ResourceType` of the `ResourceTypeList`
/// sentinel puts the scope in `ResourceTypeList`, so a document that sends the
/// sentinel with no `ResourceTypeList`, or with an empty one, scopes the policy
/// to nothing and is refused with `InvalidInputException` — for every policy
/// type, constrained or not.
#[tokio::test]
async fn the_resource_type_list_sentinel_needs_a_resource_type_list_behind_it() {
    let router = emulator();
    let policy_types = TYPE_CASES
        .iter()
        .map(|row| row.security_service_type)
        .chain(UNCONSTRAINED_TYPES.iter().copied());
    for security_service_type in policy_types {
        // The two degenerate forms: the member absent, and an empty list.
        for (case, list) in [
            (
                format!("{security_service_type}/no-resource-type-list"),
                None,
            ),
            (
                format!("{security_service_type}/empty-resource-type-list"),
                Some(&[] as &[&str]),
            ),
        ] {
            let document = policy_document(
                "sentinel-without-a-list",
                security_service_type,
                RESOURCE_TYPE_LIST,
                list,
            );
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidInputException"),
                "case {case}: the sentinel with nothing behind it must be refused, got {status} \
                 with {body}"
            );
        }
    }

    let stored = stored_policies(&router, "after-the-sentinel-refusals").await;
    assert!(
        stored.is_empty(),
        "every PutPolicy above was refused, so nothing may be stored, got {stored:?}"
    );
}

/// Rule 2: `ResourceSetIds` must name resource sets that exist — ids that do are
/// accepted and read back, ids that do not are refused with
/// `InvalidInputException`, and an id stops being acceptable once its set is
/// deleted.
#[tokio::test]
async fn resource_set_ids_must_name_resource_sets_that_exist() {
    let router = emulator();
    // `PutResourceSet` is organisation-scoped, so the administrator gate has to
    // be open before a set can be created.
    associate_admin(&router).await;

    // Three sets, so the rows below can name one, several, or a mixture of
    // known and unknown ids.
    let mut known: Vec<String> = Vec::new();
    for name in ["first-set", "second-set", "third-set"] {
        let (status, body) = call(
            &router,
            "PutResourceSet",
            &json!({ "ResourceSet": { "Name": name, "ResourceTypeList": ["AWS::EC2::VPC"] } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: PutResourceSet must answer 200, got {status} with {body}"
        );
        known.push(
            body["ResourceSet"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name}: PutResourceSet minted no Id: {body}"))
                .to_string(),
        );
    }

    // Accepted: the member absent, one id, and every id, in each case read back.
    let accepted: Vec<(&str, Option<Vec<String>>)> = vec![
        ("no-resource-sets", None),
        ("one-resource-set", Some(vec![known[0].clone()])),
        ("every-resource-set", Some(known.clone())),
    ];
    for (case, ids) in &accepted {
        let mut document = policy_document(
            "accepted-resource-sets",
            "NETWORK_FIREWALL",
            "AWS::EC2::VPC",
            None,
        );
        if let Some(ids) = ids {
            document["ResourceSetIds"] = json!(ids);
        }
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
        assert_eq!(
            status, 200,
            "case {case}: ids that name stored resource sets must be accepted, got {status} \
             with {body}"
        );
        let policy_id = body["Policy"]["PolicyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: PutPolicy minted no PolicyId: {body}"))
            .to_string();
        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        let expected = ids.clone().map(|ids| json!(ids));
        assert_eq!(status, 200, "case {case}: GetPolicy must answer 200, got {got}");
        assert_eq!(
            got["Policy"].get("ResourceSetIds"),
            expected.as_ref(),
            "case {case}: GetPolicy must report the stored ResourceSetIds, got {}",
            got["Policy"]
        );
    }
    let stored_before = stored_policies(&router, "before-the-refusals").await;
    assert_eq!(
        stored_before.len(),
        accepted.len(),
        "the accepted policies must all be stored, got {stored_before:?}"
    );

    // Refused: an unknown id alone, and an unknown id beside known ones.
    for unknown in UNKNOWN_RESOURCE_SET_IDS {
        for (case, ids) in [
            (
                format!("{unknown}/alone"),
                vec![(*unknown).to_string()],
            ),
            (
                format!("{unknown}/beside-known-ids"),
                vec![
                    known[0].clone(),
                    (*unknown).to_string(),
                    known[1].clone(),
                ],
            ),
        ] {
            let mut document = policy_document(
                "unknown-resource-sets",
                "NETWORK_FIREWALL",
                "AWS::EC2::VPC",
                None,
            );
            document["ResourceSetIds"] = json!(ids);
            let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidInputException"),
                "case {case}: a ResourceSetIds entry naming no resource set must be refused, \
                 got {status} with {body}"
            );
            let message = body
                .get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains(*unknown),
                "case {case}: the refusal must name the unknown identifier, got {message:?}"
            );
        }
    }
    let stored_after = stored_policies(&router, "after-the-refusals").await;
    assert_eq!(
        stored_after, stored_before,
        "a refused PutPolicy must store nothing, got {stored_after:?}"
    );

    // The check reads the live store, not a list captured once: deleting a set
    // makes its id unacceptable.
    let deleted = known[2].clone();
    let (status, body) = call(
        &router,
        "DeleteResourceSet",
        &json!({ "Identifier": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case deleted-set: DeleteResourceSet must answer 200, got {status} with {body}"
    );
    let mut document = policy_document(
        "deleted-resource-set",
        "NETWORK_FIREWALL",
        "AWS::EC2::VPC",
        None,
    );
    document["ResourceSetIds"] = json!([deleted]);
    let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "case deleted-set: the id of a deleted resource set must be refused, got {status} \
         with {body}"
    );
}

/// Rule 3: inclusions or exclusions, but not both — a document scoping with both
/// maps is refused with `InvalidInputException` and stores nothing, while either
/// map on its own is accepted, and a map that scopes nothing at all (no entry,
/// or only empty id lists) does not make the other one a conflict.
#[tokio::test]
async fn include_map_and_exclude_map_cannot_both_scope_one_policy() {
    // The two `CustomerPolicyScopeIdType` keys come from the model's own catalog.
    let keys = service_com_amazonaws_fms_awsfms_20180101::ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES;
    assert_eq!(
        keys.to_vec(),
        vec!["ACCOUNT", "ORG_UNIT"],
        "the model's CustomerPolicyScopeIdType catalog changed: {keys:?}"
    );
    let accounts = json!({ "ACCOUNT": ["111111111111", "222222222222"] });
    let org_units = json!({ "ORG_UNIT": ["ou-abc1-12345678"] });
    let both_keys = json!({ "ACCOUNT": ["333333333333"], "ORG_UNIT": ["ou-abc1-87654321"] });
    let empty_map = json!({});
    let empty_lists = json!({ "ACCOUNT": [], "ORG_UNIT": [] });

    // Rows differ in which map scopes the policy, in which keys each map
    // carries, and in whether a present map scopes anything at all.
    let accepted: Vec<(&str, Option<Value>, Option<Value>)> = vec![
        ("no-maps", None, None),
        ("include-accounts", Some(accounts.clone()), None),
        ("include-org-units", Some(org_units.clone()), None),
        ("include-both-keys", Some(both_keys.clone()), None),
        ("exclude-accounts", None, Some(accounts.clone())),
        ("exclude-org-units", None, Some(org_units.clone())),
        ("exclude-both-keys", None, Some(both_keys.clone())),
        (
            "include-accounts-and-an-empty-exclude-map",
            Some(accounts.clone()),
            Some(empty_map.clone()),
        ),
        (
            "exclude-accounts-and-an-empty-include-map",
            Some(empty_map.clone()),
            Some(accounts.clone()),
        ),
        (
            "include-accounts-and-an-exclude-map-of-empty-lists",
            Some(accounts.clone()),
            Some(empty_lists.clone()),
        ),
        (
            "two-maps-that-scope-nothing",
            Some(empty_map.clone()),
            Some(empty_lists.clone()),
        ),
    ];
    let refused: Vec<(&str, Value, Value)> = vec![
        ("accounts-in-both", accounts.clone(), accounts.clone()),
        ("accounts-and-org-units", accounts.clone(), org_units.clone()),
        ("org-units-and-accounts", org_units.clone(), accounts.clone()),
        ("both-keys-in-both", both_keys.clone(), both_keys.clone()),
        (
            "an-empty-key-beside-a-scoping-one",
            json!({ "ACCOUNT": [], "ORG_UNIT": ["ou-abc1-11112222"] }),
            accounts.clone(),
        ),
    ];

    let router = emulator();
    for (case, include, exclude) in &accepted {
        let mut document = policy_document(
            "one-scope-map",
            "WAFV2",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            None,
        );
        if let Some(include) = include {
            document["IncludeMap"] = include.clone();
        }
        if let Some(exclude) = exclude {
            document["ExcludeMap"] = exclude.clone();
        }
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
        assert_eq!(
            status, 200,
            "case {case}: at most one map scopes this policy, so it must be accepted, got \
             {status} with {body}"
        );
        let policy_id = body["Policy"]["PolicyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: PutPolicy minted no PolicyId: {body}"))
            .to_string();
        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(status, 200, "case {case}: GetPolicy must answer 200, got {got}");
        for (member, sent) in [("IncludeMap", include), ("ExcludeMap", exclude)] {
            assert_eq!(
                got["Policy"].get(member),
                sent.as_ref(),
                "case {case}: {member} must be stored as sent, got {:?}",
                got["Policy"].get(member)
            );
        }
    }

    let stored_before = stored_policies(&router, "before-the-refusals").await;
    for (case, include, exclude) in &refused {
        let mut document = policy_document(
            "two-scope-maps",
            "WAFV2",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            None,
        );
        document["IncludeMap"] = include.clone();
        document["ExcludeMap"] = exclude.clone();
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case}: a policy cannot be scoped by both maps, got {status} with {body}"
        );
    }
    let stored_after = stored_policies(&router, "after-the-refusals").await;
    assert_eq!(
        stored_after, stored_before,
        "a refused PutPolicy must store nothing, got {stored_after:?}"
    );
}

/// A refused UPDATE changes nothing either: the stored document and its
/// `PolicyUpdateToken` are what they were, so the token that was current before
/// the refusal still works afterwards.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_policy_exactly_as_it_was() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "PutPolicy",
        &json!({
            "Policy": policy_document(
                "updatable",
                "SHIELD_ADVANCED",
                "AWS::EC2::EIP",
                None,
            ),
            "TagList": [{ "Key": "owner", "Value": "team-a" }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the policy under test must be created, got {status} with {created}"
    );
    let policy_id = created["Policy"]["PolicyId"]
        .as_str()
        .expect("a minted PolicyId")
        .to_string();
    let token = created["Policy"]["PolicyUpdateToken"]
        .as_str()
        .expect("a minted PolicyUpdateToken")
        .to_string();

    // One update per rule, each carrying the CURRENT token so the refusal can
    // only come from the rule under test.
    let mut invalid_type = policy_document(
        "updatable",
        "SHIELD_ADVANCED",
        // Listed for the network ACL policy type, not for Shield Advanced.
        "AWS::EC2::Subnet",
        None,
    );
    let mut unknown_resource_set = policy_document(
        "updatable",
        "SHIELD_ADVANCED",
        "AWS::EC2::EIP",
        None,
    );
    unknown_resource_set["ResourceSetIds"] = json!([UNKNOWN_RESOURCE_SET_IDS[0]]);
    let mut both_maps = policy_document("updatable", "SHIELD_ADVANCED", "AWS::EC2::EIP", None);
    both_maps["IncludeMap"] = json!({ "ACCOUNT": ["111111111111"] });
    both_maps["ExcludeMap"] = json!({ "ACCOUNT": ["222222222222"] });

    for (case, expected_code, document) in [
        ("invalid-resource-type", "InvalidTypeException", &mut invalid_type),
        (
            "unknown-resource-set",
            "InvalidInputException",
            &mut unknown_resource_set,
        ),
        ("both-scope-maps", "InvalidInputException", &mut both_maps),
    ] {
        document["PolicyId"] = json!(policy_id);
        document["PolicyUpdateToken"] = json!(token);
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": document })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case}: the update must be refused, got {status} with {body}"
        );

        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            (status, &got["Policy"]),
            (200, &created["Policy"]),
            "case {case}: the refused update must leave the stored policy unchanged, got \
             {status} with {got}"
        );
        assert_eq!(
            got["Policy"]["PolicyUpdateToken"].as_str(),
            Some(token.as_str()),
            "case {case}: a refused update must not rotate the PolicyUpdateToken, got {}",
            got["Policy"]["PolicyUpdateToken"]
        );
    }

    // The token the refusals left current still authorizes a valid update.
    let mut valid = policy_document("updatable", "SHIELD_ADVANCED", "AWS::EC2::EIP", None);
    valid["PolicyId"] = json!(policy_id);
    valid["PolicyUpdateToken"] = json!(token);
    valid["PolicyDescription"] = json!("updated after the refusals");
    let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": valid })).await;
    assert_eq!(
        status, 200,
        "the token the refusals left current must still authorize an update, got {status} \
         with {body}"
    );
}
