// SPDX-License-Identifier: CC0-1.0

//! `AssociateEipToVlan` and `DisassociateEipFromVlan` over awsJson1_0: the Elastic IP
//! surface of an environment's public HCX VLAN.
//!
//! The rules under test, each from the model's own documentation
//! (`smithy_query(query="operation",operation="AssociateEipToVlan")` and the member
//! docs it quotes):
//!
//! * a well-formed `environmentId` that names no environment is refused with
//!   `ResourceNotFoundException`, whatever else the request says;
//! * `vlanName` is `hcx` and nothing else ("hcx is the only accepted VLAN name at this
//!   time"), so every other VLAN of the environment — and every spelling that is no
//!   VLAN at all — is refused with `ValidationException`;
//! * the VLAN must be PUBLIC ("This operation is only allowed for public HCX VLANs at
//!   this time"), which only a create that asked for `isHcxPublic` makes it;
//! * an accepted association is PERSISTED: `ListEnvironmentVlans` reports it
//!   afterwards, and the `ipAddress` it carries is the address the account's Amazon EC2
//!   store holds for the allocation;
//! * both operations are `@idempotent`: a repeated associate adds no second
//!   association, and a repeated disassociate still answers with the VLAN that no
//!   longer carries it;
//! * a VLAN holds at most [`MAX_EIP_ASSOCIATIONS_PER_VLAN`] associations — the store's
//!   own bound on the growth a caller choosing `allocationId` values can drive — and an
//!   associate past it is refused rather than admitted.

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state::MAX_EIP_ASSOCIATIONS_PER_VLAN;

use crate::environment_lifecycle::{call, emulator, error_type, vlans_with_prefix, Emulator};

/// An environment id that is well formed (`^(env-[a-zA-Z0-9]{10})$`) and names no
/// environment.
const ABSENT_ENVIRONMENT: &str = "env-0000000000";

/// Register one Elastic IP allocation in the account's own Amazon EC2 store, which is
/// where this service reads the address of an allocation from.
fn register_elastic_ip(engine: &engine::InMemoryEngine, allocation_id: &str, public_ip: &str) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    ec2_state.lock().addresses.insert(
        allocation_id.to_string(),
        ec2::ops::types::Address {
            allocation_id: Some(allocation_id.to_string()),
            public_ip: Some(public_ip.to_string()),
            ..Default::default()
        },
    );
}

/// Create an environment whose HCX VLAN is public, and return its id.
async fn create_public_hcx(router: &Emulator, name: &str) -> String {
    let mut request = router.create_request(Some(name));
    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    // "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).
    vlans["hcxNetworkAclId"] = json!("acl-0123456789abcdef0");
    request["initialVlans"] = vlans;
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "create {name:?}: expected 200, got {status} {body}"
    );
    body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name:?}: no environmentId in {body}"))
        .to_string()
}

/// The HCX VLAN of `environment_id`, read back through `ListEnvironmentVlans`.
async fn hcx_vlan(router: &Emulator, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(status, 200, "list vlans: expected 200, got {status} {body}");
    body["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("list vlans: no environmentVlans in {body}"))
        .iter()
        .find(|vlan| vlan["functionName"] == json!("hcx"))
        .cloned()
        .unwrap_or_else(|| panic!("list vlans: no hcx VLAN in {body}"))
}

#[tokio::test]
async fn an_eip_associates_with_the_public_hcx_vlan_and_is_read_back() {
    let router = emulator();
    let environment = create_public_hcx(&router, "eip-env").await;
    register_elastic_ip(&router.engine, "eipalloc-0123456789abcdef0", "198.51.100.7");

    let (status, body) = call(
        &router,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment,
            "vlanName": "hcx",
            "allocationId": "eipalloc-0123456789abcdef0",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "associate: expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let associations = body["vlan"]["eipAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("associate: no eipAssociations in {body}"))
        .clone();
    assert_eq!(
        associations.len(),
        1,
        "associate: expected exactly one association, got {associations:?}"
    );
    let association = associations[0].clone();
    assert_eq!(
        association["allocationId"],
        json!("eipalloc-0123456789abcdef0"),
        "associate: expected the allocation the request named, got {}",
        association["allocationId"]
    );
    assert_eq!(
        association["ipAddress"],
        json!("198.51.100.7"),
        "associate: expected the address EC2 holds for the allocation, got {}",
        association["ipAddress"]
    );
    let association_id = association["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("associate: no associationId in {association}"))
        .to_string();
    assert!(
        association_id.starts_with("eipassoc-") && association_id.len() <= 26,
        "associate: the association id must satisfy ^eipassoc-[a-zA-Z0-9_-]+$ within \
         9..26, got {association_id:?}"
    );

    // Persisted: the VLAN read back through its own lister carries the association.
    let vlan = hcx_vlan(&router, &environment).await;
    assert_eq!(
        vlan["eipAssociations"],
        json!(associations),
        "read back: expected the stored associations, got {}",
        vlan["eipAssociations"]
    );

    // `@idempotent`: the same allocation again is the same association, not a second.
    let (status, repeat) = call(
        &router,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment,
            "vlanName": "hcx",
            "allocationId": "eipalloc-0123456789abcdef0",
        }),
    )
    .await;
    assert_eq!(status, 200, "repeat associate: expected 200, got {repeat}");
    assert_eq!(
        repeat["vlan"]["eipAssociations"],
        json!(associations),
        "repeat associate: expected the association it already had, got {}",
        repeat["vlan"]["eipAssociations"]
    );

    // A DIFFERENT allocation is a second association. This one EC2 holds no record of,
    // so no address is reported for it rather than one nothing allocated.
    let (status, second) = call(
        &router,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment,
            "vlanName": "hcx",
            "allocationId": "eipalloc-00000000",
        }),
    )
    .await;
    assert_eq!(status, 200, "second associate: expected 200, got {second}");
    let after_second = second["vlan"]["eipAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("second associate: no eipAssociations in {second}"))
        .clone();
    assert_eq!(
        after_second.len(),
        2,
        "second associate: expected two associations, got {after_second:?}"
    );
    assert_eq!(
        after_second[1]["ipAddress"],
        Value::Null,
        "second associate: an allocation EC2 does not hold has no address to report, got {}",
        after_second[1]["ipAddress"]
    );

    // Disassociate removes exactly the association named, and a repeat is still a
    // success with that association gone (`@idempotent`).
    for attempt in ["the first disassociate", "a repeated disassociate"] {
        let (status, body) = call(
            &router,
            "DisassociateEipFromVlan",
            &json!({
                "environmentId": environment,
                "vlanName": "hcx",
                "associationId": association_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let left = body["vlan"]["eipAssociations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {attempt:?}: no eipAssociations in {body}"))
            .clone();
        assert_eq!(
            left.len(),
            1,
            "case {attempt:?}: expected only the other association to be left, got {left:?}"
        );
        assert_eq!(
            left[0]["allocationId"],
            json!("eipalloc-00000000"),
            "case {attempt:?}: expected the association that was not named to stay, got {}",
            left[0]["allocationId"]
        );
    }
}

#[tokio::test]
async fn a_vlan_holds_at_most_the_stores_bound_of_eip_associations() {
    // The caller chooses `allocationId`, so distinct ids are caller-driven growth of a
    // stored list that every response clones back out: the store's bound is what refuses
    // it, and a refusal past the bound leaves the stored associations as they were.
    let router = emulator();
    let environment = create_public_hcx(&router, "eip-bound").await;

    for index in 0..=MAX_EIP_ASSOCIATIONS_PER_VLAN {
        let allocation_id = format!("eipalloc-{index:017x}");
        let (status, body) = call(
            &router,
            "AssociateEipToVlan",
            &json!({
                "environmentId": environment,
                "vlanName": "hcx",
                "allocationId": allocation_id,
            }),
        )
        .await;
        if index < MAX_EIP_ASSOCIATIONS_PER_VLAN {
            assert_eq!(
                status, 200,
                "association {index}: expected HTTP 200 below the bound, got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["vlan"]["eipAssociations"].as_array().map(Vec::len),
                Some(index + 1),
                "association {index}: expected {} associations, got {}",
                index + 1,
                body["vlan"]["eipAssociations"]
            );
            continue;
        }
        // One past the bound: refused, and nothing is stored for it.
        assert_eq!(
            status, 400,
            "association {index}: expected HTTP 400 past the bound, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "association {index}: expected a ValidationException, got {}",
            error_type(&body)
        );
    }

    let vlan = hcx_vlan(&router, &environment).await;
    assert_eq!(
        vlan["eipAssociations"].as_array().map(Vec::len),
        Some(MAX_EIP_ASSOCIATIONS_PER_VLAN),
        "the refused associate must leave the VLAN at the bound, got {}",
        vlan["eipAssociations"]
    );

    // An allocation the VLAN ALREADY carries is still idempotent at the bound: it adds
    // nothing, so it is not the growth the bound refuses.
    let (status, body) = call(
        &router,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment,
            "vlanName": "hcx",
            "allocationId": format!("eipalloc-{:017x}", 0),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a repeat of a stored association at the bound: expected HTTP 200, got {status} \
         ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["vlan"]["eipAssociations"].as_array().map(Vec::len),
        Some(MAX_EIP_ASSOCIATIONS_PER_VLAN),
        "a repeat must add nothing, got {}",
        body["vlan"]["eipAssociations"]
    );
}

#[tokio::test]
async fn only_the_public_hcx_vlan_of_a_stored_environment_takes_an_eip() {
    let router = emulator();
    let public_hcx = create_public_hcx(&router, "public-hcx").await;
    // An environment whose HCX VLAN is private, which is the default a create makes.
    let private_hcx = crate::environment_lifecycle::create(&router, Some("private-hcx")).await;

    // Each row: the case, the environment the request names, the `vlanName` it gives,
    // and the error code the rule answers with — `""` for a row that is accepted.
    //
    // The VLAN names come from the `InitialVlans` members themselves
    // (`smithy_query(query="shape",shape="InitialVlans")`): every one of them is a real
    // VLAN of the environment, and only `hcx` is accepted.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the public hcx VLAN", public_hcx.as_str(), "hcx", ""),
        (
            "another real VLAN of the environment: edgeVTep",
            public_hcx.as_str(),
            "edgeVTep",
            "ValidationException",
        ),
        (
            "another real VLAN of the environment: vmkManagement",
            public_hcx.as_str(),
            "vmkManagement",
            "ValidationException",
        ),
        (
            "another real VLAN of the environment: expansionVlan2",
            public_hcx.as_str(),
            "expansionVlan2",
            "ValidationException",
        ),
        (
            "the accepted name in another case",
            public_hcx.as_str(),
            "HCX",
            "ValidationException",
        ),
        (
            "a name that is no VLAN at all",
            public_hcx.as_str(),
            "not-a-vlan",
            "ValidationException",
        ),
        (
            "the hcx VLAN of an environment that did not ask for a public one",
            private_hcx.as_str(),
            "hcx",
            "ValidationException",
        ),
        (
            "an environment this service holds no record of",
            ABSENT_ENVIRONMENT,
            "hcx",
            "ResourceNotFoundException",
        ),
        (
            // The environment is evaluated before the VLAN name, exactly as AWS does:
            // a request against an environment that is not there is a not-found
            // whatever else it says.
            "an absent environment and an unaccepted VLAN name together",
            ABSENT_ENVIRONMENT,
            "edgeVTep",
            "ResourceNotFoundException",
        ),
    ];

    for (case, environment_id, vlan_name, expected) in rows {
        for operation in ["AssociateEipToVlan", "DisassociateEipFromVlan"] {
            let mut request = json!({
                "environmentId": environment_id,
                "vlanName": vlan_name,
            });
            if operation == "AssociateEipToVlan" {
                request["allocationId"] = json!("eipalloc-0123456789abcdef0");
            } else {
                request["associationId"] = json!("eipassoc-0123456789abcdef0");
            }
            let (status, body) = call(&router, operation, &request).await;
            if expected.is_empty() {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: expected HTTP 200, got {status} ({}) {body}",
                    error_type(&body)
                );
                continue;
            }
            // Every modelled CLIENT error of this awsJson1_0 service is reported with
            // HTTP 400 — see `tests/all/error_wire_status.rs`, which owns that rule.
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_type(&body).contains(expected),
                "case {case:?} on {operation}: expected {expected}, got {} in {body}",
                error_type(&body)
            );
        }
    }

    // Nothing above was associated with the private environment's VLAN.
    let vlan = hcx_vlan(&router, &private_hcx).await;
    assert_eq!(
        vlan["eipAssociations"],
        json!([]),
        "a refused associate must leave the VLAN as it was, got {}",
        vlan["eipAssociations"]
    );
}
