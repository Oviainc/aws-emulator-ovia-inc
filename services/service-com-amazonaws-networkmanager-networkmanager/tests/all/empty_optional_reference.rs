// SPDX-License-Identifier: CC0-1.0

//! The rule every handler of this crate applies to an OPTIONAL reference member, pinned
//! end to end over HTTP: the empty string names no resource, so it resolves nothing and is
//! held as none — exactly as omitting the member is — while any non-empty value stays a
//! reference that must name a resource of the same global network.
//!
//! Every id shape of this service is modelled `len 0..50` over `^[\s\S]*$`
//! (`smithy_query query="operation" operation="CreateDevice"`: `SiteId` len 0..50), so the
//! empty string satisfies the constraint walk and reaches the handler, and the service
//! documents it as the spelling for giving no value ("To remove information for any of the
//! parameters, specify an empty string", `smithy_query query="doc" shape="UpdateDevice"`).
//! The tables below therefore drive each request TWICE — once with the member spelled empty
//! and once with it omitted — and require the two stored records to agree member for
//! member.
//!
//! The refusal rows are the other half of the rule: a non-empty id nothing is stored under
//! — a blank one included, which is a value rather than the absence of one — is still the
//! unresolvable-reference answer, and stores nothing.
//!
//! Nothing reaches into `crate::state`: every record is read back through its implemented
//! reader. The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection, fixture, read_connection};
use crate::customer_gateway_association_lifecycle::{
    associate, device_with_link, plain_cgw_arn, read_one,
};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// An id of the right shape that nothing was ever stored under, so a reference carrying it
/// names nothing.
const UNSTORED_SITE: &str = "site-00000000000000099";
/// See [`UNSTORED_SITE`].
const UNSTORED_LINK: &str = "link-00000000000000099";

/// One probe of the rule: the same request sent with the optional reference members spelled
/// as the empty string and with them omitted.
struct Probe<'a> {
    case: &'a str,
    uri: String,
    /// The member of the answer the created resource sits under (`Device`, `Connection`, …).
    wrapper: &'a str,
    /// The optional reference members this row spells as the empty string.
    members: &'a [&'a str],
    empty_body: Value,
    omitted_body: Value,
    /// The members the service mints or derives per call, which two records of the same
    /// request never share (the id, the ARN, the creation instant, the clock-derived state).
    volatile: &'a [&'a str],
}

/// `resource` with `members` dropped, so two records of the same request compare on what
/// the request put in them.
fn without(resource: &Value, members: &[&str]) -> Value {
    let mut copy = resource.clone();
    if let Some(object) = copy.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    copy
}

/// Drive one [`Probe`] and assert the empty spelling is served and stores what omitting the
/// member stores, answering the two created records.
async fn empty_reads_as_omitted(emulator: &Emulator, probe: Probe<'_>) -> (Value, Value) {
    let Probe {
        case,
        uri,
        wrapper,
        members,
        empty_body,
        omitted_body,
        volatile,
    } = probe;

    let (status, answer) = emulator.post(&uri, empty_body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {empty_body} must be served like the request that omits {members:?}, \
         got {status} with {answer}"
    );
    let created = answer[wrapper].clone();

    let (status, answer) = emulator.post(&uri, omitted_body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {omitted_body} must be served, got {status} with {answer}"
    );
    let omitted = answer[wrapper].clone();

    for member in members {
        assert_eq!(
            created[*member],
            Value::Null,
            "case {case:?}: {member} spelled as the empty string must name nothing, got {created}"
        );
        assert_eq!(
            omitted[*member],
            Value::Null,
            "case {case:?}: {member} omitted must name nothing, got {omitted}"
        );
    }
    assert_eq!(
        without(&created, volatile),
        without(&omitted, volatile),
        "case {case:?}: the empty spelling of {members:?} must store what omitting it stores, \
         got {created} against {omitted}"
    );

    (created, omitted)
}

/// The `DeviceId` of a created device.
fn device_id(case: &str, device: &Value) -> String {
    device["DeviceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no DeviceId in {device}"))
        .to_string()
}

/// Read one device back through `GetDevices`, the implemented reader for the resource.
async fn read_device(emulator: &Emulator, case: &str, network: &str, device: &str) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/devices?deviceIds={device}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDevices({device}) expected 200, got {status} with {body}"
    );
    let listed = body["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Devices array in {body}"));
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: GetDevices({device}) must answer exactly that device, got {body}"
    );
    listed[0].clone()
}

/// The devices of `network`, so a refused create can be shown to have stored nothing.
async fn device_count(emulator: &Emulator, case: &str, network: &str) -> usize {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/devices"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDevices expected 200, got {status} with {body}"
    );
    body["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Devices array in {body}"))
        .len()
}

#[tokio::test]
async fn create_device_reads_an_empty_site_id_as_no_site() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("empty site references").await;

    // The rows differ along what ELSE the request carries beside the empty `SiteId` — every
    // other member of the create, each of its optional groups, none of them — because the
    // rule is about that one member whatever the rest of the request holds.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty SiteId alone", json!({})),
        (
            "an empty SiteId beside the free-form descriptors",
            json!({
                "Description": "branch router",
                "Type": "router",
                "Vendor": "AnyCompany",
                "Model": "AC-1000",
                "SerialNumber": "SN-0001",
            }),
        ),
        (
            "an empty SiteId beside a physical Location",
            json!({ "Location": { "Address": "1 Main St", "Latitude": "47.6062" } }),
        ),
        (
            "an empty SiteId beside an AWS location",
            json!({ "AWSLocation": { "Zone": "us-west-2a" } }),
        ),
        (
            "an empty SiteId beside tags",
            json!({ "Tags": [{ "Key": "env", "Value": "test" }] }),
        ),
    ];

    for (case, extra) in rows {
        let mut empty_body = extra.clone();
        empty_body["SiteId"] = json!("");
        let (created, _) = empty_reads_as_omitted(
            &emulator,
            Probe {
                case,
                uri: format!("/global-networks/{network}/devices"),
                wrapper: "Device",
                members: &["SiteId"],
                empty_body,
                omitted_body: extra,
                volatile: &["DeviceId", "DeviceArn", "CreatedAt", "State"],
            },
        )
        .await;

        // ROUND TRIP: the stored record is what the reader answers, so the device is
        // unsited in the store rather than only in the create's answer.
        let id = device_id(case, &created);
        let read = read_device(&emulator, case, &network, &id).await;
        assert_eq!(
            read["SiteId"],
            Value::Null,
            "case {case:?}: the stored device must be unsited, got {read}"
        );
        assert_eq!(
            without(&read, &["State"]),
            without(&created, &["State"]),
            "case {case:?}: the reader must answer the created record, got {read} against {created}"
        );
    }
}

#[tokio::test]
async fn create_device_still_refuses_a_site_id_that_names_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("unresolvable sites").await;
    let elsewhere = emulator.create_global_network("another network").await;
    let foreign_site = emulator.create_site(&elsewhere, "foreign").await;
    let site = emulator.create_site(&network, "hq").await;

    // A site the request DOES name must exist in the global network the path addresses: the
    // rows span the ways a non-empty id names nothing — never stored, stored under another
    // global network, and a blank id, which is a value rather than the absence of one.
    let rows: Vec<(&str, String)> = vec![
        ("a site id nothing was stored under", UNSTORED_SITE.to_string()),
        ("a site of another global network", foreign_site),
        ("a blank SiteId, which is a value and not the absence of one", " ".to_string()),
    ];

    let before = device_count(&emulator, "before the refusals", &network).await;
    for (case, site_id) in rows {
        let (status, answer) = emulator
            .post(
                &format!("/global-networks/{network}/devices"),
                json!({ "SiteId": site_id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        assert_eq!(
            answer["Fields"][0]["Name"].as_str(),
            Some("siteId"),
            "case {case:?}: the refusal must name siteId, got {answer}"
        );
    }
    assert_eq!(
        device_count(&emulator, "after the refusals", &network).await,
        before,
        "case the refused creates: nothing may be stored"
    );

    // The site that DOES resolve is still assigned, so the rule refuses nothing it should
    // serve.
    let sited = create_device_ok(&emulator, &network, json!({ "SiteId": site.clone() })).await;
    let read = read_device(&emulator, "a site of this global network", &network, &sited).await;
    assert_eq!(
        read["SiteId"],
        json!(site),
        "case a site of this global network: the device must be stored at it, got {read}"
    );
}

#[tokio::test]
async fn create_connection_reads_an_empty_link_member_as_no_link() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "empty link references").await;
    let network = seed.network.clone();
    let both_devices = json!({
        "DeviceId": seed.device.clone(),
        "ConnectedDeviceId": seed.connected_device.clone(),
    });

    // The rows differ along WHICH of the two optional link members is spelled empty — one,
    // the other, both — and along whether the rest of the request names a stored link or
    // carries free-form members, because the rule holds per member.
    let rows: Vec<(&str, Vec<&str>, Value)> = vec![
        ("an empty LinkId", vec!["LinkId"], json!({})),
        (
            "an empty ConnectedLinkId beside a stored LinkId",
            vec!["ConnectedLinkId"],
            json!({ "LinkId": seed.link.clone() }),
        ),
        (
            "both link members empty",
            vec!["LinkId", "ConnectedLinkId"],
            json!({}),
        ),
        (
            "an empty LinkId beside a Description and tags",
            vec!["LinkId"],
            json!({
                "Description": "over no recorded link",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (case, members, extra) in rows {
        let mut omitted_body = both_devices.clone();
        for (member, value) in extra.as_object().expect("the row's extras are an object") {
            omitted_body[member] = value.clone();
        }
        let mut empty_body = omitted_body.clone();
        for member in &members {
            empty_body[*member] = json!("");
        }

        let (created, _) = empty_reads_as_omitted(
            &emulator,
            Probe {
                case,
                uri: format!("/global-networks/{network}/connections"),
                wrapper: "Connection",
                members: &members,
                empty_body,
                omitted_body,
                volatile: &["ConnectionId", "ConnectionArn", "CreatedAt", "State"],
            },
        )
        .await;

        // ROUND TRIP: the stored record is what `GetConnections` answers.
        let id = created["ConnectionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionId in {created}"))
            .to_string();
        let read = read_connection(&emulator, &network, &id).await;
        for member in &members {
            assert_eq!(
                read[*member],
                Value::Null,
                "case {case:?}: the stored connection must hold no {member}, got {read}"
            );
        }
    }

    // A link member that DOES name something must still name a link of this global network,
    // so the empty spelling is the only one that resolves nothing.
    for member in ["LinkId", "ConnectedLinkId"] {
        let mut body = both_devices.clone();
        body[member] = json!(UNSTORED_LINK);
        let (status, answer) = create_connection(&emulator, &network, body).await;
        assert_eq!(
            status, 400,
            "case an unstored {member}: expected 400, got {status} with {answer}"
        );
        let expected = if member == "LinkId" {
            "linkId"
        } else {
            "connectedLinkId"
        };
        assert_eq!(
            answer["Fields"][0]["Name"].as_str(),
            Some(expected),
            "case an unstored {member}: the refusal must name {expected}, got {answer}"
        );
    }
}

#[tokio::test]
async fn associate_customer_gateway_reads_an_empty_link_id_as_no_link() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("empty association links").await;
    let subject = device_with_link(&emulator, &network, "hq").await;

    // The association's identity is the gateway's ARN, so the two spellings of "no link"
    // are probed on two gateways of the same device; the ARN they differ in is dropped from
    // the comparison.
    let empty_arn = plain_cgw_arn("0123456789abcde01");
    let omitted_arn = plain_cgw_arn("0123456789abcde02");
    let (created, _) = empty_reads_as_omitted(
        &emulator,
        Probe {
            case: "an empty LinkId",
            uri: format!("/global-networks/{network}/customer-gateway-associations"),
            wrapper: "CustomerGatewayAssociation",
            members: &["LinkId"],
            empty_body: json!({
                "CustomerGatewayArn": empty_arn.clone(),
                "DeviceId": subject.device.clone(),
                "LinkId": "",
            }),
            omitted_body: json!({
                "CustomerGatewayArn": omitted_arn,
                "DeviceId": subject.device.clone(),
            }),
            volatile: &["CustomerGatewayArn", "State"],
        },
    )
    .await;

    // ROUND TRIP: the stored tie is what `GetCustomerGatewayAssociations` answers.
    let read = read_one(&emulator, &network, &empty_arn)
        .await
        .unwrap_or_else(|| panic!("case an empty LinkId: the association must be stored"));
    assert_eq!(
        read["LinkId"],
        Value::Null,
        "case an empty LinkId: the stored association must hold no link, got {read}"
    );
    assert_eq!(
        read["DeviceId"],
        created["DeviceId"],
        "case an empty LinkId: the stored association must name the device, got {read}"
    );

    // A link the request DOES name is still resolved, and one of another site of the same
    // global network still fails the documented precondition, so the empty spelling is the
    // only one that resolves nothing.
    let with_link = plain_cgw_arn("0123456789abcde03");
    let (status, answer) = associate(
        &emulator,
        &network,
        json!({
            "CustomerGatewayArn": with_link.clone(),
            "DeviceId": subject.device.clone(),
            "LinkId": subject.link.clone(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a link of the device: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["CustomerGatewayAssociation"]["LinkId"],
        json!(subject.link.clone()),
        "case a link of the device: the association must name the link, got {answer}"
    );

    let (status, answer) = associate(
        &emulator,
        &network,
        json!({
            "CustomerGatewayArn": plain_cgw_arn("0123456789abcde04"),
            "DeviceId": subject.device.clone(),
            "LinkId": UNSTORED_LINK,
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case a LinkId nothing was stored under: expected 404, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case a LinkId nothing was stored under: expected ResourceNotFoundException, got {answer}"
    );

    // A link of this global network that is NOT associated with the device is a value the
    // rule leaves to the documented precondition, not something the empty spelling covers.
    let other_site = emulator.create_site(&network, "spare").await;
    let loose_link = create_link_ok(&emulator, &network, &other_site, &plain_link()).await;
    let (status, answer) = associate(
        &emulator,
        &network,
        json!({
            "CustomerGatewayArn": plain_cgw_arn("0123456789abcde05"),
            "DeviceId": subject.device.clone(),
            "LinkId": loose_link,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a link not associated with the device: expected 400, got {status} with {answer}"
    );
}
