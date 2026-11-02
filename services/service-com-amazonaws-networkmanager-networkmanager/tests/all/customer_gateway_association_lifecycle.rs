// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the customer-gateway-association slice
//! (`AssociateCustomerGateway`, `GetCustomerGatewayAssociations`,
//! `DisassociateCustomerGateway`), driven over HTTP.
//!
//! The rule under test: a customer gateway association is a STORED tie between one
//! customer gateway of Amazon VPC and one device — optionally one link of that device — of
//! one global network, identified by the global network plus the gateway's ARN. The
//! associate stores it `PENDING` and the clock settles it `AVAILABLE`; the reader answers
//! the stored ties of its global network and nothing else; the disassociate REMOVES the
//! stored tie, reports the removed one `DELETING`, and leaves a later read with nothing to
//! report, so a repeat of it is a not-found. The rule holds for every well-formed customer
//! gateway ARN, not only the one shape a recording happens to carry, so the tables below
//! span the ARN's partition, Region, account and id width, and both the association with a
//! link and the one without.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: an association is only ever read back through
//! `GetCustomerGatewayAssociations`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;

/// The documented maximum number of values the reader's ARN filter may name: "One or more
/// customer gateway Amazon Resource Names (ARNs). The maximum is 10." (`smithy_query
/// query="doc" shape="GetCustomerGatewayAssociationsRequest" member="CustomerGatewayArns"`).
const MAX_FILTER_ARNS: usize = 10;

/// A customer gateway ARN of `partition`, `region` and `account`, naming the gateway id
/// `cgw-{hex}`.
///
/// A customer gateway is an Amazon VPC resource, so the form is the EC2 one this model
/// spells out for the EC2 ARNs it DOES type:
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
/// (`smithy_query shape=VpnConnectionArn`), with the `customer-gateway` resource type and a
/// `cgw-` id.
fn cgw_arn(partition: &str, region: &str, account: &str, hex: &str) -> String {
    format!("arn:{partition}:ec2:{region}:{account}:customer-gateway/cgw-{hex}")
}

/// A customer gateway ARN in the shape most requests carry, distinguished by `hex`.
pub(crate) fn plain_cgw_arn(hex: &str) -> String {
    cgw_arn("aws", PLAIN_REGION, "123456789012", hex)
}

/// The Region [`plain_cgw_arn`] names, which is therefore the Region a carrier of such a
/// gateway has to be registered in or configure an edge at.
pub(crate) const PLAIN_REGION: &str = "us-west-2";

/// `AssociateCustomerGateway` on `network` with `body`.
pub(crate) async fn associate(emulator: &Emulator, network: &str, body: Value) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/customer-gateway-associations"),
            body,
        )
        .await
}

/// `DisassociateCustomerGateway` for `arn` on `network`. The ARN travels in a NON-greedy
/// `@httpLabel`, so a client sends its `:` and `/` percent-encoded.
pub(crate) async fn disassociate(emulator: &Emulator, network: &str, arn: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/customer-gateway-associations/{}",
            escape(arn)
        ))
        .await
}

/// `GetCustomerGatewayAssociations` on `network` with `query` appended to the URI.
pub(crate) async fn get_associations(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/customer-gateway-associations")
    } else {
        format!("/global-networks/{network}/customer-gateway-associations?{query}")
    };
    emulator.get(&uri).await
}

/// The `CustomerGatewayArn`s a reader page lists, in the order returned.
pub(crate) fn arns_of(body: &Value) -> Vec<String> {
    body["CustomerGatewayAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a CustomerGatewayAssociations array in {body}"))
        .iter()
        .map(|association| {
            association["CustomerGatewayArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected CustomerGatewayArn in {association}"))
                .to_string()
        })
        .collect()
}

/// The one association the reader answers for `arn`, or `None` when it answers none.
pub(crate) async fn read_one(emulator: &Emulator, network: &str, arn: &str) -> Option<Value> {
    let (status, body) = get_associations(
        emulator,
        network,
        &format!("customerGatewayArns={}", escape(arn)),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetCustomerGatewayAssociations({arn}): expected 200, got {status} with {body}"
    );
    let page = body["CustomerGatewayAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a CustomerGatewayAssociations array in {body}"))
        .clone();
    assert!(
        page.len() <= 1,
        "case read {arn}: one ARN selects at most one association, got {body}"
    );
    page.into_iter().next()
}

/// A device of `network`, with a link of a fresh site associated with it, so an associate
/// naming that link satisfies the documented precondition.
pub(crate) struct DeviceWithLink {
    site: String,
    pub(crate) device: String,
    pub(crate) link: String,
}

/// Associate `link` with `device` in `network`, which is what makes a customer gateway
/// association naming both acceptable.
async fn associate_link(emulator: &Emulator, network: &str, device: &str, link: &str) {
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

/// Register a transit gateway of `region` with `network`: the carrier a customer gateway of
/// that Region reaches the global network through, which is what makes an associate of such
/// a gateway acceptable at all ("You can only associate customer gateways that are connected
/// to a VPN attachment on a transit gateway or core network registered in your global
/// network", `smithy_query query="doc" shape="AssociateCustomerGateway"`).
///
/// A transit gateway is registered with at most one global network, so the gateway id is
/// derived from the global network's own minted id: one call per (global network, Region)
/// pair registers one gateway, and a repeat of the same call is the register's own
/// idempotent answer. The account is the emulator's, the only one `RegisterTransitGateway`
/// accepts.
pub(crate) async fn register_vpn_carrier(emulator: &Emulator, network: &str, region: &str) {
    let minted = network
        .rsplit_once('-')
        .map_or(network, |(_, suffix)| suffix);
    let gateway = format!(
        "arn:aws:ec2:{region}:{}:transit-gateway/tgw-{minted}",
        engine::emulator_account_id()
    );
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/transit-gateway-registrations"),
            json!({ "TransitGatewayArn": gateway }),
        )
        .await;
    assert_eq!(
        status, 200,
        "RegisterTransitGateway({gateway}): expected 200, got {status} with {body}"
    );
}

/// Create a site of `network`, a device at it and a link at it, associate the pair, and
/// register a transit gateway in the Region [`plain_cgw_arn`] names, so an associate of such
/// a gateway satisfies both documented preconditions.
pub(crate) async fn device_with_link(
    emulator: &Emulator,
    network: &str,
    label: &str,
) -> DeviceWithLink {
    let site = emulator.create_site(network, label).await;
    let device = create_device_ok(emulator, network, json!({ "SiteId": site })).await;
    let link = create_link_ok(emulator, network, &site, &plain_link()).await;
    associate_link(emulator, network, &device, &link).await;
    register_vpn_carrier(emulator, network, PLAIN_REGION).await;
    DeviceWithLink {
        site,
        device,
        link,
    }
}

/// One row of the round-trip table: the customer gateway ARN being associated and whether
/// the request names the link too.
struct Row {
    name: &'static str,
    arn: String,
    with_link: bool,
}

#[tokio::test]
async fn customer_gateway_associations_round_trip_through_stored_state() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("customer gateways").await;
    let subject = device_with_link(&emulator, &network, "hq").await;
    // A customer gateway is associated only where the global network carries, in the
    // gateway's own Region, a transit gateway or core network edge its VPN attachment could
    // be on, so every Region the rows span gets a registered carrier (`device_with_link`
    // registered the one for [`PLAIN_REGION`]).
    for region in ["cn-north-1", "us-gov-west-1"] {
        register_vpn_carrier(&emulator, &network, region).await;
    }

    // The rows span what the rule generalises over: the ARN's partition, Region, account and
    // id width — the published bounds of that id are 8 and 17 hex digits — and both forms of
    // the association, with a link and without. Several gateways on ONE device is itself a
    // row of the class: the documented bound is one device per GATEWAY, not one gateway per
    // device.
    let rows = vec![
        Row {
            name: "device only",
            arn: plain_cgw_arn("0123456789abcdef1"),
            with_link: false,
        },
        Row {
            name: "device and link",
            arn: plain_cgw_arn("abcdef123"),
            with_link: true,
        },
        Row {
            name: "shortest published id",
            arn: plain_cgw_arn("0123abcd"),
            with_link: true,
        },
        Row {
            name: "longest published id",
            arn: plain_cgw_arn("fedcba98765432100"),
            with_link: false,
        },
        Row {
            name: "another partition, Region and account",
            arn: cgw_arn("aws-cn", "cn-north-1", "210987654321", "beefcafe1"),
            with_link: false,
        },
        Row {
            name: "another partition and Region, with a link",
            arn: cgw_arn("aws-us-gov", "us-gov-west-1", "123456789012", "00000000"),
            with_link: true,
        },
    ];

    for row in &rows {
        let case = row.name;
        let mut body = json!({ "CustomerGatewayArn": row.arn, "DeviceId": subject.device });
        if row.with_link {
            body["LinkId"] = json!(subject.link);
        }

        let (status, created) = associate(&emulator, &network, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from AssociateCustomerGateway, got {status} with {created}"
        );
        let association = &created["CustomerGatewayAssociation"];
        assert_eq!(
            association["CustomerGatewayArn"],
            json!(row.arn),
            "case {case}: the association must name the gateway asked for, got {created}"
        );
        assert_eq!(
            association["GlobalNetworkId"],
            json!(network),
            "case {case}: the association must name its global network, got {created}"
        );
        assert_eq!(
            association["DeviceId"],
            json!(subject.device),
            "case {case}: the association must name the device, got {created}"
        );
        assert_eq!(
            association["LinkId"],
            if row.with_link {
                json!(subject.link)
            } else {
                Value::Null
            },
            "case {case}: the association must report the link only when one was named, got {created}"
        );
        // A resource this service reports in a non-terminal state owes the clock one
        // transition, so a caller acting straight after the associate still sees PENDING.
        assert_eq!(
            association["State"],
            json!("PENDING"),
            "case {case}: a fresh association must be PENDING, got {created}"
        );

        // The stored tie — not the answer above — is what the reader answers from.
        let stored = read_one(&emulator, &network, &row.arn)
            .await
            .unwrap_or_else(|| panic!("case {case}: the reader must list the association"));
        assert_eq!(
            stored["DeviceId"],
            json!(subject.device),
            "case {case}: the reader must answer the stored device, got {stored}"
        );
        assert_eq!(
            stored["LinkId"],
            if row.with_link {
                json!(subject.link)
            } else {
                Value::Null
            },
            "case {case}: the reader must answer the stored link, got {stored}"
        );
    }

    // Every row is listed together once the unfiltered read is taken, in one stable order,
    // and the state the clock owed has fallen due for all of them by now.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, page) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case unfiltered read: expected 200, got {status} with {page}"
    );
    let mut expected: Vec<String> = rows.iter().map(|row| row.arn.clone()).collect();
    expected.sort();
    assert_eq!(
        arns_of(&page),
        expected,
        "case unfiltered read: expected every stored association, got {page}"
    );
    for association in page["CustomerGatewayAssociations"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        assert_eq!(
            association["State"],
            json!("AVAILABLE"),
            "case settled read: the transition the clock owed must have fallen due, got {association}"
        );
    }

    // Reading twice changes nothing a later caller sees.
    let (status, again) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case repeated read: expected 200, got {status} with {again}"
    );
    assert_eq!(
        arns_of(&again),
        expected,
        "case repeated read: a read must not change what a later caller sees, got {again}"
    );

    // DISASSOCIATE: the removed association is reported DELETING, a later read no longer
    // lists it, and a repeat of the same call is the not-found this operation models.
    for row in &rows {
        let case = row.name;
        let (status, removed) = disassociate(&emulator, &network, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DisassociateCustomerGateway, got {status} with {removed}"
        );
        let association = &removed["CustomerGatewayAssociation"];
        assert_eq!(
            association["CustomerGatewayArn"],
            json!(row.arn),
            "case {case}: the removal must report the gateway asked for, got {removed}"
        );
        assert_eq!(
            association["DeviceId"],
            json!(subject.device),
            "case {case}: the removal must report the stored device, got {removed}"
        );
        assert_eq!(
            association["State"],
            json!("DELETING"),
            "case {case}: the removed association must be reported DELETING, got {removed}"
        );

        assert!(
            read_one(&emulator, &network, &row.arn).await.is_none(),
            "case {case}: a removed association must not be listed any more"
        );

        let (status, body) = disassociate(&emulator, &network, &row.arn).await;
        assert_eq!(
            status, 404,
            "case {case} repeated: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} repeated: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("customer-gateway-association"),
            "case {case} repeated: the refusal must name the association kind, got {body}"
        );
    }

    let (status, empty) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after every removal: expected 200, got {status} with {empty}"
    );
    assert_eq!(
        empty["CustomerGatewayAssociations"],
        json!([]),
        "case read after every removal: expected an empty page, got {empty}"
    );
}

#[tokio::test]
async fn a_customer_gateway_is_associated_with_at_most_one_device_and_link() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("one device").await;
    let first = device_with_link(&emulator, &network, "first").await;
    let second = device_with_link(&emulator, &network, "second").await;
    // A SECOND link of the first device, so a row can ask for the same gateway under
    // another link of the device it is already associated with.
    let spare_link = create_link_ok(&emulator, &network, &first.site, &plain_link()).await;
    associate_link(&emulator, &network, &first.device, &spare_link).await;
    let arn = plain_cgw_arn("0123456789abcdef1");

    let request = json!({
        "CustomerGatewayArn": arn,
        "DeviceId": first.device,
        "LinkId": first.link,
    });
    let (status, created) = associate(&emulator, &network, request.clone()).await;
    assert_eq!(
        status, 200,
        "case first associate: expected 200, got {status} with {created}"
    );

    // The identity of the association is the global network plus the gateway, so the SAME
    // request again reports the stored association and restarts nothing: once the settle
    // span has passed, a repeat still answers AVAILABLE.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, repeated) = associate(&emulator, &network, request).await;
    assert_eq!(
        status, 200,
        "case repeated associate: expected 200, got {status} with {repeated}"
    );
    assert_eq!(
        repeated["CustomerGatewayAssociation"]["State"],
        json!("AVAILABLE"),
        "case repeated associate: a retry must not restart the settle span, got {repeated}"
    );

    // "You cannot associate a customer gateway with more than one device and link."
    // (`smithy_query query="doc" shape="AssociateCustomerGateway"`): the same gateway under
    // a different device, or under a different link of the same device, is the conflict
    // this operation models — and the stored association is untouched by it.
    let conflicts = [
        (
            "another device",
            json!({ "CustomerGatewayArn": arn, "DeviceId": second.device }),
        ),
        (
            "the same device without the link",
            json!({ "CustomerGatewayArn": arn, "DeviceId": first.device }),
        ),
        (
            "the same device with another link",
            json!({
                "CustomerGatewayArn": arn,
                "DeviceId": first.device,
                "LinkId": spare_link,
            }),
        ),
    ];
    for (case, body) in conflicts {
        let (status, refused) = associate(&emulator, &network, body).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: expected ConflictException, got {refused}"
        );
        let stored = read_one(&emulator, &network, &arn)
            .await
            .unwrap_or_else(|| panic!("case {case}: the stored association must survive"));
        assert_eq!(
            stored["DeviceId"],
            json!(first.device),
            "case {case}: a refused associate must not rewrite the stored tie, got {stored}"
        );
        assert_eq!(
            stored["LinkId"],
            json!(first.link),
            "case {case}: a refused associate must not rewrite the stored link, got {stored}"
        );
    }

    // Once the gateway is disassociated it is free again, so the device the conflict above
    // named is accepted.
    let (status, removed) = disassociate(&emulator, &network, &arn).await;
    assert_eq!(
        status, 200,
        "case disassociate before reuse: expected 200, got {status} with {removed}"
    );
    let (status, moved) = associate(
        &emulator,
        &network,
        json!({ "CustomerGatewayArn": arn, "DeviceId": second.device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case associate after disassociate: expected 200, got {status} with {moved}"
    );
    assert_eq!(
        moved["CustomerGatewayAssociation"]["DeviceId"],
        json!(second.device),
        "case associate after disassociate: expected the new device, got {moved}"
    );
}

#[tokio::test]
async fn a_request_naming_nothing_stored_or_no_customer_gateway_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("refusals").await;
    let subject = device_with_link(&emulator, &network, "hq").await;
    let other = device_with_link(&emulator, &network, "other").await;
    // Ids of the minted shape (`<kind>-<17 lowercase hex>`) that name nothing: the leading
    // `f` puts them above anything the monotonic id counter can mint.
    let unknown_network = "global-network-f0000000000000001";
    let unknown_device = "device-f0000000000000002";
    let unknown_link = "link-f0000000000000003";

    // A string that is not a customer gateway ARN at all is a FIELD failure on the member
    // that carried it, whichever of the two operations it arrived on. The rows span the
    // ways the EC2 form can be missed: not an ARN, another service's namespace, another
    // resource type, an id of the wrong prefix, an id shorter and longer than the published
    // widths, and a non-hex id.
    let malformed = [
        ("not an ARN at all", "cgw-0123456789abcdef1".to_string()),
        (
            "an ARN of this service",
            format!("arn:aws:networkmanager::123456789012:device/{}", subject.device),
        ),
        (
            "another EC2 resource type",
            "arn:aws:ec2:us-west-2:123456789012:vpn-connection/vpn-0123456789abcdef1".to_string(),
        ),
        ("another id prefix", plain_cgw_arn("0123abcd").replace("cgw-", "cgx-")),
        ("an id one digit short", plain_cgw_arn("0123abc")),
        ("an id one digit long", plain_cgw_arn("0123456789abcdef12")),
        ("a non-hex id", plain_cgw_arn("0123abcg")),
        ("no id at all", "arn:aws:ec2:us-west-2:123456789012:customer-gateway".to_string()),
    ];
    for (case, arn) in &malformed {
        let (status, refused) = associate(
            &emulator,
            &network,
            json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case associate {case}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case associate {case}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!("customerGatewayArn"),
            "case associate {case}: the refusal must name the ARN member, got {refused}"
        );

        let (status, refused) = disassociate(&emulator, &network, arn).await;
        assert_eq!(
            status, 400,
            "case disassociate {case}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case disassociate {case}: expected ValidationException, got {refused}"
        );
    }

    // A resource INSIDE the global network that names nothing stored is the missing-resource
    // answer (404); the global network in the path is judged before them, and — the
    // association being a new record stored under it — as a field failure.
    let arn = plain_cgw_arn("0123456789abcdef1");
    let (status, refused) = associate(
        &emulator,
        unknown_network,
        json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case associate under an unknown global network: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case associate under an unknown global network: expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["Fields"],
        json!([{ "Name": "globalNetworkId", "Message": "Global network not found." }]),
        "case associate under an unknown global network: expected one Fields entry naming globalNetworkId, got {refused}"
    );
    let not_found = [
        (
            "unknown device",
            network.clone(),
            json!({ "CustomerGatewayArn": arn, "DeviceId": unknown_device }),
        ),
        (
            "unknown link",
            network.clone(),
            json!({
                "CustomerGatewayArn": arn,
                "DeviceId": subject.device,
                "LinkId": unknown_link,
            }),
        ),
    ];
    for (case, target, body) in &not_found {
        let (status, refused) = associate(&emulator, target, body.clone()).await;
        assert_eq!(
            status, 404,
            "case associate {case}: expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case associate {case}: expected ResourceNotFoundException, got {refused}"
        );
    }
    let (status, refused) = disassociate(&emulator, unknown_network, &arn).await;
    assert_eq!(
        status, 404,
        "case disassociate under an unknown global network: expected 404, got {status} with {refused}"
    );
    let (status, refused) = get_associations(&emulator, unknown_network, "").await;
    assert_eq!(
        status, 404,
        "case read of an unknown global network: expected 404, got {status} with {refused}"
    );

    // "If you specify a link, it must be associated with the specified device."
    // (`smithy_query query="doc" shape="AssociateCustomerGateway"`) — the link of ANOTHER
    // device is refused against stored state, with no field at fault.
    let (status, refused) = associate(
        &emulator,
        &network,
        json!({
            "CustomerGatewayArn": arn,
            "DeviceId": subject.device,
            "LinkId": other.link,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a link of another device: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case a link of another device: expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["Reason"],
        json!("Other"),
        "case a link of another device: the refusal is about stored state, got {refused}"
    );

    // Not one of the refusals above stored anything.
    let (status, page) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {page}"
    );
    assert_eq!(
        page["CustomerGatewayAssociations"],
        json!([]),
        "case read after the refusals: a refused request must store nothing, got {page}"
    );
}

#[tokio::test]
async fn the_reader_selects_paginates_and_refuses_a_filter_past_the_documented_maximum() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("reads").await;
    let elsewhere = emulator.create_global_network("elsewhere").await;
    let subject = device_with_link(&emulator, &network, "hq").await;
    let neighbour = device_with_link(&emulator, &elsewhere, "hq").await;

    // Three associations in one global network and one in another: the reader answers the
    // ties of the global network in its path and nothing else.
    let stored: Vec<String> = (0..3).map(|index| plain_cgw_arn(&format!("0123abc{index}"))).collect();
    for arn in &stored {
        let (status, created) = associate(
            &emulator,
            &network,
            json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case fixture {arn}: expected 200, got {status} with {created}"
        );
    }
    let foreign = plain_cgw_arn("beefcafe1");
    let (status, created) = associate(
        &emulator,
        &elsewhere,
        json!({ "CustomerGatewayArn": foreign, "DeviceId": neighbour.device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case fixture in another global network: expected 200, got {status} with {created}"
    );

    // The selector rows: absent (everything of this global network), one ARN, two, an ARN
    // that names nothing, and the ARN associated in the OTHER global network — which is
    // not addressable here.
    let unknown = plain_cgw_arn("f0f0f0f01");
    let selectors: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("filter absent", vec![], stored.clone()),
        ("one ARN", vec![stored[1].clone()], vec![stored[1].clone()]),
        (
            "two ARNs",
            vec![stored[0].clone(), stored[2].clone()],
            vec![stored[0].clone(), stored[2].clone()],
        ),
        ("an ARN naming nothing", vec![unknown.clone()], vec![]),
        (
            "an ARN of another global network",
            vec![foreign.clone()],
            vec![],
        ),
        (
            "a stored ARN beside one naming nothing",
            vec![unknown.clone(), stored[0].clone()],
            vec![stored[0].clone()],
        ),
    ];
    for (case, filter, expected) in &selectors {
        let query = filter
            .iter()
            .map(|arn| format!("customerGatewayArns={}", escape(arn)))
            .collect::<Vec<String>>()
            .join("&");
        let (status, page) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {page}"
        );
        assert_eq!(
            &arns_of(&page),
            expected,
            "case {case}: expected exactly {expected:?}, got {page}"
        );
    }

    // One item per page: every stored tie is reached by following the tokens, and the last
    // page carries none.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=stored.len() {
        let query = match token.as_deref() {
            Some(next) => format!("maxResults=1&nextToken={}", escape(next)),
            None => "maxResults=1".to_string(),
        };
        let (status, page) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case paginated read: expected 200, got {status} with {page}"
        );
        seen.extend(arns_of(&page));
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, stored,
        "case paginated read: following every token must reach every stored tie, got {seen:?}"
    );
    assert!(
        token.is_none(),
        "case paginated read: the last page must carry no token, got {token:?}"
    );

    // A token this service never issued is a field failure, not a page.
    let (status, refused) = get_associations(&emulator, &network, "nextToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "case a forged token: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case a forged token: expected ValidationException, got {refused}"
    );

    // The documented maximum on the ARN filter: honoured at the bound, refused one past it,
    // naming the member that carried the list and computing no page.
    for (case, count) in [
        ("at the maximum", MAX_FILTER_ARNS),
        ("one past the maximum", MAX_FILTER_ARNS + 1),
    ] {
        let query = (0..count)
            .map(|index| {
                format!(
                    "customerGatewayArns={}",
                    escape(&plain_cgw_arn(&format!("0123abc{index:02}")))
                )
            })
            .collect::<Vec<String>>()
            .join("&");
        let (status, page) = get_associations(&emulator, &network, &query).await;
        if count > MAX_FILTER_ARNS {
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {page}"
            );
            assert_eq!(
                error_code(&page),
                "ValidationException",
                "case {case}: expected ValidationException, got {page}"
            );
            assert_eq!(
                page["Fields"][0]["Name"],
                json!("CustomerGatewayArns"),
                "case {case}: the refusal must name the filter member, got {page}"
            );
            assert!(
                page["CustomerGatewayAssociations"].is_null(),
                "case {case}: a refused request must carry no page, got {page}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {page}"
            );
            assert_eq!(
                page["CustomerGatewayAssociations"],
                json!([]),
                "case {case}: ARNs naming nothing select nothing, got {page}"
            );
        }
    }
}

#[tokio::test]
async fn a_link_carrying_a_customer_gateway_association_cannot_be_disassociated_from_its_device() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("dependency").await;
    let subject = device_with_link(&emulator, &network, "hq").await;
    let arn = plain_cgw_arn("0123456789abcdef1");

    let (status, created) = associate(
        &emulator,
        &network,
        json!({
            "CustomerGatewayArn": arn,
            "DeviceId": subject.device,
            "LinkId": subject.link,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case associate: expected 200, got {status} with {created}"
    );

    // "You must first disassociate any customer gateways that are associated with the
    // link." (`smithy_query query="doc" shape="DisassociateLink"`).
    let link_uri = format!(
        "/global-networks/{network}/link-associations?deviceId={}&linkId={}",
        subject.device, subject.link
    );
    let (status, refused) = emulator.delete(&link_uri).await;
    assert_eq!(
        status, 400,
        "case DisassociateLink while the gateway is associated: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case DisassociateLink while the gateway is associated: expected ValidationException, got {refused}"
    );

    // Once the gateway is disassociated the link association can be removed.
    let (status, removed) = disassociate(&emulator, &network, &arn).await;
    assert_eq!(
        status, 200,
        "case disassociate the gateway: expected 200, got {status} with {removed}"
    );
    let (status, body) = emulator.delete(&link_uri).await;
    assert_eq!(
        status, 200,
        "case DisassociateLink afterwards: expected 200, got {status} with {body}"
    );
}
