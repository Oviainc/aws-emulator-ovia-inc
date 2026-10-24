// SPDX-License-Identifier: CC0-1.0

//! What an `allocationId` may NAME: an Elastic IP allocation the caller's own Amazon EC2
//! account already holds.
//!
//! `AssociateEipToVlan`'s `allocationId` is "The Elastic IP address allocation ID"
//! (`smithy_query(query="operation",operation="AssociateEipToVlan")`) of the caller's
//! Amazon EC2 account — Amazon EVS allocates none — so the rule under test, one level
//! above the example, is the one `CreateEnvironment` already follows for the `vpcId`,
//! `serviceAccessSubnetId`, `placementGroupId` and `dedicatedHostId` it is given: a
//! member naming an Amazon EC2 resource of the caller's account that the account's own
//! EC2 store does not hold is refused with the operation's modelled
//! `ValidationException`, whose `fieldList` names that member, and the request records
//! nothing. So:
//!
//! * an `allocationId` the account holds an Elastic IP allocation for is accepted, and
//!   the association reports the public address EC2 holds for it (absent when EC2 holds
//!   the allocation with no address);
//! * an `allocationId` the account holds no allocation for — however it is spelled — is
//!   refused with HTTP 400 and a `ValidationException` naming `allocationId`, and no
//!   association is minted, stored or reported by `ListEnvironmentVlans`;
//! * an allocation released back out of the account after an association was made is no
//!   longer associable, while the association it already carries stays idempotently
//!   reportable.
//!
//! The rows differ along what the rule generalises over: whether the account holds the
//! allocation, and the LENGTH and character classes of the id the request names — the
//! modelled `AllocationId` (`smithy_query(query="shape",shape="AllocationId")`) is
//! length 9..26 with pattern `^eipalloc-[a-zA-Z0-9_-]+$`, so the table carries both
//! published bounds and an id past the issue's own 17-hex-digit example.

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator};
use crate::hcx_eip_associations::{create_public_hcx, hcx_vlan, register_elastic_ip};

/// Whether the account's own Amazon EC2 store holds the allocation a row names, and the
/// public address it holds for it.
#[derive(Debug, Clone, Copy)]
enum Held {
    /// The account holds no Elastic IP allocation of that id.
    Not,
    /// The account holds the allocation, and EC2 holds no public address for it.
    WithoutAddress,
    /// The account holds the allocation, with this public address.
    WithAddress(&'static str),
}

/// The `fieldList` member names a `ValidationException` body carries.
fn refused_members(body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Associate `allocation_id` with the HCX VLAN of `environment`.
async fn associate(router: &Emulator, environment: &str, allocation_id: &str) -> (u16, Value) {
    call(
        router,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment,
            "vlanName": "hcx",
            "allocationId": allocation_id,
        }),
    )
    .await
}

/// The `allocationId` of every association the HCX VLAN of `environment` carries, in
/// stored order, read back through `ListEnvironmentVlans`.
async fn stored_allocations(router: &Emulator, environment: &str) -> Vec<String> {
    hcx_vlan(router, environment).await["eipAssociations"]
        .as_array()
        .map(|associations| {
            associations
                .iter()
                .filter_map(|association| {
                    association["allocationId"].as_str().map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Take `allocation_id` back out of the account's own Amazon EC2 store, as a release of
/// the Elastic IP address does: the account then holds no allocation of that id.
fn release_allocation(engine: &engine::InMemoryEngine, allocation_id: &str) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    ec2_state.lock().addresses.remove(allocation_id);
}

/// Every allocation id the account's own Amazon EC2 store holds an address record for.
fn held_allocations(engine: &engine::InMemoryEngine) -> Vec<String> {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut held: Vec<String> = ec2_state.lock().addresses.keys().cloned().collect();
    held.sort();
    held
}

#[tokio::test]
async fn an_allocation_the_account_does_not_hold_is_refused_and_records_nothing() {
    let router = emulator();
    let environment = create_public_hcx(&router, "eip-existence").await;

    // Each row: the case, the `allocationId` the request names, and whether the account's
    // own Amazon EC2 store holds an allocation of that id. The ids sit at both published
    // bounds of the modelled `AllocationId` and span its character classes, so no row's
    // outcome can depend on the shape of the id rather than on the allocation existing.
    let rows: Vec<(&str, &str, Held)> = vec![
        (
            "the shortest id the model admits, held with an address",
            "eipalloc-a",
            Held::WithAddress("198.51.100.1"),
        ),
        (
            "the shortest id the model admits, held by no allocation",
            "eipalloc-b",
            Held::Not,
        ),
        (
            "the longest id the model admits, held with an address",
            "eipalloc-0123456789abcdef0",
            Held::WithAddress("198.51.100.2"),
        ),
        (
            "the longest id the model admits, held by no allocation",
            "eipalloc-0123456789abcdef1",
            Held::Not,
        ),
        (
            "an id of underscores and dashes, held with no address",
            "eipalloc-a_b-c",
            Held::WithoutAddress,
        ),
        (
            "an id of underscores and dashes, held by no allocation",
            "eipalloc-d_e-f",
            Held::Not,
        ),
        (
            "an id differing from a held allocation in its case alone",
            "eipalloc-A_B-C",
            Held::Not,
        ),
        (
            "an all-digit id held by no allocation",
            "eipalloc-00000000",
            Held::Not,
        ),
    ];

    let mut registered: Vec<String> = Vec::new();
    let mut accepted: Vec<String> = Vec::new();
    for (case, allocation_id, held) in rows {
        match held {
            Held::WithAddress(public_ip) => {
                register_elastic_ip(&router.engine, allocation_id, Some(public_ip));
                registered.push(allocation_id.to_string());
            }
            Held::WithoutAddress => {
                register_elastic_ip(&router.engine, allocation_id, None);
                registered.push(allocation_id.to_string());
            }
            Held::Not => {}
        }

        let (status, body) = associate(&router, &environment, allocation_id).await;
        match held {
            Held::Not => {
                // Every modelled CLIENT error of this awsJson1_0 service is reported with
                // HTTP 400 — `tests/all/error_wire_status.rs` owns that rule.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
                    error_type(&body)
                );
                assert_eq!(
                    error_type(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body}"
                );
                let members = refused_members(&body);
                assert!(
                    members.iter().any(|name| name == "allocationId"),
                    "case {case:?}: expected a fieldList entry for \"allocationId\", got \
                     {members:?} in {body}"
                );
                // The refusal is built from the request: it names the id the caller gave.
                assert!(
                    body["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(allocation_id)),
                    "case {case:?}: expected the refusal to name {allocation_id:?}, got {}",
                    body["message"]
                );
            }
            _ => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                    error_type(&body)
                );
                accepted.push(allocation_id.to_string());
                let reported = body["vlan"]["eipAssociations"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no eipAssociations in {body}"))
                    .iter()
                    .find(|association| association["allocationId"] == json!(allocation_id))
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: no association for it in {body}"));
                let expected_ip = match held {
                    Held::WithAddress(public_ip) => json!(public_ip),
                    _ => Value::Null,
                };
                assert_eq!(
                    reported["ipAddress"], expected_ip,
                    "case {case:?}: expected the address EC2 holds ({expected_ip}), got {}",
                    reported["ipAddress"]
                );
                assert!(
                    reported["associationId"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("eipassoc-")),
                    "case {case:?}: expected an eipassoc- association id, got {}",
                    reported["associationId"]
                );
            }
        }

        // Whatever the row asked, the stored VLAN carries exactly the accepted rows'
        // allocations: a refused associate mints and stores no association.
        let stored = stored_allocations(&router, &environment).await;
        assert_eq!(
            stored, accepted,
            "case {case:?}: expected the accepted allocations {accepted:?} to be stored, \
             got {stored:?}"
        );
    }

    // And the refusals invented nothing in Amazon EC2 either: the account holds exactly
    // the allocations the rows registered.
    registered.sort();
    let held = held_allocations(&router.engine);
    assert_eq!(
        held, registered,
        "expected the account to hold exactly {registered:?}, got {held:?}"
    );
}

#[tokio::test]
async fn an_allocation_released_out_of_the_account_is_no_longer_associable() {
    let router = emulator();
    let environment = create_public_hcx(&router, "eip-released").await;
    let associated = "eipalloc-0000000000000000f";
    let released_unused = "eipalloc-0000000000000000d";
    let never_held = "eipalloc-0000000000000000e";
    register_elastic_ip(&router.engine, associated, Some("198.51.100.3"));
    register_elastic_ip(&router.engine, released_unused, Some("198.51.100.4"));

    let (status, body) = associate(&router, &environment, associated).await;
    assert_eq!(
        status, 200,
        "the first associate: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let association_id = body["vlan"]["eipAssociations"][0]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("the first associate: no associationId in {body}"))
        .to_string();

    // The account gives both allocations back. The association the VLAN already carries
    // is a stored record of this service, so the `@idempotent` repeat still reports it...
    release_allocation(&router.engine, associated);
    release_allocation(&router.engine, released_unused);
    let (status, body) = associate(&router, &environment, associated).await;
    assert_eq!(
        status, 200,
        "a repeat of a stored association: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["vlan"]["eipAssociations"][0]["associationId"],
        json!(association_id),
        "a repeat of a stored association: expected the association it had, got {}",
        body["vlan"]["eipAssociations"][0]["associationId"]
    );

    // ...while a NEW association of an allocation the account no longer holds — and one
    // it never held — is refused, and the VLAN keeps the one association it had.
    for (case, allocation_id) in [
        (
            "an allocation released out of the account before it was associated",
            released_unused,
        ),
        ("an allocation the account never held", never_held),
    ] {
        let (status, body) = associate(&router, &environment, allocation_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }
    let stored = stored_allocations(&router, &environment).await;
    assert_eq!(
        stored,
        vec![associated.to_string()],
        "expected the VLAN to keep exactly the association it had, got {stored:?}"
    );
}
