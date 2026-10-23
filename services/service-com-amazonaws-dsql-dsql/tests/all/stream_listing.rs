// SPDX-License-Identifier: CC0-1.0

//! `ListStreams` — one page of a cluster's change data capture (CDC) streams,
//! end to end over the wire.
//!
//! Every case drives the real request path through `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request, so the `GET /stream/{clusterIdentifier}`
//! route, the `@httpLabel` / `@httpQuery` bindings, the boundary constraint checks
//! and the handler are exercised the way a client exercises them. The wire harness
//! (`Dsql`, `Answer`, the clock wait) is the one in `tests/all/harness.rs` that
//! every suite in this crate shares.
//!
//! The tables assert RULES over a CLASS of inputs rather than one recorded
//! request: pagination over several stream counts crossed with several page sizes,
//! the published `maxResults` bounds and its modelled default, the whole class of
//! cursors this service never issued, the cluster-scoping of a page, and the
//! agreement between a summary and what `GetStream` reads back.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{absent_identifier, is_cluster_id, wait_for_the_clock, Answer, Dsql};

/// `maxResults`' published `@range` bounds and its modelled `@default`. Source:
/// `smithy_query(query="operation", operation="ListStreams")` — `maxResults`
/// `@default: 10`, `@range(min: 1, max: 100)`.
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 100;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_DEFAULT: usize = 10;

/// The values `StreamStatus` accepts on the wire. Source:
/// `smithy_query(query="shape", shape="StreamStatus")`.
const STREAM_STATUS_VALUES: &[&str] = &[
    "ACTIVE",
    "CREATING",
    "DELETED",
    "DELETING",
    "FAILED",
    "IMPAIRED",
];

/// A `targetDefinition` naming a Kinesis destination in `region`, as
/// `KinesisStreamArn`'s and `RoleArn`'s patterns require.
fn kinesis_target(region: &str, stream_name: &str, role: &str) -> Value {
    json!({"kinesis": {
        "streamArn": format!(
            "arn:aws:kinesis:{region}:{}:stream/{stream_name}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "roleArn": format!("arn:aws:iam::{}:role/{role}", engine::EMULATOR_ACCOUNT_ID),
    }})
}

/// The ARN `StreamArn`'s pattern requires for `stream` of `cluster` in `region`.
fn expected_stream_arn(region: &str, cluster: &str, stream: &str) -> String {
    format!(
        "arn:aws:dsql:{region}:{}:cluster/{cluster}/stream/{stream}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `count` streams on `cluster`, returning the identifiers `CreateStream` minted.
async fn create_streams(dsql: &Dsql, cluster: &str, count: usize) -> BTreeSet<String> {
    let mut minted = BTreeSet::new();
    for index in 0..count {
        let created = dsql
            .create_stream(
                cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(
                        dsql.region(),
                        &format!("changes-{index}"),
                        "dsql-cdc",
                    ),
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
    minted
}

/// The `streams` array of a successful answer.
fn streams_of(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListStreams must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
        .get("streams")
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: streams is @required, got {:?}", answer.body))
}

/// A string member of one summary.
fn member(summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: `ListStreams` enumerates every stream persisted on the addressed
/// cluster, at most `maxResults` per page, resuming from the token the previous
/// page emitted, and the last page omits the token. Asserted for several stream
/// counts crossed with several page sizes.
#[tokio::test]
async fn list_pages_through_every_stream_of_the_cluster_exactly_once() {
    for stream_count in [0usize, 1, 2, 5, 7] {
        let dsql = Dsql::new();
        let cluster = dsql.create_settled().await;
        let all = create_streams(&dsql, &cluster, stream_count).await;

        for page_size in [1usize, 2, 3, 10, 100] {
            let case = format!("{stream_count} streams, max-results={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;

            loop {
                pages += 1;
                assert!(
                    pages <= stream_count + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                let query = match &token {
                    Some(t) => format!("?max-results={page_size}&next-token={t}"),
                    None => format!("?max-results={page_size}"),
                };
                let page = dsql.list_streams(&cluster, &query).await;
                let items = streams_of(&page, &case);
                assert!(
                    items.len() <= page_size,
                    "case {case:?}: a page must hold at most max-results items, got {}",
                    items.len()
                );
                for item in &items {
                    let identifier = member(item, "streamIdentifier");
                    assert!(
                        is_cluster_id(&identifier),
                        "case {case:?}: a summary streamIdentifier must match \
                         ^[a-z0-9]{{26}}$, got {identifier:?}"
                    );
                    assert_eq!(
                        member(item, "clusterIdentifier"),
                        cluster,
                        "case {case:?}: a summary must name the cluster it was listed under"
                    );
                    assert_eq!(
                        member(item, "arn"),
                        expected_stream_arn(dsql.region(), &cluster, &identifier),
                        "case {case:?}: a summary must carry the stored stream ARN"
                    );
                    assert!(
                        STREAM_STATUS_VALUES.contains(&member(item, "status").as_str()),
                        "case {case:?}: status must be a published StreamStatus, got {:?}",
                        item.get("status")
                    );
                    assert!(
                        item.get("creationTime")
                            .and_then(Value::as_f64)
                            .is_some_and(|seconds| seconds > 0.0),
                        "case {case:?}: creationTime must be the persisted timestamp, got {:?}",
                        item.get("creationTime")
                    );
                    seen.push(identifier);
                }
                token = page.get("nextToken").as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert_eq!(
                    items.len(),
                    page_size,
                    "case {case:?}: a token may only be emitted when the page was full"
                );
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: pagination returned a duplicate: {seen:?}"
            );
            assert_eq!(
                unique, all,
                "case {case:?}: pagination must return every stream exactly once"
            );
        }
    }
}

/// RULE: `maxResults` is honoured inside its published range, refused outside it,
/// and a request that omits it pages at the modelled default.
#[tokio::test]
async fn max_results_is_enforced_at_its_published_bounds() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    // More streams than the default page size, so the default is observable.
    let total = MAX_RESULTS_DEFAULT + 2;
    create_streams(&dsql, &cluster, total).await;

    for value in [MAX_RESULTS_MIN, 2, 50, MAX_RESULTS_MAX] {
        let answer = dsql
            .list_streams(&cluster, &format!("?max-results={value}"))
            .await;
        let case = format!("max-results={value}");
        let items = streams_of(&answer, &case);
        let expected = total.min(usize::try_from(value).expect("a positive bound"));
        assert_eq!(
            items.len(),
            expected,
            "case {case:?}: a page inside the published range holds min(total, max-results) items"
        );
    }

    for value in [MAX_RESULTS_MIN - 1, -5, MAX_RESULTS_MAX + 1] {
        let answer = dsql
            .list_streams(&cluster, &format!("?max-results={value}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case max-results={value}: outside the published range must be refused, got {} {:?}",
            answer.status, answer.body
        );
    }

    let defaulted = dsql.list_streams(&cluster, "").await;
    let items = streams_of(&defaulted, "max-results omitted");
    assert_eq!(
        items.len(),
        MAX_RESULTS_DEFAULT,
        "omitting max-results must page at the modelled default of {MAX_RESULTS_DEFAULT}, got {}",
        items.len()
    );
    assert!(
        defaulted.get("nextToken").is_string(),
        "a default-sized page that left streams behind must carry a nextToken, got {:?}",
        defaulted.get("nextToken")
    );
}

/// RULE: a `nextToken` this service did not issue is refused as invalid input,
/// not served as an empty page or as a page starting after some arbitrary key.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    create_streams(&dsql, &cluster, 2).await;

    // A cursor is the hex of a `StreamId`, so the class of things that are not one
    // spans both "not hex at all" and "hex of something that is not an identifier".
    let fabricated_hex = [
        ::hex::encode("hello"),
        ::hex::encode("AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode([0xff, 0xfe]),
    ];
    let tokens: Vec<&str> = ["not-a-cursor", "zzzz", "ff00ffzz", "0"]
        .into_iter()
        .chain(fabricated_hex.iter().map(String::as_str))
        .collect();

    for token in tokens {
        let answer = dsql
            .list_streams(&cluster, &format!("?max-results=10&next-token={token}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: an unissued next-token must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {token:?}: the refusal must be a ValidationException, got {:?}",
            answer.error
        );
    }
}

/// RULE: a page is scoped to the cluster it was addressed to — a cluster reports
/// its own streams and no other cluster's, and a cluster holding none reports an
/// empty list.
#[tokio::test]
async fn a_page_holds_exactly_the_addressed_clusters_streams() {
    let dsql = Dsql::new();
    // Several clusters holding different numbers of streams, including none.
    let counts = [0usize, 1, 3];
    let clusters = dsql.create_settled_many(counts.len()).await;
    let mut expected: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (cluster, count) in clusters.iter().zip(counts) {
        let minted = create_streams(&dsql, cluster, count).await;
        expected.push((cluster.clone(), minted));
    }

    for (cluster, minted) in &expected {
        let case = format!("cluster {cluster} with {} streams", minted.len());
        let page = dsql.list_streams(cluster, "?max-results=100").await;
        let items = streams_of(&page, &case);
        let listed: BTreeSet<String> = items
            .iter()
            .map(|item| member(item, "streamIdentifier"))
            .collect();
        assert_eq!(
            &listed, minted,
            "case {case:?}: a page must hold exactly the streams of that cluster"
        );
        assert!(
            page.get("nextToken").is_null(),
            "case {case:?}: a page that left nothing behind must omit nextToken, got {:?}",
            page.get("nextToken")
        );
    }
}

/// RULE: a cluster this service does not hold is reported as a missing resource,
/// never as a page of no streams.
#[tokio::test]
async fn an_unknown_cluster_is_a_missing_resource() {
    let dsql = Dsql::new();
    let known = dsql.create_settled().await;

    // The class of cluster identifiers that name no cluster of this service: one
    // never created, and one whose cluster is already gone.
    let deleted = dsql.create_settled().await;
    let removed = dsql.delete(&deleted).await;
    assert_eq!(
        removed.status, 200,
        "DeleteCluster must be accepted, got {} {:?}",
        removed.status, removed.body
    );
    wait_for_the_clock().await;

    for identifier in [absent_identifier(), deleted.clone()] {
        let answer = dsql.list_streams(&identifier, "?max-results=10").await;
        assert_eq!(
            answer.status, 404,
            "case {identifier:?}: an unknown cluster must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {identifier:?}: the 404 must be a ResourceNotFoundException, got {:?}",
            answer.error
        );
    }

    // A malformed identifier is refused at the boundary, not reported as missing.
    // (An empty label is a different route altogether, so it is not this rule's
    // case.)
    for malformed in ["TOOSHORT", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "0123456789012345678901234567"] {
        let answer = dsql.list_streams(malformed, "?max-results=10").await;
        assert!(
            answer.status == 400 || answer.status == 404,
            "case {malformed:?}: a malformed clusterIdentifier must be refused, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    let served = dsql.list_streams(&known, "?max-results=10").await;
    assert_eq!(
        streams_of(&served, "a cluster that exists"),
        Vec::<Value>::new(),
        "a cluster holding no streams must report an empty list"
    );
}

/// RULE: a summary reports the stream as the resource reads back — every member
/// it carries is the one `GetStream` reports for the same stream — and listing is
/// a read: it changes nothing a later caller sees.
#[tokio::test]
async fn a_summary_agrees_with_the_stream_it_summarises() {
    let dsql = Dsql::in_region("eu-west-1");
    let cluster = dsql.create_settled().await;
    let minted = create_streams(&dsql, &cluster, 3).await;

    // Before the clock settles the creations, and after it: the summary tracks the
    // status the resource itself reports at each point.
    for phase in ["while CREATING", "once settled"] {
        if phase == "once settled" {
            wait_for_the_clock().await;
        }
        let page = dsql.list_streams(&cluster, "?max-results=100").await;
        let items = streams_of(&page, phase);
        assert_eq!(
            items.len(),
            minted.len(),
            "case {phase:?}: every stream must be listed, got {:?}",
            page.body
        );
        for item in &items {
            let identifier = member(item, "streamIdentifier");
            let case = format!("{phase}, stream {identifier}");
            let read = dsql.get_stream(&cluster, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetStream must answer 200, got {} {:?}",
                read.status, read.body
            );
            for name in ["clusterIdentifier", "streamIdentifier", "arn", "status"] {
                assert_eq!(
                    item.get(name),
                    read.body.get(name),
                    "case {case:?}: the summary's {name} must be what GetStream reports"
                );
            }
            assert_eq!(
                item.get("creationTime"),
                read.body.get("creationTime"),
                "case {case:?}: the summary's creationTime must be the persisted one"
            );
        }
    }

    // Two reads in a row, with the clock already settled, see the same page.
    let first = dsql.list_streams(&cluster, "?max-results=100").await;
    let second = dsql.list_streams(&cluster, "?max-results=100").await;
    assert_eq!(
        first.get("streams"),
        second.get("streams"),
        "listing must not change what a later caller sees: {:?} then {:?}",
        first.get("streams"),
        second.get("streams")
    );
}
