// SPDX-License-Identifier: CC0-1.0

//! One service, ONE refusal for a `ResourceType` outside the set an operation reports on.
//!
//! Every operation of this service that selects by resource type enforces the same rule on
//! the same member: a value outside the set that operation reports on names no resource
//! kind, so the request is refused on the member rather than answered with an empty page or
//! a zero count. The rule under test is that the refusal is ONE body whichever operation
//! answered it: HTTP 400 with `x-amzn-errortype: ValidationException`, `Reason` =
//! `FieldValidationFailed`, and exactly one `Fields` entry whose `Name` is `ResourceType`
//! and whose `Message` is the top-level `Message` — the fixed prose naming the member and
//! nothing else: the offending value is not echoed back and the accepted set is not
//! enumerated. A client keying on `Fields[].Message` therefore reads one answer for
//! one rule, not one per operation.
//!
//! The rows span the operations the rule reaches (the three that report out of a global
//! network's network-resource inventory and the one that reports telemetry over its own,
//! narrower set) crossed with the WAYS a value can fall outside the set it is checked
//! against: an invented name, a documented name in the wrong case, pluralised, padded,
//! written with the wrong separator, a prefix of a documented name, an id and an ARN rather
//! than a type — plus, for telemetry, documented network-resource types that have no session
//! to measure, which the inventory operations accept in the same request.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::network_resource_counts::SUPPORTED_TYPES;
use crate::resource_tagging::escape;

/// The three values `GetNetworkTelemetry`'s `ResourceType` accepts, from the enumeration in
/// the member's own documentation (`smithy_query query="doc"
/// shape="GetNetworkTelemetryRequest" member="ResourceType"`): a Cloud WAN Connect peer, a
/// transit gateway Connect peer and a Site-to-Site VPN connection.
const TELEMETRY_TYPES: [&str; 3] = [
    "connect-peer",
    "transit-gateway-connect-peer",
    "vpn-connection",
];

/// Every supported network-resource type, as the catalogue the counts suite sources from the
/// member's documentation lists them.
const NETWORK_TYPES: &[&str] = &SUPPORTED_TYPES;

/// One operation that selects by `ResourceType`: where it lives under a global network, and
/// the set it reports on — the set its refusal enumerates and every value it answers.
struct Selector {
    /// The operation's name, for the case labels.
    operation: &'static str,
    /// The path segment under `/global-networks/{GlobalNetworkId}` the operation is bound to.
    path: &'static str,
    /// The values the operation accepts.
    supported: &'static [&'static str],
}

/// The four operations of this service whose request carries a `ResourceType`
/// (`smithy_query operation=GetNetworkResources,GetNetworkResourceCounts,GetNetworkResourceRelationships,GetNetworkTelemetry`).
const SELECTORS: [Selector; 4] = [
    Selector {
        operation: "GetNetworkResources",
        path: "network-resources",
        supported: NETWORK_TYPES,
    },
    Selector {
        operation: "GetNetworkResourceCounts",
        path: "network-resource-count",
        supported: NETWORK_TYPES,
    },
    Selector {
        operation: "GetNetworkResourceRelationships",
        path: "network-resource-relationships",
        supported: NETWORK_TYPES,
    },
    Selector {
        operation: "GetNetworkTelemetry",
        path: "network-telemetry",
        supported: &TELEMETRY_TYPES,
    },
];

/// The prose every refusal of this member carries, whichever operation and whichever value
/// was refused: the member named, and the fixed statement that its value is invalid.
///
/// `pub(crate)` because `crate::refusal_wording` tables the same refusal for one of these
/// operations, so one rule keeps one expected string.
pub(crate) const INVALID_RESOURCE_TYPE: &str = "ResourceType is invalid";

impl Selector {
    /// The operation's URI against `network`, selecting `resource_type` when one is given.
    fn uri(&self, network: &str, resource_type: Option<&str>) -> String {
        let path = format!("/global-networks/{network}/{}", self.path);
        match resource_type {
            Some(resource_type) => format!("{path}?resourceType={}", escape(resource_type)),
            None => path,
        }
    }
}

/// The ways one value can fall outside the set it is checked against, each a row of the
/// table below. The names are generic, so the same rows run against every operation.
const OUTSIDE_EVERY_SET: [(&str, &str); 8] = [
    ("an invented type", "not-a-real-type"),
    ("a documented type in the wrong case", "Site"),
    ("a documented type pluralised", "sites"),
    ("a documented type with a trailing space", "site "),
    ("an underscore instead of a hyphen", "core_network"),
    ("a prefix of a documented type", "transit"),
    ("a resource id instead of a type", "site-00000000000000001"),
    (
        "an ARN instead of a type",
        "arn:aws:networkmanager::111122223333:site/site-00000000000000001",
    ),
];

/// Drive one request and assert the one refusal the rule allows, returning the body so two
/// operations' answers can be compared with each other.
async fn assert_refusal(
    emulator: &Emulator,
    selector: &Selector,
    network: &str,
    case: &str,
    resource_type: &str,
) -> Value {
    let case = format!("{} / {case} ({resource_type:?})", selector.operation);
    let (status, body) = emulator.get(&selector.uri(network, Some(resource_type))).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case {case}: expected Reason FieldValidationFailed, got {body}"
    );
    let expected = INVALID_RESOURCE_TYPE;
    assert_eq!(
        body["Message"].as_str(),
        Some(expected),
        "case {case}: expected Message {expected:?}, got {body}"
    );
    assert_eq!(
        body["Fields"],
        json!([{ "Name": "ResourceType", "Message": expected }]),
        "case {case}: expected one Fields entry naming ResourceType and carrying the \
         top-level message, got {body}"
    );
    body
}

#[tokio::test]
async fn a_resource_type_outside_the_set_is_refused_under_one_body_whatever_the_operation() {
    let emulator = Emulator::new();
    // A live global network, so every refusal below turns on the `ResourceType` member and
    // nothing wider: the same requests carrying a supported type are answered.
    let network = emulator.create_global_network("resource type rule").await;
    emulator.create_site(&network, "hq").await;

    for selector in &SELECTORS {
        // The rows that fall outside EVERY set, plus — for an operation reporting on a
        // narrower set — the documented network-resource types that set leaves out.
        let narrower: Vec<(String, &str)> = NETWORK_TYPES
            .iter()
            .filter(|resource_type| !selector.supported.contains(resource_type))
            .map(|resource_type| {
                (
                    "a documented network-resource type outside this set".to_string(),
                    *resource_type,
                )
            })
            .collect();
        for (case, resource_type) in OUTSIDE_EVERY_SET
            .iter()
            .map(|(case, resource_type)| ((*case).to_string(), *resource_type))
            .chain(narrower)
        {
            assert_refusal(&emulator, selector, &network, &case, resource_type).await;
        }

        // And every value the operation DOES report on is answered, so the refusals above
        // are the member check and not the operation refusing its own catalogue.
        for resource_type in selector.supported {
            let (status, body) = emulator
                .get(&selector.uri(&network, Some(resource_type)))
                .await;
            assert_eq!(
                status, 200,
                "case {} / a supported type ({resource_type}): expected 200, got {status} with {body}",
                selector.operation
            );
        }
    }
}

#[tokio::test]
async fn the_operations_sharing_one_set_answer_the_identical_refusal() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("one rule").await;

    // The three operations that report out of the network-resource inventory enforce the rule
    // over the SAME set, so for one offending value their refusals agree member for member —
    // not merely in status and error code.
    let sharing: Vec<&Selector> = SELECTORS
        .iter()
        .filter(|selector| selector.supported == NETWORK_TYPES)
        .collect();
    assert_eq!(
        sharing.len(),
        3,
        "expected the three inventory operations to share one set, got {}",
        sharing.len()
    );

    for (case, resource_type) in OUTSIDE_EVERY_SET {
        let mut answers: Vec<(&str, Value)> = Vec::new();
        for selector in &sharing {
            let body = assert_refusal(&emulator, selector, &network, case, resource_type).await;
            answers.push((selector.operation, body));
        }
        let (first_operation, first) = &answers[0];
        for (operation, body) in &answers[1..] {
            assert_eq!(
                (&body["Message"], &body["Reason"], &body["Fields"]),
                (&first["Message"], &first["Reason"], &first["Fields"]),
                "case {case} ({resource_type:?}): {operation} must answer the refusal \
                 {first_operation} answers, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn omitting_the_resource_type_selects_every_type_rather_than_being_refused() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("no filter").await;
    emulator.create_site(&network, "hq").await;

    // The member is optional on all four operations, so its absence is no filter at all: the
    // refusal is of a value outside the set, never of the member being left out.
    for selector in &SELECTORS {
        let (status, body) = emulator.get(&selector.uri(&network, None)).await;
        assert_eq!(
            status, 200,
            "case {} / no resourceType: expected 200, got {status} with {body}",
            selector.operation
        );
    }
}
