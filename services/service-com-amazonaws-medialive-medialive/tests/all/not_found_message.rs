// SPDX-License-Identifier: CC0-1.0

//! What a MediaLive request carrying an identifier that resolves to NO resource
//! of the requested type is answered with, end to end over the restJson1 wire.
//!
//! The rule: whichever operation answers it, the refusal's `message` is the one
//! message this service composes from the request — the resource's own label,
//! the identifier the request named, then `not found`. The LABEL is that
//! resource family's own and is never derived from its shape name: MediaLive
//! words some families' label as the shape name (`InputSecurityGroup`) and
//! others as an English phrase (`SDI source`).
//! The resource's own get/update/delete word it exactly as the batch operations
//! and the reference checks of the same resource do, so a client cannot tell from
//! the wording which entry point it asked through.
//!
//! The suite is a TABLE over two axes, because that is what the rule generalises
//! over:
//!
//! 1. the OPERATION that resolves the identifier — three resource families
//!    (network, SdiSource, input security group) times their own describe, update
//!    and delete, so every HTTP method a label travels on and both error classes
//!    these operations model for an unresolvable id (the 404 `NotFoundException`
//!    most of them declare, and the 400 `BadRequestException` `UpdateNetwork`
//!    declares in its place, since its error list carries no 404);
//! 2. the IDENTIFIER's form — an unminted id of the numeric shape this service
//!    mints, the longest numeric id, a non-numeric identifier, one carrying
//!    characters an `@httpLabel` must percent-encode, and the LIVE id of another
//!    family, which names nothing here because each family mints its ids from a
//!    range of its own.
//!
//! Every row runs against a store that really holds one resource of each family,
//! so what is asserted is the RESOLUTION refusing an identifier rather than an
//! empty store refusing everything — and the suite ends by reading all three back
//! through their describes, since a refused request writes nothing.

use serde_json::{json, Value};

use crate::networks::{created, id_of, NETWORKS};
use crate::support::{input_security_group, percent_encoded, sdi_source};
use crate::wire::{call, error_code, error_message, medialive};

/// The collection URI `CreateSdiSource` and the SdiSource's own operations are
/// bound to (`smithy_query(query="operation", operation="DescribeSdiSource")`:
/// GET /prod/sdiSources/{SdiSourceId}).
const SDI_SOURCES: &str = "/prod/sdiSources";

/// The collection URI the input security group's own operations are bound to
/// (`smithy_query(query="operation",
/// operation="DescribeInputSecurityGroup")`: GET
/// /prod/inputSecurityGroups/{InputSecurityGroupId}).
const INPUT_SECURITY_GROUPS: &str = "/prod/inputSecurityGroups";

/// One resource family: the collection its identifier is a path segment of, and
/// the resource type its refusals name.
struct Family {
    /// The family, as the case name spells it.
    name: &'static str,
    /// The collection URI its single-resource operations hang off.
    collection: &'static str,
    /// The resource type its refusal message opens with.
    resource_type: &'static str,
    /// Where its describe reports the resource's `Id`: the SdiSource's response
    /// wraps the resource in a member of its own
    /// (`smithy_query(query="operation", operation="DescribeSdiSource")`:
    /// `sdiSource`), while the network's and the group's are flat.
    id_wrapper: Option<&'static str>,
}

const NETWORK: Family = Family {
    name: "network",
    collection: NETWORKS,
    resource_type: "Network",
    id_wrapper: None,
};

const SDI_SOURCE: Family = Family {
    name: "SdiSource",
    collection: SDI_SOURCES,
    // MediaLive words this resource's not-found sentence as the English phrase
    // `SDI source`, not as the shape name.
    resource_type: "SDI source",
    id_wrapper: Some("sdiSource"),
};

const INPUT_SECURITY_GROUP: Family = Family {
    name: "input security group",
    collection: INPUT_SECURITY_GROUPS,
    // And this one as the shape name `InputSecurityGroup`: the two conventions
    // are per-resource, so neither can be derived from the other.
    resource_type: "InputSecurityGroup",
    id_wrapper: None,
};

/// One row of the operation axis: the operation, how it reaches the emulator, and
/// the error class it models for an identifier that resolves to nothing.
struct Refused {
    /// The operation's name, for the case the assertions report.
    op: &'static str,
    /// The family whose identifier it resolves.
    family: &'static Family,
    /// The HTTP method it is bound to.
    method: &'static str,
    /// The status its refusal carries.
    status: u16,
    /// The error shape its refusal carries.
    code: &'static str,
}

/// The body an update carries, so the request reaches the identifier resolution
/// instead of being refused for the members it names. A read and a delete carry
/// none.
fn body_for(refusal: &Refused) -> Option<Value> {
    match (refusal.method, refusal.family.collection) {
        ("PUT", NETWORKS) => Some(json!({ "name": "renamed-by-a-refused-update" })),
        ("PUT", SDI_SOURCES) => Some(json!({ "name": "renamed-by-a-refused-update" })),
        ("PUT", INPUT_SECURITY_GROUPS) => {
            Some(json!({ "whitelistRules": [{ "cidr": "203.0.113.0/24" }] }))
        }
        _ => None,
    }
}

/// Rule: a request whose identifier resolves to no resource of the type it names
/// is refused with the message this service composes from the request — the
/// resource type, that identifier, then `not found` — whichever of the resource's
/// own operations answered it.
#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_refused_with_the_services_one_message() {
    // `UpdateNetwork`'s error list carries no 404
    // (`smithy_query(query="operation", operation="UpdateNetwork")`), so its
    // refusal for an id naming nothing is the 400 it does model; every other row
    // models the 404.
    let table = [
        Refused {
            op: "DescribeNetwork",
            family: &NETWORK,
            method: "GET",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "UpdateNetwork",
            family: &NETWORK,
            method: "PUT",
            status: 400,
            code: "BadRequestException",
        },
        Refused {
            op: "DeleteNetwork",
            family: &NETWORK,
            method: "DELETE",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "DescribeSdiSource",
            family: &SDI_SOURCE,
            method: "GET",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "UpdateSdiSource",
            family: &SDI_SOURCE,
            method: "PUT",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "DeleteSdiSource",
            family: &SDI_SOURCE,
            method: "DELETE",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "DescribeInputSecurityGroup",
            family: &INPUT_SECURITY_GROUP,
            method: "GET",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "UpdateInputSecurityGroup",
            family: &INPUT_SECURITY_GROUP,
            method: "PUT",
            status: 404,
            code: "NotFoundException",
        },
        Refused {
            op: "DeleteInputSecurityGroup",
            family: &INPUT_SECURITY_GROUP,
            method: "DELETE",
            status: 404,
            code: "NotFoundException",
        },
    ];

    let router = medialive();

    // One live resource of each family, so every refusal below is the resolution
    // of an identifier and not an empty store.
    let network = created(&router, "the live network", json!({ "name": "live" })).await;
    let live_network = id_of("the live network", &network);
    let live_sdi_source = sdi_source(&router, "live").await;
    let live_group = input_security_group(&router).await;
    let live: [(&str, &Family, &str); 3] = [
        ("the live network's id", &NETWORK, live_network.as_str()),
        ("the live SdiSource's id", &SDI_SOURCE, live_sdi_source.as_str()),
        (
            "the live input security group's id",
            &INPUT_SECURITY_GROUP,
            live_group.as_str(),
        ),
    ];

    for refusal in &table {
        // The identifier axis: forms that name no resource of this family,
        // including each OTHER family's live id.
        let mut identifiers: Vec<(String, &str)> = vec![
            ("an unminted id of the shape this service mints".to_string(), "9999999"),
            ("the longest numeric id a request may carry".to_string(), "12345678901"),
            ("an identifier that is not numeric at all".to_string(), "no-such-resource"),
            (
                "an identifier a label must percent-encode".to_string(),
                "no such resource",
            ),
        ];
        for (case, family, id) in &live {
            if family.name != refusal.family.name {
                identifiers.push(((*case).to_string(), id));
            }
        }

        for (identifier_case, identifier) in identifiers {
            let case = format!("{} with {identifier_case}", refusal.op);
            let uri = format!(
                "{}/{}",
                refusal.family.collection,
                percent_encoded(identifier)
            );
            let (status, body) =
                call(&router, refusal.method, &uri, body_for(refusal)).await;

            assert_eq!(
                status, refusal.status,
                "case {case:?}: expected {}, got {status}: {body}",
                refusal.status
            );
            assert_eq!(
                error_code(status, &body),
                refusal.code,
                "case {case:?}: expected {}, got {body}",
                refusal.code
            );
            let expected = format!("{} {identifier} not found", refusal.family.resource_type);
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                error_message(&body)
            );
        }
    }

    // Every refusal above wrote nothing: all three live resources still read back
    // through their own describes, reporting the very id they were minted under.
    for (case, family, id) in &live {
        let (status, body) = call(&router, "GET", &format!("{}/{id}", family.collection), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the live {} to still be readable, got {status}: {body}",
            family.name
        );
        let reported = match family.id_wrapper {
            Some(wrapper) => &body[wrapper]["id"],
            None => &body["id"],
        };
        assert_eq!(
            reported,
            &json!(id),
            "case {case:?}: expected the stored {} to still report its own id, got {:?}",
            family.name,
            reported
        );
    }
}
