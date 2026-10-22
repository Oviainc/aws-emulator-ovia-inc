// SPDX-License-Identifier: CC0-1.0

//! `CreateEnvironment`'s request-level preconditions, over the wire.
//!
//! The rule under test: a `CreateEnvironment` request that fails any precondition the
//! service evaluates is refused with ONE HTTP 400 `ValidationException` that reports
//! EVERY failed precondition — one `fieldList` entry per failure, naming the input
//! member, and a top-level `message` that joins those entries — and the refused
//! request creates nothing.
//!
//! The preconditions themselves come from the model's own documentation
//! (`smithy_query(query="operation",operation="CreateEnvironment")` and the member
//! docs it quotes): `termsAccepted` is the caller's confirmation that it holds the VCF
//! licenses, `SELF_DEPLOYED` supports none of `licenseInfo`, `hosts`, `vcfHostnames`,
//! `siteId`, `connectivityInfo`, `dedicatedHostId` and `placementGroupId` cannot be
//! used together, a VLAN subnet block is between /24 and /28 and overlaps no other,
//! `hcxNetworkAclId` is required for a public HCX VLAN, and an environment name is
//! unique per Region.
//!
//! Each row below breaks a DIFFERENT number and mix of those preconditions, from one
//! to six at a time, so what is asserted is the aggregation rule and not one recorded
//! request.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice::state::{
    VALIDATION_FAILURE_PREFIX, VALIDATION_FAILURE_SEPARATOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{call, create_request, emulator, error_type, vlans_with_prefix};

/// The `ValidationExceptionReason` a field-level refusal carries (the model's enum
/// values are `unknownOperation`, `cannotParse`, `fieldValidationFailed`, `other`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// Four hosts, as `hosts` requires (len 4..4), with `extra` merged into the host at
/// each index in `at`.
fn hosts_with(extra: &Value, at: &[usize]) -> Value {
    let mut hosts: Vec<Value> = (1..=4)
        .map(|index| {
            json!({
                "hostName": format!("esx-{index}"),
                "keyName": "evs-key",
                "instanceType": "i4i.metal",
            })
        })
        .collect();
    for index in at {
        for (key, value) in extra.as_object().expect("the extra members are an object") {
            hosts[*index][key] = value.clone();
        }
    }
    Value::Array(hosts)
}

/// How many environments the service holds, so a refusal can be shown to have
/// created nothing.
async fn environment_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "ListEnvironments", &json!({})).await;
    assert_eq!(status, 200, "list: expected 200, got {status} {body}");
    body["environmentSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("list: no environmentSummaries in {body}"))
        .len()
}

/// Assert the aggregated-refusal rule on one response: HTTP 400, a
/// `ValidationException`, one `fieldList` entry per expected member, and a top-level
/// `message` that joins exactly those entries' text.
fn assert_aggregated_refusal(case: &str, status: u16, body: &Value, expected_members: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert!(
        error_type(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {}",
        error_type(body)
    );
    assert_eq!(
        body["reason"],
        json!(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {}",
        body["reason"]
    );

    let entries = body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"));
    let names: BTreeSet<&str> = entries
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    let expected: BTreeSet<&str> = expected_members.iter().copied().collect();
    assert_eq!(
        names, expected,
        "case {case:?}: expected one fieldList entry per failed precondition \
         ({expected:?}), got {names:?} in {body}"
    );
    assert_eq!(
        entries.len(),
        expected.len(),
        "case {case:?}: expected {} fieldList entries, got {} in {body}",
        expected.len(),
        entries.len()
    );

    // The top-level message is those same entries, joined: a client that reads only
    // `message` learns every failure a client that reads `fieldList` learns.
    let texts: Vec<&str> = entries
        .iter()
        .map(|entry| {
            entry["message"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a fieldList entry with no message: {entry}")
            })
        })
        .collect();
    for (entry, text) in entries.iter().zip(&texts) {
        let name = entry["name"].as_str().unwrap_or_default();
        assert!(
            text.contains(name),
            "case {case:?}: the entry for {name} must name the member it refuses, got {text:?}"
        );
    }
    let joined = format!(
        "{VALIDATION_FAILURE_PREFIX}{}",
        texts.join(VALIDATION_FAILURE_SEPARATOR)
    );
    assert_eq!(
        body["message"],
        json!(joined),
        "case {case:?}: expected the message to join every fieldList entry ({joined:?}), got {}",
        body["message"]
    );
}

#[tokio::test]
async fn a_create_that_fails_preconditions_is_refused_once_naming_every_failure() {
    let license = json!([{
        "solutionKey": "AAAAA-BBBBB-CCCCC-DDDDD-EEEEE",
        "vsanKey": "FFFFF-GGGGG-HHHHH-IIIII-JJJJJ",
    }]);
    let hostnames = json!({
        "vCenter": "vcenter", "nsx": "nsx", "nsxManager1": "nsxm1", "nsxManager2": "nsxm2",
        "nsxManager3": "nsxm3", "nsxEdge1": "nsxe1", "nsxEdge2": "nsxe2",
        "sddcManager": "sddc", "cloudBuilder": "builder",
    });
    let connectivity = json!({ "privateRouteServerPeerings": ["rsp-1", "rsp-2"] });
    let both_placements = json!({
        "dedicatedHostId": "h-0123456789abcdef0",
        "placementGroupId": "pg-0123456789abcdef0",
    });

    // One VLAN subnet larger than the /24 maximum and one smaller than the /28
    // minimum, in blocks that overlap nothing else.
    let mut two_bad_sizes = vlans_with_prefix(28);
    two_bad_sizes["vmkManagement"] = json!({ "cidr": "10.1.0.0/23" });
    two_bad_sizes["vmManagement"] = json!({ "cidr": "10.2.0.0/29" });

    // A public HCX VLAN with no network ACL id, whose block is also vTep's.
    let mut public_overlapping_hcx = vlans_with_prefix(28);
    public_overlapping_hcx["isHcxPublic"] = json!(true);
    public_overlapping_hcx["hcx"] = json!({ "cidr": "10.0.8.0/28" });

    // Each row: the case, what it overrides in an otherwise valid request, and the
    // input members the refusal must name — none for a row that is accepted.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an otherwise valid request is accepted",
            json!({}),
            vec![],
        ),
        (
            "terms not accepted is the only failure",
            json!({ "termsAccepted": false }),
            vec!["termsAccepted"],
        ),
        (
            "terms not accepted together with an unsupported install parameter",
            json!({
                "termsAccepted": false,
                "vcfVersion": "SELF_DEPLOYED",
                "siteId": "site-1",
            }),
            vec!["termsAccepted", "siteId"],
        ),
        (
            "every parameter SELF_DEPLOYED refuses, and terms not accepted",
            json!({
                "termsAccepted": false,
                "vcfVersion": "SELF_DEPLOYED",
                "siteId": "site-1",
                "licenseInfo": license,
                "vcfHostnames": hostnames,
                "connectivityInfo": connectivity,
                "hosts": hosts_with(&json!({}), &[]),
            }),
            vec![
                "termsAccepted",
                "siteId",
                "licenseInfo",
                "vcfHostnames",
                "connectivityInfo",
                "hosts",
            ],
        ),
        (
            "two of four hosts name both placement mechanisms",
            json!({ "hosts": hosts_with(&both_placements, &[1, 3]) }),
            vec!["hosts[1]", "hosts[3]"],
        ),
        (
            "one VLAN subnet too large, one too small, and terms not accepted",
            json!({ "termsAccepted": false, "initialVlans": two_bad_sizes }),
            vec![
                "termsAccepted",
                "initialVlans.vmkManagement",
                "initialVlans.vmManagement",
            ],
        ),
        (
            "a public HCX VLAN with no ACL id, overlapping another VLAN subnet",
            json!({ "initialVlans": public_overlapping_hcx }),
            vec!["initialVlans.hcxNetworkAclId", "initialVlans.hcx"],
        ),
        (
            "a /29 VLAN subnet is one past the /28 minimum",
            json!({ "initialVlans": vlans_with_prefix(29) }),
            // Every one of the ten members is too small, so every one is reported.
            vec![
                "initialVlans.vmkManagement",
                "initialVlans.vmManagement",
                "initialVlans.vMotion",
                "initialVlans.vSan",
                "initialVlans.vTep",
                "initialVlans.edgeVTep",
                "initialVlans.nsxUplink",
                "initialVlans.hcx",
                "initialVlans.expansionVlan1",
                "initialVlans.expansionVlan2",
            ],
        ),
        (
            "a /24 VLAN subnet is the published maximum and is accepted",
            // /24 blocks at 10.0.{0,2,..}.0 would overlap, so each gets its own /24.
            json!({ "initialVlans": {
                "vmkManagement": { "cidr": "10.0.0.0/24" },
                "vmManagement": { "cidr": "10.0.1.0/24" },
                "vMotion": { "cidr": "10.0.2.0/24" },
                "vSan": { "cidr": "10.0.3.0/24" },
                "vTep": { "cidr": "10.0.4.0/24" },
                "edgeVTep": { "cidr": "10.0.5.0/24" },
                "nsxUplink": { "cidr": "10.0.6.0/24" },
                "hcx": { "cidr": "10.0.7.0/24" },
                "expansionVlan1": { "cidr": "10.0.8.0/24" },
                "expansionVlan2": { "cidr": "10.0.9.0/24" },
            } }),
            vec![],
        ),
        (
            "a /28 VLAN subnet is the published minimum and is accepted",
            json!({ "initialVlans": vlans_with_prefix(28) }),
            vec![],
        ),
    ];

    for (case, overrides, expected_members) in rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let mut request = create_request(None);
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            request[member] = value.clone();
        }

        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                environment_count(&router).await,
                1,
                "case {case:?}: an accepted create must persist exactly one environment"
            );
            continue;
        }

        assert_aggregated_refusal(case, status, &body, &expected_members);
        assert_eq!(
            environment_count(&router).await,
            0,
            "case {case:?}: a refused create must not persist an environment"
        );
    }
}

#[tokio::test]
async fn a_refusal_aggregates_the_state_scoped_precondition_too() {
    let router = emulator();
    let (status, body) = call(&router, "CreateEnvironment", &create_request(Some("taken"))).await;
    assert_eq!(status, 200, "first create: expected 200, got {status} {body}");

    // The name is taken AND the terms are not accepted: both are reported at once,
    // even though only one of them can be decided from the request alone.
    let mut request = create_request(Some("taken"));
    request["termsAccepted"] = json!(false);
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_aggregated_refusal(
        "a taken name and unaccepted terms",
        status,
        &body,
        &["environmentName", "termsAccepted"],
    );
    assert_eq!(
        environment_count(&router).await,
        1,
        "a refused create must leave the store as it was"
    );
}

#[tokio::test]
async fn a_vpc_carries_one_deployed_environment_at_a_time() {
    // "The VPC cannot be used with any other deployed Amazon EVS environment. Amazon
    // EVS does not provide multi-VPC support for environments at this time."
    // (`vpcId` doc.)
    let router = emulator();
    let first = create_request(Some("vpc-owner"));
    let vpc = first["vpcId"].clone();
    let (status, body) = call(&router, "CreateEnvironment", &first).await;
    assert_eq!(status, 200, "first create: expected 200, got {status} {body}");
    let owner = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("first create: no environmentId in {body}"))
        .to_string();

    // A second environment in that VPC is refused — and when the same request also
    // leaves the terms unaccepted, BOTH failures are reported at once.
    let mut second = create_request(Some("vpc-squatter"));
    second["vpcId"] = vpc.clone();
    let (status, body) = call(&router, "CreateEnvironment", &second).await;
    assert_aggregated_refusal(
        "a VPC already carrying an environment",
        status,
        &body,
        &["vpcId"],
    );

    let mut third = create_request(Some("vpc-squatter"));
    third["vpcId"] = vpc.clone();
    third["termsAccepted"] = json!(false);
    let (status, body) = call(&router, "CreateEnvironment", &third).await;
    assert_aggregated_refusal(
        "a taken VPC and unaccepted terms",
        status,
        &body,
        &["vpcId", "termsAccepted"],
    );
    assert_eq!(
        environment_count(&router).await,
        1,
        "the refused creates must leave the store as it was"
    );

    // An environment on its way out still holds its VPC; once its deletion has come
    // due the VPC is free again.
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": owner }),
    )
    .await;
    assert_eq!(status, 200, "delete: expected 200, got {status} {body}");
    let mut while_deleting = create_request(Some("vpc-next-while-deleting"));
    while_deleting["vpcId"] = vpc.clone();
    let (status, body) = call(&router, "CreateEnvironment", &while_deleting).await;
    assert_aggregated_refusal(
        "a VPC whose environment is still DELETING",
        status,
        &body,
        &["vpcId"],
    );

    // Long enough for the deletion this service owes to come due (its span is under a
    // second by construction).
    std::thread::sleep(std::time::Duration::from_millis(900));
    let mut reuse = create_request(Some("vpc-next"));
    reuse["vpcId"] = vpc.clone();
    let (status, body) = call(&router, "CreateEnvironment", &reuse).await;
    assert_eq!(
        status, 200,
        "reuse after the deletion came due: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["environment"]["vpcId"], vpc,
        "reuse: expected the environment to report the VPC the request named, got {}",
        body["environment"]["vpcId"]
    );
}

#[tokio::test]
async fn accepting_the_terms_is_what_lets_the_same_request_through() {
    let router = emulator();

    let mut request = create_request(Some("terms-env"));
    request["termsAccepted"] = json!(false);
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_aggregated_refusal("terms not accepted", status, &body, &["termsAccepted"]);

    // The same request with the terms accepted is created, and the stored record
    // reports the acceptance when it is read back.
    request["termsAccepted"] = json!(true);
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "accepted terms: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("accepted terms: no environmentId in {body}"))
        .to_string();

    let (status, got) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
    assert_eq!(status, 200, "read back: expected 200, got {status} {got}");
    assert_eq!(
        got["environment"]["termsAccepted"],
        json!(true),
        "read back: expected the stored termsAccepted of true, got {}",
        got["environment"]["termsAccepted"]
    );
    assert_eq!(
        got["environment"]["environmentName"],
        json!("terms-env"),
        "read back: expected the name the accepted request gave, got {}",
        got["environment"]["environmentName"]
    );
}
