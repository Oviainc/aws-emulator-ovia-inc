// SPDX-License-Identifier: CC0-1.0

//! A continuation token is honoured only by the listing that minted it, driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`, the
//! core-network helpers in `crate::core_network_lifecycle` and
//! `crate::attachment_lifecycle`).
//!
//! The rule under test: a `NextToken` names a position in the listing that minted it and in
//! no other, so a listing handed a token another listing minted refuses it on the member —
//! `ValidationException` (HTTP 400) with `Reason` `FieldValidationFailed` and one `Fields`
//! entry naming `NextToken`, the one refusal every listing of this service gives a token it
//! did not mint — instead of resolving a foreign key offset against its own key space. That
//! resolution is what must not happen: a foreign key no key of this listing sorts after
//! yields an empty page, and one every key sorts after yields this listing's first page
//! again, each indistinguishable from a truthful answer.
//!
//! The table below spans that class: five of this service's paginated listings, chosen so
//! their key spaces differ in form AND in how they sort against one another — global network
//! ids, site ids, device ids, attachment ids and the zero-padded version numbers a policy
//! history is keyed by, which sort before all four id spaces — crossed in BOTH directions,
//! so the pairs cover a foreign key past every key of the recipient listing and a foreign
//! key before all of them, each against a listing that really holds records. The
//! counter-case is the second test: the token each listing minted still resumes THAT
//! listing, over every listing of the table, so the refusals are about which listing minted
//! the token and not about pagination.
//!
//! Nothing here reaches into `crate::state`: every resource is created and every token is
//! obtained through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_vpc_attachment, policy_document, subnet_arn, vpc_arn,
};
use crate::core_network_lifecycle::create_core_network;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};

/// The `Reason` a `ValidationException` carries when ONE input member is at fault, from the
/// modelled enumeration (`smithy_query shape=ValidationExceptionReason`: `UnknownOperation`,
/// `CannotParse`, `FieldValidationFailed`, `Other`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The input member every paginated listing carries its continuation token in (`NextToken`,
/// `@httpQuery("nextToken")` on each listing's request shape).
const FIELD_NEXT_TOKEN: &str = "NextToken";

/// The Region the fixture's core network configures an edge in, so a VPC in it can be
/// attached.
const EDGE_LOCATION: &str = "us-west-2";

/// One paginated listing under test: the case name, the URI it is reached at, the output
/// member its page arrives in, the id member of each item on that page, and the two items it
/// holds in the key order its pages walk.
struct Listing {
    case: &'static str,
    uri: String,
    list: &'static str,
    id_member: &'static str,
    items: Vec<String>,
}

/// Request one page of `listing`, of size one, optionally carrying `token` in `nextToken`.
async fn page(emulator: &Emulator, listing: &Listing, token: Option<&str>) -> (u16, Value) {
    let separator = if listing.uri.contains('?') { '&' } else { '?' };
    let mut uri = format!("{}{separator}maxResults=1", listing.uri);
    if let Some(token) = token {
        uri.push_str(&format!("&nextToken={token}"));
    }
    emulator.get(&uri).await
}

/// The ids one page of `listing` reports, in the order reported. Rendered as text whatever
/// the modelled type of the id member is, so a numeric id (a policy version) and a string
/// one compare the same way.
fn reported(body: &Value, listing: &Listing) -> Vec<String> {
    body[listing.list]
        .as_array()
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .map(|item| match &item[listing.id_member] {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
}

/// The first page of `listing` and the token that resumes it, asserting the listing really
/// holds the two items the table claims.
async fn first_page(emulator: &Emulator, listing: &Listing) -> String {
    let (status, body) = page(emulator, listing, None).await;
    assert_eq!(
        status, 200,
        "case {}/first page: expected 200, got {status} with {body}",
        listing.case
    );
    let got = reported(&body, listing);
    assert_eq!(
        got,
        listing.items[..1].to_vec(),
        "case {}/first page: one item is due on a page of one, got {got:?} from {body}",
        listing.case
    );
    body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {}/first page: two items need a NextToken, got {body}",
                listing.case
            )
        })
        .to_string()
}

/// Five listings, each holding exactly two records, whose key spaces differ in form and in
/// how they sort against each other: two global networks, two sites and two devices of the
/// first of them, two VPC attachments on a core network of it, and the two policy versions
/// of that core network.
async fn fixture(emulator: &Emulator) -> Vec<Listing> {
    let first_network = emulator.create_global_network("page token scope a").await;
    let second_network = emulator.create_global_network("page token scope b").await;

    let mut sites = vec![
        emulator.create_site(&first_network, "scope site a").await,
        emulator.create_site(&first_network, "scope site b").await,
    ];
    let mut devices = vec![
        create_device_ok(
            emulator,
            &first_network,
            json!({ "Description": "scope device a" }),
        )
        .await,
        create_device_ok(
            emulator,
            &first_network,
            json!({ "Description": "scope device b" }),
        )
        .await,
    ];

    // The core network lives in the FIRST global network, so the global network listing
    // holds the two networks above and no more.
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": first_network,
            "Description": "page token scope",
            "PolicyDocument": policy_document(&[EDGE_LOCATION]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkId in {created}"))
        .to_string();

    let mut attachments = Vec::new();
    for index in 1..=2usize {
        let (status, created) = create_vpc_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(EDGE_LOCATION, index),
                "SubnetArns": [subnet_arn(EDGE_LOCATION, index)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateVpcAttachment({index}): expected 200, got {status} with {created}"
        );
        attachments.push(attachment_id(
            "the scope attachment",
            &created["VpcAttachment"],
        ));
    }

    // A second policy version, so the version history pages too. Its page keys are the
    // version numbers, which sort before every minted id.
    let (status, put) = emulator
        .post(
            &format!("/core-networks/{core_network}/core-network-policy"),
            json!({
                "PolicyDocument": policy_document(&[EDGE_LOCATION]),
                "Description": "the second scope policy",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy: expected 200, got {status} with {put}"
    );

    let mut networks = vec![first_network.clone(), second_network];
    // Every listing walks its records in key order, which for these listings is the
    // ascending order of the ids minted above.
    networks.sort();
    sites.sort();
    devices.sort();
    attachments.sort();

    vec![
        Listing {
            case: "DescribeGlobalNetworks",
            uri: "/global-networks".to_string(),
            list: "GlobalNetworks",
            id_member: "GlobalNetworkId",
            items: networks,
        },
        Listing {
            case: "GetSites",
            uri: format!("/global-networks/{first_network}/sites"),
            list: "Sites",
            id_member: "SiteId",
            items: sites,
        },
        Listing {
            case: "GetDevices",
            uri: format!("/global-networks/{first_network}/devices"),
            list: "Devices",
            id_member: "DeviceId",
            items: devices,
        },
        Listing {
            case: "ListAttachments",
            uri: "/attachments".to_string(),
            list: "Attachments",
            id_member: "AttachmentId",
            items: attachments,
        },
        Listing {
            case: "ListCoreNetworkPolicyVersions",
            uri: format!("/core-networks/{core_network}/core-network-policy-versions"),
            list: "CoreNetworkPolicyVersions",
            id_member: "PolicyVersionId",
            items: vec!["1".to_string(), "2".to_string()],
        },
    ]
}

#[tokio::test]
async fn a_token_another_listing_minted_is_refused_on_the_member() {
    let emulator = Emulator::new();
    let listings = fixture(&emulator).await;

    // The token each listing really minted, beside the listing that minted it.
    let mut minted: Vec<String> = Vec::new();
    for listing in &listings {
        minted.push(first_page(&emulator, listing).await);
    }

    for (donor_index, donor) in listings.iter().enumerate() {
        for (recipient_index, recipient) in listings.iter().enumerate() {
            if donor_index == recipient_index {
                continue;
            }
            let case = format!("{} token into {}", donor.case, recipient.case);
            let token = &minted[donor_index];
            let (status, body) = page(&emulator, recipient, Some(token)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a token another listing minted, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Reason"],
                json!(REASON_FIELD_VALIDATION_FAILED),
                "case {case:?}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"],
                json!(FIELD_NEXT_TOKEN),
                "case {case:?}: the refusal must name the {FIELD_NEXT_TOKEN} member, got {body}"
            );
            assert!(
                body["Fields"][1].is_null(),
                "case {case:?}: one member is at fault, so one Fields entry is due, got {body}"
            );
            assert!(
                body[recipient.list].is_null() && body["NextToken"].is_null(),
                "case {case:?}: a refused request must carry no page and no token, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_token_a_listing_minted_still_resumes_that_listing() {
    let emulator = Emulator::new();
    let listings = fixture(&emulator).await;

    for listing in &listings {
        let token = first_page(&emulator, listing).await;
        let (status, resumed) = page(&emulator, listing, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {}/resumed page: the token this listing minted must be honoured, got {status} with {resumed}",
            listing.case
        );
        let got = reported(&resumed, listing);
        assert_eq!(
            got,
            listing.items[1..].to_vec(),
            "case {}/resumed page: the token must resume after the first item, got {got:?} from {resumed}",
            listing.case
        );
        assert!(
            resumed["NextToken"].is_null(),
            "case {}/resumed page: the last page carries no token, got {resumed}",
            listing.case
        );
    }
}
