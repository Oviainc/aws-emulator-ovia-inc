// SPDX-License-Identifier: CC0-1.0

//! The VOCABULARY of VPC endpoint states: every state a VPC endpoint or a connection to one
//! can be reported in, checked against the enum the model generates.
//!
//! The rule under test: each published state is the shared `State` enum's member name with
//! its first letter lowercased — "`vpc-endpoint-state` - The state of the endpoint
//! (`pendingAcceptance` | `pending` | `available` | `deleting` | `deleted` | `rejected` |
//! `failed`)" (`DescribeVpcEndpointsRequest$Filters`). A client, waiter or poller compares
//! against the published value, so the spelling is the contract.
//!
//! This covers the whole catalogue rather than the states an operation writes today: the
//! five no operation writes yet (`pendingAcceptance`, `deleting`, `deleted`, `rejected`,
//! `failed`) are spelled here before the operation that writes one lands, and a value added
//! to the catalogue in some other spelling is caught here too. What a created endpoint
//! REPORTS, and the poll a client makes on it, are the sibling suites
//! `vpc_endpoint_state.rs` and `vpc_endpoint_readiness_poll.rs`.

use ec2::ops::types::STATE_VALUES;
use ec2::state::VPC_ENDPOINT_STATE_VALUES;
use service_com_amazonaws_ec2_amazonec2 as ec2;

/// `member` with its first character lowercased — the lowerCamelCase form of a Smithy enum
/// member name.
fn lower_camel(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The catalogue spans the states the filter's doc list publishes, so a value dropped or
/// misspelled in it is caught rather than silently un-asserted.
///
/// Source: "`vpc-endpoint-state` - The state of the endpoint (`pendingAcceptance` |
/// `pending` | `available` | `deleting` | `deleted` | `rejected` | `failed`)"
/// (`DescribeVpcEndpointsRequest$Filters`).
#[test]
fn the_catalogue_spans_the_documented_states() {
    for documented in [
        "pendingAcceptance",
        "pending",
        "available",
        "deleting",
        "deleted",
        "rejected",
        "failed",
    ] {
        assert!(
            VPC_ENDPOINT_STATE_VALUES.contains(&documented),
            "case {documented:?}: expected it among {VPC_ENDPOINT_STATE_VALUES:?}"
        );
    }
}

/// Every published state names exactly one member of the shared `State` enum and is that
/// member lowercased, and no published state is a member name itself.
#[test]
fn every_published_state_is_a_state_enum_member_lowercased() {
    for state in VPC_ENDPOINT_STATE_VALUES {
        let modelled: Vec<&str> = STATE_VALUES
            .iter()
            .copied()
            .filter(|member| lower_camel(member) == state)
            .collect();
        assert_eq!(
            modelled.len(),
            1,
            "case {state:?}: expected exactly one member of {STATE_VALUES:?} to name this \
             state, got {modelled:?}"
        );
        assert!(
            !STATE_VALUES.contains(&state),
            "case {state:?}: a published state must not be a capitalized member name"
        );
    }
}

/// The states this service's own writers store are published ones, so what a create and an
/// acceptance put in the store is what a client compares against.
#[test]
fn the_states_the_writers_store_are_published_states() {
    for state in [
        ec2::state::VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE,
        ec2::state::VPC_ENDPOINT_STATE_PENDING,
        ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
    ] {
        assert!(
            VPC_ENDPOINT_STATE_VALUES.contains(&state),
            "case {state:?}: expected one of {VPC_ENDPOINT_STATE_VALUES:?}"
        );
    }
}
