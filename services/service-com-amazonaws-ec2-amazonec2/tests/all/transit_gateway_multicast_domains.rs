// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayMulticastDomains`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeTransitGatewayMulticastDomains`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one every EC2 describe follows: it REPORTS THE RESOURCES THE
//! STORE HOLDS, each with every member it carries, narrowed by the id list and by every
//! filter the operation documents (joined with AND), in resource-id order, paged by
//! `MaxResults` with a token naming the id the next page starts at — and it refuses what it
//! cannot resolve rather than answering an absent row: an unknown id with that resource's
//! own `NotFound` code, a value that is not an id of the kind with its `Malformed` code, a
//! filter name the operation does not document and a token the service did not mint.
//!
//! Every table row below is a distinct input along what that generalises over: which
//! transit gateway a domain belongs to, which state it is in, how many domains one gateway
//! holds, which selector the request uses, how many ids it names, and how large a page it
//! asks for (both published bounds of `TransitGatewayMaxResults` and one past each). The
//! domains are seeded through the store's own writer
//! (`insert_transit_gateway_multicast_domain`), because
//! `CreateTransitGatewayMulticastDomain` is still a scaffold stub — which is also why a
//! stored domain carries no ARN, options, creation time or tags for this describe to report:
//! nothing has recorded them, and a describe reports what the store holds.

use ec2::ops::types::TRANSIT_GATEWAY_MULTICAST_DOMAIN_STATE_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TransitGatewayMulticastDomainRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The ec2Query `Action` under test.
const ACTION: &str = "DescribeTransitGatewayMulticastDomains";

/// One seeded domain: the facts that distinguish it along what the rule generalises over.
struct Fixture {
    /// The transit gateway the domain belongs to — the `transit-gateway-id` filter's value.
    transit_gateway_id: &'static str,
    /// The domain's state, a member of `TransitGatewayMulticastDomainState`.
    state: &'static str,
}

/// A member of `TransitGatewayMulticastDomainState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayMulticastDomainState")`, whose values
/// are `available | deleted | deleting | pending`) rather than written out here.
fn domain_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_MULTICAST_DOMAIN_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayMulticastDomainState member"))
}

/// The seven domains every test here is written against: three transit gateways holding two,
/// two and three domains, and all four states the modelled catalog publishes (`available`
/// three times and `pending` twice, so a state filter selects more than one row).
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            state: domain_state("available"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000a",
            state: domain_state("pending"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            state: domain_state("available"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000b",
            state: domain_state("deleting"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: domain_state("deleted"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: domain_state("available"),
        },
        Fixture {
            transit_gateway_id: "tgw-0000000000000000c",
            state: domain_state("pending"),
        },
    ]
}

/// Seed [`fixtures`] into the store through its own writer and answer the ids it minted, in
/// fixture order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for fixture in fixtures() {
        let domain_id = guard
            .insert_transit_gateway_multicast_domain(TransitGatewayMulticastDomainRecord {
                // The store mints the id; what a writer puts here is replaced by it.
                transit_gateway_multicast_domain_id: String::new(),
                transit_gateway_id: fixture.transit_gateway_id.to_string(),
                owner_id: engine::emulator_account_id().to_string(),
                state: fixture.state.to_string(),
                associations: Vec::new(),
            })
            .expect("the store holds the seeded multicast domain");
        ids.push(domain_id);
    }
    ids
}

/// The emulator with the domains above seeded: the router to call and the ids they were
/// given, in fixture order.
fn seeded() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed(&engine);
    (router, engine, ids)
}

/// One `DescribeTransitGatewayMulticastDomains` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The same call, requiring HTTP 200, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// The ids of every domain a response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "transitGatewayMulticastDomainId")
}

/// `TransitGatewayMulticastDomainIds.N` form fields for `ids`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("TransitGatewayMulticastDomainIds", ids)
}

/// Read something off the one domain element a response carries — the element that has a
/// `transitGatewayMulticastDomainId` child. One lookup and one panic message, shared by the
/// two readers below.
fn with_domain<T>(
    case: &str,
    body: &str,
    read: impl FnOnce(roxmltree::Node<'_, '_>) -> T,
) -> T {
    let parsed = document(case, body);
    let domain = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "transitGatewayMulticastDomainId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no multicast domain in {body}"));
    read(domain)
}

/// A member of the one domain a response carries, read through the element that has a
/// `transitGatewayMulticastDomainId` child, or `None` when the domain carries no such member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_domain(case, body, |domain| child_text(domain, name))
}

/// Whether the one domain a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member the store holds no fact for: [`member`] reads an element's
/// TEXT, which a present-but-empty container (`<options/>`, `<tagSet/>`) does not carry, so
/// only element absence shows that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_domain(case, body, |domain| child(domain, name).is_some())
}

/// Every fact a stored domain carries reads back: the id the store minted, the transit
/// gateway it belongs to, the account that owns it and the state it is in. One row per seeded
/// domain, so no single domain is the specification.
///
/// The members a stored record holds no fact for — `transitGatewayMulticastDomainArn`,
/// `options`, `creationTime` and `tagSet` — are asserted ABSENT: nothing in this emulator has
/// recorded them (`CreateTransitGatewayMulticastDomain` is still a scaffold stub), and a
/// describe must report what the store holds rather than invent a value for it.
#[tokio::test]
async fn every_fact_a_stored_domain_carries_reads_back() {
    let (router, _engine, ids) = seeded();

    for (fixture, domain_id) in fixtures().into_iter().zip(&ids) {
        let case = format!("domain {domain_id} of {}", fixture.transit_gateway_id);
        let body = describe_ok(&case, &router, &id_params(&[domain_id.as_str()])).await;

        assert_eq!(
            reported_ids(&body),
            vec![domain_id.clone()],
            "case {case:?}: expected exactly the domain named, got {body}"
        );
        for (name, expected) in [
            ("transitGatewayId", fixture.transit_gateway_id.to_string()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("state", fixture.state.to_string()),
        ] {
            let actual = member(&case, &body, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> {expected:?}, got {actual:?} in {body}"
            );
        }
        for name in [
            "transitGatewayMulticastDomainArn",
            "options",
            "creationTime",
            "tagSet",
        ] {
            assert!(
                !has_member(&case, &body, name),
                "case {case:?}: no stored domain carries <{name}>, so none may be reported, \
                 got {body}"
            );
        }
    }
}

/// A request that names no selector reports every domain the store holds, in
/// `TransitGatewayMulticastDomainId` order.
#[tokio::test]
async fn an_unselective_describe_reports_every_stored_domain_in_id_order() {
    let (router, _engine, ids) = seeded();
    let case = "no selector";

    let body = describe_ok(case, &router, &[]).await;
    let mut expected = ids.clone();
    expected.sort();
    let actual = reported_ids(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?} in id order, got {actual:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: a complete answer carries no nextToken, got {body}"
    );
}

/// An empty store is an empty answer, not a refusal: a describe of a resource kind nothing
/// has created reports no domains at all.
#[tokio::test]
async fn an_empty_store_reports_no_domains() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "empty store";

    let body = describe_ok(case, &router, &[]).await;
    let actual = reported_ids(&body);
    assert!(
        actual.is_empty(),
        "case {case:?}: expected no domains, got {actual:?} in {body}"
    );
}

/// Each filter the operation documents selects exactly the domains carrying its value, and
/// several filters are joined with AND. The rows span the three documented names, a value
/// nothing carries, a wildcard value, and a two-filter combination.
#[tokio::test]
async fn each_documented_filter_selects_the_domains_that_carry_its_value() {
    let (router, _engine, ids) = seeded();
    let rows = fixtures();

    // (case, the filters as sent, the fixture positions expected back)
    let cases: Vec<(&str, Vec<Param>, Vec<usize>)> = vec![
        (
            "state available",
            filters(&[("state", &[domain_state("available")])]),
            vec![0, 2, 5],
        ),
        (
            "state pending",
            filters(&[("state", &[domain_state("pending")])]),
            vec![1, 6],
        ),
        (
            "state deleting",
            filters(&[("state", &[domain_state("deleting")])]),
            vec![3],
        ),
        (
            "state deleted",
            filters(&[("state", &[domain_state("deleted")])]),
            vec![4],
        ),
        (
            "two states in one filter",
            filters(&[(
                "state",
                &[domain_state("deleting"), domain_state("deleted")],
            )]),
            vec![3, 4],
        ),
        (
            "transit-gateway-id of two domains",
            filters(&[("transit-gateway-id", &[rows[0].transit_gateway_id])]),
            vec![0, 1],
        ),
        (
            "transit-gateway-id of three domains",
            filters(&[("transit-gateway-id", &[rows[4].transit_gateway_id])]),
            vec![4, 5, 6],
        ),
        (
            // "Filter values support wildcards" is EC2's filter contract: `*` stands for
            // zero or more characters, so a prefix pattern selects every transit gateway.
            "transit-gateway-id as a wildcard",
            filters(&[("transit-gateway-id", &["tgw-*"])]),
            (0..rows.len()).collect(),
        ),
        (
            "transit-gateway-id nothing carries",
            filters(&[("transit-gateway-id", &["tgw-00000000000000fff"])]),
            Vec::new(),
        ),
        (
            "state and transit-gateway-id joined with AND",
            filters(&[
                ("state", &[domain_state("available")]),
                ("transit-gateway-id", &[rows[4].transit_gateway_id]),
            ]),
            vec![5],
        ),
        (
            "state and transit-gateway-id that no one domain carries together",
            filters(&[
                ("state", &[domain_state("deleting")]),
                ("transit-gateway-id", &[rows[0].transit_gateway_id]),
            ]),
            Vec::new(),
        ),
    ];

    for (case, params, expected_positions) in cases {
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }

    // The id filter is the id selector's twin, so it is asserted over every seeded domain
    // rather than over one row.
    for domain_id in &ids {
        let case = format!("transit-gateway-multicast-domain-id {domain_id}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[("transit-gateway-multicast-domain-id", &[domain_id.as_str()])]),
        )
        .await;
        let actual = reported_ids(&body);
        assert_eq!(
            actual,
            vec![domain_id.clone()],
            "case {case:?}: expected exactly that domain, got {actual:?} in {body}"
        );
    }
}

/// One row of the id-list table: the case, the ids the request names, the filters it
/// carries beside them, and the fixture positions the answer must carry.
type IdListCase<'a> = (&'static str, Vec<&'a str>, Vec<Param>, Vec<usize>);

/// The id list selects exactly the domains it names, whatever its length and however often
/// an id is repeated, and it narrows what a filter would have selected.
#[tokio::test]
async fn an_id_list_selects_exactly_the_domains_it_names() {
    let (router, _engine, ids) = seeded();

    let cases: Vec<IdListCase<'_>> = vec![
        ("one id", vec![ids[3].as_str()], Vec::new(), vec![3]),
        (
            "three ids, named out of order",
            vec![ids[5].as_str(), ids[0].as_str(), ids[2].as_str()],
            Vec::new(),
            vec![0, 2, 5],
        ),
        (
            "the same id twice",
            vec![ids[1].as_str(), ids[1].as_str()],
            Vec::new(),
            vec![1],
        ),
        (
            "every id",
            ids.iter().map(String::as_str).collect(),
            Vec::new(),
            (0..ids.len()).collect(),
        ),
        (
            "two ids narrowed by a state filter",
            vec![ids[0].as_str(), ids[1].as_str()],
            filters(&[("state", &[domain_state("pending")])]),
            vec![1],
        ),
    ];

    for (case, named, extra, expected_positions) in cases {
        let mut params = id_params(&named);
        params.extend(extra);
        let body = describe_ok(case, &router, &params).await;
        let mut expected: Vec<String> = expected_positions
            .iter()
            .map(|position| ids[*position].clone())
            .collect();
        expected.sort();
        let actual = reported_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

/// A well-formed id naming no domain is refused with the domain's own `NotFound` code, and a
/// value that is not a domain id at all with its `Malformed` code — so a client can tell the
/// two apart, and neither is reported as an absent row. Both are asserted beside an id that
/// DOES name a domain, so the refusal does not depend on the request naming nothing real.
#[tokio::test]
async fn an_unknown_id_is_refused_not_found_and_a_value_that_is_no_id_malformed() {
    let (router, _engine, ids) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX;

    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "one unknown id",
            vec![format!("{prefix}-00000000000000fff")],
            ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            vec![ids[0].clone(), format!("{prefix}-00000000000000eee")],
            ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND,
        ),
        (
            "a suffix that is not hexadecimal",
            vec![format!("{prefix}-not-hex")],
            ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        ),
        (
            "an id of another resource of the same family",
            vec!["tgw-attach-0000000000000000a".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        ),
        (
            "a known id beside a value that is no id",
            vec![ids[0].clone(), "not-an-id".to_string()],
            ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        ),
    ];

    for (case, named, expected) in cases {
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &id_params(&borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The refusal names the first id the CALLER listed that could not be resolved, not
    // whichever of them sorts first: the two rows name the same pair of unknown ids in the two
    // possible orders, so an implementation that walked a sorted set would fail one of them.
    let lower = format!("{prefix}-00000000000000aaa");
    let higher = format!("{prefix}-00000000000000bbb");
    for (case, first, second) in [
        ("the lower id listed first", &lower, &higher),
        ("the higher id listed first", &higher, &lower),
    ] {
        let (status, body) =
            describe(&router, &id_params(&[first.as_str(), second.as_str()])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(first.as_str()),
            "case {case:?}: the refusal must name {first:?}, got {message:?}"
        );
    }
}

/// A filter name this operation does not document is refused, naming the filter — never read
/// as "no results". The rows span a misspelling, a filter of ANOTHER describe, this
/// operation's own names in the wrong case ("Filter names are case-sensitive"), the tag
/// filters this operation documents none of, and an empty name.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, _engine, _ids) = seeded();

    for name in [
        "transit-gateway-multicast-domain-state",
        "owner-id",
        "vpc-id",
        "State",
        "Transit-Gateway-Id",
        "tag:Name",
        "tag-key",
        "",
    ] {
        let case = format!("filter {name:?}");
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the filter, got {message:?}"
        );
    }
}

/// The ceilings this handler puts on the SHAPE of a request, which the model bounds nowhere
/// (`FilterList`, `Filter$Values` and `TransitGatewayMulticastDomainIdStringList` carry no
/// `@length`), so the handler is what enforces them. Every row is asserted AT the published
/// ceiling — where the request is answered, not refused for its size — and one past it, where
/// it is refused `InvalidParameterValue`: a ceiling that drifted either way fails here.
///
/// At the id ceiling the ids still name no domain, so that row is answered with the domain's
/// own `NotFound` code: proof the SIZE check passed and the request went on to be resolved.
#[tokio::test]
async fn a_request_past_a_published_shape_ceiling_is_refused() {
    let (router, _engine, _ids) = seeded();
    let prefix = ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX;
    // A state no domain carries, so an in-bounds filter row selects nothing and the assertion
    // is about the ceiling rather than about what matched.
    let unmatched = "no-such-state";
    let one_unmatched: [&str; 1] = [unmatched];
    let at_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let past_byte_ceiling = "v".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);

    // (case, the request's fields, the error code expected — `None` where HTTP 200 is)
    let mut cases: Vec<(String, Vec<Param>, Option<&str>)> = Vec::new();
    for (case, count, expected) in [
        ("filters at the ceiling", ec2::state::MAX_FILTERS, None),
        (
            "one filter past the ceiling",
            ec2::state::MAX_FILTERS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &one_unmatched[..]))
            .collect();
        cases.push((case.to_string(), filters(&entries), expected));
    }
    for (case, count, expected) in [
        ("filter values at the ceiling", ec2::state::MAX_FILTER_VALUES, None),
        (
            "one filter value past the ceiling",
            ec2::state::MAX_FILTER_VALUES + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let values: Vec<&str> = (0..count).map(|_| unmatched).collect();
        cases.push((case.to_string(), filters(&[("state", &values)]), expected));
    }
    for (case, value, expected) in [
        ("a filter value at the byte ceiling", &at_byte_ceiling, None),
        (
            "a filter value one byte past the ceiling",
            &past_byte_ceiling,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        cases.push((
            case.to_string(),
            filters(&[("state", &[value.as_str()])]),
            expected,
        ));
    }
    for (case, count, expected) in [
        (
            "ids at the ceiling",
            ec2::state::MAX_RESOURCE_IDS,
            Some(ec2::state::INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND),
        ),
        (
            "one id past the ceiling",
            ec2::state::MAX_RESOURCE_IDS + 1,
            Some(ec2::state::INVALID_PARAMETER_VALUE),
        ),
    ] {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("{prefix}-{:017x}", index + 1))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        cases.push((case.to_string(), id_params(&borrowed), expected));
    }

    for (case, params, expected) in cases {
        let (status, body) = describe(&router, &params).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request at a ceiling is answered, got {status} with body \
                     {body}"
                );
                let reported = reported_ids(&body);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: no domain carries {unmatched:?}, got {reported:?} in {body}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    code,
                    "case {case:?}: expected {code}, got {body}"
                );
            }
        }
    }
}

/// The answer pages in id order: a page carries at most `MaxResults` domains and a token
/// naming the id the next page starts at, the token walks the whole set exactly once, and
/// the last page carries none. Asserted at the published floor of `TransitGatewayMaxResults`
/// and at a size that holds everything.
#[tokio::test]
async fn the_answer_pages_in_id_order_with_a_token_that_advances() {
    let (router, _engine, ids) = seeded();
    let mut all = ids.clone();
    all.sort();

    for page_size in [5_usize, 7, 1000] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One iteration per page, plus one to catch a token that never clears.
        for _ in 0..=all.len() {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size}, got {page:?}"
            );
            walked.extend(page.clone());
            token = element(&body, "nextToken");
            match token.as_deref() {
                None => break,
                Some(cursor) => assert!(
                    !cursor.is_empty() && page.len() == page_size,
                    "case {case:?}: a token is minted only for a full page, got {body}"
                ),
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end with no token, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must cover every domain once, in id order, got {walked:?}"
        );
    }
}

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused at the boundary, one past each published bound, while each
/// bound itself is served. The refusal is the boundary validator's `ValidationError`, whose
/// message names the member and the constraint it failed — asserted, so a 400 for some other
/// reason could not pass for it.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine, _ids) = seeded();

    for (case, page_size, expected_status) in [
        ("one below the published minimum", 4, 400),
        ("the published minimum", 5, 200),
        ("the published maximum", 1000, 200),
        ("one above the published maximum", 1001, 400),
    ] {
        let (status, body) = describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                BOUNDARY_VALIDATION_ERROR,
                "case {case:?}: expected {BOUNDARY_VALIDATION_ERROR}, got {body}"
            );
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains("maxResults"),
                "case {case:?}: the refusal must name the member it rejected, got {message:?}"
            );
        }
    }
}

/// The wire error code the boundary validator refuses a request with when a modelled
/// constraint rejects it before the handler runs — as `MaxResults` outside
/// `TransitGatewayMaxResults`' published range is.
const BOUNDARY_VALIDATION_ERROR: &str = "ValidationError";

/// A `NextToken` this service did not mint is refused rather than read as an id, whatever
/// else the request names.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let (router, _engine, ids) = seeded();

    for (case, token) in [
        ("plain text", "not-a-token"),
        ("an id of the kind being paged", ids[0].as_str()),
        ("odd-length hex", "abc"),
    ] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// An authorized dry run reports the permission outcome and describes nothing, whichever
/// selector the request carries — "checks whether you have the required permissions for the
/// action, WITHOUT ACTUALLY MAKING THE REQUEST".
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome_and_describes_nothing() {
    let (router, _engine, ids) = seeded();

    let cases: Vec<(&str, Vec<Param>)> = vec![
        ("no selector", Vec::new()),
        ("an id that names a domain", id_params(&[ids[0].as_str()])),
        (
            "a filter that selects domains",
            filters(&[("state", &[domain_state("available")])]),
        ),
    ];

    for (case, selector) in cases {
        let mut params = selector;
        params.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a dry run describes nothing, got {body}"
        );
    }
}

/// Describing changes nothing a later caller sees: the same request answers the same domains
/// twice over, and the store holds exactly the domains it held before — a read neither mints
/// nor moves a resource.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine, ids) = seeded();
    let case = "the same request twice";

    let stored_before = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_multicast_domains
        .len();
    let first = describe_ok(case, &router, &[]).await;
    let second = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(&first),
        reported_ids(&second),
        "case {case:?}: two reads must report the same domains, got {first} then {second}"
    );
    for name in ["state", "transitGatewayId", "creationTime", "ownerId"] {
        assert_eq!(
            elements(&first, name),
            elements(&second, name),
            "case {case:?}: <{name}> must not change between reads, got {first} then {second}"
        );
    }
    let stored_after = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_multicast_domains
        .len();
    assert_eq!(
        stored_after, stored_before,
        "case {case:?}: the store held {stored_before} domains, now holds {stored_after}"
    );
    assert_eq!(
        stored_after,
        ids.len(),
        "case {case:?}: the store must still hold every seeded domain"
    );
}
