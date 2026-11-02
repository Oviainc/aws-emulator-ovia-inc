// SPDX-License-Identifier: CC0-1.0

//! `AssociateLink`'s two refusal classes, and the fact that neither writes anything.
//!
//! An association is a new record stored under the global network the path addresses, and
//! it ties two resources that global network already holds. That splits the refusals in
//! three, and this suite tables all of them:
//!
//! * the global network the path addresses, naming nothing stored, is a FIELD failure on
//!   the member that carried it: 400 `ValidationException` with
//!   `Reason` = `FieldValidationFailed` and one `Fields` entry naming `globalNetworkId`,
//!   the refusal every store under a global network gives for that parent reference
//!   (`crate::unknown_global_network_on_a_new_record` owns that rule over the operations
//!   that share it).
//! * a device or link INSIDE a global network that does resolve, naming nothing stored
//!   there, is the missing-resource answer:
//!   404 `ResourceNotFoundException`, carrying the service's constant message and
//!   identifying the resource in `ResourceId` / `ResourceType`
//!   (`crate::resource_not_found_message` owns that rule, and this suite asserts
//!   `AssociateLink` through it). The references are resolved outside-in, so the global
//!   network in the path is the one reported when more than one of them is unresolvable.
//! * a pair that does resolve but does not sit at one site is a refusal about stored
//!   state rather than about an input member — "The device and link must be in the same
//!   global network and the same site" (`smithy_query query="doc" shape="AssociateLink"`)
//!   — so it is a 400 `ValidationException` with the `Other` reason and no `Fields` entry.
//!
//! The rows differ in WHICH reference is unresolvable and HOW it got that way (never
//! minted, deleted, or stored under another global network), and in which half of the
//! pair is at the wrong site (including a device at no site at all, `SiteId` being
//! optional on a device). Afterwards every refused participant is read back through
//! `GetLinkAssociations`: a refusal stores nothing, so each of them lists nothing, while
//! the one accepted pair is there.
//!
//! The harness is the one `global_network_lifecycle` defines, and the association
//! requests go through the helpers `link_association_lifecycle` shares.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_association_lifecycle::{associate, associate_ok, get_associations, pairs_of};
use crate::link_lifecycle::{create_link_ok, plain_link};
// The `ResourceType` values — the ARN resource types this service mints — live with the
// not-found message rule they travel with, in `crate::resource_not_found_message`.
use crate::resource_not_found_message::{assert_not_found_answer, DEVICE, LINK};

/// The `ValidationException$Reason` a refusal about stored state carries: no single input
/// member is at fault, so it is the modelled catch-all.
const REASON_OTHER: &str = "Other";

/// The `ValidationException$Reason` a refusal of one input MEMBER carries, and the field
/// name and prose this service reports for a global-network reference that resolves to
/// nothing (the spelling and text the recorded refusals answer, pinned over every operation
/// that shares them by `crate::unknown_global_network_on_a_new_record`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
const GLOBAL_NETWORK_NOT_FOUND: &str = "Global network not found.";

/// The route `AssociateLink` is bound to (`smithy_query query="operation"
/// operation="AssociateLink"`): POST on the global network's link-association collection.
fn associate_uri(network: &str) -> String {
    format!("/global-networks/{network}/link-associations")
}

/// One row of the not-found table: the request, and the resource its refusal must name.
struct MissingCase {
    case: &'static str,
    network: String,
    device: String,
    link: String,
    resource_type: &'static str,
    resource_id: String,
}

/// One row of the same-site table: a pair that both resolves and is refused.
struct SiteMismatchCase {
    case: &'static str,
    device: String,
    link: String,
}

#[tokio::test]
async fn associate_link_refuses_every_unresolvable_reference_and_stores_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("associate-link refusals").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;

    // A device and a link of this global network that are then DELETED, so their ids are
    // well-formed and resolve to nothing.
    let gone_device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let gone_link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    for (what, uri) in [
        ("DeleteDevice", format!("/global-networks/{network}/devices/{gone_device}")),
        ("DeleteLink", format!("/global-networks/{network}/links/{gone_link}")),
    ] {
        let (status, body) = emulator.delete(&uri).await;
        assert_eq!(
            status, 200,
            "{what}: expected 200 while preparing a deleted reference, got {status} with {body}"
        );
    }

    // An EMPTY global network that is then deleted: the path can still address it, and it
    // holds nothing.
    let gone_network = emulator.create_global_network("emptied").await;
    let (status, body) = emulator
        .delete(&format!("/global-networks/{gone_network}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200 while preparing a deleted global network, got {status} with {body}"
    );

    // A second global network holding its own sited device and link, to prove a resource
    // is only addressable through the global network that owns it.
    let other_network = emulator.create_global_network("foreign").await;
    let foreign_site = emulator.create_site(&other_network, "foreign-hq").await;
    let foreign_device =
        create_device_ok(&emulator, &other_network, json!({ "SiteId": foreign_site })).await;
    let foreign_link =
        create_link_ok(&emulator, &other_network, &foreign_site, &plain_link()).await;

    // Ids of the right shape that were never minted.
    let unknown_network = "global-network-0000000000000beef".to_string();
    let unknown_device = "device-0000000000000beef".to_string();
    let unknown_link = "link-0000000000000beef".to_string();

    let rows = vec![
        MissingCase {
            case: "a device that was never minted",
            network: network.clone(),
            device: unknown_device.clone(),
            link: link.clone(),
            resource_type: DEVICE,
            resource_id: unknown_device.clone(),
        },
        MissingCase {
            case: "a device that was deleted",
            network: network.clone(),
            device: gone_device.clone(),
            link: link.clone(),
            resource_type: DEVICE,
            resource_id: gone_device.clone(),
        },
        MissingCase {
            case: "a device stored under another global network",
            network: network.clone(),
            device: foreign_device.clone(),
            link: link.clone(),
            resource_type: DEVICE,
            resource_id: foreign_device.clone(),
        },
        MissingCase {
            case: "a link that was never minted",
            network: network.clone(),
            device: device.clone(),
            link: unknown_link.clone(),
            resource_type: LINK,
            resource_id: unknown_link.clone(),
        },
        MissingCase {
            case: "a link that was deleted",
            network: network.clone(),
            device: device.clone(),
            link: gone_link.clone(),
            resource_type: LINK,
            resource_id: gone_link.clone(),
        },
        MissingCase {
            case: "a link stored under another global network",
            network: network.clone(),
            device: device.clone(),
            link: foreign_link.clone(),
            resource_type: LINK,
            resource_id: foreign_link.clone(),
        },
        // The outside-in order: with both resources of the pair unresolvable, the one
        // reported is the device.
        MissingCase {
            case: "neither resource of the pair resolving",
            network: network.clone(),
            device: unknown_device.clone(),
            link: unknown_link.clone(),
            resource_type: DEVICE,
            resource_id: unknown_device.clone(),
        },
    ];

    for row in &rows {
        assert_not_found_answer(
            &emulator,
            row.case,
            "POST",
            &associate_uri(&row.network),
            Some(json!({ "DeviceId": row.device, "LinkId": row.link })),
            &row.resource_id,
            row.resource_type,
        )
        .await;
    }

    // The global network the path addresses is a different refusal: the association is a
    // NEW record stored under it, so that parent reference is validated before anything is
    // stored and an id naming no stored global network is the field failure — whether it
    // was never minted or minted and deleted, and whatever else the request names.
    for (case, network, device_named, link_named) in [
        (
            "a global network that was never minted",
            unknown_network.clone(),
            device.clone(),
            link.clone(),
        ),
        (
            "a global network that was deleted",
            gone_network.clone(),
            device.clone(),
            link.clone(),
        ),
        (
            "no reference resolving at all",
            unknown_network.clone(),
            unknown_device.clone(),
            unknown_link.clone(),
        ),
    ] {
        let (status, refused) = associate(&emulator, &network, &device_named, &link_named).await;
        assert_eq!(
            status, 400,
            "case {case:?}: AssociateLink expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Reason"].as_str(),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {refused}"
        );
        assert_eq!(
            refused["Fields"],
            json!([{ "Name": FIELD_GLOBAL_NETWORK_ID, "Message": GLOBAL_NETWORK_NOT_FOUND }]),
            "case {case:?}: expected one Fields entry naming {FIELD_GLOBAL_NETWORK_ID}, got {refused}"
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(GLOBAL_NETWORK_NOT_FOUND),
            "case {case:?}: expected Message {GLOBAL_NETWORK_NOT_FOUND:?}, got {refused}"
        );
    }

    // Not one of those requests stored anything: the only resolvable pair among them is
    // still unassociated, so every selector reads an empty page — and so does the other
    // global network's own pair, which the rows above named but never tied.
    for (case, network_read, selector) in [
        ("the device the rows named", &network, format!("deviceId={device}")),
        ("the link the rows named", &network, format!("linkId={link}")),
        (
            "the device of the other global network",
            &other_network,
            format!("deviceId={foreign_device}"),
        ),
        (
            "the link of the other global network",
            &other_network,
            format!("linkId={foreign_link}"),
        ),
    ] {
        let (status, listed) = get_associations(&emulator, network_read, &selector).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLinkAssociations expected 200, got {status} with {listed}"
        );
        let actual = pairs_of(&listed);
        assert!(
            actual.is_empty(),
            "case {case:?}: a refused AssociateLink stores nothing, so {selector} must list no association, got {actual:?}"
        );
    }

    // The same request against references that all resolve is accepted, and the
    // association is readable back through `GetLinkAssociations`.
    let created = associate_ok(&emulator, &network, &device, &link).await;
    assert_eq!(
        created["GlobalNetworkId"],
        json!(network),
        "the accepted association must name the global network of the path, got {created}"
    );
    let (_, listed) = get_associations(&emulator, &network, &format!("linkId={link}")).await;
    assert_eq!(
        pairs_of(&listed),
        vec![(device.clone(), link.clone())],
        "the accepted pair must be the one association stored, got {listed}"
    );
}

#[tokio::test]
async fn associate_link_refuses_a_pair_that_does_not_sit_at_one_site_with_the_other_reason() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("one-site rule").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_site = emulator.create_site(&network, "branch").await;

    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let device_elsewhere =
        create_device_ok(&emulator, &network, json!({ "SiteId": other_site })).await;
    let link_elsewhere = create_link_ok(&emulator, &network, &other_site, &plain_link()).await;
    // `SiteId` is optional on a device (`smithy_query query="operation"
    // operation="CreateDevice"`), so a device that sits at no site exists too — and sits
    // at the site of no link.
    let unsited_device = create_device_ok(&emulator, &network, json!({})).await;

    let rows = vec![
        SiteMismatchCase {
            case: "a link at a site the device does not sit at",
            device: device.clone(),
            link: link_elsewhere.clone(),
        },
        SiteMismatchCase {
            case: "a device at a site the link does not sit at",
            device: device_elsewhere.clone(),
            link: link.clone(),
        },
        SiteMismatchCase {
            case: "a device at no site and a link at one",
            device: unsited_device.clone(),
            link: link.clone(),
        },
        SiteMismatchCase {
            case: "a device at no site and a link at the other site",
            device: unsited_device.clone(),
            link: link_elsewhere.clone(),
        },
    ];

    for row in &rows {
        let SiteMismatchCase { case, device, link } = row;
        let (status, body) = associate(&emulator, &network, device, link).await;
        assert_eq!(
            status, 400,
            "case {case:?}: AssociateLink expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some(REASON_OTHER),
            "case {case:?}: no input member is at fault, so the reason is {REASON_OTHER}, got {body}"
        );
        assert_eq!(
            body["Fields"],
            Value::Null,
            "case {case:?}: a refusal about stored state names no field, got {body}"
        );
        // The model declares `Message` required on the shape
        // (`smithy_query shape="ValidationException"`).
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must carry a message, got {body}"
        );
    }

    // None of them stored anything, and the rule is not a blanket refusal: the device and
    // the link of ONE site associate, and that is the only pair the store holds.
    for row in &rows {
        let (_, listed) =
            get_associations(&emulator, &network, &format!("deviceId={}", row.device)).await;
        let actual = pairs_of(&listed);
        assert!(
            actual.is_empty(),
            "case {:?}: a refused AssociateLink stores nothing, got {actual:?}",
            row.case
        );
    }
    associate_ok(&emulator, &network, &device, &link).await;
    let (_, listed) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        pairs_of(&listed),
        vec![(device.clone(), link.clone())],
        "the pair of one site must be the one association stored, got {listed}"
    );
    let (_, listed) =
        get_associations(&emulator, &network, &format!("linkId={link_elsewhere}")).await;
    assert_eq!(
        pairs_of(&listed),
        Vec::<(String, String)>::new(),
        "the link at the other site must still be associated with nothing, got {listed}"
    );
}
