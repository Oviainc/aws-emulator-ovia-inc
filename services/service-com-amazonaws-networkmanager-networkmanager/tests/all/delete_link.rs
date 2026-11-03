// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteLink`, driven over HTTP.
//!
//! The rule under test: a delete of a link removes the stored record the path addresses
//! and answers THAT record with `DELETING` for its state, so a later read no longer finds
//! it and a sibling is untouched; a link that is not reachable through the global network
//! in the path — unknown, already deleted, or stored under another global network — is the
//! not-found refusal (404), and "You must first disassociate the link from any devices and
//! customer gateways" (`smithy_query query="doc" shape="DeleteLink"`) makes a link that is
//! still named by an association of either kind the dependency `ValidationException` (400)
//! every delete of this service raises, until the associations are gone.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`: a link is only ever read back through `GetLinks`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::connection_lifecycle::without_state;
use crate::customer_gateway_association_lifecycle::register_vpn_carrier;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;

/// The `ResourceType` a not-found refusal reports for each kind this suite addresses: the
/// ARN resource type this service mints.
const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
const LINK: &str = "link";

/// How the dependency refusal spells each blocking kind: the resource kind read as words.
const LINK_ASSOCIATION_LABEL: &str = "link association";
/// See [`LINK_ASSOCIATION_LABEL`].
const GATEWAY_ASSOCIATION_LABEL: &str = "customer gateway association";

/// `DeleteLink` for `link` of `network`.
async fn delete_link(emulator: &Emulator, network: &str, link: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/links/{link}"))
        .await
}

/// The `LinkId`s `GetLinks` lists for `network`.
async fn link_ids(emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/links?maxResults=500"))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks({network}): expected 200, got {status} with {body}"
    );
    ids_of(&body, "Links", "LinkId")
}

/// The one `Link` `GetLinks` answers for `link`, or `None` once it is gone.
async fn read_link(emulator: &Emulator, network: &str, link: &str) -> Option<Value> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/links?linkIds={link}"))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks({link}): expected 200, got {status} with {body}"
    );
    body["Links"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Links array in {body}"))
        .first()
        .cloned()
}

/// `AssociateLink` tying `device` to `link` of `network`, which must succeed.
async fn associate_link_ok(emulator: &Emulator, network: &str, device: &str, link: &str) {
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/link-associations"),
            json!({ "DeviceId": device, "LinkId": link }),
        )
        .await;
    assert_eq!(
        status, 200,
        "AssociateLink({device}, {link}): expected 200, got {status} with {body}"
    );
}

/// `DisassociateLink` for the `device`/`link` pair of `network`.
async fn disassociate_link_ok(emulator: &Emulator, network: &str, device: &str, link: &str) {
    let (status, body) = emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DisassociateLink({device}, {link}): expected 200, got {status} with {body}"
    );
}

/// A customer gateway ARN naming the gateway id `cgw-{hex}`: a customer gateway is an
/// Amazon VPC resource, so the form is EC2's, as `customer_gateway_association_lifecycle`
/// builds it.
fn cgw_arn(hex: &str) -> String {
    format!("arn:aws:ec2:{CUSTOMER_GATEWAY_REGION}:123456789012:customer-gateway/cgw-{hex}")
}

/// The Region the gateways above live in, which is therefore the Region their carrier is
/// registered in ([`register_vpn_carrier`]).
const CUSTOMER_GATEWAY_REGION: &str = "us-west-2";

/// `AssociateCustomerGateway` tying `arn` to `device` of `network` and, when `link` is
/// given, to that link of the device. Must succeed.
///
/// A customer gateway is associated only where the global network carries, in the gateway's
/// Region, a transit gateway or core network edge its VPN attachment could be on, so the
/// carrier for the Region [`cgw_arn`] names is registered first.
async fn associate_gateway_ok(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: Option<&str>,
    arn: &str,
) {
    register_vpn_carrier(emulator, network, CUSTOMER_GATEWAY_REGION).await;
    let mut body = json!({ "CustomerGatewayArn": arn, "DeviceId": device });
    if let Some(link) = link {
        body["LinkId"] = json!(link);
    }
    let (status, answer) = emulator
        .post(
            &format!("/global-networks/{network}/customer-gateway-associations"),
            body,
        )
        .await;
    assert_eq!(
        status, 200,
        "AssociateCustomerGateway({arn}): expected 200, got {status} with {answer}"
    );
}

/// `DisassociateCustomerGateway` for `arn` on `network`. The ARN travels in a non-greedy
/// `@httpLabel`, so its `:` and `/` go percent-encoded.
async fn disassociate_gateway_ok(emulator: &Emulator, network: &str, arn: &str) {
    let (status, body) = emulator
        .delete(&format!(
            "/global-networks/{network}/customer-gateway-associations/{}",
            escape(arn)
        ))
        .await;
    assert_eq!(
        status, 200,
        "DisassociateCustomerGateway({arn}): expected 200, got {status} with {body}"
    );
}

/// One row of the delete table: the `CreateLink` body, whether the row waits out the
/// settle span the create owes before deleting, and how many siblings the delete must
/// leave alone.
struct DeleteCase {
    name: &'static str,
    /// Members beside `SiteId` the create carries.
    body: Value,
    /// Whether the row sleeps past the `PENDING` -> `AVAILABLE` transition first, so the
    /// delete is exercised on a link in either lifecycle state.
    settled: bool,
    /// How many other links the global network holds, which the delete must leave alone.
    siblings: usize,
}

/// The rows differ along the form of `Bandwidth` (both speeds, one speed, neither), along
/// the size and character set of each free-form descriptor, along the tags the record
/// carries, along the lifecycle state the link is in when the delete arrives, and along how
/// many siblings the global network holds — everything the removal rule generalises over.
/// 256 is the modelled maximum of `Description` (`ConstrainedString`, len 0..256) while 128
/// is the documented maximum of `Type` and `Provider` (`smithy_query query="doc"
/// shape="CreateLinkRequest" member="Provider"`).
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "a bare link, deleted while still PENDING",
            body: json!({ "Bandwidth": {} }),
            settled: false,
            siblings: 0,
        },
        DeleteCase {
            name: "both speeds and every descriptor, deleted once AVAILABLE",
            body: json!({
                "Bandwidth": { "UploadSpeed": 100, "DownloadSpeed": 200 },
                "Description": "primary uplink",
                "Type": "broadband",
                "Provider": "AnyCompany Networks",
            }),
            settled: true,
            siblings: 2,
        },
        DeleteCase {
            name: "one speed and tags, beside one sibling",
            body: json!({
                "Bandwidth": { "UploadSpeed": 50 },
                "Tags": [{ "Key": "env", "Value": "test" }, { "Key": "tier", "Value": "primary" }],
            }),
            settled: false,
            siblings: 1,
        },
        DeleteCase {
            name: "descriptors at their published maximum length, beside three siblings",
            body: json!({
                "Bandwidth": { "UploadSpeed": i32::MAX, "DownloadSpeed": i32::MAX },
                "Description": "d".repeat(256),
                "Type": "t".repeat(128),
                "Provider": "p".repeat(128),
            }),
            settled: true,
            siblings: 3,
        },
        DeleteCase {
            name: "multi-line and non-ascii descriptors with zero speeds",
            body: json!({
                "Bandwidth": { "UploadSpeed": 0, "DownloadSpeed": 0 },
                "Description": "line one\nline two",
                "Type": "fibre \u{1f310}",
                "Provider": "Ünternet",
                "Tags": [{ "Key": "ключ", "Value": "значение" }],
            }),
            settled: false,
            siblings: 0,
        },
    ]
}

#[tokio::test]
async fn delete_link_removes_the_stored_record_it_reports_for_every_form_of_link() {
    for case in delete_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;

        let link = create_link_ok(&emulator, &network, &site, &case.body).await;
        let mut siblings: Vec<String> = Vec::new();
        for _ in 0..case.siblings {
            siblings.push(create_link_ok(&emulator, &network, &site, &plain_link()).await);
        }
        siblings.sort();
        if case.settled {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // What the reader answers for the link before the delete: the delete must report
        // THIS record rather than a shape built from the request.
        let stored = read_link(&emulator, &network, &link)
            .await
            .unwrap_or_else(|| panic!("case {name:?}: GetLinks did not list the created link"));

        let (status, deleted) = delete_link(&emulator, &network, &link).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteLink expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            without_state(&deleted["Link"]),
            without_state(&stored),
            "case {name:?}: the delete must answer the stored record, got {deleted} for stored {stored}"
        );
        assert_eq!(
            deleted["Link"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );
        // The whole-record comparison above already covers the ARN's text; what the delete
        // must not do is answer an ARN that names some other link, whatever form the
        // service mints ARNs in.
        let arn = deleted["Link"]["LinkArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no LinkArn in {deleted}"));
        assert!(
            arn.ends_with(&link),
            "case {name:?}: the removed record's ARN must name the link {link}, got {arn:?}"
        );

        // The removal is in stored state: the reader no longer finds it, every sibling is
        // still listed, and a second delete of the same id is the not-found refusal.
        assert!(
            read_link(&emulator, &network, &link).await.is_none(),
            "case {name:?}: GetLinks still lists the deleted link {link}"
        );
        let listed = link_ids(&emulator, &network).await;
        assert_eq!(
            listed, siblings,
            "case {name:?}: expected only the siblings {siblings:?} to remain, got {listed:?}"
        );
        let (status, again) = delete_link(&emulator, &network, &link).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a second DeleteLink expected 404, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {again}"
        );
    }
}

/// One row of the dependency table: how many devices the link is associated with, whether a
/// customer gateway association names the link, whether one names only a device (which must
/// NOT block), whether a sibling link carries its own association (which must not block
/// either), and the kinds the refusal must then name.
struct AssociationCase {
    name: &'static str,
    /// Devices this link is associated with, through `AssociateLink`.
    devices: usize,
    /// Whether a customer gateway association names this link as well as its device.
    gateway_on_link: bool,
    /// Whether a customer gateway association names a device but NO link.
    gateway_without_link: bool,
    /// Whether a sibling link of the same site carries its own link association.
    sibling_associated: bool,
    /// The dependent kinds the refusal must name, as it spells them, in no particular
    /// order; empty when the delete has to succeed.
    blocking: &'static [&'static str],
}

/// "You must first disassociate the link from any devices and customer gateways."
/// (`smithy_query query="doc" shape="DeleteLink"`) — the rows differ along WHICH of the two
/// kinds of association names the link and HOW MANY of each, with control rows holding
/// neither, holding a gateway association that names no link, and holding associations that
/// belong to a sibling link.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            name: "no association at all",
            devices: 0,
            gateway_on_link: false,
            gateway_without_link: false,
            sibling_associated: false,
            blocking: &[],
        },
        AssociationCase {
            name: "one device",
            devices: 1,
            gateway_on_link: false,
            gateway_without_link: false,
            sibling_associated: false,
            blocking: &[LINK_ASSOCIATION_LABEL],
        },
        AssociationCase {
            name: "four devices",
            devices: 4,
            gateway_on_link: false,
            gateway_without_link: false,
            sibling_associated: false,
            blocking: &[LINK_ASSOCIATION_LABEL],
        },
        AssociationCase {
            name: "one device and a customer gateway over the link",
            devices: 1,
            gateway_on_link: true,
            gateway_without_link: false,
            sibling_associated: false,
            blocking: &[LINK_ASSOCIATION_LABEL, GATEWAY_ASSOCIATION_LABEL],
        },
        AssociationCase {
            name: "a customer gateway that names a device but no link",
            devices: 0,
            gateway_on_link: false,
            gateway_without_link: true,
            sibling_associated: false,
            blocking: &[],
        },
        AssociationCase {
            name: "only a sibling link is associated",
            devices: 0,
            gateway_on_link: false,
            gateway_without_link: false,
            sibling_associated: true,
            blocking: &[],
        },
        AssociationCase {
            name: "two devices beside an associated sibling and a link-less gateway",
            devices: 2,
            gateway_on_link: false,
            gateway_without_link: true,
            sibling_associated: true,
            blocking: &[LINK_ASSOCIATION_LABEL],
        },
    ]
}

#[tokio::test]
async fn a_link_still_associated_with_a_device_or_a_gateway_is_refused_until_it_is_disassociated()
{
    for case in association_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;
        let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;

        let mut devices: Vec<String> = Vec::new();
        for _ in 0..case.devices {
            let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
            associate_link_ok(&emulator, &network, &device, &link).await;
            devices.push(device);
        }

        // A customer gateway can only be associated with a link that is already associated
        // with its device, so the row that puts one on the link uses the first device.
        let gateway_on_link = if case.gateway_on_link {
            let device = devices
                .first()
                .unwrap_or_else(|| panic!("case {name:?}: a gateway on the link needs a device"));
            let arn = cgw_arn("0123456789abcde01");
            associate_gateway_ok(&emulator, &network, device, Some(&link), &arn).await;
            Some((device.clone(), arn))
        } else {
            None
        };
        if case.gateway_without_link {
            let device = create_device_ok(&emulator, &network, json!({})).await;
            associate_gateway_ok(&emulator, &network, &device, None, &cgw_arn("0123456789abcde02"))
                .await;
        }
        let sibling = if case.sibling_associated {
            let sibling = create_link_ok(&emulator, &network, &site, &plain_link()).await;
            let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
            associate_link_ok(&emulator, &network, &device, &sibling).await;
            Some(sibling)
        } else {
            None
        };

        let (status, body) = delete_link(&emulator, &network, &link).await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a link no association names deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Link"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            if let Some(sibling) = &sibling {
                assert!(
                    read_link(&emulator, &network, sibling).await.is_some(),
                    "case {name:?}: the delete must leave the sibling link {sibling} alone"
                );
            }
            continue;
        }

        assert_eq!(
            status, 400,
            "case {name:?}: a dependency refusal is HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"],
            json!("Other"),
            "case {name:?}: no input field is at fault, so the reason is the modelled Other, got {body}"
        );
        assert_eq!(
            body["Fields"],
            Value::Null,
            "case {name:?}: a dependency refusal blames no field, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no Message in {body}"));
        let expected = crate::delete_live_reference_refusal::refusal_message("Link", &link);
        assert_eq!(
            message, expected,
            "case {name:?}: expected the message {expected:?}, got {message:?}"
        );
        // The refusal names the resource being deleted and NOT which kind of record still
        // names it, whichever kinds those are.
        for label in [LINK_ASSOCIATION_LABEL, GATEWAY_ASSOCIATION_LABEL] {
            assert!(
                !message.contains(label),
                "case {name:?}: the refusal must not name a {label}, got {message:?}"
            );
        }

        // The refusal changed nothing: the link is still readable, with the state the clock
        // gives it rather than DELETING.
        let still_there = read_link(&emulator, &network, &link)
            .await
            .unwrap_or_else(|| panic!("case {name:?}: a refused delete removed the link"));
        assert_ne!(
            still_there["State"],
            json!("DELETING"),
            "case {name:?}: a refused delete must leave the link's own state alone, got {still_there}"
        );

        // Removing all but the last association still blocks: the refusal tracks what is
        // left, not how many there once were. The gateway association goes first, since a
        // link stays associated with its device until the gateway over it is gone.
        if let Some((device, arn)) = &gateway_on_link {
            disassociate_gateway_ok(&emulator, &network, arn).await;
            let (status, blocked) = delete_link(&emulator, &network, &link).await;
            assert_eq!(
                status, 400,
                "case {name:?}: the device {device} still blocks the delete, got {status} with {blocked}"
            );
        }
        let (last, rest) = devices
            .split_last()
            .unwrap_or_else(|| panic!("case {name:?}: expected at least one association"));
        for device in rest {
            disassociate_link_ok(&emulator, &network, device, &link).await;
            let (status, blocked) = delete_link(&emulator, &network, &link).await;
            assert_eq!(
                status, 400,
                "case {name:?}: one association still blocks the delete, got {status} with {blocked}"
            );
        }

        // With the last one gone the identical call succeeds, so the refusal tracked stored
        // state rather than the shape of the request.
        disassociate_link_ok(&emulator, &network, last, &link).await;
        let (status, deleted) = delete_link(&emulator, &network, &link).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with the associations gone the same delete must succeed, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["Link"]["LinkId"],
            json!(link.clone()),
            "case {name:?}: the delete must answer the link it removed, got {deleted}"
        );
        assert!(
            read_link(&emulator, &network, &link).await.is_none(),
            "case {name:?}: GetLinks still lists the deleted link"
        );
    }
}

/// One row of the not-found table: the ids the request addresses, and the resource the
/// refusal has to identify.
struct NotFoundCase {
    name: &'static str,
    /// Built from the ids the test mints, so each row is a closure over them.
    network: String,
    link: String,
    resource_id: String,
    resource_type: &'static str,
}

#[tokio::test]
async fn a_link_the_path_does_not_reach_is_the_not_found_refusal() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("links").await;
    let site = emulator.create_site(&network, "hq").await;
    let other = emulator.create_global_network("other").await;
    let other_site = emulator.create_site(&other, "branch").await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let elsewhere = create_link_ok(&emulator, &other, &other_site, &plain_link()).await;

    // A link that was created and then deleted, so its id is well-formed and names nothing.
    let (status, deleted) = delete_link(&emulator, &network, &link).await;
    assert_eq!(
        status, 200,
        "DeleteLink expected 200, got {status} with {deleted}"
    );

    // Ids that were never minted, in the form this service mints.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_link = "link-00000000000000099".to_string();

    // The rows differ along WHICH half of the path is unreachable: the global network, the
    // link, or the pairing of the two (a link of another global network).
    let rows = vec![
        NotFoundCase {
            name: "a global network that never existed",
            network: unknown_network.clone(),
            link: unknown_link.clone(),
            resource_id: unknown_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            name: "a link that never existed",
            network: network.clone(),
            link: unknown_link.clone(),
            resource_id: unknown_link.clone(),
            resource_type: LINK,
        },
        NotFoundCase {
            name: "a link that was already deleted",
            network: network.clone(),
            link: link.clone(),
            resource_id: link.clone(),
            resource_type: LINK,
        },
        NotFoundCase {
            name: "a link of another global network",
            network: network.clone(),
            link: elsewhere.clone(),
            resource_id: elsewhere.clone(),
            resource_type: LINK,
        },
    ];

    for row in &rows {
        let name = row.name;
        let (status, body) = delete_link(&emulator, &row.network, &row.link).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(row.resource_id.as_str()),
            "case {name:?}: expected ResourceId {:?}, got {body}",
            row.resource_id
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {name:?}: expected ResourceType {:?}, got {body}",
            row.resource_type
        );
    }

    // The refused rows left the link of the other global network alone, so it is still
    // deletable through ITS own path.
    let (status, deleted) = delete_link(&emulator, &other, &elsewhere).await;
    assert_eq!(
        status, 200,
        "the link of the other global network must still delete, got {status} with {deleted}"
    );
}

#[tokio::test]
async fn deleting_the_links_unblocks_the_deletes_of_their_site_and_global_network() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("teardown").await;
    let site = emulator.create_site(&network, "hq").await;
    let first = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let second = create_link_ok(&emulator, &network, &site, &plain_link()).await;

    // While the site holds a link its delete is refused, and so is the global network's.
    let (status, blocked) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 400,
        "DeleteSite with a link at the site expected 400, got {status} with {blocked}"
    );
    let (status, blocked) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 400,
        "DeleteGlobalNetwork with links expected 400, got {status} with {blocked}"
    );

    for link in [&first, &second] {
        let (status, deleted) = delete_link(&emulator, &network, link).await;
        assert_eq!(
            status, 200,
            "DeleteLink({link}) expected 200, got {status} with {deleted}"
        );
    }
    assert!(
        link_ids(&emulator, &network).await.is_empty(),
        "every link of the global network was deleted, so GetLinks lists none"
    );

    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 200,
        "with the links gone DeleteSite must succeed, got {status} with {deleted}"
    );
    let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "with the site and links gone DeleteGlobalNetwork must succeed, got {status} with {deleted}"
    );
}
