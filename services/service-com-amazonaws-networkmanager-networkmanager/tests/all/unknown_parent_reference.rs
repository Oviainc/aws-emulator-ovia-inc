// SPDX-License-Identifier: CC0-1.0

//! The rule for a CREATE whose input references a parent resource that does not exist,
//! over every create of this service and every reference member each one carries.
//!
//! A create validates the references its input carries before anything is minted, so a
//! reference that resolves to no stored resource is a FIELD-validation failure on the
//! member that carried it, not a missing resource: HTTP 400 with
//! `x-amzn-errortype: ValidationException`, `Reason` = `FieldValidationFailed` and one
//! `Fields` entry whose `Name` is that member (the service's own lowerCamelCase field
//! name) and whose `Message` is the top-level `Message` — "<Kind> not found." for the kind
//! the member references. Every other store under a global network — an association, a
//! registration, an analysis — refuses its parent reference the same way, which
//! `crate::unknown_global_network_on_a_new_record` pins over that family.
//! An operation that acts on an EXISTING resource keeps answering
//! the absent resource with `ResourceNotFoundException` (404); that half of the rule is
//! pinned by `resource_not_found_message`.
//!
//! The rows differ along the create, the reference member, the kind it references and the
//! form of the reference that resolves to nothing (never minted, deleted, not a Network
//! Manager id at all, both references bad at once, and a site that exists but under a
//! different global network).
//!
//! Which member a create reports is its IMMEDIATE parent reference: `CreateLink` creates
//! "a new link for a specified site" and addresses that site inside a global network, so
//! a chain that does not resolve is reported on `siteId` even when the global network is
//! unknown too (the recorded case), while `CreateSite`, `CreateDevice` and
//! `CreateCoreNetwork` hang straight off the global network and report `globalNetworkId`.
//! `CreateDevice`'s `SiteId` is an OPTIONAL extra reference, so it is reported on `siteId`
//! only once the global network it is addressed inside resolves. An attachment create
//! hangs off the CORE network, so it reports `coreNetworkId`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, ids_of, Emulator};

/// The field names the refusals report, from the recordings of `CreateSite` under an
/// unknown global network (`globalNetworkId`) and `CreateLink` with an unknown site
/// (`siteId`).
const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`FIELD_GLOBAL_NETWORK_ID`].
const FIELD_SITE_ID: &str = "siteId";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same `<kind>Id` spelling for the core network an
/// attachment create addresses.
const FIELD_CORE_NETWORK_ID: &str = "coreNetworkId";

/// The prose each kind's refusal carries, as recorded.
const GLOBAL_NETWORK_NOT_FOUND: &str = "Global network not found.";
/// See [`GLOBAL_NETWORK_NOT_FOUND`].
const SITE_NOT_FOUND: &str = "Site not found.";
/// See [`GLOBAL_NETWORK_NOT_FOUND`]: the same "<Kind> not found." prose for the core
/// network kind.
const CORE_NETWORK_NOT_FOUND: &str = "Core network not found.";

/// A VPN connection ARN of the modelled form
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`,
/// `smithy_query shape=VpnConnectionArn`), so a `CreateSiteToSiteVpnAttachment` row is
/// refused on its core-network reference rather than by the schema.
const VPN_CONNECTION_ARN: &str = "arn:aws:ec2:us-west-2:111122223333:vpn-connection/vpn-0000abcd";

/// One row: a create whose input carries a reference that names nothing, and the member
/// and prose the refusal has to report.
struct BadReference {
    case: &'static str,
    uri: String,
    body: Value,
    field: &'static str,
    message: &'static str,
}

/// A `CreateLink` body naming `site_id`; `Bandwidth` is modelled required.
fn link_body(site_id: &str) -> Value {
    json!({
        "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 },
        "SiteId": site_id,
    })
}

/// Assert the refusal `row` produces: 400, `ValidationException`, the reason, and the one
/// field entry naming the offending member with the same message as the envelope.
async fn assert_field_validation_failed(emulator: &Emulator, row: &BadReference) {
    let BadReference {
        case,
        uri,
        body,
        field,
        message,
    } = row;
    let (status, answer) = emulator.post(uri, body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case}: expected ValidationException, got {answer}"
    );
    assert_eq!(
        answer["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case {case}: expected Reason FieldValidationFailed, got {answer}"
    );
    assert_eq!(
        answer["Message"].as_str(),
        Some(*message),
        "case {case}: expected Message {message:?}, got {answer}"
    );
    assert_eq!(
        answer["Fields"],
        json!([{ "Name": field, "Message": message }]),
        "case {case}: expected one Fields entry naming {field:?}, got {answer}"
    );
}

#[tokio::test]
async fn every_create_refuses_a_reference_that_names_nothing_as_a_field_validation_failure() {
    let emulator = Emulator::new();
    let live_network = emulator.create_global_network("kept").await;
    let live_site = emulator.create_site(&live_network, "hq").await;
    let other_network = emulator.create_global_network("other").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    // A global network created and then deleted: a well-formed id that named a record and
    // names none now.
    let gone_network = emulator.create_global_network("to be deleted").await;
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{gone_network}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {deleted}"
    );
    // A site created and then deleted, for the link rows.
    let gone_site = emulator.create_site(&live_network, "to be deleted").await;
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{live_network}/sites/{gone_site}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {deleted}"
    );

    // Ids of the right shape that were never minted, and a string that is not a Network
    // Manager id at all.
    let unknown_network = "global-network-00000000000000099";
    let unknown_site = "site-00000000000000099";
    let not_an_id = "nope";

    let rows = vec![
        BadReference {
            case: "CreateSite under a global network that never existed",
            uri: format!("/global-networks/{unknown_network}/sites"),
            body: json!({ "Description": "orphan" }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateSite under a deleted global network",
            uri: format!("/global-networks/{gone_network}/sites"),
            body: json!({ "Description": "orphan" }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateSite under an id that is not a Network Manager id",
            uri: format!("/global-networks/{not_an_id}/sites"),
            body: json!({}),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            // The link's immediate parent is the site, so the reference that fails is the
            // `SiteId`: a live site of another global network does not resolve inside a
            // global network that never existed either.
            case: "CreateLink under a global network that never existed",
            uri: format!("/global-networks/{unknown_network}/links"),
            body: link_body(&live_site),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateLink with both references naming nothing",
            uri: format!("/global-networks/{unknown_network}/links"),
            body: link_body(unknown_site),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateLink on a site that never existed",
            uri: format!("/global-networks/{live_network}/links"),
            body: link_body(unknown_site),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateLink on a deleted site",
            uri: format!("/global-networks/{live_network}/links"),
            body: link_body(&gone_site),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateLink on a site id that is not a Network Manager id",
            uri: format!("/global-networks/{live_network}/links"),
            body: link_body(not_an_id),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateLink on a site of another global network",
            uri: format!("/global-networks/{live_network}/links"),
            body: link_body(&other_site),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice under a global network that never existed",
            uri: format!("/global-networks/{unknown_network}/devices"),
            body: json!({ "Description": "orphan" }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice under a deleted global network",
            uri: format!("/global-networks/{gone_network}/devices"),
            body: json!({}),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            // The device hangs off the global network, so an unknown global network is
            // reported there even when the `SiteId` the body carries is live.
            case: "CreateDevice under an unknown global network naming a live site",
            uri: format!("/global-networks/{unknown_network}/devices"),
            body: json!({ "SiteId": live_site.clone() }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice naming a site that never existed",
            uri: format!("/global-networks/{live_network}/devices"),
            body: json!({ "SiteId": unknown_site }),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice naming a deleted site",
            uri: format!("/global-networks/{live_network}/devices"),
            body: json!({ "SiteId": gone_site.clone() }),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice naming a site id that is not a Network Manager id",
            uri: format!("/global-networks/{live_network}/devices"),
            body: json!({ "SiteId": not_an_id }),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateDevice naming a site of another global network",
            uri: format!("/global-networks/{live_network}/devices"),
            body: json!({ "SiteId": other_site.clone() }),
            field: FIELD_SITE_ID,
            message: SITE_NOT_FOUND,
        },
        BadReference {
            case: "CreateCoreNetwork under a global network that never existed",
            uri: "/core-networks".to_string(),
            body: json!({ "GlobalNetworkId": unknown_network }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateCoreNetwork under a deleted global network",
            uri: "/core-networks".to_string(),
            body: json!({ "GlobalNetworkId": gone_network.clone() }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            case: "CreateCoreNetwork under an id that is not a Network Manager id",
            uri: "/core-networks".to_string(),
            body: json!({ "GlobalNetworkId": not_an_id }),
            field: FIELD_GLOBAL_NETWORK_ID,
            message: GLOBAL_NETWORK_NOT_FOUND,
        },
        BadReference {
            // An attachment hangs off the CORE network, so its reference member and the
            // kind its prose names are the core network's. Only the ids that satisfy the
            // modelled `CoreNetworkId` pattern are rows: this reference travels in the
            // BODY, so a string outside `^core-network-([0-9a-f]{8,17})$` is refused by
            // the schema before the handler sees it.
            case: "CreateSiteToSiteVpnAttachment on a core network that never existed",
            uri: "/site-to-site-vpn-attachments".to_string(),
            body: json!({
                "CoreNetworkId": "core-network-00000000000000099",
                "VpnConnectionArn": VPN_CONNECTION_ARN,
            }),
            field: FIELD_CORE_NETWORK_ID,
            message: CORE_NETWORK_NOT_FOUND,
        },
    ];

    for row in &rows {
        assert_field_validation_failed(&emulator, row).await;
    }

    // A refused create stores nothing: the live global network still holds only the one
    // site that was created here (the deleted one is gone) and no link at all.
    let (status, sites) = emulator
        .get(&format!("/global-networks/{live_network}/sites"))
        .await;
    assert_eq!(
        status, 200,
        "GetSites: expected 200, got {status} with {sites}"
    );
    assert_eq!(
        ids_of(&sites, "Sites", "SiteId"),
        vec![live_site.clone()],
        "a refused create must not have stored a site, got {sites}"
    );
    let (status, links) = emulator
        .get(&format!("/global-networks/{live_network}/links"))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks: expected 200, got {status} with {links}"
    );
    assert_eq!(
        ids_of(&links, "Links", "LinkId"),
        Vec::<String>::new(),
        "a refused create must not have stored a link, got {links}"
    );
    let (status, devices) = emulator
        .get(&format!("/global-networks/{live_network}/devices"))
        .await;
    assert_eq!(
        status, 200,
        "GetDevices: expected 200, got {status} with {devices}"
    );
    assert_eq!(
        ids_of(&devices, "Devices", "DeviceId"),
        Vec::<String>::new(),
        "a refused create must not have stored a device, got {devices}"
    );

    // And the references that DO resolve are still accepted, so the refusals above are the
    // reference check and nothing wider.
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{live_network}/links"),
            link_body(&live_site),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateLink with both references live: expected 200, got {status} with {created}"
    );
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{live_network}/devices"),
            json!({ "SiteId": live_site.clone() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateDevice with both references live: expected 200, got {status} with {created}"
    );
    let (status, created) = emulator
        .post(
            "/core-networks",
            json!({ "GlobalNetworkId": other_network.clone() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork with a live global network: expected 200, got {status} with {created}"
    );
}
