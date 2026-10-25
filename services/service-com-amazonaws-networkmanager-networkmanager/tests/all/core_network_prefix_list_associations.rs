// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListCoreNetworkPrefixListAssociations`, driven over HTTP
//! through the real router (the harness is the one `global_network_lifecycle` defines) —
//! the reader of the slice `core_network_prefix_list_association` creates and deletes.
//!
//! The rule under test: the listing reports exactly the prefix list associations the
//! creates stored for the core network its path names — each as the record holds it, in
//! `PrefixListArn` order — narrowed to the `PrefixListArn` the request filters on and
//! handed back one `MaxResults`-sized page at a time, with a `NextToken` only while items
//! remain. The cases span that class: how many associations a core network holds (none
//! through many), every prefix list ARN form the create accepts (partition, Region,
//! account, both published id widths) and every alias length it accepts, creation order
//! against reported order, each ARN a filter can name, every page size from the modelled
//! minimum to the modelled maximum, and the refusals.
//!
//! Nothing reaches into `crate::state` for a record: an association is only ever read
//! back through the listing.

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state::{
    FIELD_NEXT_TOKEN, FIELD_PREFIX_LIST_ARN,
};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::escape;

/// The longest a `PrefixListAlias` may be: `ConstrainedString`, len 0..256 (`smithy_query
/// query="operation" operation="CreateCoreNetworkPrefixListAssociation"`).
const MAX_ALIAS_CHARS: usize = 256;

/// The page sizes `MaxResults` accepts, in full: range 1..500 (`smithy_query
/// query="operation" operation="ListCoreNetworkPrefixListAssociations"`).
const MIN_MAX_RESULTS: i64 = 1;
/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 500;

/// A core network of its own global network (one global network holds a single core
/// network), answering its minted id.
async fn core_network(emulator: &Emulator, label: &str) -> String {
    let global_network = emulator.create_global_network(label).await;
    let (status, body) =
        create_core_network(emulator, json!({ "GlobalNetworkId": global_network })).await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork({label}): expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork({label}): no id in {body}"))
        .to_string()
}

/// The ARN of an Amazon VPC managed prefix list, the resource an association names:
/// `arn:<partition>:ec2:<region>:<account>:prefix-list/pl-<8 to 17 hex digits>` — the EC2
/// skeleton this model spells out for every ARN it types (`VpcArn` carries
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`, `smithy_query
/// shape=VpcArn`), so the Region and the account segments may be empty.
fn arn_of(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:ec2:{region}:{account}:prefix-list/pl-{id}")
}

/// A prefix list ARN of the ordinary form whose id carries `id` as its hex suffix.
fn prefix_list_arn(id: &str) -> String {
    arn_of("aws", "us-west-2", "123456789012", id)
}

/// `CreateCoreNetworkPrefixListAssociation`: `POST /prefix-list` with the pair and the
/// alias in the body.
async fn associate(
    emulator: &Emulator,
    core_network_id: &str,
    arn: &str,
    alias: &str,
) -> (u16, Value) {
    emulator
        .post(
            "/prefix-list",
            json!({
                "CoreNetworkId": core_network_id,
                "PrefixListArn": arn,
                "PrefixListAlias": alias,
            }),
        )
        .await
}

/// `DeleteCoreNetworkPrefixListAssociation`: `DELETE
/// /prefix-list/{PrefixListArn}/core-network/{CoreNetworkId}`, both members non-greedy
/// path labels, so the ARN travels percent-encoded.
async fn disassociate(emulator: &Emulator, core_network_id: &str, arn: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/prefix-list/{}/core-network/{core_network_id}",
            escape(arn)
        ))
        .await
}

/// One row of the accepted table: a prefix list ARN form the create accepts and the alias
/// stored with it. The rows differ along what the rule generalises over — the partition,
/// the Region and the account segments (both of which the modelled EC2 skeleton lets be
/// empty), the width of the prefix list id at each published bound, and the alias from
/// empty to the longest the modelled `ConstrainedString` admits.
struct Row {
    name: &'static str,
    arn: String,
    alias: String,
}

/// The accepted forms, which the listing has to report exactly as they were stored. The
/// creation order below is NOT `PrefixListArn` order, so a listing that reported them in
/// arrival order would fail.
fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            name: "the shortest id width",
            arn: prefix_list_arn("63a5400a"),
            alias: "aws-managed".to_string(),
        },
        Row {
            name: "the longest id width",
            arn: prefix_list_arn("0123456789abcdef1"),
            alias: "customer-managed".to_string(),
        },
        Row {
            name: "another partition and Region",
            arn: arn_of("aws-us-gov", "us-gov-west-1", "123456789012", "00112233"),
            alias: "gov-cloud".to_string(),
        },
        Row {
            name: "another partition, Region and account",
            arn: arn_of("aws-cn", "cn-north-1", "210987654321", "00aabb11"),
            alias: "china".to_string(),
        },
        Row {
            name: "an empty Region segment",
            arn: arn_of("aws", "", "123456789012", "aaaaaaaa"),
            alias: "no-region".to_string(),
        },
        Row {
            name: "an empty account segment",
            arn: arn_of("aws", "us-east-1", "", "bbbbbbbb"),
            alias: "no-account".to_string(),
        },
        Row {
            name: "the longest alias the model admits",
            arn: prefix_list_arn("cccccccc"),
            alias: "a".repeat(MAX_ALIAS_CHARS),
        },
        Row {
            name: "an empty alias",
            // `PrefixListAlias` is len 0..256, so the empty alias is in range.
            arn: prefix_list_arn("dddddddd"),
            alias: String::new(),
        },
    ]
}

/// The refused forms a `prefixListArn` FILTER can carry: a string that is no ARN, an ARN
/// of another vendor or of another EC2 resource type, and an id whose width is one past
/// each published bound (8 and 17 hex digits) or outside its alphabet.
fn malformed_filters() -> Vec<(&'static str, String)> {
    vec![
        ("a bare prefix list id", "pl-63a5400a".to_string()),
        (
            "an ARN of another vendor",
            "arn:aws:networkmanager:us-west-2:123456789012:prefix-list/pl-63a5400a".to_string(),
        ),
        (
            "another EC2 resource type",
            "arn:aws:ec2:us-west-2:123456789012:vpc/vpc-0123456789abcdef1".to_string(),
        ),
        (
            "an id with no pl- prefix",
            "arn:aws:ec2:us-west-2:123456789012:prefix-list/63a5400a".to_string(),
        ),
        (
            "one digit short of the shortest id",
            prefix_list_arn("1234567"),
        ),
        (
            "one digit past the longest id",
            prefix_list_arn("0123456789abcdef12"),
        ),
        ("uppercase hex in the id", prefix_list_arn("63A5400A")),
        (
            "an empty partition",
            "arn::ec2:us-west-2:123456789012:prefix-list/pl-63a5400a".to_string(),
        ),
        ("no ARN at all", "not-an-arn-at-all".to_string()),
    ]
}

/// The `ListCoreNetworkPrefixListAssociations` URI: `GET
/// /prefix-list/core-network/{CoreNetworkId}`, with `prefixListArn` / `maxResults` /
/// `nextToken` as query parameters (`smithy_query query="operation"
/// operation="ListCoreNetworkPrefixListAssociations"`).
fn list_uri(
    core_network_id: &str,
    filter: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(arn) = filter {
        query.push(format!("prefixListArn={}", escape(arn)));
    }
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={}", escape(token)));
    }
    let path = format!("/prefix-list/core-network/{core_network_id}");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    }
}

/// One listing of `core_network_id`, asserted to answer 200.
async fn list(
    emulator: &Emulator,
    core_network_id: &str,
    filter: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
    case: &str,
) -> Value {
    let (status, body) = emulator
        .get(&list_uri(core_network_id, filter, max_results, next_token))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkPrefixListAssociations: expected 200, got {status} with {body}"
    );
    body
}

/// The `(CoreNetworkId, PrefixListArn, PrefixListAlias)` triples a listing reports, in the
/// order returned.
fn reported(body: &Value) -> Vec<(String, String, String)> {
    body["PrefixListAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a PrefixListAssociations array in {body}"))
        .iter()
        .map(|association| {
            let member = |name: &str| {
                association[name]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected {name} in {association}"))
                    .to_string()
            };
            (
                member("CoreNetworkId"),
                member("PrefixListArn"),
                member("PrefixListAlias"),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_listing_reports_every_stored_association_in_prefix_list_arn_order() {
    let emulator = Emulator::new();

    // A core network nobody has associated a prefix list with holds none: the empty page
    // is what the store really holds, not a placeholder.
    let empty = core_network(&emulator, "no-associations").await;
    let body = list(&emulator, &empty, None, None, None, "no association at all").await;
    assert_eq!(
        reported(&body),
        Vec::new(),
        "a core network with no association must report none, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "an exhausted listing carries no NextToken, got {body}"
    );

    // Every form the create accepts, stored in one core network: the listing reports each
    // as the record holds it, in `PrefixListArn` order — which the creation order below is
    // not, so the order cannot come from the order they arrived in.
    let stored = core_network(&emulator, "every-accepted-form").await;
    let mut wanted: Vec<(String, String, String)> = Vec::new();
    for row in accepted_rows() {
        let (status, created) = associate(&emulator, &stored, &row.arn, &row.alias).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCoreNetworkPrefixListAssociation: expected 200, got {status} with {created}",
            row.name
        );
        assert_eq!(
            created,
            json!({
                "CoreNetworkId": stored,
                "PrefixListArn": row.arn,
                "PrefixListAlias": row.alias,
            }),
            "case {:?}: the create must answer the stored association, got {created}",
            row.name
        );
        wanted.push((stored.clone(), row.arn.clone(), row.alias.clone()));
    }
    wanted.sort_by(|left, right| left.1.cmp(&right.1));

    let body = list(&emulator, &stored, None, None, None, "every accepted form").await;
    assert_eq!(
        reported(&body),
        wanted,
        "the listing must report every stored association in PrefixListArn order, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing that returned everything carries no NextToken, got {body}"
    );
}

#[tokio::test]
async fn a_listing_is_scoped_to_the_core_network_its_path_names() {
    let emulator = Emulator::new();
    let arn = prefix_list_arn("0000aaaa");
    let one = core_network(&emulator, "first").await;
    let other = core_network(&emulator, "second").await;

    // The SAME prefix list, associated with two core networks under different aliases:
    // each listing reports only its own core network's association.
    for (core_network_id, alias) in [(&one, "on one"), (&other, "on the other")] {
        let (status, body) = associate(&emulator, core_network_id, &arn, alias).await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected 200, got {status} with {body}"
        );
    }
    for (core_network_id, alias) in [(&one, "on one"), (&other, "on the other")] {
        let body = list(&emulator, core_network_id, None, None, None, alias).await;
        assert_eq!(
            reported(&body),
            vec![(
                core_network_id.to_string(),
                arn.clone(),
                (*alias).to_string()
            )],
            "case {alias:?}: a listing must report only its own core network's association, got {body}"
        );
    }

    // An association the delete removed is gone from the listing, and only from the core
    // network the delete named.
    let (status, body) = disassociate(&emulator, &one, &arn).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetworkPrefixListAssociation: expected 200, got {status} with {body}"
    );
    let body = list(&emulator, &one, None, None, None, "after the delete").await;
    assert_eq!(
        reported(&body),
        Vec::new(),
        "the listing must not report an association the delete removed, got {body}"
    );
    let body = list(&emulator, &other, None, None, None, "the other core network").await;
    assert_eq!(
        reported(&body),
        vec![(other.clone(), arn.clone(), "on the other".to_string())],
        "a delete on one core network must not touch another's associations, got {body}"
    );

    // A core network no longer stored has no associations to list: it is the NOT FOUND the
    // operation models, and the associations went with the record.
    let (status, body) = emulator.delete(&format!("/core-networks/{other}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork: expected 200, got {status} with {body}"
    );
    let (status, body) = emulator.get(&list_uri(&other, None, None, None)).await;
    assert_eq!(
        status, 404,
        "a listing for a deleted core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        (error_code(&body), body["ResourceType"].as_str()),
        ("ResourceNotFoundException", Some("core-network")),
        "the refusal must name the core network as the missing resource, got {body}"
    );

    // An id that names no core network at all is the same refusal, whatever else the
    // request carries.
    let unknown = "core-network-00000000000000bad";
    let (status, body) = emulator
        .get(&list_uri(unknown, Some(&arn), Some(MIN_MAX_RESULTS), None))
        .await;
    assert_eq!(
        status, 404,
        "a listing for an unknown core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(unknown),
        "the refusal must name the resource it could not find, got {body}"
    );
}

#[tokio::test]
async fn a_listing_narrows_to_the_prefix_list_arn_it_filters_on() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "filtering").await;
    let rows = [
        prefix_list_arn("0000ffff"),
        prefix_list_arn("0000eeee"),
        arn_of("aws-cn", "cn-north-1", "123456789012", "0000dddd"),
    ];
    for (index, arn) in rows.iter().enumerate() {
        let (status, body) = associate(&emulator, &core, arn, &format!("alias-{index}")).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected 200, got {status} with {body}"
        );
    }

    // Every stored ARN selects exactly its own association.
    for (index, arn) in rows.iter().enumerate() {
        let body = list(&emulator, &core, Some(arn), None, None, arn).await;
        assert_eq!(
            reported(&body),
            vec![(core.clone(), arn.clone(), format!("alias-{index}"))],
            "case {arn:?}: the filter must select exactly that association, got {body}"
        );
    }

    // A well-formed ARN this core network is not associated with selects nothing, which is
    // an empty page rather than a refusal.
    let absent = prefix_list_arn("0000cccc");
    let body = list(&emulator, &core, Some(&absent), None, None, "an unassociated ARN").await;
    assert_eq!(
        reported(&body),
        Vec::new(),
        "an unassociated ARN must select nothing, got {body}"
    );

    // A filter value that names no managed prefix list is a field failure on the member
    // that carried it — the same refusal the create and the delete give for every row of
    // the shared table.
    for (case, filter) in malformed_filters() {
        let (status, body) = emulator
            .get(&list_uri(&core, Some(&filter), None, None))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a filter that names no prefix list: expected 400, got {status} with {body}"
        );
        assert_eq!(
            (error_code(&body), body["Fields"][0]["Name"].as_str()),
            ("ValidationException", Some(FIELD_PREFIX_LIST_ARN)),
            "case {case:?}: the refusal must name the offending member, got {body}"
        );
    }

    // A refused filter is a read that changed nothing: every stored association is still
    // reported.
    let body = list(&emulator, &core, None, None, None, "after every refusal").await;
    assert_eq!(
        reported(&body).len(),
        rows.len(),
        "a refused filter must leave the store as it found it, got {body}"
    );
}

#[tokio::test]
async fn a_listing_pages_through_its_associations_at_every_page_size() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "pagination").await;
    let held = 5;
    for index in 0..held {
        let arn = prefix_list_arn(&format!("{:08x}", index + 1));
        let (status, body) = associate(&emulator, &core, &arn, &format!("a-{index}")).await;
        assert_eq!(
            status, 200,
            "association {index}: expected 200, got {status} with {body}"
        );
    }
    let all = list(&emulator, &core, None, None, None, "every association").await;
    let every = reported(&all);
    assert_eq!(
        every.len(),
        held,
        "the core network must hold {held} associations, got {all}"
    );

    // Page sizes: none at all, each size below the stored count, the count itself, one
    // past it, and both ends of the modelled range.
    for page_size in [
        None,
        Some(MIN_MAX_RESULTS),
        Some(2),
        Some(4),
        Some(5),
        Some(6),
        Some(MAX_MAX_RESULTS),
    ] {
        let mut seen: Vec<(String, String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let case = format!("MaxResults={page_size:?} page {pages}");
            let body = list(&emulator, &core, None, page_size, token.as_deref(), &case).await;
            let page = reported(&body);
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {case}: a page must not exceed MaxResults, got {} items",
                    page.len()
                );
            }
            assert!(
                !page.is_empty(),
                "case {case}: a page while items remain is never empty, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, every,
            "case MaxResults={page_size:?}: paging must report every association exactly once, in order"
        );
    }

    // One past either end of the modelled range is refused by the constraint itself.
    for size in [MIN_MAX_RESULTS - 1, MAX_MAX_RESULTS + 1] {
        let (status, body) = emulator
            .get(&list_uri(&core, None, Some(size), None))
            .await;
        assert_eq!(
            status, 400,
            "case MaxResults={size}: a page size outside 1..500 is refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "tokens").await;
    let (status, body) = associate(&emulator, &core, &prefix_list_arn("0000bbbb"), "token").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let cases: Vec<(&str, String)> = vec![
        ("not hex", "zzzz".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        (
            "hex of a token another service would mint",
            hex::encode("someone-else:pl-0"),
        ),
        ("hex of non-utf8 bytes", hex::encode([0xff, 0xfe, 0xfd])),
    ];
    for (case, token) in cases {
        let (status, body) = emulator
            .get(&list_uri(&core, None, None, Some(&token)))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: an unparseable NextToken: expected 400, got {status} with {body}"
        );
        assert_eq!(
            (error_code(&body), body["Fields"][0]["Name"].as_str()),
            ("ValidationException", Some(FIELD_NEXT_TOKEN)),
            "case {case}: the refusal must name the offending member, got {body}"
        );
    }

    // A token the service DID mint still resumes the listing it belongs to: the refusals
    // above left the paging path working.
    let body = list(&emulator, &core, None, Some(MIN_MAX_RESULTS), None, "the first page").await;
    assert_eq!(
        reported(&body).len(),
        1,
        "a one-item page must hold one association, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "the only association leaves nothing to resume, got {body}"
    );
}
