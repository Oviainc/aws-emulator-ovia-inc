// SPDX-License-Identifier: CC0-1.0

//! The two halves of a REFUSAL a client branches on — the error CODE and the request id
//! the envelope reports it under — over the read and write paths an instance inventory
//! tool exercises, driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! EC2's model declares no error shapes at all (`smithy_query(query="operation", …)`
//! reports `errors: []` for every operation), so the code on the wire and the envelope
//! around it are the whole contract a client can branch on. Two rules:
//!
//! 1. A VALUE THAT IS NOT A RESOURCE ID OF ITS KIND IS REFUSED AS MALFORMED, BEFORE ANY
//!    LOOKUP, and a well-formed id the account holds no resource for is refused as not
//!    found — two different answers, because a client cannot tell a typo from a deleted
//!    resource otherwise and its repair-or-retry logic takes the wrong branch.
//! 2. EVERY REFUSAL REPORTS THE REQUEST IT ANSWERS, in the one `<RequestID>` the ec2Query
//!    error envelope closes with, whichever operation answered and whichever refusal it
//!    is — which is what a client logs, correlates and quotes to support.
//!
//! The rows span what those generalise over: which operation answered (a volume
//! attribute modification, an address allocation, an instance describe), which id kind the
//! value was meant to be, which refusal it is (malformed, not found, a value the operation
//! does not accept), and the FORM the bad value took (an id of another kind, a non-hex
//! digit, the wrong number of digits, free text).
//!
//! Both rules are asserted through the readers the envelope's own suite publishes
//! ([`super::ec2_query_error_envelope::refusal_request_id`]), so there is one definition of
//! "this envelope is intact" in the crate rather than one per suite.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::call;
use super::ec2_query_error_envelope::refusal_request_id;
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One row: what it calls, with which form fields, and the status and `<Code>` this
/// service must refuse it with.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// A `ModifyVolumeAttribute` that sets `autoEnableIO` on `volume_id` — the attribute is
/// what makes the request one that names something to modify, so the refusal under test is
/// about the id rather than about a missing attribute.
fn modify_volume(volume_id: &str) -> Vec<Param> {
    vec![
        param("VolumeId", volume_id),
        param("AutoEnableIO.Value", "true"),
    ]
}

/// Every refusal row.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            // A well-formed volume id the account holds no volume for: the OTHER half of
            // the id rule, which is what makes the malformed rows above a distinct answer
            // rather than the only one.
            case: "a modification of a volume the account does not hold",
            action: "ModifyVolumeAttribute",
            params: modify_volume("vol-0000000000000beef"),
            status: 400,
            code: ec2::state::INVALID_VOLUME_ID_NOT_FOUND,
        },
        Row {
            case: "an allocation of an address the account was never allocated",
            action: "AllocateAddress",
            params: vec![param("Domain", "vpc"), param("Address", "192.0.2.88")],
            status: 400,
            code: ec2::state::INVALID_ADDRESS_NOT_FOUND,
        },
        Row {
            // A SHORT-form instance id: that form carries no obfuscated identifier for the
            // describe to decode, so it is looked up and reported absent — the other half
            // of the id rule, which is what makes the malformed row below a distinct answer
            // rather than the only one.
            case: "a describe of an instance the account does not hold",
            action: "DescribeInstances",
            params: vec![param("InstanceId.1", "i-0000beef")],
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        Row {
            case: "a describe of a long-form instance id this service never issued",
            action: "DescribeInstances",
            params: vec![param("InstanceId.1", "i-0000000000000beef")],
            status: 400,
            code: "InvalidInstanceID.Malformed",
        },
        Row {
            case: "a describe of something that is not an instance id at all",
            action: "DescribeInstances",
            params: vec![param("InstanceId.1", "not-an-instance")],
            status: 400,
            code: "InvalidInstanceID.Malformed",
        },
    ];
    // The malformed-volume-id rows: the FORMS a value that is not a volume id takes — an
    // id of another kind, non-hexadecimal digits, too few digits, uppercase, and free
    // text. Every one of them names no volume in any account, so every one is malformed
    // rather than not found.
    for malformed in [
        "i-0123456789abcdef0",
        "vol-zzzz",
        "vol-0123456",
        "vol-0123456789ABCDEF0",
        "not-a-volume",
    ] {
        rows.push(Row {
            case: "a modification of a value that is not a volume id",
            action: "ModifyVolumeAttribute",
            params: modify_volume(malformed),
            status: 400,
            code: ec2::state::INVALID_VOLUME_ID_MALFORMED,
        });
    }
    rows
}

/// Every row is refused with its own code and status, and every refusal reports the
/// request it answers in one `<RequestID>` of its own.
#[tokio::test]
async fn every_refusal_carries_its_own_code_and_request_id() {
    let (router, _engine) = super::ec2_query::emulator();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for row in rows() {
        let case = format!("{} ({})", row.case, row.action);
        let (status, body) = call(&router, REGION, row.action, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        // Reads the envelope AND the code: the `<Code>` the row names, a non-empty
        // `<Message>`, and exactly one `<RequestID>` closing the envelope.
        let request_id = refusal_request_id(&case, &body, row.code);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: the request id {request_id:?} was already reported for \
             another refusal"
        );
    }
}

/// The same operation, refused twice, reports two different request ids: the id names ONE
/// call, which is what makes it something to correlate on.
#[tokio::test]
async fn two_refusals_of_one_operation_report_two_request_ids() {
    let (router, _engine) = super::ec2_query::emulator();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for attempt in 1..=3 {
        let case = format!("attempt {attempt}");
        let (status, body) = call(
            &router,
            REGION,
            "ModifyVolumeAttribute",
            &modify_volume("vol-zzzz"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let request_id = refusal_request_id(&case, &body, ec2::state::INVALID_VOLUME_ID_MALFORMED);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: expected a fresh request id, got {request_id:?} again"
        );
    }
}

/// A refusal writes nothing: the volume a malformed-id modification named cannot have been
/// created, and a store that held no volume still holds none.
#[tokio::test]
async fn a_refused_modification_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let held = |engine: &engine::InMemoryEngine| {
        engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .volumes
            .len()
    };
    let before = held(&engine);
    for volume_id in ["vol-zzzz", "vol-0000000000000beef"] {
        let (status, _body) = call(
            &router,
            REGION,
            "ModifyVolumeAttribute",
            &modify_volume(volume_id),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {volume_id:?}: the modification must be refused"
        );
    }
    assert_eq!(
        held(&engine),
        before,
        "a refused modification must not write a volume"
    );
}
