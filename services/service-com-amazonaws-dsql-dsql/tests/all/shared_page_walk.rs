// SPDX-License-Identifier: CC0-1.0

//! The one page walk both paginated readers of this service cut their page with,
//! driven end to end over the wire through an operation from each.
//!
//! RULE under test: a list operation returns a window over its resource's
//! identifier order — at most `maxResults` summaries, ascending, resuming after
//! the cursor the previous page issued — and it issues a `nextToken` exactly when
//! an entry remains beyond the page, naming the last entry the page returned.
//! `maxResults` is bounded by the one `MaxResults` shape both operations bind
//! (`smithy_query(query="shape", shape="MaxResults")`: `@range(min: 1, max:
//! 100)`), while the size a request that omits it pages at is the operation's own
//! `@default` (`smithy_query(query="operation",
//! operation="ListClusters,ListStreams")`: 20 and 10).
//!
//! The table spans the class rather than one request: BOTH readers
//! (`ListClusters` over cluster order, `ListStreams` over a cluster's stream
//! order) crossed with several resource counts and several page sizes, plus each
//! published bound, one value past each bound, and the omitted member. What this
//! suite adds over `tests/all/cluster_lifecycle.rs` and
//! `tests/all/stream_listing.rs`, which each pin one reader, is that the two
//! readers answer by the SAME rule: the bound neither of them may drift from, and
//! the cursor both of them spell with this service's shared encoding.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    cluster_page_scope, encode_pagination_cursor, stream_page_scope, MAX_RESULTS_MAX,
    MAX_RESULTS_MIN,
};

use crate::harness::{Answer, Dsql};

/// One paginated reader: how to drive it, how to read its answer, and the
/// `@default` page size its own model declares.
#[derive(Clone, Copy, Debug)]
struct Reader {
    /// The operation, for naming the case in a failure.
    name: &'static str,
    /// The `@required` list member its output carries.
    items_member: &'static str,
    /// The member each of its summaries names the paged resource with.
    identifier_member: &'static str,
    /// Its own modelled `maxResults` `@default`.
    modeled_default: usize,
}

/// Every paginated reader of this service. Source for the members and the
/// defaults: `smithy_query(query="operation",
/// operation="ListClusters,ListStreams")` — `clusters` / `streams`, and
/// `maxResults` `@default` 20 / 10.
const READERS: [Reader; 2] = [
    Reader {
        name: "ListClusters",
        items_member: "clusters",
        identifier_member: "identifier",
        modeled_default: 20,
    },
    Reader {
        name: "ListStreams",
        items_member: "streams",
        identifier_member: "streamIdentifier",
        modeled_default: 10,
    },
];

/// The walk `reader` pages, which the cursor it issues is bound to: the account's
/// clusters, or the streams of the ONE cluster a stream page is addressed at.
fn reader_scope(reader: Reader, cluster: &str) -> String {
    match reader.name {
        "ListClusters" => cluster_page_scope(),
        _ => stream_page_scope(cluster),
    }
}

/// One page from `reader`, addressed at `cluster` when the reader is scoped to a
/// cluster.
async fn read_page(reader: Reader, dsql: &Dsql, cluster: &str, query: &str) -> Answer {
    match reader.name {
        "ListClusters" => dsql.list(query).await,
        _ => dsql.list_streams(cluster, query).await,
    }
}

/// A store holding `count` of the resource `reader` pages over: the cluster a
/// stream page is addressed to (empty for the cluster reader, which is not
/// scoped to one), and the identifiers that were minted.
async fn populate(reader: Reader, dsql: &Dsql, count: usize) -> (String, BTreeSet<String>) {
    match reader.name {
        "ListClusters" => {
            let mut minted = BTreeSet::new();
            for index in 0..count {
                let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
                assert_eq!(
                    created.status, 200,
                    "cluster {index} must be created, got {} {:?}",
                    created.status, created.body
                );
                minted.insert(created.str("identifier"));
            }
            (String::new(), minted)
        }
        _ => {
            let cluster = dsql.create_settled().await;
            let mut minted = BTreeSet::new();
            for index in 0..count {
                let created = dsql
                    .create_stream(
                        &cluster,
                        json!({
                            "format": "JSON",
                            "ordering": "UNORDERED",
                            "targetDefinition": {"kinesis": {
                                "streamArn": format!(
                                    "arn:aws:kinesis:{}:{}:stream/changes-{index}",
                                    dsql.region(),
                                    engine::EMULATOR_ACCOUNT_ID
                                ),
                                "roleArn": format!(
                                    "arn:aws:iam::{}:role/dsql-cdc",
                                    engine::EMULATOR_ACCOUNT_ID
                                ),
                            }},
                        }),
                    )
                    .await;
                assert_eq!(
                    created.status, 200,
                    "stream {index} must be created, got {} {:?}",
                    created.status, created.body
                );
                minted.insert(created.str("streamIdentifier"));
            }
            (cluster, minted)
        }
    }
}

/// The identifiers one successful page reports, in the order it reported them.
fn page_identifiers(reader: Reader, case: &str, answer: &Answer) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {} must answer 200, got {} {:?}",
        reader.name, answer.status, answer.body
    );
    let items = answer
        .get(reader.items_member)
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} is @required, got {:?}",
                reader.items_member, answer.body
            )
        });
    items
        .iter()
        .map(|item| {
            item.get(reader.identifier_member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a summary must name its {}, got {item:?}",
                        reader.identifier_member
                    )
                })
                .to_string()
        })
        .collect()
}

/// RULE: both readers page by the same walk — at most `maxResults` summaries in
/// ascending identifier order, a `nextToken` exactly when an entry remains beyond
/// the page, that token being this service's cursor for the page's last entry, and
/// the whole walk returning every persisted entry exactly once.
#[tokio::test]
async fn both_readers_walk_their_resource_by_the_same_page_rule() {
    for reader in READERS {
        for count in [0usize, 1, 2, 5, 7] {
            let dsql = Dsql::new();
            let (cluster, all) = populate(reader, &dsql, count).await;

            for page_size in [1usize, 2, 3, 100] {
                let case = format!("{}, {count} items, max-results={page_size}", reader.name);
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0usize;

                loop {
                    pages += 1;
                    assert!(
                        pages <= count + 2,
                        "case {case:?}: the walk did not terminate after {pages} pages"
                    );
                    let query = match &token {
                        Some(cursor) => format!("?max-results={page_size}&next-token={cursor}"),
                        None => format!("?max-results={page_size}"),
                    };
                    let answer = read_page(reader, &dsql, &cluster, &query).await;
                    let identifiers = page_identifiers(reader, &case, &answer);

                    assert!(
                        identifiers.len() <= page_size,
                        "case {case:?}: a page holds at most max-results items, got {}",
                        identifiers.len()
                    );
                    assert!(
                        identifiers.windows(2).all(|pair| pair[0] < pair[1]),
                        "case {case:?}: a page is a window over ascending identifier \
                         order, got {identifiers:?}"
                    );
                    if let (Some(previous), Some(first)) = (seen.last(), identifiers.first()) {
                        assert!(
                            previous < first,
                            "case {case:?}: a page resumes after the previous page's last \
                             identifier {previous:?}, got {first:?}"
                        );
                    }
                    seen.extend(identifiers.iter().cloned());

                    token = answer.get("nextToken").as_str().map(str::to_string);
                    assert_eq!(
                        token.is_some(),
                        seen.len() < all.len(),
                        "case {case:?}: a nextToken is issued exactly when an entry \
                         remains beyond the page ({} of {} seen), got {token:?}",
                        seen.len(),
                        all.len()
                    );
                    match (&token, identifiers.last()) {
                        (Some(cursor), Some(last)) => assert_eq!(
                            cursor,
                            &encode_pagination_cursor(&reader_scope(reader, &cluster), last),
                            "case {case:?}: the token must be this walk's cursor for \
                             the page's last identifier {last:?}, got {cursor:?}"
                        ),
                        (Some(cursor), None) => panic!(
                            "case {case:?}: an empty page can leave nothing behind, \
                             got the token {cursor:?}"
                        ),
                        _ => break,
                    }
                }

                let unique: BTreeSet<String> = seen.iter().cloned().collect();
                assert_eq!(
                    unique.len(),
                    seen.len(),
                    "case {case:?}: the walk returned a duplicate: {seen:?}"
                );
                assert_eq!(
                    unique, all,
                    "case {case:?}: the walk must return every persisted entry exactly once"
                );
            }
        }
    }
}

/// RULE: `maxResults` is bounded by the one shape both readers bind — honoured
/// inside the published range, refused outside it, by either reader — and a
/// request that omits it pages at that operation's own modelled default.
#[tokio::test]
async fn both_readers_bound_max_results_by_the_one_published_range() {
    let min = i64::try_from(MAX_RESULTS_MIN).expect("the published minimum fits");
    let max = i64::try_from(MAX_RESULTS_MAX).expect("the published maximum fits");

    for reader in READERS {
        let dsql = Dsql::new();
        // More entries than the default page size, so the default is observable.
        let total = reader.modeled_default + 2;
        let (cluster, all) = populate(reader, &dsql, total).await;
        assert_eq!(
            all.len(),
            total,
            "{}: the store must hold every minted entry",
            reader.name
        );

        // Inside the published range, including each bound itself.
        for value in [MAX_RESULTS_MIN, 2, MAX_RESULTS_MAX] {
            let case = format!("{}, max-results={value}", reader.name);
            let answer = read_page(reader, &dsql, &cluster, &format!("?max-results={value}")).await;
            let identifiers = page_identifiers(reader, &case, &answer);
            assert_eq!(
                identifiers.len(),
                total.min(value),
                "case {case:?}: a page inside the published range holds \
                 min(total, max-results) items, got {}",
                identifiers.len()
            );
        }

        // One value past each bound, and further out: refused, whichever reader
        // received it.
        for value in [min - 1, -5, max + 1, 1_000] {
            let case = format!("{}, max-results={value}", reader.name);
            let answer = read_page(reader, &dsql, &cluster, &format!("?max-results={value}")).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a value outside the published range must be refused, \
                 got {} {:?}",
                answer.status, answer.body
            );
        }

        // Omitted: the operation's own modelled default, which is the one thing
        // the shared walk takes per operation.
        let case = format!("{}, max-results omitted", reader.name);
        let defaulted = read_page(reader, &dsql, &cluster, "").await;
        let identifiers = page_identifiers(reader, &case, &defaulted);
        assert_eq!(
            identifiers.len(),
            reader.modeled_default,
            "case {case:?}: omitting max-results pages at the modelled default of {}, got {}",
            reader.modeled_default,
            identifiers.len()
        );
        assert!(
            defaulted.get("nextToken").is_string(),
            "case {case:?}: a default-sized page that left entries behind must carry a \
             nextToken, got {:?}",
            defaulted.get("nextToken")
        );
    }
}
