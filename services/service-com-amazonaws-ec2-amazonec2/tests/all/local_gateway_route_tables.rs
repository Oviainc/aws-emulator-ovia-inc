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

    // An id the report cannot resolve is REFUSED rather than reported as a missing row: a
    // value that is not an id of this resource at all is `.Malformed`, and a well-formed id
    // the account holds no table for is `.NotFound` — the answer the search of one table
    // gives for the same id, and the answer every sibling describe of this crate gives for an
    // id list. Rows differ in which id is unresolvable, in how it is unresolvable, and in
    // where it sits beside a known id — and the form is read before the lookup, so a
    // malformed id is `.Malformed` whatever it is asked with.
    let unknown = "lgw-rtb-0ffffffffffffffff";
    let short_unknown = "lgw-rtb-0fffffff";
    let known = seeded[2].clone();
    let unresolvable_cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec![unknown.to_string()],
            "InvalidLocalGatewayRouteTableID.NotFound",
        ),
        (
            vec![short_unknown.to_string()],
            "InvalidLocalGatewayRouteTableID.NotFound",
        ),
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
            vec![known.clone(), unknown.to_string()],
            "InvalidLocalGatewayRouteTableID.NotFound",
        ),
        (
            vec![unknown.to_string(), known.clone()],
            "InvalidLocalGatewayRouteTableID.NotFound",
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
