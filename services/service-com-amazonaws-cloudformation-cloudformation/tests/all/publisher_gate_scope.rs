// SPDX-License-Identifier: CC0-1.0

//! WHICH type-registry calls the extension-publisher registration gates, over the awsQuery
//! wire.
//!
//! The rule every case here holds the service to: **a CloudFormation extension-publisher
//! registration is a PRE-FLIGHT of `TestType` and of nothing beside it — the type-registry
//! operations whose contract asks nothing of the caller's publisher state are NOT gated on
//! it: whatever the caller's publisher state, they resolve the extension they were given and
//! answer the registry's modelled `TypeNotFoundException` (404) `The type '<TypeName>' cannot
//! be found.` for a name no registry holds, and they keep reporting the public extensions
//! every account's registry holds without anybody registering them.**
//!
//! The table spans what the rule generalises over: names spanning what the modelled
//! `TypeName` admits — both published length bounds, a name bearing digits, the optional
//! `::MODULE` suffix — each named under a different `RegistryType` and none of them held by
//! any Region's registry ([`unregistered`], which `test_type_publisher_gate.rs` drives
//! through the GATED operation so both halves of the rule are held to one set of names),
//! crossed with the five ungated operations the registry's not-found surface is walked
//! through: `DescribeType`, `DeactivateType`, `DeregisterType`, `SetTypeDefaultVersion` and
//! `SetTypeConfiguration`. (The registry's LIST operations answer no per-extension refusal
//! and have suites of their own; `PublishType`, the other publisher-gated operation, carries
//! a refusal sentence and an ARN-precedence rule of its own and has an open issue for them.)
//! The recorded trace this suite was written for is ONE row of that table: those five
//! operations on one unregistered three-part resource-type name, beside the `TestType` call
//! that is gated.
//!
//! `test_type_publisher_gate.rs` is the table for the gate itself — its position ahead of the
//! registry, over every identifier form and every record an identifier can resolve to — and
//! `type_testing.rs` the suite for what `TestType` does once the gate is open; this one is
//! the gate's SCOPE: the operations it must not leak into.

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registry_type, visibility,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;

use crate::extension_registry::router_with_registry;
use crate::stack_lifecycle::{call, member, xml_text};
use crate::type_testing::assert_not_found;

/// One extension no registry holds, and the kind a request names it under.
pub(crate) struct Unregistered {
    pub(crate) case: &'static str,
    pub(crate) type_kind: &'static str,
    pub(crate) type_name: String,
}

/// Extension names spanning what the modelled `TypeName` member admits — `@length 10..204`,
/// `@pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$`, from
/// `smithy_query(query="operation", operation="TestType")` — at both published bounds of that
/// length and over each `RegistryType` an extension can be registered under.
///
/// None of them is held by this crate's registry fixture, in this Region or in another, so
/// every one of them is an extension the registry resolves to nothing.
pub(crate) fn unregistered() -> Vec<Unregistered> {
    // The SHORTEST name the member admits: three two-character segments plus the four
    // characters of the two `::` separators is exactly the modelled minimum of 10.
    let shortest = "Ab::Cd::Ef".to_string();
    // The LONGEST: three 64-character segments, the two separators and the optional
    // `::MODULE` suffix is exactly the modelled maximum of 204.
    let longest = format!(
        "{}::{}::{}::MODULE",
        "A".repeat(64),
        "B".repeat(64),
        "C".repeat(64)
    );
    vec![
        Unregistered {
            case: "the shortest resource-type name the model admits",
            type_kind: registry_type::RESOURCE,
            type_name: shortest,
        },
        Unregistered {
            case: "a resource-type name bearing digits",
            type_kind: registry_type::RESOURCE,
            type_name: "Catalog9::Probe::Widget42".to_string(),
        },
        Unregistered {
            case: "a module name, carrying the optional ::MODULE suffix",
            type_kind: registry_type::MODULE,
            type_name: "My::Other::Thing::MODULE".to_string(),
        },
        Unregistered {
            case: "a hook name",
            type_kind: registry_type::HOOK,
            type_name: "Guard::Sample::Hook".to_string(),
        },
        Unregistered {
            case: "the longest name the model admits",
            type_kind: registry_type::MODULE,
            type_name: longest,
        },
    ]
}

/// One type-registry operation that identifies an extension by `Type` and `TypeName` and
/// whose contract asks NOTHING about the caller's publisher registration.
struct Ungated {
    operation: &'static str,
    /// What it needs beside `Type` and `TypeName`: `SetTypeConfiguration$Configuration` is
    /// `@required`, so a request without it identifies no configuration to set.
    extra: &'static [(&'static str, &'static str)],
}

/// The ungated operations, every one of which models `TypeNotFoundException` at httpError 404
/// (`smithy_query(query="operation", operation="DescribeType,DeactivateType,DeregisterType,\
/// SetTypeDefaultVersion,SetTypeConfiguration")`).
const UNGATED: &[Ungated] = &[
    Ungated {
        operation: "DescribeType",
        extra: &[],
    },
    Ungated {
        operation: "DeactivateType",
        extra: &[],
    },
    Ungated {
        operation: "DeregisterType",
        extra: &[],
    },
    Ungated {
        operation: "SetTypeDefaultVersion",
        extra: &[("VersionId", "00000001")],
    },
    Ungated {
        operation: "SetTypeConfiguration",
        extra: &[("Configuration", "{\"SampleKey\":\"sample-value\"}")],
    },
];

#[tokio::test]
async fn the_publisher_gate_does_not_reach_the_operations_that_do_not_require_one() {
    let (router, _engine) = router_with_registry();

    // An account holding NO publisher registration, calling the operations whose contract
    // says nothing about publisher state: each one resolves the extension it was given and is
    // answered the registry's modelled not-found, naming what the request named — never the
    // publisher refusal, which belongs to `TestType` alone.
    for Unregistered {
        case,
        type_kind,
        type_name,
    } in unregistered()
    {
        for Ungated { operation, extra } in UNGATED {
            let mut members = vec![member("Type", type_kind), member("TypeName", &type_name)];
            members.extend(extra.iter().map(|(key, value)| member(key, value)));
            let (status_code, body) = call(&router, operation, &members).await;
            let case = format!("{operation} on {case}");
            assert_not_found(&case, status_code, &body, &type_name);
        }
    }
}

#[tokio::test]
async fn a_caller_that_is_no_publisher_still_reads_the_public_extensions_the_registry_holds() {
    let (router, _engine) = router_with_registry();

    // The positive control the gate must not swallow: the public extensions Amazon Web
    // Services publishes are in every account's registry without anybody registering them,
    // so a caller holding no publisher registration reads one of them as it always did.
    // Taken from the catalogue of resource types this emulator provisions
    // (`state::template::IMPLEMENTED_RESOURCE_TYPES`) rather than written down here.
    let catalogue = [
        IMPLEMENTED_RESOURCE_TYPES
            .first()
            .expect("the catalogue holds at least one resource type"),
        IMPLEMENTED_RESOURCE_TYPES
            .last()
            .expect("the catalogue holds at least one resource type"),
    ];
    for type_name in catalogue {
        let members = vec![
            member("Type", registry_type::RESOURCE),
            member("TypeName", type_name),
        ];
        let (status_code, body) = call(&router, "DescribeType", &members).await;
        assert_eq!(
            status_code, 200,
            "case {type_name:?}: expected 200, got {status_code} {body}"
        );
        let reported = xml_text(&body, "TypeName");
        assert_eq!(
            reported.as_deref(),
            Some(*type_name),
            "case {type_name:?}: expected TypeName {type_name:?}, got {reported:?}"
        );
        let reported = xml_text(&body, "Visibility");
        assert_eq!(
            reported.as_deref(),
            Some(visibility::PUBLIC),
            "case {type_name:?}: expected Visibility {:?}, got {reported:?}",
            visibility::PUBLIC
        );
    }
}
