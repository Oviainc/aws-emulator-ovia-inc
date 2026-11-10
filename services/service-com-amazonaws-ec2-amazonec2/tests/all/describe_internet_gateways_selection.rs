// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `DescribeInternetGateways` report IS: the set the request
//! selected out of the internet gateway store, walked one page at a time. Driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: **a report is the SELECTED set, paged** — `InternetGatewayIds` and
//! `Filters` narrow the store first ("The default is to describe all your internet
//! gateways", `DescribeInternetGatewaysRequest$InternetGatewayIds`), and `MaxResults` with
//! `NextToken` then walk exactly that narrowed set in `InternetGatewayId` order, every page
//! but the last filled to the size asked for and a token answered only while a further
//! SELECTED gateway follows ("This value is null when there are no more items to return",
//! `DescribeInternetGatewaysResult$NextToken`). A page size outside the published range is
//! refused rather than clamped, and the whole walk is a READ: it leaves the store exactly as
//! the creates and the attaches left it.
//!
//! The sibling suite `internet_gateways` pins what one gateway's record carries and how each
//! selector narrows an unpaged report; this one pins the INTERACTION the paged clients of
//! this operation depend on — that the page walk is over the narrowed set rather than over
//! the store, whichever selector narrowed it.
//!
//! Every row is a distinct input along what that rule generalises over — which kind of
//! selector narrows the report (none, an id list, a tag filter, a tag-key filter, an
//! owner filter, either `attachment.…` filter, an id list AND a filter, a filter nothing
//! satisfies), how large the selected set is (none, one, some, all) and how large a page is
//! asked for (none, the published minimum, a size between, the published maximum) — so no
//! single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed, member};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for. An internet gateway carries no Region member,
/// so the Region only picks which account's store the calls land in.
const REGION: &str = "us-east-1";

/// How many gateways the estate holds. Larger than the biggest page any row asks for, so a
/// walk of the published minimum page size takes more than one page for the widest
/// selection.
const GATEWAYS: usize = 11;

/// The published bounds of this operation's page size, from
/// `smithy_query(query="shape",shape="DescribeInternetGatewaysMaxResults")`:
/// `range 5..1000`.
const MIN_PAGE_SIZE: usize = 5;
const MAX_PAGE_SIZE: usize = 1_000;

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The `TagSpecifications` form fields that assign `tags` to the gateway a create mints, as
/// ec2Query flattens them.
fn tag_params(tags: &[(&str, String)]) -> Vec<Param> {
    let mut params = Vec::new();
    if tags.is_empty() {
        return params;
    }
    params.push(param("TagSpecification.1.ResourceType", "internet-gateway"));
    for (position, (key, value)) in tags.iter().enumerate() {
        let index = position + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            value,
        ));
    }
    params
}

/// The ids of the gateways a `DescribeInternetGateways` response carries, in report order.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "internetGatewaySet")
        .into_iter()
        .filter_map(|item| child_text(item, "internetGatewayId"))
        .collect()
}

/// The `(state, vpcId)` attachments the one gateway of a narrowed report carries.
fn reported_attachments(case: &str, body: &str) -> Vec<(Option<String>, Option<String>)> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "internetGatewaySet")
        .into_iter()
        .flat_map(|gateway| set_items(gateway, "attachmentSet"))
        .map(|item| (child_text(item, "state"), child_text(item, "vpcId")))
        .collect()
}

/// The response's `nextToken`, told apart from one that is present and empty: either reads
/// as "no further page is due".
fn next_token(body: &str) -> Option<String> {
    member(body, "nextToken").filter(|token| !token.is_empty())
}

/// The estate every row reads: the gateways the account holds, which VPC each attached one
/// is attached to, and how the tag plane cuts across that.
#[derive(Debug)]
struct Estate {
    /// Every gateway the account holds, in `InternetGatewayId` order.
    all: Vec<String>,
    /// Each attached gateway as `(vpc_id, internet_gateway_id)`, in attach order.
    attached: Vec<(String, String)>,
    /// The gateways tagged `Group=a`, in `InternetGatewayId` order.
    group_a: Vec<String>,
    /// The gateways tagged `Group=b`, in `InternetGatewayId` order.
    group_b: Vec<String>,
    /// The account every gateway of this emulator belongs to.
    account: String,
}

/// Build the estate: [`GATEWAYS`] gateways, every other one attached to a VPC of its own (a
/// VPC takes at most one internet gateway), and the `Group` tag spread over a different
/// partition than attachment, so a tag selection and an attachment selection are genuinely
/// different subsets rather than the same one asked for twice.
async fn build_estate(router: &MultiServiceRouter) -> Estate {
    let case = "the estate";
    let mut all = Vec::new();
    let mut attached = Vec::new();
    let mut group_a = Vec::new();
    let mut group_b = Vec::new();

    for index in 0..GATEWAYS {
        let group = if index % 3 == 0 { "a" } else { "b" };
        let tags = [("Group", group.to_string()), ("Index", index.to_string())];
        let body = ok(case, router, "CreateInternetGateway", &tag_params(&tags)).await;
        // The created record is nested inside `<internetGateway>`, so the id is read off the
        // tree rather than from the envelope's own children.
        let parsed = document(case, &body);
        let internet_gateway_id = parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "internetGatewayId")
            .and_then(|node| node.text())
            .unwrap_or_else(|| panic!("case {case:?}: no <internetGatewayId> in {body}"))
            .to_string();

        if group == "a" {
            group_a.push(internet_gateway_id.clone());
        } else {
            group_b.push(internet_gateway_id.clone());
        }

        if index % 2 == 0 {
            let vpc_body = ok(
                case,
                router,
                "CreateVpc",
                &[param("CidrBlock", &format!("10.{index}.0.0/16"))],
            )
            .await;
            let vpc_id = super::ec2_query::element(&vpc_body, "vpcId")
                .unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {vpc_body}"));
            ok(
                case,
                router,
                "AttachInternetGateway",
                &[
                    param("InternetGatewayId", &internet_gateway_id),
                    param("VpcId", &vpc_id),
                ],
            )
            .await;
            attached.push((vpc_id, internet_gateway_id.clone()));
        }

        all.push(internet_gateway_id);
    }

    all.sort();
    group_a.sort();
    group_b.sort();
    Estate {
        all,
        attached,
        group_a,
        group_b,
        account: engine::emulator_account_id().to_string(),
    }
}

/// One row: the selector that narrows the report, and the gateways it must select, in
/// `InternetGatewayId` order.
struct Selection {
    case: String,
    selector: Vec<Param>,
    expected: Vec<String>,
}

/// `ids` in `InternetGatewayId` order.
fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids
}

/// Every selector row, over the estate just built.
fn selections(estate: &Estate) -> Vec<Selection> {
    let row = |case: &str, selector: Vec<Param>, expected: Vec<String>| Selection {
        case: case.to_string(),
        selector,
        expected,
    };

    // An id list naming most of the estate, handed over in DESCENDING id order: the report's
    // order is the id order, never the order the request listed.
    let mut listed: Vec<String> = estate.all.iter().take(7).cloned().collect();
    listed.sort();
    let requested_backwards: Vec<&str> = listed.iter().rev().map(String::as_str).collect();
    let listed_and_group_b: Vec<String> = listed
        .iter()
        .filter(|id| estate.group_b.contains(id))
        .cloned()
        .collect();

    let (attached_vpc, attached_gateway) = estate
        .attached
        .first()
        .cloned()
        .expect("the estate attaches at least one gateway");

    vec![
        row(
            "no selector reports every gateway",
            Vec::new(),
            estate.all.clone(),
        ),
        row(
            "owner-id names the one account this emulator serves",
            filters(&[("owner-id", &[estate.account.as_str()])]),
            estate.all.clone(),
        ),
        row(
            "an id list of seven, handed over backwards",
            indexed("InternetGatewayId", &requested_backwards),
            listed.clone(),
        ),
        row(
            "an id list of one",
            indexed("InternetGatewayId", &[estate.all[3].as_str()]),
            vec![estate.all[3].clone()],
        ),
        row(
            "tag:Group selects the gateways under that value",
            filters(&[("tag:Group", &["a"])]),
            estate.group_a.clone(),
        ),
        row(
            "tag:Group selects the complementary value just as well",
            filters(&[("tag:Group", &["b"])]),
            estate.group_b.clone(),
        ),
        row(
            "tag-key names a key every gateway carries",
            filters(&[("tag-key", &["Index"])]),
            estate.all.clone(),
        ),
        row(
            "attachment.state selects every attached gateway",
            filters(&[("attachment.state", &["available"])]),
            sorted(estate.attached.iter().map(|(_, id)| id.clone())),
        ),
        row(
            "attachment.vpc-id selects the one gateway of that VPC",
            filters(&[("attachment.vpc-id", &[attached_vpc.as_str()])]),
            vec![attached_gateway],
        ),
        row(
            "an id list AND a tag filter is their intersection",
            indexed("InternetGatewayId", &requested_backwards)
                .into_iter()
                .chain(filters(&[("tag:Group", &["b"])]))
                .collect(),
            listed_and_group_b,
        ),
        row(
            "a filter nothing satisfies selects nothing",
            filters(&[("tag:Group", &["c"])]),
            Vec::new(),
        ),
        row(
            "an attachment filter no VPC of the estate matches selects nothing",
            filters(&[("attachment.vpc-id", &["vpc-00000000000000999"])]),
            Vec::new(),
        ),
    ]
}

/// Walk `selector`'s report with `page_size` (none asks for no `MaxResults` at all) and
/// answer the gateways reported, in report order, and the number of pages the walk took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    selector: &[Param],
    page_size: Option<usize>,
    expected_len: usize,
) -> (Vec<String>, usize) {
    let mut seen: Vec<String> = Vec::new();
    let mut page_lengths: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut params = selector.to_vec();
        if let Some(size) = page_size {
            params.push(param("MaxResults", &size.to_string()));
        }
        if let Some(cursor) = token.as_deref() {
            params.push(param("NextToken", cursor));
        }
        let (status, body) = call(router, "DescribeInternetGateways", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {} expected HTTP 200, got {status} with body {body}",
            page_lengths.len() + 1
        );
        let page = reported_ids(case, &body);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: page {} expected at most {size} gateways, got {page:?}",
                page_lengths.len() + 1
            );
        }
        page_lengths.push(page.len());
        seen.extend(page);
        token = next_token(&body);
        assert!(
            page_lengths.len() <= expected_len + 2,
            "case {case:?}: the walk must terminate, took {} pages for {expected_len} \
             gateways, last body {body}",
            page_lengths.len()
        );
        if token.is_none() {
            break;
        }
    }

    // A token is due only while a further SELECTED gateway follows, so every page but the
    // last is filled to the size asked for.
    if let Some(size) = page_size {
        if let Some((_, leading)) = page_lengths.split_last() {
            for (position, length) in leading.iter().enumerate() {
                assert_eq!(
                    *length,
                    size,
                    "case {case:?}: page {} is followed by a token, so it must be full at \
                     {size} gateways, carried {length}",
                    position + 1
                );
            }
        }
    }

    (seen, page_lengths.len())
}

/// The rule: every selector's report is exactly the selected set, walked in
/// `InternetGatewayId` order at whatever page size the request asks for — and the walk
/// changes nothing a later caller sees.
#[tokio::test]
async fn a_report_is_the_selected_set_walked_one_page_at_a_time() {
    let (router, _engine) = super::ec2_query::emulator();
    let estate = build_estate(&router).await;

    // No `MaxResults` at all, the published minimum, a size between it and the estate, and
    // the published maximum: the four page sizes a client of this operation can ask for.
    let page_sizes = [None, Some(MIN_PAGE_SIZE), Some(6), Some(MAX_PAGE_SIZE)];

    for selection in selections(&estate) {
        for page_size in page_sizes {
            let case = format!("{} at page size {page_size:?}", selection.case);
            let (seen, pages) = walk(
                &router,
                &case,
                &selection.selector,
                page_size,
                selection.expected.len(),
            )
            .await;
            assert_eq!(
                seen, selection.expected,
                "case {case:?}: expected exactly the selected gateways in id order, got {seen:?}"
            );
            let expected_pages = match page_size {
                Some(size) => selection.expected.len().div_ceil(size).max(1),
                // A request naming no page size is answered with the largest page the
                // operation publishes, which holds the whole estate.
                None => 1,
            };
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {} selected gateways to take {expected_pages} \
                 page(s), took {pages}",
                selection.expected.len()
            );
        }
    }

    // The walk is a READ: every gateway the estate holds is still there, and every
    // attachment still reads back under the VPC it was attached to.
    let body = ok("after the walk", &router, "DescribeInternetGateways", &[]).await;
    assert_eq!(
        reported_ids("after the walk", &body),
        estate.all,
        "after the walk: expected the store untouched, got {body}"
    );
    for (vpc_id, internet_gateway_id) in &estate.attached {
        let case = format!("after the walk, the gateway of {vpc_id}");
        let body = ok(
            &case,
            &router,
            "DescribeInternetGateways",
            &filters(&[("attachment.vpc-id", &[vpc_id.as_str()])]),
        )
        .await;
        assert_eq!(
            reported_ids(&case, &body),
            vec![internet_gateway_id.clone()],
            "case {case:?}: expected the attachment to survive the walk, got {body}"
        );
        assert_eq!(
            reported_attachments(&case, &body),
            vec![(Some("available".to_string()), Some(vpc_id.clone()))],
            "case {case:?}: expected the one attachment the attach recorded, got {body}"
        );
    }
}

/// The rule's bound: a page size outside the published range is refused rather than clamped
/// to the nearest size the operation serves, so a client is never silently answered a page
/// it did not ask for. Each published bound is served, one step past either is refused
/// (`DescribeInternetGatewaysMaxResults`, `range 5..1000`).
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let estate = build_estate(&router).await;

    for size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
        let case = format!("the published page size {size}");
        let body = ok(
            &case,
            &router,
            "DescribeInternetGateways",
            &[param("MaxResults", &size.to_string())],
        )
        .await;
        let reported = reported_ids(&case, &body);
        let expected = estate.all.len().min(size);
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} gateways on a page of {size}, got {reported:?}"
        );
    }

    // One step past either published bound is refused with HTTP 400. Below the smallest page
    // the operation pages with, EC2 answers its own code for a page size it will not page
    // with; above the largest, the refusal is the constraint clause for the member, the same
    // code and sentence every other paged describe of this crate answers there.
    for (size, code, bound) in [
        (MIN_PAGE_SIZE - 1, "InvalidMaxResults", MIN_PAGE_SIZE),
        (MAX_PAGE_SIZE + 1, "ValidationError", MAX_PAGE_SIZE),
    ] {
        let case = format!("the page size {size}, one step past a published bound");
        let (status, body) = call(
            &router,
            "DescribeInternetGateways",
            &[param("MaxResults", &size.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the refusal {code}, got body {body}"
        );
        // A refusal has to say WHICH member it refused and what it would have taken, so a
        // right code carrying the wrong sentence is caught too: the member the model
        // declares, the value the request sent, and the bound that value missed — all three
        // taken from the request and the model rather than written into the handler.
        let message = error_message(&body);
        for part in ["maxResults", &size.to_string(), &bound.to_string()] {
            assert!(
                message.contains(part),
                "case {case:?}: expected the refusal to name {part:?}, got message \
                 {message:?} in body {body}"
            );
        }
    }
}
