// SPDX-License-Identifier: CC0-1.0

//! The ARN-LIST filters of this service's reporting operations, judged for the FORM of the
//! values they carry, driven end-to-end over HTTP.
//!
//! The rule under test: a filter member whose values are the ARNs of a resource of ANOTHER
//! service refuses a value that does not read as an ARN of the resource type that member
//! documents — HTTP 400 `ValidationException`, `Reason` = `FieldValidationFailed`, one
//! `Fields` entry naming the member that carried the list, and no page at all — because the
//! form of such an ARN is what the model leaves to the handler (every one of these members
//! targets a string shape carrying only a length and the permissive pattern `^[\s\S]*$`:
//! `smithy_query shape=CustomerGatewayArn,TransitGatewayArn,TransitGatewayConnectPeerArn`).
//! A value that DOES read as such an ARN and names nothing stored is not an error: it
//! selects nothing, which is what a filter does.
//!
//! So one malformed ARN is answered the same whichever operation of its slice it arrives on:
//! the listing below and the operation that WRITES the tie it reports answer the same prose,
//! the same reason and the same field message, each naming its own member.
//! `crate::core_network_prefix_list_associations` pins the rule for the one ARN filter that
//! is a single value (`ListCoreNetworkPrefixListAssociations$PrefixListArn`); this suite
//! spans the LIST-valued ones, and `crate::malformed_resource_arn_refusal` the ARN members
//! that address a resource of THIS service.
//!
//! The rows of each listing are built from the walk that listing's own slice judges its ARN
//! member with, so each spans the ways THAT form can be missed. They differ by slice, which
//! is the point: the customer gateway walk judges the id's prefix and hex width and admits
//! an ARN carrying no Region, while the two transit-gateway walks require the Region (it is
//! what the Connect peer's telemetry entry and the peering's edge location are read from) and
//! judge the resource type and a non-empty id only. Every row that IS well formed is driven
//! through the same listing as a control, so each refusal turns on the form and nothing else.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the query
//! bindings and the error envelopes are exercised as a client hits them. Nothing reaches into
//! `crate::state`: every tie is created and read back through an operation. The harness is the
//! one `global_network_lifecycle` defines — one shared engine, this service, the production
//! dispatch.

use serde_json::{json, Value};

use crate::customer_gateway_association_lifecycle::{device_with_link, plain_cgw_arn, PLAIN_REGION};
use crate::device_lifecycle::create_device_ok;
use crate::filter_maximum_table::{filtered_uri, MAX_FILTER_VALUES};
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::transit_gateway_connect_peer_association_lifecycle::{connect_peer_arn, register_gateway};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The partition every well-formed ARN below carries unless a row says otherwise.
const PARTITION: &str = "aws";

/// One listing whose filter member carries the ARNs of a resource of another service.
struct Listing {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The input member the filter arrives in; a refusal of a value must name it.
    member: &'static str,
    /// The query parameter that member binds to (`@httpQuery`).
    query: &'static str,
    /// The listing's URI, without a query string.
    uri: String,
    /// The output list a page carries, and the ARN member of its items.
    list: &'static str,
    arn_member: &'static str,
    /// Whether the member's own documentation publishes the 10-value maximum
    /// `crate::filter_maximum_table` states, which is refused ahead of the form.
    publishes_maximum: bool,
    /// The ARNs the fixture stored, in the order the listing answers them.
    stored: Vec<String>,
    /// Values that DO read as an ARN of the documented resource type but name nothing
    /// stored: each selects nothing.
    well_formed_naming_nothing: Vec<(&'static str, String)>,
    /// Values that read as no such ARN at all: each is refused on `member`.
    malformed: Vec<(&'static str, String)>,
    /// The operation that WRITES one of these ties, as the request a client sends: the URI,
    /// the body built around the ARN under test, and the member its refusal names.
    write_uri: String,
    write_member: &'static str,
    write_body: fn(&str, &str) -> Value,
    /// The device the write above ties the resource to, where its request carries one.
    device: String,
}

impl Listing {
    /// The ARNs one page of this listing carries, in the order returned.
    fn reported(&self, case: &str, body: &Value) -> Vec<String> {
        body[self.list]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a {} array in {body}", self.list))
            .iter()
            .map(|item| {
                item[self.arn_member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected {} in {item}", self.arn_member)
                    })
                    .to_string()
            })
            .collect()
    }

    /// Read this listing with `values` in its filter.
    async fn read(&self, emulator: &Emulator, values: &[String]) -> (u16, Value) {
        emulator
            .get(&filtered_uri(&self.uri, self.query, values, ""))
            .await
    }

    /// Send `arn` to the operation that writes one of these ties.
    async fn write(&self, emulator: &Emulator, arn: &str) -> (u16, Value) {
        emulator
            .post(&self.write_uri, (self.write_body)(arn, &self.device))
            .await
    }
}

/// `AssociateCustomerGateway`'s body: the gateway ARN and the device it is tied to.
fn customer_gateway_body(arn: &str, device: &str) -> Value {
    json!({ "CustomerGatewayArn": arn, "DeviceId": device })
}

/// `RegisterTransitGateway`'s body: the gateway ARN alone.
fn transit_gateway_body(arn: &str, _device: &str) -> Value {
    json!({ "TransitGatewayArn": arn })
}

/// `AssociateTransitGatewayConnectPeer`'s body: the peer ARN and the device it is tied to.
fn connect_peer_body(arn: &str, device: &str) -> Value {
    json!({ "TransitGatewayConnectPeerArn": arn, "DeviceId": device })
}

/// The customer-gateway-association listing over a global network holding two ties.
///
/// The malformed rows span the ways the EC2 customer gateway form can be missed, which is
/// the table `customer_gateway_association_lifecycle` feeds the associate and the
/// disassociate: not an ARN, another service's namespace, another EC2 resource type, an id of
/// the wrong prefix, an id one digit outside each published width, a non-hex id, and no id at
/// all. The well-formed rows span the parts of that form the walk leaves free: the partition,
/// the Region (including none at all), the account and the id's width.
async fn customer_gateway_associations(emulator: &Emulator) -> Listing {
    let network = emulator.create_global_network("customer gateway ARN forms").await;
    let subject = device_with_link(emulator, &network, "hq").await;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..2 {
        let arn = plain_cgw_arn(&format!("0123abc{index}"));
        let (status, body) = emulator
            .post(
                &format!("/global-networks/{network}/customer-gateway-associations"),
                customer_gateway_body(&arn, &subject.device),
            )
            .await;
        assert_eq!(
            status, 200,
            "fixture AssociateCustomerGateway({arn}): expected 200, got {status} with {body}"
        );
        stored.push(arn);
    }
    stored.sort();

    Listing {
        operation: "GetCustomerGatewayAssociations",
        member: "CustomerGatewayArns",
        query: "customerGatewayArns",
        uri: format!("/global-networks/{network}/customer-gateway-associations"),
        list: "CustomerGatewayAssociations",
        arn_member: "CustomerGatewayArn",
        publishes_maximum: true,
        stored,
        well_formed_naming_nothing: vec![
            ("the plain form", plain_cgw_arn("f0f0f0f01")),
            (
                "another partition",
                "arn:aws-cn:ec2:cn-north-1:123456789012:customer-gateway/cgw-0f0f0f0f1".to_string(),
            ),
            (
                "no Region at all, which this form admits",
                "arn:aws:ec2::123456789012:customer-gateway/cgw-00ff00ff1".to_string(),
            ),
            (
                "the shortest published id",
                plain_cgw_arn("0123abcd"),
            ),
            (
                "the longest published id",
                plain_cgw_arn("0123456789abcdef1"),
            ),
        ],
        malformed: vec![
            ("not an ARN at all", "cgw-0123456789abcdef1".to_string()),
            ("the empty string", String::new()),
            (
                "an ARN of this service",
                "arn:aws:networkmanager::123456789012:device/device-0123456789abcdef1".to_string(),
            ),
            (
                "another EC2 resource type",
                "arn:aws:ec2:us-west-2:123456789012:vpn-connection/vpn-0123456789abcdef1"
                    .to_string(),
            ),
            (
                "an empty partition",
                "arn::ec2:us-west-2:123456789012:customer-gateway/cgw-0123abcd".to_string(),
            ),
            (
                "another id prefix",
                plain_cgw_arn("0123abcd").replace("cgw-", "cgx-"),
            ),
            ("an id one digit short", plain_cgw_arn("0123abc")),
            ("an id one digit long", plain_cgw_arn("0123456789abcdef12")),
            ("a non-hex id", plain_cgw_arn("0123abcg")),
            ("uppercase hex in the id", plain_cgw_arn("0123ABCD")),
            (
                "no id at all",
                "arn:aws:ec2:us-west-2:123456789012:customer-gateway".to_string(),
            ),
            (
                "an empty id",
                "arn:aws:ec2:us-west-2:123456789012:customer-gateway/".to_string(),
            ),
        ],
        write_uri: format!("/global-networks/{network}/customer-gateway-associations"),
        write_member: "customerGatewayArn",
        write_body: customer_gateway_body,
        device: subject.device,
    }
}

/// The transit-gateway-registration listing over a global network holding two registrations.
///
/// The walk this slice judges a transit gateway ARN with reads the EC2 skeleton, the
/// `transit-gateway` resource type, a non-empty id and the REGION the gateway is in (the
/// register needs it to place the gateway at all), so the malformed rows span those and the
/// well-formed ones span the parts it leaves free: the partition, the Region's value and the
/// id's width.
async fn transit_gateway_registrations(emulator: &Emulator) -> Listing {
    let network = emulator
        .create_global_network("transit gateway ARN forms")
        .await;
    let account = own_account();
    let mut stored: Vec<String> = Vec::new();
    // The ids sit well above anything a fixture of another suite derives from a global
    // network's own minted id: a transit gateway is registered with at most one global
    // network, so two fixtures of one emulator must not name the same gateway.
    for index in 0..2 {
        let arn = transit_gateway_arn(PARTITION, PLAIN_REGION, &account, 0xa1 + index, 17);
        register_gateway(emulator, &network, &arn).await;
        stored.push(arn);
    }
    stored.sort();

    Listing {
        operation: "GetTransitGatewayRegistrations",
        member: "TransitGatewayArns",
        query: "transitGatewayArns",
        uri: format!("/global-networks/{network}/transit-gateway-registrations"),
        list: "TransitGatewayRegistrations",
        arn_member: "TransitGatewayArn",
        publishes_maximum: true,
        stored,
        well_formed_naming_nothing: vec![
            (
                "another Region",
                transit_gateway_arn(PARTITION, "eu-central-1", &account, 0xf1, 17),
            ),
            (
                "another partition and Region",
                transit_gateway_arn("aws-cn", "cn-north-1", &account, 0xf2, 12),
            ),
            (
                "another account",
                transit_gateway_arn(PARTITION, PLAIN_REGION, "123456789012", 0xf3, 8),
            ),
        ],
        malformed: vec![
            ("not an ARN at all", "tgw-0123456789abcdef1".to_string()),
            ("the empty string", String::new()),
            (
                "an ARN of this service",
                "arn:aws:networkmanager::123456789012:device/device-0123456789abcdef1".to_string(),
            ),
            (
                "another EC2 resource type",
                "arn:aws:ec2:us-west-2:123456789012:vpn-connection/vpn-0123456789abcdef1"
                    .to_string(),
            ),
            (
                "an empty partition",
                "arn::ec2:us-west-2:123456789012:transit-gateway/tgw-0123abcd".to_string(),
            ),
            (
                "no Region at all, which this form requires",
                "arn:aws:ec2::123456789012:transit-gateway/tgw-0123abcd".to_string(),
            ),
            (
                "no id at all",
                "arn:aws:ec2:us-west-2:123456789012:transit-gateway".to_string(),
            ),
            (
                "an empty id",
                "arn:aws:ec2:us-west-2:123456789012:transit-gateway/".to_string(),
            ),
        ],
        write_uri: format!("/global-networks/{network}/transit-gateway-registrations"),
        write_member: "transitGatewayArn",
        write_body: transit_gateway_body,
        device: String::new(),
    }
}

/// The transit-gateway-Connect-peer-association listing over a global network holding two
/// ties. The rows are the Connect peer counterparts of the transit gateway ones above: the
/// same walk, with the `transit-gateway-connect-peer` resource type.
///
/// This member publishes no maximum, so every length of filter is served — which is what
/// makes it the row that shows the form refusal is about the VALUES and not the count.
async fn connect_peer_associations(emulator: &Emulator) -> Listing {
    let network = emulator
        .create_global_network("connect peer ARN forms")
        .await;
    let device = create_device_ok(emulator, &network, json!({ "Description": "peered" })).await;
    let account = own_account();
    register_gateway(
        emulator,
        &network,
        &transit_gateway_arn(PARTITION, PLAIN_REGION, &account, 0xb0, 17),
    )
    .await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..2 {
        let arn = connect_peer_arn(PARTITION, PLAIN_REGION, &account, 0xc1 + index, 17);
        let (status, body) = emulator
            .post(
                &format!("/global-networks/{network}/transit-gateway-connect-peer-associations"),
                connect_peer_body(&arn, &device),
            )
            .await;
        assert_eq!(
            status, 200,
            "fixture AssociateTransitGatewayConnectPeer({arn}): expected 200, got {status} with {body}"
        );
        stored.push(arn);
    }
    stored.sort();

    Listing {
        operation: "GetTransitGatewayConnectPeerAssociations",
        member: "TransitGatewayConnectPeerArns",
        query: "transitGatewayConnectPeerArns",
        uri: format!("/global-networks/{network}/transit-gateway-connect-peer-associations"),
        list: "TransitGatewayConnectPeerAssociations",
        arn_member: "TransitGatewayConnectPeerArn",
        publishes_maximum: false,
        stored,
        well_formed_naming_nothing: vec![
            (
                "the same Region as the fixture",
                connect_peer_arn(PARTITION, PLAIN_REGION, &account, 0xf1, 17),
            ),
            (
                "another partition and Region",
                connect_peer_arn("aws-cn", "cn-north-1", &account, 0xf2, 12),
            ),
            (
                "another account",
                connect_peer_arn(PARTITION, PLAIN_REGION, "123456789012", 0xf3, 8),
            ),
        ],
        malformed: vec![
            (
                "not an ARN at all",
                "tgw-connect-peer-0123456789abcdef1".to_string(),
            ),
            ("the empty string", String::new()),
            (
                "an ARN of this service",
                "arn:aws:networkmanager::123456789012:device/device-0123456789abcdef1".to_string(),
            ),
            (
                "another EC2 resource type",
                "arn:aws:ec2:us-west-2:123456789012:transit-gateway/tgw-0123456789abcdef1"
                    .to_string(),
            ),
            (
                "an empty partition",
                "arn::ec2:us-west-2:123456789012:transit-gateway-connect-peer/tgw-connect-peer-0123abcd"
                    .to_string(),
            ),
            (
                "no Region at all, which this form requires",
                "arn:aws:ec2::123456789012:transit-gateway-connect-peer/tgw-connect-peer-0123abcd"
                    .to_string(),
            ),
            (
                "no id at all",
                "arn:aws:ec2:us-west-2:123456789012:transit-gateway-connect-peer".to_string(),
            ),
            (
                "an empty id",
                "arn:aws:ec2:us-west-2:123456789012:transit-gateway-connect-peer/".to_string(),
            ),
        ],
        write_uri: format!(
            "/global-networks/{network}/transit-gateway-connect-peer-associations"
        ),
        write_member: "transitGatewayConnectPeerArn",
        write_body: connect_peer_body,
        device,
    }
}

/// Assert `body` is the modelled field failure naming `member`, carrying no `list`, and
/// answer its `Fields[0].Message` — the prose that fixes WHICH form the value missed.
fn assert_field_refusal(case: &str, status: u16, body: &Value, member: &str, list: &str) -> String {
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("FieldValidationFailed"),
        "case {case}: expected a field-level Reason, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!(member),
        "case {case}: the refusal must name {member}, got {body}"
    );
    assert!(
        body[list].is_null(),
        "case {case}: a refused request must carry no page, got {body}"
    );
    body["Fields"][0]["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Fields[0].Message in {body}"))
        .to_string()
}

#[tokio::test]
async fn a_filter_value_that_is_no_arn_of_the_documented_type_is_refused_on_its_member() {
    let emulator = Emulator::new();
    let listings = [
        customer_gateway_associations(&emulator).await,
        transit_gateway_registrations(&emulator).await,
        connect_peer_associations(&emulator).await,
    ];

    for listing in &listings {
        let operation = listing.operation;

        // The fixture round trip: every stored ARN is reported by the unfiltered read, and
        // selects exactly itself.
        let (status, page) = listing.read(&emulator, &[]).await;
        let case = format!("{operation}/the unfiltered read");
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {page}"
        );
        assert_eq!(
            listing.reported(&case, &page),
            listing.stored,
            "case {case}: expected every stored ARN, got {page}"
        );
        for arn in &listing.stored {
            let case = format!("{operation}/the stored ARN {arn}");
            let (status, page) = listing.read(&emulator, std::slice::from_ref(arn)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {page}"
            );
            assert_eq!(
                listing.reported(&case, &page),
                vec![arn.clone()],
                "case {case}: the filter must select exactly that record, got {page}"
            );
        }

        // A value of the documented form that names nothing stored selects nothing: 200 with
        // an empty page, alone and beside a stored ARN.
        for (form, arn) in &listing.well_formed_naming_nothing {
            let case = format!("{operation}/a well-formed ARN naming nothing: {form}");
            let (status, page) = listing.read(&emulator, std::slice::from_ref(arn)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {page}"
            );
            assert_eq!(
                listing.reported(&case, &page),
                Vec::<String>::new(),
                "case {case}: a well-formed ARN naming nothing must select nothing, got {page}"
            );

            let beside = vec![listing.stored[0].clone(), arn.clone()];
            let case = format!("{case}, beside a stored one");
            let (status, page) = listing.read(&emulator, &beside).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {page}"
            );
            assert_eq!(
                listing.reported(&case, &page),
                vec![listing.stored[0].clone()],
                "case {case}: expected the stored ARN alone, got {page}"
            );
        }

        // A value that is no such ARN at all is refused on the member that carried it —
        // alone, and beside a stored ARN, since the filter is judged whole — and the
        // operation that WRITES such a tie answers the same field message for the same
        // string, each naming its own member.
        for (form, arn) in &listing.malformed {
            let case = format!("{operation}/a value that is {form}");
            let (status, body) = listing.read(&emulator, std::slice::from_ref(arn)).await;
            let listed_message =
                assert_field_refusal(&case, status, &body, listing.member, listing.list);

            let beside = vec![listing.stored[0].clone(), arn.clone()];
            let case_beside = format!("{case}, beside a stored one");
            let (status, body) = listing.read(&emulator, &beside).await;
            assert_field_refusal(&case_beside, status, &body, listing.member, listing.list);

            let case_write = format!("the write of a tie naming a value that is {form}");
            let (status, body) = listing.write(&emulator, arn).await;
            let written_message = assert_field_refusal(
                &case_write,
                status,
                &body,
                listing.write_member,
                listing.list,
            );
            assert_eq!(
                listed_message, written_message,
                "case {case}: the listing and the write of such a tie must explain the form \
                 identically, got {listed_message:?} and {written_message:?}"
            );
        }

        // None of the reads or the refused writes above changed what a later caller sees.
        let case = format!("{operation}/the unfiltered read after every refusal");
        let (status, page) = listing.read(&emulator, &[]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {page}"
        );
        assert_eq!(
            listing.reported(&case, &page),
            listing.stored,
            "case {case}: expected every stored ARN, got {page}"
        );
    }
}

#[tokio::test]
async fn the_published_length_of_an_arn_filter_is_judged_before_the_form_of_its_values() {
    let emulator = Emulator::new();
    let listings = [
        customer_gateway_associations(&emulator).await,
        transit_gateway_registrations(&emulator).await,
        connect_peer_associations(&emulator).await,
    ];

    for listing in &listings {
        let operation = listing.operation;
        let malformed = listing.malformed[0].1.clone();

        // One value past the maximum its member publishes, every one of them malformed: the
        // member that publishes the bound answers the LENGTH (its message names the maximum),
        // and the member that publishes none reaches the form instead — so what each listing
        // refuses is the rule its own member states.
        let values: Vec<String> = (0..=MAX_FILTER_VALUES)
            .map(|_| malformed.clone())
            .collect();
        let case = format!("{operation}/a filter past the published maximum, all malformed");
        let (status, body) = listing.read(&emulator, &values).await;
        let message = assert_field_refusal(&case, status, &body, listing.member, listing.list);
        assert_eq!(
            message.contains(&MAX_FILTER_VALUES.to_string()),
            listing.publishes_maximum,
            "case {case}: the length is answered first only where the member publishes it, got {message:?}"
        );

        // Inside the bound the same values are refused for their FORM, whichever member
        // carried them.
        let inside: Vec<String> = (0..MAX_FILTER_VALUES).map(|_| malformed.clone()).collect();
        let case = format!("{operation}/a filter at the published maximum, all malformed");
        let (status, body) = listing.read(&emulator, &inside).await;
        let message = assert_field_refusal(&case, status, &body, listing.member, listing.list);
        assert!(
            !message.contains(&MAX_FILTER_VALUES.to_string()),
            "case {case}: a filter inside the bound must be refused for its form, got {message:?}"
        );
    }
}
