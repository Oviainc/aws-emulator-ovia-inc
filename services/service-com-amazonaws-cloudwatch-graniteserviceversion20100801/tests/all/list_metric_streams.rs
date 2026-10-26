// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListMetricStreams`: the listing of the metric streams this
//! account's store holds.
//!
//! The rule under test is that the listing is COMPUTED from the store on every call — every
//! stream `PutMetricStream` created and no stream `DeleteMetricStream` removed, each entry
//! carrying only the members `MetricStreamEntry` models, ordered by name so repeated calls
//! and successive pages agree, cut at `MaxResults` with a `NextToken` that resumes exactly
//! where the previous page stopped. Each table below spans that rule along what it
//! generalises over (how many streams exist, the page size asked for, which page is being
//! read, the output format and state a stream carries, and what a create or delete has
//! changed since the last call) rather than along one recorded call.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The members `MetricStreamEntry` declares, from
/// `smithy_query(query="shape", shape="MetricStreamEntry")`: the listing carries this subset
/// of a stream's configuration and nothing else.
const ENTRY_MEMBERS: [&str; 7] = [
    "Arn",
    "CreationDate",
    "LastUpdateDate",
    "Name",
    "FirehoseArn",
    "State",
    "OutputFormat",
];

/// Members a stream carries that `MetricStreamEntry` does NOT model, so a caller that needs
/// them has to read the stream itself with `GetMetricStream`.
const MEMBERS_ONLY_GET_REPORTS: [&str; 5] = [
    "IncludeFilters",
    "ExcludeFilters",
    "RoleArn",
    "StatisticsConfigurations",
    "IncludeLinkedAccountsMetrics",
];

/// Create one metric stream called `name`, with `extra` merged over the members the model
/// makes required.
async fn given_stream(router: &MultiServiceRouter, name: &str, output_format: &str, extra: Value) {
    let mut request = json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:us-east-1:000000000000:deliverystream/{name}"),
        "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
        "OutputFormat": output_format,
    });
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        request[member.as_str()] = value.clone();
    }
    ok(router, "PutMetricStream", &request).await;
}

/// One `ListMetricStreams` page: its entries and its `NextToken`.
async fn list(router: &MultiServiceRouter, request: &Value) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "ListMetricStreams", request).await;
    let entries = body
        .get("Entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListMetricStreams must report an Entries array, got {body}"));
    let token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (entries, token)
}

/// The `Name` of each entry, in the order the page reported them.
fn names(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every stream name the listing reports, walking the pages at `max_results` per page.
async fn all_names_paged(router: &MultiServiceRouter, max_results: u32) -> Vec<String> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    // A page size of at least one makes each page consume at least one stream, so the walk
    // is bounded by the number of streams; the cap only stops a non-terminating token.
    for _ in 0..100 {
        let mut request = json!({"MaxResults": max_results});
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let (entries, next) = list(router, &request).await;
        assert!(
            entries.len() <= max_results as usize,
            "a page must hold at most the MaxResults asked for ({max_results}), got {}",
            entries.len()
        );
        collected.extend(names(&entries));
        match next {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!("the paged walk did not terminate: collected {collected:?}");
}

#[tokio::test]
async fn the_listing_reports_every_stored_stream_with_the_members_the_entry_models() {
    // Rows differ along the configuration each stream carries — the output format, which of
    // the mutually exclusive namespace filters is used, whether statistics are configured —
    // because the listing is the same subset of every one of them.
    let streams: Vec<(&str, &str, Value)> = vec![
        ("bare-json", "json", json!({})),
        (
            "with-include-filter",
            "opentelemetry0.7",
            json!({"IncludeFilters": [{"Namespace": "AWS/EC2"}]}),
        ),
        (
            "with-exclude-filter",
            "opentelemetry1.0",
            json!({
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
                "IncludeLinkedAccountsMetrics": true,
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
                    "AdditionalStatistics": ["p99"],
                }],
            }),
        ),
    ];

    let router = build_router();
    // Created out of name order, so the reported order cannot be the creation order.
    for (name, format, extra) in streams.iter().rev() {
        given_stream(&router, name, format, extra.clone()).await;
    }

    let (entries, token) = list(&router, &json!({})).await;
    assert_eq!(
        token, None,
        "case \"every stream on one page\": a listing that reported every stream must carry no \
         NextToken, got {token:?}"
    );
    assert_eq!(
        names(&entries),
        vec!["bare-json", "with-exclude-filter", "with-include-filter"],
        "case \"every stream on one page\": expected every stored stream ordered by name, got \
         {entries:?}"
    );

    for (entry, name) in entries.iter().zip(names(&entries)) {
        let case = name.clone();
        // Each entry's members are the stream's own, as `GetMetricStream` reports them.
        let stream = ok(&router, "GetMetricStream", &json!({"Name": &name})).await;
        for member in ENTRY_MEMBERS {
            assert_eq!(
                entry.get(member),
                stream.get(member),
                "case {case:?}: the entry's {member} must be the stream's own, got {entry:?}"
            );
            assert!(
                entry.get(member).is_some(),
                "case {case:?}: a configured stream's entry must report {member}, got {entry:?}"
            );
        }
        // "This structure contains the configuration information about one metric stream."
        // — the entry models the subset above, so the rest is reported only by the reader
        // that models it.
        for member in MEMBERS_ONLY_GET_REPORTS {
            assert!(
                entry.get(member).is_none(),
                "case {case:?}: MetricStreamEntry does not model {member}, got {entry:?}"
            );
        }
        assert_eq!(
            entry.get("Arn").and_then(Value::as_str),
            Some(
                format!("arn:aws:cloudwatch:us-east-1:000000000000:metric-stream/{name}").as_str()
            ),
            "case {case:?}: expected the stream's own ARN, got {entry:?}"
        );
    }

    // A read changes nothing a later caller sees.
    let (again, again_token) = list(&router, &json!({})).await;
    assert_eq!(
        (again, again_token),
        (entries, token),
        "case \"a repeated listing\": the listing must be stable across repeated calls"
    );
}

#[tokio::test]
async fn an_empty_store_lists_no_stream() {
    let router = build_router();
    let (entries, token) = list(&router, &json!({})).await;
    assert!(
        entries.is_empty() && token.is_none(),
        "case \"no stream configured\": expected an empty listing and no NextToken, got \
         {entries:?} / {token:?}"
    );

    // The same holds once every stream that ever existed has been deleted.
    given_stream(&router, "transient", "json", json!({})).await;
    ok(&router, "DeleteMetricStream", &json!({"Name": "transient"})).await;
    let (entries, token) = list(&router, &json!({})).await;
    assert!(
        entries.is_empty() && token.is_none(),
        "case \"every stream deleted\": expected an empty listing and no NextToken, got \
         {entries:?} / {token:?}"
    );
}

#[tokio::test]
async fn the_pages_cover_every_stream_once_whatever_page_size_is_asked_for() {
    // Rows differ along the number of streams and the page size: the whole listing in one
    // page, pages of one, a size that divides the count exactly, one that leaves a partial
    // last page, a size wider than the count, and the widest the member's modelled 1..500
    // range allows.
    let cases: Vec<(&str, usize, u32)> = vec![
        ("one stream, pages of one", 1, 1),
        ("six streams, pages of one", 6, 1),
        ("six streams, pages of two", 6, 2),
        ("six streams, pages of four", 6, 4),
        ("six streams, a page wider than the store", 6, 50),
        ("six streams, the widest modelled page", 6, 500),
    ];

    for (case, count, max_results) in cases {
        let router = build_router();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            // Names that do not sort in creation order, so the ordering is asserted rather
            // than coincidental.
            let name = format!("stream-{}", (count - index) * 7 % 10);
            given_stream(&router, &name, "json", json!({})).await;
            expected.push(name);
        }
        expected.sort();
        expected.dedup();

        let walked = all_names_paged(&router, max_results).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must cover every stream exactly once, ordered by name"
        );
    }
}

#[tokio::test]
async fn a_page_boundary_names_the_stream_the_next_page_starts_at() {
    let router = build_router();
    for name in ["alpha", "bravo", "charlie", "delta"] {
        given_stream(&router, name, "json", json!({})).await;
    }

    // The token of a page is opaque to the caller but resumes exactly where that page
    // stopped, and asking for the same page twice reports the same entries.
    let (first, first_token) = list(&router, &json!({"MaxResults": 2})).await;
    assert_eq!(
        names(&first),
        vec!["alpha", "bravo"],
        "case \"the first page of two\": got {first:?}"
    );
    let token = first_token.expect("a truncated listing must carry a NextToken");

    let (second, second_token) =
        list(&router, &json!({"MaxResults": 2, "NextToken": &token})).await;
    assert_eq!(
        names(&second),
        vec!["charlie", "delta"],
        "case \"the second page of two\": got {second:?}"
    );
    assert_eq!(
        second_token, None,
        "case \"the last page\": expected no NextToken, got {second_token:?}"
    );

    // The same token reports the same page again: a read of a page consumes nothing.
    let (replayed, _) = list(&router, &json!({"MaxResults": 2, "NextToken": &token})).await;
    assert_eq!(
        names(&replayed),
        names(&second),
        "case \"the same token twice\": a token must keep naming the same page, got {replayed:?}"
    );

    // A token read at one page size still names the same STREAM at another, so the pages of
    // a walk whose size changes still cover each stream once.
    let (wider, _) = list(&router, &json!({"MaxResults": 500, "NextToken": &token})).await;
    assert_eq!(
        names(&wider),
        vec!["charlie", "delta"],
        "case \"a wider page from the same token\": got {wider:?}"
    );
}

#[tokio::test]
async fn a_token_the_service_did_not_issue_is_refused() {
    let router = build_router();
    given_stream(&router, "listed", "json", json!({})).await;

    // Rows differ along the form of the bad token: text that is not a token at all, the bare
    // stream name a caller might guess the cursor is, and one with an odd nibble count.
    for token in ["not-a-token", "listed", "zz", "abc"] {
        let (status, body) = call(
            &router,
            "ListMetricStreams",
            &json!({"NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextToken"),
            "case {token:?}: expected HTTP 400 InvalidNextToken, got {status} {body}"
        );
        // The refusal is one fixed sentence whichever token was offered: nothing of the
        // request is echoed back.
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some("Invalid NextToken"),
            "case {token:?}: expected CloudWatch's fixed refusal sentence, got {body}"
        );
    }

    // The refused call changed nothing: the listing still reports the stream.
    let (entries, _) = list(&router, &json!({})).await;
    assert_eq!(
        names(&entries),
        vec!["listed"],
        "case \"after a refused token\": the listing must be unchanged, got {entries:?}"
    );
}

#[tokio::test]
async fn the_listing_follows_every_change_the_lifecycle_makes() {
    let router = build_router();
    given_stream(&router, "lifecycle-a", "json", json!({})).await;
    given_stream(&router, "lifecycle-b", "opentelemetry1.0", json!({})).await;

    /// The `State` and `OutputFormat` the listing reports for `name`.
    async fn listed(router: &MultiServiceRouter, name: &str) -> Option<(String, String)> {
        let (entries, _) = list(router, &json!({})).await;
        entries.iter().find_map(|entry| {
            (entry.get("Name").and_then(Value::as_str) == Some(name)).then(|| {
                (
                    entry
                        .get("State")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    entry
                        .get("OutputFormat")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
        })
    }

    // "When you use PutMetricStream to create a new metric stream, the stream is created in
    // the running state."
    assert_eq!(
        listed(&router, "lifecycle-a").await,
        Some(("running".to_string(), "json".to_string())),
        "case \"a newly created stream\": expected it listed as running"
    );

    // Each row changes the store through a different operation, and the listing must report
    // the store as that operation left it.
    /// One lifecycle step: the case, the operation that changes the store, its request, and
    /// the `State` and `OutputFormat` the listing must report afterwards (`None` once the
    /// stream is gone).
    type Step = (&'static str, &'static str, Value, Option<(&'static str, &'static str)>);
    let steps: Vec<Step> = vec![
        (
            "stopped",
            "StopMetricStreams",
            json!({"Names": ["lifecycle-a"]}),
            Some(("stopped", "json")),
        ),
        (
            "started again",
            "StartMetricStreams",
            json!({"Names": ["lifecycle-a"]}),
            Some(("running", "json")),
        ),
        (
            "reconfigured to another output format",
            "PutMetricStream",
            json!({
                "Name": "lifecycle-a",
                "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/second",
                "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
                "OutputFormat": "opentelemetry0.7",
            }),
            Some(("running", "opentelemetry0.7")),
        ),
        (
            "deleted",
            "DeleteMetricStream",
            json!({"Name": "lifecycle-a"}),
            None,
        ),
    ];

    for (case, operation, request, expected) in steps {
        ok(&router, operation, &request).await;
        let got = listed(&router, "lifecycle-a").await;
        let expected = expected.map(|(state, format)| (state.to_string(), format.to_string()));
        assert_eq!(
            got, expected,
            "case {case:?}: the listing must report the store as {operation} left it"
        );
    }

    // Only the stream each step named changed: the other one is still listed as created.
    assert_eq!(
        listed(&router, "lifecycle-b").await,
        Some(("running".to_string(), "opentelemetry1.0".to_string())),
        "case \"an untouched stream\": expected it still listed as created"
    );
}
