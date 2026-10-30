// SPDX-License-Identifier: CC0-1.0

//! `GetLinkAssociations` decides what each of its two selectors NAMES before it reports
//! anything, driven end to end over HTTP.
//!
//! Both selectors are OPTIONAL reference members, so each is read the way every optional
//! reference of this crate is read (`crate::empty_reference_filter` pins that reading over
//! the five listings that select on one): the EMPTY string is the ABSENCE of the reference,
//! and every other value — a blank one included — still names a resource. On this operation
//! the reading decides a refusal as well as a filter, because "either the device ID or the
//! link ID must be specified" (`smithy_query query="doc" shape="GetLinkAssociations"`), and
//! three consequences of that fall out which no other suite pins:
//!
//! 1. Whether a selector is present is decided from the REQUEST alone, so a request whose
//!    only selectors are empty is refused before the global network in the path is looked
//!    up, while one that carries a value reaches that lookup.
//! 2. A blank-but-not-empty value is not an absence: it specifies a subject, so the request
//!    is answered, and it narrows the page by a value nothing is stored under.
//! 3. An empty selector beside a real one narrows nothing, so it leaves the page sequence
//!    the real selector alone produces exactly as it is.
//!
//! Every call goes through the real `MultiServiceRouter` with the harness
//! `crate::global_network_lifecycle` defines, and an association is only ever read back
//! through `GetLinkAssociations`.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_association_lifecycle::{associate_ok, get_associations, pairs_of};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// A global network id of the shape this service mints that nothing is stored under, so a
/// read addressed through it reaches the path lookup and finds nothing.
const UNSTORED_NETWORK: &str = "global-network-00000000000000099";

/// The refusal the operation answers a request that specifies neither selector.
const NEITHER_SPECIFIED: &str = "either DeviceId or LinkId must be specified";

/// One stored device, one stored link, and the association tying them, in a fresh global
/// network: the fixture every test here narrows.
struct Fixture {
    network: String,
    device: String,
    links: Vec<String>,
}

/// Create `links` links at one site, a device at that site, and associate the device with
/// every one of them.
async fn seed(emulator: &Emulator, links: usize) -> Fixture {
    let network = emulator.create_global_network("selector-absence").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let mut created = Vec::new();
    for _ in 0..links {
        let link = create_link_ok(emulator, &network, &site, &plain_link()).await;
        associate_ok(emulator, &network, &device, &link).await;
        created.push(link);
    }
    Fixture {
        network,
        device,
        links: created,
    }
}

/// Walk every page of `GetLinkAssociations` for `selector` at `page_size`, answering the
/// pages in the order they were served.
async fn walk(
    emulator: &Emulator,
    network: &str,
    case: &str,
    selector: &str,
    page_size: u32,
) -> Vec<Vec<(String, String)>> {
    let mut pages = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = vec![selector.to_string(), format!("maxResults={page_size}")];
        if let Some(ref resume) = token {
            query.push(format!("nextToken={resume}"));
        }
        let (status, page) = get_associations(emulator, network, &query.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLinkAssociations({selector}) expected 200, got {status} with {page}"
        );
        pages.push(pairs_of(&page));
        token = page["NextToken"].as_str().map(str::to_string);
        assert!(
            pages.len() <= 8,
            "case {case:?}: the token walk must terminate, saw {pages:?}"
        );
        if token.is_none() {
            return pages;
        }
    }
}

#[tokio::test]
async fn an_empty_selector_is_an_absence_decided_before_the_global_network_is_looked_up() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator, 1).await;
    let network = fixture.network.clone();
    let device = fixture.device.clone();
    let link = fixture.links[0].clone();

    // Each row is the global network the read is addressed through and the query string it
    // carries, against the answer the rule demands. The rows span the two axes that decide
    // it: whether each selector is absent (omitted or empty) or names a value, and whether
    // the global network in the path is stored — so an empty selector is the 400 even where
    // the path names nothing, and a selector that carries a value reaches the lookup and is
    // that path's 404.
    let rows: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "no selector at all, stored network",
            network.as_str(),
            String::new(),
            400,
            "ValidationException",
        ),
        (
            "no selector at all, unstored network",
            UNSTORED_NETWORK,
            String::new(),
            400,
            "ValidationException",
        ),
        (
            "an empty deviceId, unstored network",
            UNSTORED_NETWORK,
            "deviceId=".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an empty linkId, unstored network",
            UNSTORED_NETWORK,
            "linkId=".to_string(),
            400,
            "ValidationException",
        ),
        (
            "both selectors empty, unstored network",
            UNSTORED_NETWORK,
            "deviceId=&linkId=".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an empty deviceId beside the paging members, unstored network",
            UNSTORED_NETWORK,
            "deviceId=&maxResults=500".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a stored deviceId, unstored network",
            UNSTORED_NETWORK,
            format!("deviceId={device}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored linkId beside an empty deviceId, unstored network",
            UNSTORED_NETWORK,
            format!("deviceId=&linkId={link}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a deviceId nothing is stored under, unstored network",
            UNSTORED_NETWORK,
            "deviceId=device-00000000000000099".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored linkId beside an empty deviceId, stored network",
            network.as_str(),
            format!("deviceId=&linkId={link}"),
            200,
            "",
        ),
    ];

    for (case, in_network, query, expected, code) in rows {
        let (status, body) = get_associations(&emulator, in_network, &query).await;
        assert_eq!(
            status, expected,
            "case {case:?}: GetLinkAssociations({query}) expected {expected}, got {status} with {body}"
        );
        if expected == 200 {
            assert_eq!(
                pairs_of(&body),
                vec![(device.clone(), link.clone())],
                "case {case:?}: the surviving selector must still report its association, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        if expected == 400 {
            assert_eq!(
                body["Message"].as_str(),
                Some(NEITHER_SPECIFIED),
                "case {case:?}: expected the modelled refusal, got {body}"
            );
        } else {
            assert_eq!(
                (body["ResourceType"].clone(), body["ResourceId"].clone()),
                (json!("global-network"), json!(UNSTORED_NETWORK)),
                "case {case:?}: the refusal must name the global network it could not find, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_blank_selector_is_not_the_absence_of_that_selector() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator, 1).await;
    let network = fixture.network.clone();
    let device = fixture.device.clone();
    let link = fixture.links[0].clone();

    // `DeviceId` and `LinkId` are each modelled `length 0..50` over `^[\s\S]*$`
    // (`smithy_query query="operation" operation="GetLinkAssociations"`), so a value of
    // nothing but whitespace satisfies the constraint walk and reaches the handler just as
    // the empty string does. Only the EMPTY string is the absence of the reference, so each
    // row below SPECIFIES a selector — the request is answered rather than refused — and
    // selects the associations carrying a value nothing is stored under, which is none. The
    // rows span the whitespace spellings a query string can carry (`+` and `%20` for a
    // space, `%09` for a tab, and a run of them), which selector carries it, and a blank
    // value beside a selector that does name a stored resource.
    let blank: Vec<(&str, String)> = vec![
        ("a deviceId of one space, sent as +", "deviceId=+".to_string()),
        (
            "a deviceId of one space, sent as %20",
            "deviceId=%20".to_string(),
        ),
        ("a deviceId of one tab", "deviceId=%09".to_string()),
        ("a deviceId of three spaces", "deviceId=%20%20%20".to_string()),
        ("a linkId of one space", "linkId=%20".to_string()),
        (
            "a blank deviceId beside a stored linkId",
            format!("deviceId=%20&linkId={link}"),
        ),
        (
            "a stored deviceId beside a blank linkId",
            format!("deviceId={device}&linkId=%20"),
        ),
    ];
    for (case, query) in blank {
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a blank selector still specifies a subject, so GetLinkAssociations({query}) \
             must be answered, got {status} with {body}"
        );
        let reported = pairs_of(&body);
        assert_eq!(
            reported,
            Vec::<(String, String)>::new(),
            "case {case:?}: a blank selector names a resource nothing is stored under, so it must \
             select nothing, got {reported:?}"
        );
    }

    // The contrast that makes the rule the empty string's and not every blank value's: with
    // the SAME selector spelled empty, the other selector's associations are answered.
    for (case, query, expected) in [
        (
            "an empty deviceId beside a stored linkId",
            format!("deviceId=&linkId={link}"),
            vec![(device.clone(), link.clone())],
        ),
        (
            "a stored deviceId beside an empty linkId",
            format!("deviceId={device}&linkId="),
            vec![(device.clone(), link.clone())],
        ),
    ] {
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLinkAssociations({query}) expected 200, got {status} with {body}"
        );
        let reported = pairs_of(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_selector_leaves_the_pages_of_the_other_selector_untouched() {
    let emulator = Emulator::new();
    let fixture = seed(&emulator, 3).await;
    let network = fixture.network.clone();
    let device = fixture.device.clone();
    let link = fixture.links[0].clone();

    // An absent selector narrows nothing, so the two spellings of absence must serve the
    // same pages in the same order, down to where each page ends. The rows differ in which
    // selector names the resource, how the other is spelled absent, and the page size:
    // `MaxResults` is modelled `range 1..500` (`smithy_query query="operation"
    // operation="GetLinkAssociations"`), so the walk is driven at its lower bound, in
    // between, and at its upper bound.
    let pairs: Vec<(&str, String, String)> = vec![
        (
            "deviceId with linkId spelled empty",
            format!("deviceId={device}"),
            format!("deviceId={device}&linkId="),
        ),
        (
            "linkId with deviceId spelled empty",
            format!("linkId={link}"),
            format!("deviceId=&linkId={link}"),
        ),
    ];
    for (case, omitted, empty) in pairs {
        for page_size in [1, 2, 500] {
            let case = format!("{case} at maxResults={page_size}");
            let without = walk(&emulator, &network, &case, &omitted, page_size).await;
            let with = walk(&emulator, &network, &case, &empty, page_size).await;
            assert_eq!(
                with, without,
                "case {case:?}: the empty selector must leave the pages of {omitted} as they are, \
                 got {with:?} against {without:?}"
            );
            assert!(
                !without.is_empty() && without.iter().all(|page| !page.is_empty()),
                "case {case:?}: every page served must carry an association, got {without:?}"
            );
            let flattened: Vec<(String, String)> =
                without.into_iter().flatten().collect::<Vec<_>>();
            let mut deduplicated = flattened.clone();
            deduplicated.sort();
            deduplicated.dedup();
            assert_eq!(
                deduplicated.len(),
                flattened.len(),
                "case {case:?}: the walk must visit each association once, got {flattened:?}"
            );
        }
    }
}
