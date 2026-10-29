// SPDX-License-Identifier: CC0-1.0

//! The ONE gate `AssociateEipToVlan` and `DisassociateEipFromVlan` resolve the VLAN a
//! request addresses with, driven over awsJson1_0 through both operations.
//!
//! Both operations take the same three-step reading of `vlanName`, from the same
//! documentation (`smithy_query(query="operation",operation="AssociateEipToVlan,
//! DisassociateEipFromVlan")` and the `vlanName` member doc it quotes):
//!
//! 1. the name is `hcx` — "hcx is the only accepted VLAN name at this time";
//! 2. the environment HAS that VLAN;
//! 3. that VLAN is public — the operations are "only allowed for public HCX VLANs at
//!    this time".
//!
//! The gate lives in ONE place, [`evs::state::StoredEnvironment::addressed_public_hcx_vlan`]
//! and its `_mut` twin, so this suite asserts the WHOLE refusal — status, error code,
//! `reason`, `fieldList` entry and message — for every step and every operation, which
//! is what pins the two operations to the same rule. The only part of a refusal that
//! differs between them is how each words the action on an Elastic IP address
//! ("associated with" / "disassociated from"), so each row below is asserted once per
//! operation with that operation's wording.
//!
//! The rows differ along what the gate generalises over: which step refuses the request,
//! and — for step 1 — whether the name is another real VLAN of the environment (an
//! `InitialVlans` member), a differently-cased spelling of the accepted one, or no VLAN
//! of this service at all. The accepted row is asserted too, so the refusals are not a
//! blanket one: a public HCX VLAN takes an address through one operation and gives it
//! back through the other.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, create_settled, emulator, error_type, Emulator};
use crate::hcx_eip_associations::{create_public_hcx, hcx_vlan, register_elastic_ip};

/// An Elastic IP allocation of the account's own Amazon EC2 store, so no row below is
/// refused for the `allocationId` instead of for the VLAN rule under test
/// (`tests/all/eip_allocation_existence.rs` owns that rule).
const ALLOCATION: &str = "eipalloc-0123456789abcdef0";

/// Which step of the gate a row expects to be refused by.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Step 1: `vlanName` is not the only accepted name.
    UnacceptedName,
    /// Step 2: the accepted name is no VLAN of this environment.
    NoSuchVlan,
    /// Step 3: the VLAN the name resolves to is not public.
    PrivateVlan,
}

impl Step {
    /// The sentence this step refuses `vlanName` with, worded for the operation whose
    /// action on an Elastic IP address is `verb`.
    fn problem(self, verb: &str) -> String {
        match self {
            // `hcx` is the accepted name itself, quoted from the `vlanName` doc.
            Self::UnacceptedName => format!(
                "must be hcx, the only VLAN an Elastic IP address can be {verb} at this time"
            ),
            Self::NoSuchVlan => "names no VLAN of this environment".to_string(),
            Self::PrivateVlan => format!(
                "names a private VLAN; an Elastic IP address can only be {verb} a public HCX VLAN"
            ),
        }
    }
}

/// One of the two Elastic IP operations: its name on the wire, and the phrase it words
/// the action with.
#[derive(Debug, Clone, Copy)]
struct Operation {
    name: &'static str,
    verb: &'static str,
}

/// Both operations the gate serves, each with its own wording of the action.
const OPERATIONS: [Operation; 2] = [
    Operation {
        name: "AssociateEipToVlan",
        verb: "associated with",
    },
    Operation {
        name: "DisassociateEipFromVlan",
        verb: "disassociated from",
    },
];

/// A request for `operation` addressing `environment_id`'s `vlan_name`, carrying the id
/// member that operation also requires.
fn request(operation: Operation, environment_id: &str, vlan_name: &str) -> Value {
    let mut body = json!({
        "environmentId": environment_id,
        "vlanName": vlan_name,
    });
    if operation.name == "AssociateEipToVlan" {
        body["allocationId"] = json!(ALLOCATION);
    } else {
        body["associationId"] = json!("eipassoc-0123456789abcdef0");
    }
    body
}

/// Drop the HCX VLAN from a stored environment, so step 2 of the gate — the accepted
/// name resolving to no VLAN of the environment — can be driven over the wire. The
/// modelled `InitialVlans` makes `hcx` a required member of every create, so this state
/// is not reachable through `CreateEnvironment`.
fn drop_hcx_vlan(router: &Emulator, environment_id: &str) {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = store.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    environment
        .vlans
        .retain(|vlan| vlan.function_name != evs::state::HCX_FUNCTION);
}

/// Assert the whole refusal a gate step states: this service's validation envelope,
/// naming `vlanName` and nothing else.
fn assert_refused(case: &str, status: u16, body: &Value, problem: &str) {
    // Every modelled CLIENT error of this awsJson1_0 service goes on the wire as a 400 —
    // `tests/all/error_wire_status.rs` owns that rule.
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body}"
    );
    let sentence = format!("vlanName {problem}");
    let expected = format!("{}{sentence}", evs::state::VALIDATION_FAILURE_PREFIX);
    assert_eq!(
        body["message"],
        json!(expected),
        "case {case:?}: expected message {expected:?}, got {}",
        body["message"]
    );
    assert_eq!(
        body["reason"],
        json!(evs::state::REASON_FIELD_VALIDATION_FAILED),
        "case {case:?}: expected reason {:?}, got {}",
        evs::state::REASON_FIELD_VALIDATION_FAILED,
        body["reason"]
    );
    assert_eq!(
        body["fieldList"],
        json!([{ "name": "vlanName", "message": sentence }]),
        "case {case:?}: expected one fieldList entry against vlanName, got {}",
        body["fieldList"]
    );
}

#[tokio::test]
async fn both_elastic_ip_operations_resolve_the_addressed_vlan_by_the_same_gate() {
    let router = emulator();
    let public_hcx = create_public_hcx(&router, "eip-gate-public").await;
    // The default a create makes: an HCX VLAN that did not ask to be public.
    let private_hcx = create_settled(&router, Some("eip-gate-private")).await;
    // An environment whose HCX VLAN is not in the store at all.
    let no_hcx = create_public_hcx(&router, "eip-gate-absent").await;
    drop_hcx_vlan(&router, &no_hcx);
    register_elastic_ip(&router.engine, ALLOCATION, Some("198.51.100.7"));

    // Each row: the case, the environment the request names, the `vlanName` it gives,
    // and the step of the gate that refuses it. The VLAN names come from the
    // `InitialVlans` members themselves
    // (`smithy_query(query="shape",shape="InitialVlans")`).
    let rows: Vec<(&str, &str, String, Step)> = vec![
        (
            "another real VLAN of the environment: vmkManagement",
            public_hcx.as_str(),
            "vmkManagement".to_string(),
            Step::UnacceptedName,
        ),
        (
            "another real VLAN of the environment: nsxUplink",
            public_hcx.as_str(),
            "nsxUplink".to_string(),
            Step::UnacceptedName,
        ),
        (
            "another real VLAN of the environment: expansionVlan1",
            public_hcx.as_str(),
            "expansionVlan1".to_string(),
            Step::UnacceptedName,
        ),
        (
            "the accepted name upper-cased",
            public_hcx.as_str(),
            "HCX".to_string(),
            Step::UnacceptedName,
        ),
        (
            "the accepted name with the first letter capitalised",
            public_hcx.as_str(),
            "Hcx".to_string(),
            Step::UnacceptedName,
        ),
        (
            "the accepted name with surrounding space",
            public_hcx.as_str(),
            " hcx ".to_string(),
            Step::UnacceptedName,
        ),
        (
            "a name that is no VLAN at all",
            public_hcx.as_str(),
            "not-a-vlan".to_string(),
            Step::UnacceptedName,
        ),
        (
            // The modelled `vlanName` is @length(1..200)
            // (`smithy_query(query="shape",shape="AssociateEipToVlanRequest")`), so a
            // one-character name is the shortest value the gate ever sees.
            "a one-character name",
            public_hcx.as_str(),
            "h".to_string(),
            Step::UnacceptedName,
        ),
        (
            "a name at the length ceiling the model admits",
            public_hcx.as_str(),
            "v".repeat(200),
            Step::UnacceptedName,
        ),
        (
            "the accepted name against an environment holding no such VLAN",
            no_hcx.as_str(),
            evs::state::HCX_FUNCTION.to_string(),
            Step::NoSuchVlan,
        ),
        (
            "the hcx VLAN of an environment that did not ask for a public one",
            private_hcx.as_str(),
            evs::state::HCX_FUNCTION.to_string(),
            Step::PrivateVlan,
        ),
    ];

    for (case, environment_id, vlan_name, step) in rows {
        for operation in OPERATIONS {
            let case = format!("{case} on {}", operation.name);
            let (status, body) = call(
                &router,
                operation.name,
                &request(operation, environment_id, &vlan_name),
            )
            .await;
            assert_refused(&case, status, &body, &step.problem(operation.verb));
        }
    }

    // Neither refused call wrote anything: the public VLAN still carries no association.
    let vlan = hcx_vlan(&router, &public_hcx).await;
    assert_eq!(
        vlan["eipAssociations"],
        json!([]),
        "a refused request must leave the addressed VLAN as it was, got {}",
        vlan["eipAssociations"]
    );
}

#[tokio::test]
async fn a_public_hcx_vlan_passes_the_gate_in_both_operations() {
    // The other side of the gate: the one VLAN it admits is admitted by BOTH operations,
    // so the refusals above are the three steps and nothing wider.
    let router = emulator();
    let environment = create_public_hcx(&router, "eip-gate-served").await;
    register_elastic_ip(&router.engine, ALLOCATION, Some("198.51.100.7"));

    let (status, body) = call(
        &router,
        "AssociateEipToVlan",
        &request(OPERATIONS[0], &environment, evs::state::HCX_FUNCTION),
    )
    .await;
    assert_eq!(
        status, 200,
        "associate past the gate: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let association_id = body["vlan"]["eipAssociations"][0]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("associate past the gate: no associationId in {body}"))
        .to_string();

    let (status, body) = call(
        &router,
        "DisassociateEipFromVlan",
        &json!({
            "environmentId": environment,
            "vlanName": evs::state::HCX_FUNCTION,
            "associationId": association_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "disassociate past the gate: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["vlan"]["eipAssociations"],
        json!([]),
        "disassociate past the gate: expected the association to be gone, got {}",
        body["vlan"]["eipAssociations"]
    );
}
