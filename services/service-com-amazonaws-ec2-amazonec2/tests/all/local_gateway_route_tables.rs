// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLocalGatewayRouteTables`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeLocalGatewayRouteTables`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: "By default, all local
//! gateway route tables are described", narrowed by the `LocalGatewayRouteTableIds` and
//! `Filters` the request names and cut into `MaxResults`-sized pages. Every table row is a
//! distinct input along what that generalises over — which filter, how many values, which
//! ids, which page size — and an account with no Outpost (the empty store) is one row among
//! them rather than the specification.

use std::collections::BTreeMap;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayRouteTableSpec};
use ec2::ops::types::Tag;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};
use super::wire::document;

/// The Region every call here is signed for, and the Region the seeded tables' ARNs name.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGatewayRouteTables";

/// The XML element each reported route table's id arrives in
/// (`LocalGatewayRouteTable$LocalGatewayRouteTableId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableId";

/// One seeded route table: what it belongs to, and in what state and mode.
struct TableRow {
    local_gateway_id: &'static str,
    outpost: &'static str,
    /// "The state of the local gateway route table" (`LocalGatewayRouteTable$State`) — an
    /// unconstrained string in the model, carrying the lifecycle states a table moves
    /// through.
    state: &'static str,
    /// A member of the model's own `LocalGatewayRouteTableMode` catalog
    /// (`smithy_query(query="shape",shape="LocalGatewayRouteTableMode")`:
    /// `direct-vpc-routing` and `coip`), both of which the rows span.
    mode: &'static str,
    tags: Vec<Tag>,
}

/// A tag, as the seeded tables carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The ARN of the Outpost `name`, as `LocalGatewayRouteTable$OutpostArn` carries it.
fn outpost_arn(name: &str) -> String {
    format!(
        "arn:aws:outposts:{REGION}:{}:outpost/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of a local gateway route table, as
/// `LocalGatewayRouteTable$LocalGatewayRouteTableArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

/// The route tables the filter and paging tables are written against. They differ along
/// every attribute the documented filters select on — the local gateway, the Outpost and the
/// state — and along the mode and the tag count besides; two rows share a local gateway and
/// two share an Outpost, so a filter on one of those reports more than a single row. Seven
/// rows, so the model's minimum page size (`range 5..1000`) cuts them into two pages.
fn seeded_rows() -> Vec<TableRow> {
    vec![
        TableRow {
            local_gateway_id: "lgw-00000000000000001",
            outpost: "op-00000000000000001",
            state: "available",
            mode: "direct-vpc-routing",
            tags: vec![tag("Name", "rack-a")],
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000001",
            outpost: "op-00000000000000001",
            state: "available",
            mode: "coip",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000001",
            outpost: "op-00000000000000002",
            state: "pending",
            mode: "direct-vpc-routing",
            tags: Vec::new(),
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000002",
            outpost: "op-00000000000000003",
            state: "available",
            mode: "coip",
            tags: vec![tag("Name", "rack-b")],
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000002",
            outpost: "op-00000000000000003",
            state: "deleting",
            mode: "direct-vpc-routing",
            tags: Vec::new(),
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000003",
            outpost: "op-00000000000000004",
            state: "deleted",
            mode: "coip",
            tags: Vec::new(),
        },
        TableRow {
            local_gateway_id: "lgw-00000000000000004",
            outpost: "op-00000000000000005",
            state: "available",
            mode: "direct-vpc-routing",
            tags: vec![tag("Name", "rack-c")],
        },
    ]
}

/// Write the seeded route tables into the engine's store, the way the operation that creates
/// a local gateway route table would, and report the ids they were given in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        let case = row.outpost;
        ids.push(
            guard
                .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
                    local_gateway_id: row.local_gateway_id.to_string(),
                    outpost_arn: outpost_arn(row.outpost),
                    region: REGION.to_string(),
                    state: row.state.to_string(),
                    // Seeded in the state the row names, so the clock owes these tables
                    // nothing.
                    becomes: None,
                    mode: row.mode.to_string(),
                    tags: row.tags,
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded route table")
                }),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The route table ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` form field.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// Every route table a response carries, as its scalar ec2Query members, in the order it
/// reported them. The tags are read separately ([`reported_tags`]), a tag having a `<value>`
/// of its own.
fn reported_tables(case: &str, body: &str) -> Vec<BTreeMap<String, String>> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && node
                    .children()
                    .any(|inner| inner.is_element() && inner.tag_name().name() == ID_ELEMENT)
        })
        .map(|node| {
            node.children()
                .filter(|inner| inner.is_element() && inner.tag_name().name() != "tagSet")
                .map(|inner| {
                    (
                        inner.tag_name().name().to_string(),
                        inner.text().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// The tags of every route table a response carries, in the order they were reported.
fn reported_tags(case: &str, body: &str) -> Vec<Vec<(String, String)>> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && node
                    .children()
                    .any(|inner| inner.is_element() && inner.tag_name().name() == ID_ELEMENT)
        })
        .map(|node| {
            node.children()
                .find(|inner| inner.is_element() && inner.tag_name().name() == "tagSet")
                .into_iter()
                .flat_map(|set| set.children().collect::<Vec<_>>())
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| {
                    let child = |name: &str| {
                        item.children()
                            .find(|inner| inner.is_element() && inner.tag_name().name() == name)
                            .and_then(|inner| inner.text())
                            .unwrap_or_default()
                            .to_string()
                    };
                    (child("key"), child("value"))
                })
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account with no Outpost holds no local gateway route table — a local gateway and its
/// route tables come with an Outpost rather than from any EC2 call — so the operation answers
/// HTTP 200 with an empty set, and describing is a READ: two calls report the same thing and
/// the store is untouched.
#[tokio::test]
async fn an_account_with_no_outpost_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "fresh account: expected no route table, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: a complete report carries no NextToken, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, reported,
        "a read must not change what a later caller sees: {reported:?} then {second:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().local_gateway_route_tables.len(),
        0,
        "describing route tables must not write to the store"
    );
}

/// Every route table the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_route_table_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all route tables: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all route tables: expected every stored table in store order, got {reported:?}"
    );

    let tables = reported_tables("all route tables", &body);
    let tags = reported_tags("all route tables", &body);
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        let table = &tables[index];
        for (member, value) in [
            (ID_ELEMENT, seeded[index].clone()),
            ("localGatewayRouteTableArn", route_table_arn(&seeded[index])),
            ("localGatewayId", row.local_gateway_id.to_string()),
            ("outpostArn", outpost_arn(row.outpost)),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("state", row.state.to_string()),
            ("mode", row.mode.to_string()),
        ] {
            assert_eq!(
                table.get(member).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: expected <{member}>{value}</{member}>, got {table:?}"
            );
        }
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|stored| {
                (
                    stored.key.clone().unwrap_or_default(),
                    stored.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            tags[index], expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {:?}",
            tags[index]
        );
    }

    let late = engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
            local_gateway_id: "lgw-00000000000000009".to_string(),
            outpost_arn: outpost_arn("op-00000000000000009"),
            region: REGION.to_string(),
            state: "available".to_string(),
            becomes: None,
            mode: "coip".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the route table");
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Ids and filters
// ---------------------------------------------------------------------------

/// The ids narrow the report to the route tables named, in store order whatever order they
/// were asked for; an id the account holds no route table for, or that is not an id of this
/// resource at all, is refused rather than reported as a missing row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_route_tables_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![6],
        vec![2, 4],
        vec![4, 2],
        vec![5, 1, 3],
        (0..seeded.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed("LocalGatewayRouteTableId", &borrowed),
        )
        .await;
        let mut expected_indices = asked.clone();
        expected_indices.sort_unstable();
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // An id the report cannot RESOLVE is a selector that matches nothing rather than a
    // refusal: a well-formed id the account holds no table for selects no row and the rows
    // that DO match come back, which is what this surface answers such a request with — while
    // a value that is not an id of this resource at all is still `.Malformed`, the form being
    // read before anything is selected. Rows differ in which id is unresolvable, in how it is
    // unresolvable, and in where it sits beside a known id.
    let unknown = "lgw-rtb-0ffffffffffffffff";
    let short_unknown = "lgw-rtb-0fffffff";
    let known = seeded[2].clone();
    let selecting_nothing: Vec<Vec<String>> = vec![
        vec![unknown.to_string()],
        vec![short_unknown.to_string()],
    ];
    for ids in selecting_nothing {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed:?}");
        let (status, body) =
            describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {case:?}: an id nothing is stored under selects no route table, got {body}"
        );
    }
    // An id beside a KNOWN one narrows the report to the known one.
    let borrowed = vec![known.as_str(), unknown];
    let (status, body) = describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
    assert_eq!(
        status, 200,
        "case {borrowed:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, ID_ELEMENT),
        vec![known.clone()],
        "case {borrowed:?}: expected the known route table alone, got {body}"
    );
    let unresolvable_cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec!["not-a-route-table-id".to_string()],
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            vec!["lgw-rtb-0zzzzzzzzzzzzzzzz".to_string()],
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            vec![String::new()],
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            vec![known, "not-a-route-table-id".to_string()],
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
    ];
    for (ids, expected_code) in unresolvable_cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed:?}");
        let (status, body) =
            describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {case:?}: a refused request reports no route table, got {body}"
        );
    }
}

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is case-sensitive with EC2's `*`/`?` wildcards.
#[tokio::test]
async fn each_documented_filter_selects_the_route_tables_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = seeded_rows();
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "local-gateway-id",
            vec![rows[0].local_gateway_id.to_string()],
            vec![0, 1, 2],
        ),
        // Two values for one filter: joined with OR.
        (
            "local-gateway-id",
            vec![
                rows[3].local_gateway_id.to_string(),
                rows[5].local_gateway_id.to_string(),
            ],
            vec![3, 4, 5],
        ),
        ("local-gateway-id", vec!["lgw-*".to_string()], all.clone()),
        ("local-gateway-id", vec!["Lgw-*".to_string()], vec![]),
        (
            "local-gateway-route-table-id",
            vec![seeded[3].clone()],
            vec![3],
        ),
        (
            "local-gateway-route-table-arn",
            vec![route_table_arn(&seeded[4])],
            vec![4],
        ),
        ("outpost-arn", vec![outpost_arn(rows[3].outpost)], vec![3, 4]),
        (
            "outpost-arn",
            vec![outpost_arn("op-0ffffffffffffffff")],
            vec![],
        ),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".to_string()], vec![]),
        ("state", vec!["available".to_string()], vec![0, 1, 3, 6]),
        (
            "state",
            vec!["deleting".to_string(), "deleted".to_string()],
            vec![4, 5],
        ),
        ("state", vec!["pending".to_string()], vec![2]),
        // A documented filter carrying no value at all: there is no value for the attribute
        // to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("outpost-arn", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Several filters are joined with an AND: only the tables matching every one.
    let combinations: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("state", &["available"][..]),
            ],
            vec![0, 1],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000002"][..]),
                ("state", &["deleted"][..]),
            ],
            vec![],
        ),
    ];
    for (entries, expected_indices) in combinations {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the route tables that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name with
    // the wrong case, the plural, a member the shape carries but no filter names, the tag
    // filters this operation's `Filters` doc does NOT name (even though a table carries
    // tags), and an empty name.
    let cases = [
        "local-gateway-route-table",
        "local_gateway_id",
        "Local-Gateway-Id",
        "local-gateway-ids",
        "mode",
        "tag-key",
        "tag:Name",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &[][..])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {name:?}: a refused request reports no route table, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination and DryRun
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every route table exactly once, in store
/// order, and the last page carries no token. `LocalGatewayMaxResults` is modelled
/// `range 5..1000`, so the rows are its published lower bound, a size that divides the store
/// exactly, one that does not, and the upper bound.
#[tokio::test]
async fn paging_covers_every_route_table_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for max_results in [5, 6, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, ID_ELEMENT);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every route table exactly once in store order, got \
             {walked:?}"
        );
    }

    // A page size outside the published range is refused by the model's own bounds, one past
    // each end.
    for max_results in [4, 1001] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the published range, got {status} with \
             body {body}"
        );
    }

    // A `NextToken` this service did not issue is refused rather than read as an id.
    for token in ["lgw-rtb-00000000000000001", "not-a-token", "zz"] {
        let params = vec![("NextToken".to_string(), token.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// A `DryRun` request reports that it would have succeeded — HTTP 412 `DryRunOperation` —
/// and reports no route table.
#[tokio::test]
async fn a_dry_run_reports_that_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let params = vec![("DryRun".to_string(), "true".to_string())];
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, ID_ELEMENT).is_empty(),
        "a dry run: reports no route table, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The ids and the filters together, and the size of the request itself
// ---------------------------------------------------------------------------

/// One row of the ids-and-filters table: the indices of the ids to name, the filter sent
/// alongside them with its values, and the indices of the route tables that must come back.
type IdsAndFilterCase<'a> = (Vec<usize>, &'a str, Vec<String>, Vec<usize>);

/// The two narrowings are JOINED: a route table is reported only when it is both named by
/// `LocalGatewayRouteTableIds` and matched by every filter, so neither can widen what the
/// other selected. And a `NextToken` is a cursor into that same selection, so walking a
/// filtered report with its filter re-sent covers exactly the selected tables, once each, in
/// store order — the pages of a narrowed report, not of the whole store.
#[tokio::test]
async fn the_ids_and_the_filters_are_joined_and_a_token_walks_only_the_selection() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = seeded_rows();
    let account = engine::emulator_account_id().to_string();

    // Rows differ in which narrowing is the stricter one, in how many ids are named, in the
    // attribute the filter selects on, and in the order the ids are asked for.
    let cases: Vec<IdsAndFilterCase<'_>> = vec![
        // The filter narrows the named ids further.
        (
            vec![0, 1, 2],
            "state",
            vec![rows[0].state.to_string()],
            vec![0, 1],
        ),
        // A named table the filter excludes: naming it does not report it.
        (vec![2], "state", vec![rows[0].state.to_string()], vec![]),
        (
            vec![3, 4, 5, 6],
            "local-gateway-id",
            vec![rows[3].local_gateway_id.to_string()],
            vec![3, 4],
        ),
        // The ids narrow a filter that would otherwise match more.
        (
            (0..seeded.len()).collect(),
            "outpost-arn",
            vec![outpost_arn(rows[0].outpost)],
            vec![0, 1],
        ),
        (vec![6], "owner-id", vec![account], vec![6]),
        // Asked for in an order of its own, under a filter every table matches: the report is
        // still in store order.
        (
            vec![4, 0],
            "local-gateway-id",
            vec!["lgw-*".to_string()],
            vec![0, 4],
        ),
    ];

    for (asked, name, values, expected_indices) in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed_ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let borrowed_values: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed_ids:?} {name}={borrowed_values:?}");
        let mut params = indexed("LocalGatewayRouteTableId", &borrowed_ids);
        params.extend(filters(&[(name, &borrowed_values)]));
        let reported = described_ids(&router, &case, &params).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Paging a FILTERED report: three local gateways select six of the seven stored tables,
    // so the model's smallest page size (`LocalGatewayMaxResults`, `range 5..1000`) cuts the
    // SELECTION into a full page and a remainder.
    let gateways = [
        rows[0].local_gateway_id.to_string(),
        rows[3].local_gateway_id.to_string(),
        rows[5].local_gateway_id.to_string(),
    ];
    let borrowed_gateways: Vec<&str> = gateways.iter().map(String::as_str).collect();
    let page_size = 5;
    let filtered_params = || {
        let mut params = max_results_param(page_size);
        params.extend(filters(&[("local-gateway-id", &borrowed_gateways)]));
        params
    };
    let selection = ids_at(&seeded, &[0, 1, 2, 3, 4, 5]);
    let mut params = filtered_params();
    let mut walked: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    loop {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "a filtered walk: expected HTTP 200, got {status} with body {body}"
        );
        let page = elements(&body, ID_ELEMENT);
        page_sizes.push(page.len());
        walked.extend(page);
        assert!(
            page_sizes.len() <= selection.len() + 1,
            "a filtered walk: paging did not terminate, walked {walked:?}"
        );
        match element(&body, "nextToken") {
            Some(token) => {
                params = filtered_params();
                params.push(("NextToken".to_string(), token));
            }
            None => break,
        }
    }
    assert_eq!(
        walked, selection,
        "a filtered walk: expected the selected tables once each in store order, got {walked:?}"
    );
    assert_eq!(
        page_sizes,
        vec![5, 1],
        "a filtered walk: expected a full page then the remainder, got {page_sizes:?}"
    );
}

/// How big the REQUEST is, is read before the work it would cost: at each cap the request is
/// evaluated as usual, and one past it the size itself is refused with EC2's
/// `InvalidParameterValue` rather than evaluated.
#[tokio::test]
async fn a_request_past_a_published_cap_is_refused_rather_than_evaluated() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = seeded_rows();

    // A well-formed local gateway route table id this account holds no table for:
    // `lgw-rtb-` and 17 lowercase hex digits.
    let unheld_id = |index: usize| format!("lgw-rtb-{index:017x}");

    // The id list AT its cap is READ rather than counted, so the request is served; one past
    // the cap, the count is the answer. A handful of unheld ids proves the reading itself:
    // they select no row, so the report comes back empty.
    let unheld: Vec<String> = (0..3).map(unheld_id).collect();
    let borrowed: Vec<&str> = unheld.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
    assert_eq!(
        status, 200,
        "a few unheld ids: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, ID_ELEMENT).is_empty(),
        "a few unheld ids: ids that name no table select no row, got {body}"
    );
    let ids: Vec<String> = (0..ec2::state::MAX_RESOURCE_IDS).map(unheld_id).collect();
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
    assert_eq!(
        status, 200,
        "ids at the cap: the list is at the published cap, so the request is served rather \
         than refused for its size, got {status} with body {body}"
    );

    let ids: Vec<String> = (0..=ec2::state::MAX_RESOURCE_IDS).map(unheld_id).collect();
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &indexed("LocalGatewayRouteTableId", &borrowed)).await;
    assert_eq!(
        status, 400,
        "ids past the cap: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "ids past the cap: expected InvalidParameterValue, got {body}"
    );

    // The number of filters, at the cap and one past it. Every filter here is documented and
    // carries no value, so at the cap the request is evaluated and selects nothing.
    let valueless: (&str, &[&str]) = ("state", &[]);
    let at_cap = vec![valueless; ec2::state::MAX_FILTERS];
    let reported = described_ids(&router, "filters at the cap", &filters(&at_cap)).await;
    assert!(
        reported.is_empty(),
        "filters at the cap: a valueless filter selects nothing, got {reported:?}"
    );

    let past_cap = vec![valueless; ec2::state::MAX_FILTERS + 1];
    let (status, body) = describe(&router, &filters(&past_cap)).await;
    assert_eq!(
        status, 400,
        "filters past the cap: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "filters past the cap: expected InvalidParameterValue, got {body}"
    );

    // The number of values on ONE filter, at the cap and one past it. At the cap the values
    // are compared, so the one value a table carries still selects it.
    let mut values = vec![rows[0].state.to_string()];
    values.extend((1..ec2::state::MAX_FILTER_VALUES).map(|index| format!("state-{index}")));
    let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
    let reported = described_ids(
        &router,
        "filter values at the cap",
        &filters(&[("state", &borrowed)]),
    )
    .await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[0, 1, 3, 6]),
        "filter values at the cap: the values are compared, got {reported:?}"
    );

    values.push("state-past-the-cap".to_string());
    let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &filters(&[("state", &borrowed)])).await;
    assert_eq!(
        status, 400,
        "filter values past the cap: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "filter values past the cap: expected InvalidParameterValue, got {body}"
    );

    // The SIZE of one value, at the cap and one past it: a capped number of comparisons is
    // not capped work, the matcher's cost being the product of the two lengths.
    let at_cap_value = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let reported = described_ids(
        &router,
        "a value at the byte cap",
        &filters(&[("state", &[at_cap_value.as_str()])]),
    )
    .await;
    assert!(
        reported.is_empty(),
        "a value at the byte cap: it is compared and matches no table, got {reported:?}"
    );

    let past_cap_value = "a".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);
    let (status, body) =
        describe(&router, &filters(&[("state", &[past_cap_value.as_str()])])).await;
    assert_eq!(
        status, 400,
        "a value past the byte cap: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a value past the byte cap: expected InvalidParameterValue, got {body}"
    );
}
