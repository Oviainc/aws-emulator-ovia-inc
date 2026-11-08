// SPDX-License-Identifier: CC0-1.0

//! `ListAnycastIpLists`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's Anycast static IP list
//! listing states:
//!
//! * the answer reports the Anycast static IP lists the STORE HOLDS, each item
//!   carrying the same record a `GetAnycastIpList` of that list answers with, in one stable
//!   total order over the ids the records are held under — so a store that holds none
//!   lists none;
//! * an item is "an abbreviated version of the `AnycastIpList` structure" that "omits the
//!   allocated static IP addresses", so no page reports them however many a list holds,
//!   while it does carry the version token the read reports in its `ETag` header;
//! * the `Items` element is part of every answer this collection serves — a page that lists
//!   nothing carries an empty one rather than leaving it out;
//! * `MaxItems` cuts the page and is echoed as asked for, `Quantity` states the page's
//!   length, `IsTruncated` and `NextMarker` are present exactly when lists remain and the
//!   marker names the id the page ended at, and a request resumes AFTER the id its `Marker`
//!   names — so walking the pages at ANY admissible page size enumerates every list exactly
//!   once, in one order;
//! * a page size that names no page is refused with `InvalidArgument` (400), and a refusal
//!   changes nothing;
//! * `Status` is owed by the CLOCK: a list that is still deploying is listed as such until
//!   its deadline passes and as deployed after;
//! * a listing is a READ: it answers the same records again and moves no list's status or
//!   version token.
//!
//! The rows differ along everything these rules generalise over: the number of lists the
//! store holds (none, two, several), the page size (one, several, the collection size,
//! past it and past the server's own ceiling), where the resume point sits and whether it
//! names a stored list at all, and the shape of the record listed — the create table's own
//! rows, which span both documented `IpCount` values, every `IpAddressType` the model
//! admits plus the create that names none, and a create with no IPAM CIDR configuration
//! and one with two.
//!
//! The wire harness (the router, the request helper and the element scanner) is
//! [`crate::distribution_lifecycle`]'s, and the create path and its table are the Anycast
//! static IP list lifecycle suite's, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ANYCAST_IP_LIST_STATUS_DEPLOYING, MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE, STATUS_DEPLOYED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{
    anycast_ips, create_anycast_ip_list, create_table, get_anycast_ip_list, Create,
};
use crate::distribution_lifecycle::{call, emulator, past_propagation, Answer, Node, API_VERSION};

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/anycast-ip-list`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=ipl-…`), from the operation's `http` binding.
async fn list_anycast_ip_lists(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/anycast-ip-list{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// One `AnycastIpListSummary` a page reports: every member the shape carries, with the IPAM
/// configuration read down to the CIDRs it names and the status each of them is in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    id: String,
    name: String,
    status: String,
    arn: String,
    ip_count: String,
    last_modified_time: String,
    ip_address_type: Option<String>,
    e_tag: Option<String>,
    ipam: Vec<(String, String, String)>,
}

/// The IPAM CIDR configurations one document reports, as `(Cidr, IpamPoolArn, Status)`.
fn ipam_configs(list: &Node) -> Vec<(String, String, String)> {
    list.at("IpamConfig/IpamCidrConfigs")
        .map(|wrapper| {
            wrapper
                .children_named("IpamCidrConfig")
                .into_iter()
                .map(|config| {
                    (
                        config.text_at("Cidr").unwrap_or_default().to_string(),
                        config.text_at("IpamPoolArn").unwrap_or_default().to_string(),
                        config.text_at("Status").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read one summary element into an [`Item`]. `e_tag` is the member the SUMMARY carries;
/// the full structure reports the same token in the response's `ETag` header instead.
fn read_item(summary: &Node) -> Item {
    Item {
        id: summary.text_at("Id").unwrap_or_default().to_string(),
        name: summary.text_at("Name").unwrap_or_default().to_string(),
        status: summary.text_at("Status").unwrap_or_default().to_string(),
        arn: summary.text_at("Arn").unwrap_or_default().to_string(),
        ip_count: summary.text_at("IpCount").unwrap_or_default().to_string(),
        last_modified_time: summary
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string(),
        ip_address_type: summary.text_at("IpAddressType").map(str::to_string),
        e_tag: summary.text_at("ETag").map(str::to_string),
        ipam: ipam_configs(summary),
    }
}

/// Every text one element and its descendants carry, so an assertion about a value the
/// document must NOT report is about elements rather than about a substring of the body (an
/// address is a substring of a CIDR that contains it).
fn texts(node: &Node) -> Vec<String> {
    let mut out = vec![node.text().to_string()];
    for child in &node.children {
        out.extend(texts(child));
    }
    out
}

/// Every list a page reports, in page order.
fn page_items(answer: &Answer) -> Vec<Item> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("AnycastIpListSummary")
                .into_iter()
                .map(read_item)
                .collect()
        })
        .unwrap_or_default()
}

/// The ids a page reports, in the order the page carries them.
fn page_ids(answer: &Answer) -> Vec<String> {
    page_items(answer)
        .into_iter()
        .map(|item| item.id)
        .collect()
}

/// The list `id`'s record as `GetAnycastIpList` reports it — what a listing's own item is
/// held to. The version token comes from that read's `ETag` header, which is where the full
/// structure carries it.
async fn described_item(router: &MultiServiceRouter, id: &str) -> Item {
    let answer = get_anycast_ip_list(router, id).await;
    assert_eq!(
        answer.status, 200,
        "case {id}: expected HTTP 200 from GetAnycastIpList, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    Item {
        id: list.text_at("Id").unwrap_or_default().to_string(),
        name: list.text_at("Name").unwrap_or_default().to_string(),
        status: list.text_at("Status").unwrap_or_default().to_string(),
        arn: list.text_at("Arn").unwrap_or_default().to_string(),
        ip_count: list.text_at("IpCount").unwrap_or_default().to_string(),
        last_modified_time: list
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string(),
        ip_address_type: list.text_at("IpAddressType").map(str::to_string),
        e_tag: Some(answer.header("etag").to_string()),
        ipam: ipam_configs(list),
    }
}

// ---------------------------------------------------------------------------
// The lists under test
// ---------------------------------------------------------------------------

/// What one create of the table left behind: the row, the id it minted, the version token
/// it handed back and the addresses it allocated.
#[derive(Debug, Clone)]
struct Created {
    case: &'static str,
    id: String,
    etag: String,
    addresses: Vec<String>,
}

/// Create every row of `table` through the API and report what each create left behind.
async fn create_all(router: &MultiServiceRouter, table: &[Create]) -> Vec<Created> {
    let mut created: Vec<Created> = Vec::new();
    for create in table {
        let answer = create_anycast_ip_list(router, &create.xml()).await;
        assert_eq!(
            answer.status, 202,
            "case {}: expected HTTP 202 from CreateAnycastIpList, got {} with {}",
            create.case, answer.status, answer.raw
        );
        created.push(Created {
            case: create.case,
            id: answer.root().text_at("Id").unwrap_or_default().to_string(),
            etag: answer.header("etag").to_string(),
            addresses: anycast_ips(answer.root()),
        });
    }
    created
}

/// The ids the created lists are held under, in the order a listing must report them: the
/// one total order over the ids the store holds the records in.
fn expected_order(created: &[Created]) -> Vec<String> {
    let mut ids: Vec<String> = created.iter().map(|one| one.id.clone()).collect();
    ids.sort();
    ids
}

/// Two creates, for the rules that need a collection that is neither empty nor the whole
/// table: the first two rows of the create table.
fn two_rows() -> Vec<Create> {
    create_table().into_iter().take(2).collect()
}

/// Walk the listing at `size` items a page, asserting every page's own bookkeeping, and
/// report every id the walk reported in walk order.
async fn walk_pages(router: &MultiServiceRouter, size: usize, total: usize) -> Vec<String> {
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(value) => format!("?MaxItems={size}&Marker={value}"),
            None => format!("?MaxItems={size}"),
        };
        let page = list_anycast_ip_lists(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case {query}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page.root().text_at("MaxItems"),
            Some(size.to_string().as_str()),
            "case {query}: expected the requested MaxItems echoed, got {}",
            page.raw
        );
        assert_eq!(
            page.root().text_at("Marker"),
            Some(marker.clone().unwrap_or_default().as_str()),
            "case {query}: expected the requested Marker echoed, got {}",
            page.raw
        );
        let ids = page_ids(&page);
        assert!(
            ids.len() <= size.min(ceiling),
            "case {query}: a page may serve at most {} items, got {} in {}",
            size.min(ceiling),
            ids.len(),
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {query}: expected Quantity to state the page's length {}, got {}",
            ids.len(),
            page.raw
        );
        walked.extend(ids.iter().cloned());
        let truncated = walked.len() < total;
        assert_eq!(
            page.root().text_at("IsTruncated"),
            Some(truncated.to_string().as_str()),
            "case {query}: expected IsTruncated {truncated} after {} of {total} lists, got {}",
            walked.len(),
            page.raw
        );
        let next = page.root().text_at("NextMarker").map(str::to_string);
        assert_eq!(
            next.is_some(),
            truncated,
            "case {query}: expected NextMarker present {truncated} after {} of {total} lists, \
             got {}",
            walked.len(),
            page.raw
        );
        match next {
            Some(value) => {
                assert_eq!(
                    Some(value.as_str()),
                    ids.last().map(String::as_str),
                    "case {query}: expected NextMarker to name the id the page ended at, got {}",
                    page.raw
                );
                marker = Some(value);
            }
            None => break,
        }
    }
    walked
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Every Anycast static IP list a caller can see was created by `CreateAnycastIpList`, so
/// a store that holds none lists none: an empty `Items` element, no resume point and
/// `IsTruncated` false, whatever parameters the request carries.
#[tokio::test]
async fn a_store_that_holds_no_list_lists_nothing() {
    let router = emulator();

    // Rows: the bare listing, each parameter a client may send on its own, and both
    // together.
    for query in [
        "",
        "?MaxItems=1",
        "?Marker=ipl-08d1a1b2c3d4e5f67",
        "?MaxItems=5&Marker=zzz",
    ] {
        let listed = list_anycast_ip_lists(&router, query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: expected HTTP 200 from ListAnycastIpLists, got {} with {}",
            listed.status, listed.raw
        );
        assert_eq!(
            page_items(&listed),
            Vec::<Item>::new(),
            "case {query:?}: expected no Anycast static IP lists at all, got {}",
            listed.raw
        );
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some("0"),
            "case {query:?}: expected Quantity 0, got {}",
            listed.raw
        );
        assert!(
            listed.root().at("Items").is_some(),
            "case {query:?}: a page that lists nothing still carries the Items element, got {}",
            listed.raw
        );
        assert_eq!(
            listed.root().text_at("IsTruncated"),
            Some("false"),
            "case {query:?}: expected IsTruncated false when nothing remains, got {}",
            listed.raw
        );
        assert!(
            listed.root().text_at("NextMarker").is_none(),
            "case {query:?}: expected no NextMarker when nothing remains, got {}",
            listed.raw
        );
    }

    let listed = list_anycast_ip_lists(&router, "").await;
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case the default page size: expected MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Marker"),
        Some(""),
        "case the default page: expected the empty Marker a request naming none reports, got {}",
        listed.raw
    );
}

/// The listing reports each stored record: every list the store holds, exactly once, in
/// the one total order over the ids they are held under, each item carrying the record a
/// read of that list answers with — and never the allocated static IP addresses, which the
/// summary omits.
#[tokio::test]
async fn the_listing_reports_every_created_list_in_id_order() {
    let router = emulator();
    let table = create_table();
    let created = create_all(&router, &table).await;
    // Past the deployment span, so the status every list reports is the terminal one
    // whichever call reads it — the listing and the read it is compared against.
    tokio::time::sleep(past_propagation()).await;

    let listed = list_anycast_ip_lists(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case the whole table: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let items = page_items(&listed);
    assert_eq!(
        items.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        expected_order(&created),
        "case the whole table: expected every list once in id order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(table.len().to_string().as_str()),
        "case the whole table: expected Quantity {}, got {}",
        table.len(),
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case the whole table: one page holds them all, got {}",
        listed.raw
    );

    for one in &created {
        let item = items
            .iter()
            .find(|item| item.id == one.id)
            .unwrap_or_else(|| panic!("case {}: expected the listing to report it", one.case));
        // The item is the record the read answers with, down to the version token the read
        // reports in its header.
        let described = described_item(&router, &one.id).await;
        assert_eq!(
            item, &described,
            "case {}: expected the listing to report the record the read answers with, got \
             {item:?}",
            one.case
        );
        assert_eq!(
            item.e_tag,
            Some(one.etag.clone()),
            "case {}: expected the create's version token, got {:?}",
            one.case,
            item.e_tag
        );
        assert_eq!(
            item.status, STATUS_DEPLOYED,
            "case {}: expected the settled status, got {:?}",
            one.case, item.status
        );
        // "Omits the allocated static IP addresses": the read reports them, the page never
        // does — neither in an `AnycastIps` element nor as the text of any element of the
        // summary.
        assert!(
            !one.addresses.is_empty(),
            "case {}: the list under test holds addresses to omit",
            one.case
        );
        let reported = texts(
            listed
                .root()
                .at("Items")
                .expect("the page carries the Items element"),
        );
        for address in &one.addresses {
            assert!(
                !reported.contains(address),
                "case {}: expected the allocated address {address} omitted, got {}",
                one.case,
                listed.raw
            );
        }
    }
    assert!(
        listed.root().descendant("AnycastIps").is_none(),
        "case the whole table: a summary carries no AnycastIps element, got {}",
        listed.raw
    );
}

/// `MaxItems` cuts the page, `NextMarker` names the id the page ended at and a request
/// resumes after it, so walking the pages at ANY admissible size enumerates every list
/// exactly once in the one order the unpaged listing reports.
#[tokio::test]
async fn walking_the_pages_enumerates_every_list_exactly_once() {
    let router = emulator();
    let table = create_table();
    let created = create_all(&router, &table).await;
    let order = expected_order(&created);
    let total = order.len();

    // Rows: one list a page, several, exactly the collection size, one past it, and past
    // the server's own ceiling (where the page is cut at the ceiling, not at the size asked
    // for).
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    for size in [1, 2, total, total + 1, ceiling + 1] {
        let walked = walk_pages(&router, size, total).await;
        assert_eq!(
            walked, order,
            "case MaxItems={size}: expected every list exactly once in one order, got {walked:?}"
        );
    }
}

/// "The response includes items in the list that occur after the marker": a request resumes
/// strictly after the id its `Marker` names, whether or not that id still names a list.
#[tokio::test]
async fn a_marker_resumes_after_the_id_it_names() {
    let router = emulator();
    let table = create_table();
    let created = create_all(&router, &table).await;
    let order = expected_order(&created);

    // Rows: every id a list is held under (so the resume point sits at the start, in the
    // middle and at the end of the order), plus well-formed ids no list carries — one
    // sorting before every stored id, one that is an id's own prefix, and one sorting after
    // all of them.
    let mut markers: Vec<String> = order.clone();
    markers.extend([
        "ipl-00000000000000000".to_string(),
        order[0][.."ipl-".len() + 4].to_string(),
        "ipl-fffffffffffffffff".to_string(),
    ]);

    for marker in markers {
        let wanted: Vec<String> = order
            .iter()
            .filter(|id| id.as_str() > marker.as_str())
            .cloned()
            .collect();
        let page = list_anycast_ip_lists(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            page.status, 200,
            "case Marker={marker}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page_ids(&page),
            wanted,
            "case Marker={marker}: expected the ids after the marker, got {}",
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(wanted.len().to_string().as_str()),
            "case Marker={marker}: expected Quantity {}, got {}",
            wanted.len(),
            page.raw
        );
        assert_eq!(
            page.root().text_at("Marker"),
            Some(marker.as_str()),
            "case Marker={marker}: expected the marker echoed, got {}",
            page.raw
        );
    }
}

/// `Status` is owed by the CLOCK: a list that has just been created is listed as still
/// deploying however often the listing is read, and as deployed once its deadline has
/// passed — and nothing else about the record moves with it.
#[tokio::test]
async fn the_clock_moves_a_listed_list_to_deployed() {
    let router = emulator();
    let table = two_rows();
    let created = create_all(&router, &table).await;

    // Straight after the creates every list is still deploying, and a read does not bring
    // the deadline forward: two listings in a row answer the same non-terminal status.
    let mut first: Vec<Item> = Vec::new();
    for attempt in ["the first read", "the second read"] {
        let page = list_anycast_ip_lists(&router, "").await;
        assert_eq!(
            page.status, 200,
            "case {attempt}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        let items = page_items(&page);
        assert_eq!(
            items.len(),
            table.len(),
            "case {attempt}: expected every created list reported, got {}",
            page.raw
        );
        for item in &items {
            assert_eq!(
                item.status, ANYCAST_IP_LIST_STATUS_DEPLOYING,
                "case {attempt}: expected the transient status, got {item:?}"
            );
        }
        first = items;
    }

    // The clock — not a call — owes every one of them the terminal status.
    tokio::time::sleep(past_propagation()).await;
    let settled = list_anycast_ip_lists(&router, "").await;
    let items = page_items(&settled);
    assert_eq!(
        items.len(),
        table.len(),
        "case past the deployment: expected every list still reported, got {}",
        settled.raw
    );
    for (item, before) in items.iter().zip(first.iter()) {
        assert_eq!(
            item.status, STATUS_DEPLOYED,
            "case past the deployment: expected the terminal status, got {item:?}"
        );
        // Only the status moved: every other member is the one the create stored.
        assert_eq!(
            (
                &item.id,
                &item.name,
                &item.arn,
                &item.ip_count,
                &item.last_modified_time,
                &item.e_tag,
                &item.ip_address_type
            ),
            (
                &before.id,
                &before.name,
                &before.arn,
                &before.ip_count,
                &before.last_modified_time,
                &before.e_tag,
                &before.ip_address_type
            ),
            "case past the deployment: a settled status moves nothing else, got {item:?}"
        );
    }
    for one in &created {
        let item = items
            .iter()
            .find(|item| item.id == one.id)
            .unwrap_or_else(|| panic!("case {}: expected the listing to report it", one.case));
        assert_eq!(
            item.e_tag,
            Some(one.etag.clone()),
            "case {}: a settled status mints no new version token, got {:?}",
            one.case,
            item.e_tag
        );
    }
}

/// A page size that names no page at all is refused with `InvalidArgument` (400), stating
/// the constraint on the parameter — and the refusal leaves the listing answering exactly
/// what it answered before.
#[tokio::test]
async fn a_page_size_that_names_no_page_is_refused() {
    let router = emulator();
    let table = create_table();
    let created = create_all(&router, &table).await;

    // Rows: negative sizes of two magnitudes — every value no page can be taken from.
    for size in ["-1", "-100"] {
        let answer = list_anycast_ip_lists(&router, &format!("?MaxItems={size}")).await;
        assert_eq!(
            answer.status, 400,
            "case MaxItems={size}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case MaxItems={size}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(MAX_ITEMS_INVALID_MESSAGE),
            "case MaxItems={size}: expected the parameter's constraint, got {}",
            answer.raw
        );
    }

    let listed = list_anycast_ip_lists(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        expected_order(&created),
        "case after the refusals: a refused listing changes nothing, got {}",
        listed.raw
    );
}

/// A listing is a READ: it answers the same records again, whatever parameters it is asked
/// under, and moves no list's version token.
#[tokio::test]
async fn the_listing_is_a_read() {
    let router = emulator();
    let table = create_table();
    let created = create_all(&router, &table).await;
    tokio::time::sleep(past_propagation()).await;

    let first = page_items(&list_anycast_ip_lists(&router, "").await);
    // Reads over every parameter combination this listing serves, so none of them can be
    // the one that writes.
    let resume = expected_order(&created)[0].clone();
    for query in [
        "?MaxItems=1".to_string(),
        format!("?MaxItems=2&Marker={resume}"),
        "?Marker=ipl-00000000000000000".to_string(),
        "?MaxItems=1000".to_string(),
    ] {
        let answer = list_anycast_ip_lists(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {query}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
    }
    let second = page_items(&list_anycast_ip_lists(&router, "").await);
    assert_eq!(
        second, first,
        "case repeated listings: expected the same records again, got {second:?}"
    );

    for one in &created {
        let read = get_anycast_ip_list(&router, &one.id).await;
        assert_eq!(
            read.header("etag"),
            one.etag,
            "case {}: a listing is no mutation, so the version token stands",
            one.case
        );
    }
}
