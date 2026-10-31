// SPDX-License-Identifier: CC0-1.0

//! What a page size and a query id a request names are ANSWERED with.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — and every resource a case needs is built through an implemented
//! operation, so what is asserted is what a client sees end to end.
//!
//! The rules under test:
//!
//! * a page maximum a paginated operation documents of the results RETURNED bounds the
//!   PAGE, not the request: `LookupEvents` ("The default number of results returned is
//!   50, with a maximum of 50 possible") serves any larger `MaxResults` with the page
//!   capped at that maximum and a `NextToken` while events remain, and refuses only a
//!   limit below the modelled minimum, which asks for a page that cannot exist;
//! * the modelled `InvalidMaxResultsException` ("thrown if the limit specified is not
//!   valid") answers an out-of-range value for the member an operation declares as its
//!   `@paginated` `pageSize`; a bounded integer member an operation does NOT declare as
//!   its page size — `GetQueryResults$MaxQueryResults`, whose `@paginated` trait names
//!   only the token members — is refused in the generic `ValidationException` envelope
//!   naming the member and the bound it broke;
//! * a query is resolved on its QUERY ID: `EventDataStore` is deprecated on
//!   `CancelQuery` / `DescribeQuery` ("no longer required by …Request"), so a request
//!   that names no store needs no store to exist and an id naming no query is
//!   `QueryIdNotFoundException`;
//! * `TagsLimitExceededException` carries the service's fixed sentence for the rule it
//!   refuses by — a resultant tag set larger than a resource may carry — whatever the
//!   request listed, and the refusal writes nothing.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, Reply};

/// The HTTP status every refusal below answers with: the status CloudTrail answers its
/// client errors with, which for the two not-found shapes is not the `@httpError(404)`
/// its model annotates (see `NOT_FOUND_STATUS` in `src/ops/describe_query.rs`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// `LookupEvents$MaxResults`' modelled `@range`, from the model rather than from this
/// service's code (`smithy_query(query="operation", operation="LookupEvents")` gives
/// `MaxResults` `@range{min:1,max:50}`).
const LOOKUP_MAX_RESULTS_MIN: i64 = 1;
/// See [`LOOKUP_MAX_RESULTS_MIN`].
const LOOKUP_MAX_RESULTS_MAX: i64 = 50;

/// `GetQueryResults$MaxQueryResults`' modelled `@range`
/// (`smithy_query(query="operation", operation="GetQueryResults")` gives
/// `@range{min:1,max:1000}` on a member its `@paginated` trait does not name).
const MAX_QUERY_RESULTS_MIN: i64 = 1;
/// See [`MAX_QUERY_RESULTS_MIN`].
const MAX_QUERY_RESULTS_MAX: i64 = 1000;

/// `ListEventDataStores$MaxResults`' modelled `@range`
/// (`smithy_query(query="operation", operation="ListEventDataStores")` gives
/// `@range{min:1,max:1000}` on the member that operation declares as its `pageSize`).
const LIST_STORES_MAX_RESULTS_MIN: i64 = 1;
/// See [`LIST_STORES_MAX_RESULTS_MIN`].
const LIST_STORES_MAX_RESULTS_MAX: i64 = 1000;

/// How many management events the lookup cases work over: more than the documented page
/// maximum, so a capped page always leaves a remainder to page through.
const RECORDED_EVENTS: usize = 60;

/// What these cases read off a reply.
impl Reply {
    /// The reported events' ids, in the order they were reported.
    fn event_ids(&self, case: &str) -> Vec<String> {
        self.expect_ok(case)
            .get("Events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: the reply carries no Events list: {self:?}"))
            .iter()
            .map(|event| {
                event
                    .get("EventId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The reply's `NextToken`, if it carried one.
    fn continuation(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }
}

/// Record `count` management events by SERVING `count` writes: every mutating call of
/// this service is written to the account's management-event log, which is what
/// `LookupEvents` reports from.
async fn record_events(emulator: &Emulator, count: usize) {
    for index in 0..count {
        let name = format!("page-trail-{index:03}");
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "page-bucket"}),
            )
            .await
            .expect_ok(&format!("CreateTrail {name}"));
    }
}

/// A `LookupEvents` request for the WRITE events alone.
///
/// Every call this service serves is recorded, a lookup included, so the cases below ask
/// for the events their own writes made rather than for every event: without the filter a
/// page walk would also report the records of the lookups the walk itself made, and no
/// count over the history would be stable. `ReadOnly` is one of the eight lookup
/// attributes the model's `LookupAttributeKey` declares.
fn lookup_writes(max_results: Option<i64>, token: Option<&str>) -> Value {
    let mut body = json!({
        "LookupAttributes": [{"AttributeKey": "ReadOnly", "AttributeValue": "false"}],
    });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    body
}

/// RULE: a page maximum documented of the results RETURNED bounds the page, not the
/// request — so a `MaxResults` above it is served with the page capped at the maximum
/// and a `NextToken` while events remain, and the whole history stays walkable by a
/// client that asks for any page size.
///
/// The rows differ along the page size: each published bound of the modelled `@range`,
/// one past the maximum, several far past it (the sizes an SDK paginator defaults to),
/// and no `MaxResults` at all.
#[tokio::test]
async fn a_lookup_page_size_above_its_maximum_is_served_with_the_page_capped() {
    let emulator = Emulator::new();
    record_events(&emulator, RECORDED_EVENTS).await;
    let recorded = i64::try_from(RECORDED_EVENTS).expect("the count fits an i64");

    // (case, the MaxResults the lookup asks for — None = the member omitted)
    let rows: [(&str, Option<i64>); 10] = [
        ("the modelled minimum", Some(LOOKUP_MAX_RESULTS_MIN)),
        ("inside the modelled range", Some(7)),
        ("one below the maximum", Some(LOOKUP_MAX_RESULTS_MAX - 1)),
        ("the modelled maximum", Some(LOOKUP_MAX_RESULTS_MAX)),
        ("one past the maximum", Some(LOOKUP_MAX_RESULTS_MAX + 1)),
        ("two past the maximum", Some(LOOKUP_MAX_RESULTS_MAX + 2)),
        ("far past the maximum", Some(100)),
        ("a paginator's default page size", Some(1_000)),
        ("one past that", Some(1_001)),
        ("no MaxResults at all", None),
    ];

    for (case, max_results) in rows {
        let reply = emulator
            .call("LookupEvents", &lookup_writes(max_results, None))
            .await;
        // The page the maximum allows, over the events recorded.
        let expected = max_results
            .unwrap_or(LOOKUP_MAX_RESULTS_MAX)
            .min(LOOKUP_MAX_RESULTS_MAX)
            .min(recorded);
        let page = reply.event_ids(case);
        assert_eq!(
            i64::try_from(page.len()).expect("a page length fits an i64"),
            expected,
            "case {case:?}: expected a page of {expected} events, got {} ({:?})",
            page.len(),
            reply.body
        );
        // "If the token does not appear, there are no more results to return."
        assert_eq!(
            reply.continuation().is_some(),
            expected < recorded,
            "case {case:?}: expected a token exactly while events remain, got {:?}",
            reply.body
        );
    }

    // And the whole history is walkable at a page size far above the maximum: the
    // capped pages report every recorded event exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=RECORDED_EVENTS {
        let case = format!("walking at MaxResults 1001, page {page}");
        let reply = emulator
            .call(
                "LookupEvents",
                &lookup_writes(Some(1_001), token.as_deref()),
            )
            .await;
        let ids = reply.event_ids(&case);
        assert!(
            i64::try_from(ids.len()).expect("a page length fits an i64") <= LOOKUP_MAX_RESULTS_MAX,
            "case {case:?}: a page may not pass the documented maximum, got {}",
            ids.len()
        );
        walked.extend(ids);
        match reply.continuation() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    let unique: std::collections::BTreeSet<&String> = walked.iter().collect();
    assert_eq!(
        (walked.len(), unique.len()),
        (RECORDED_EVENTS, RECORDED_EVENTS),
        "case walking at MaxResults 1001: expected every recorded event once, walked {}",
        walked.len()
    );
}

/// RULE: a `MaxResults` below the modelled minimum asks for a page that cannot exist,
/// and is refused with the error the operation models for a limit that is not valid.
///
/// The rows are the values below the published minimum, including the one just under it
/// and the extreme a 32-bit integer carries.
#[tokio::test]
async fn a_lookup_page_size_below_the_minimum_is_refused() {
    let emulator = Emulator::new();
    record_events(&emulator, 1).await;

    for max_results in [LOOKUP_MAX_RESULTS_MIN - 1, -1, i64::from(i32::MIN)] {
        let case = format!("MaxResults={max_results}");
        let reply = emulator
            .call("LookupEvents", &json!({"MaxResults": max_results}))
            .await;
        reply.expect_error(&case, CLIENT_ERROR_STATUS, "InvalidMaxResultsException");
        assert_eq!(
            reply.message(),
            format!(
                "The limit at 'maxResults' is not valid: it must have value greater than \
                 or equal to {LOOKUP_MAX_RESULTS_MIN}."
            ),
            "case {case:?}: expected the member and the bound it broke, got {:?}",
            reply.body
        );
    }
}

/// RULE: the modelled limit error answers an out-of-range value for the member an
/// operation declares as its `@paginated` `pageSize`; a bounded integer member an
/// operation does not declare as its page size keeps the generic validation envelope,
/// naming the member and the bound it broke and quoting nothing of the value.
///
/// The rows differ along the operation, the member, which published bound is broken and
/// how far past it the value is; each bound comes from the model, not from this
/// service's code.
#[tokio::test]
async fn an_out_of_range_page_size_carries_the_envelope_its_member_belongs_to() {
    let emulator = Emulator::new();
    // A well-formed `UUID` (`@length 36`, `^[a-f0-9\-]+$`) that names no query: the
    // page-size refusal precedes the query-id lookup, so every row answers the limit.
    let query_id = "3bdb1ba3-0b2e-4b0a-9f1e-6c9d5a2f7e10";

    // (case, operation, request, the error code, the message the refusal must carry)
    let rows: [(&str, &str, Value, &str, String); 8] = [
        (
            "a page size the operation does not declare, one past its maximum",
            "GetQueryResults",
            json!({"QueryId": query_id, "MaxQueryResults": MAX_QUERY_RESULTS_MAX + 1}),
            "ValidationException",
            validation_clause("maxQueryResults", "less than or equal to 1000"),
        ),
        (
            "the same member far past its maximum",
            "GetQueryResults",
            json!({"QueryId": query_id, "MaxQueryResults": 1_000_000}),
            "ValidationException",
            validation_clause("maxQueryResults", "less than or equal to 1000"),
        ),
        (
            "the same member one below its minimum",
            "GetQueryResults",
            json!({"QueryId": query_id, "MaxQueryResults": MAX_QUERY_RESULTS_MIN - 1}),
            "ValidationException",
            validation_clause("maxQueryResults", "greater than or equal to 1"),
        ),
        (
            "the same member far below its minimum",
            "GetQueryResults",
            json!({"QueryId": query_id, "MaxQueryResults": -42}),
            "ValidationException",
            validation_clause("maxQueryResults", "greater than or equal to 1"),
        ),
        (
            "a DECLARED page size one past its maximum",
            "ListEventDataStores",
            json!({"MaxResults": LIST_STORES_MAX_RESULTS_MAX + 1}),
            "InvalidMaxResultsException",
            limit_clause("maxResults", "less than or equal to 1000"),
        ),
        (
            "a DECLARED page size one below its minimum",
            "ListEventDataStores",
            json!({"MaxResults": LIST_STORES_MAX_RESULTS_MIN - 1}),
            "InvalidMaxResultsException",
            limit_clause("maxResults", "greater than or equal to 1"),
        ),
        (
            "a DECLARED page size on another listing, past its maximum",
            "ListQueries",
            json!({
                "EventDataStore": "00000000-0000-4000-8000-000000000000",
                "MaxResults": LIST_STORES_MAX_RESULTS_MAX + 1,
            }),
            "InvalidMaxResultsException",
            limit_clause("maxResults", "less than or equal to 1000"),
        ),
        (
            "a DECLARED page size on another listing, below its minimum",
            "ListQueries",
            json!({
                "EventDataStore": "00000000-0000-4000-8000-000000000000",
                "MaxResults": LIST_STORES_MAX_RESULTS_MIN - 1,
            }),
            "InvalidMaxResultsException",
            limit_clause("maxResults", "greater than or equal to 1"),
        ),
    ];

    for (case, operation, request, code, expected) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, CLIENT_ERROR_STATUS, code);
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.body
        );
    }

    // Every value INSIDE the range is accepted, so the rows above are about the values
    // past the bounds: the call then answers for the query id it carries, which is the
    // step that follows the limit.
    for max_query_results in [MAX_QUERY_RESULTS_MIN, 5, MAX_QUERY_RESULTS_MAX] {
        let case = format!("MaxQueryResults={max_query_results} on an unknown query");
        let reply = emulator
            .call(
                "GetQueryResults",
                &json!({"QueryId": query_id, "MaxQueryResults": max_query_results}),
            )
            .await;
        reply.expect_error(&case, CLIENT_ERROR_STATUS, "QueryIdNotFoundException");
    }
}

/// The generic validation envelope's clause for an out-of-range `member`: the member
/// path and the bound it broke, and none of the rejected value.
fn validation_clause(member: &str, bound: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have value {bound}"
    )
}

/// The modelled limit refusal's clause for a declared page-size `member`.
fn limit_clause(member: &str, bound: &str) -> String {
    format!("The limit at '{member}' is not valid: it must have value {bound}.")
}

/// RULE: a query is resolved on the id (or alias) a request carries — the deprecated
/// `EventDataStore` member only narrows which store's query a request addresses — so a
/// request that names no store needs no store to exist, and an id naming no query is
/// `QueryIdNotFoundException` on every operation that resolves one.
///
/// The rows are well-formed `UUID` ids no `StartQuery` ever minted, over the two
/// operations whose store member the model deprecates, with no store in the Region at
/// all and then with one — the id is what the caller is answered about either way.
#[tokio::test]
async fn an_unissued_query_id_is_not_found_with_no_store_member() {
    let emulator = Emulator::new();

    let ids = [
        "44444444-4444-4444-4444-444444444444",
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    ];
    for stores_exist in [false, true] {
        if stores_exist {
            emulator
                .call("CreateEventDataStore", &json!({"Name": "query-id-store"}))
                .await
                .expect_ok("CreateEventDataStore");
        }
        for operation in ["CancelQuery", "DescribeQuery"] {
            for id in ids {
                let case = format!("{operation} on {id}, stores in the Region: {stores_exist}");
                let reply = emulator.call(operation, &json!({"QueryId": id})).await;
                reply.expect_error(&case, CLIENT_ERROR_STATUS, "QueryIdNotFoundException");
                assert_eq!(
                    reply.message(),
                    format!(
                        "The QueryId value {id} does not exist or is not in the same region."
                    ),
                    "case {case:?}: expected the member and the value it named, got {:?}",
                    reply.body
                );
            }
        }
    }
}

/// RULE: a query id a `StartQuery` DID mint resolves without any store member, through
/// both operations — the cancellation persists, so the read agrees with it.
#[tokio::test]
async fn a_minted_query_id_cancels_and_reads_back_with_no_store_member() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateEventDataStore", &json!({"Name": "cancel-me-store"}))
        .await;
    let arn = created
        .expect_ok("CreateEventDataStore")
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // An event data store id is the resource suffix of its ARN.
    let store_id = arn
        .rsplit('/')
        .next()
        .expect("an event data store ARN carries its id")
        .to_string();

    let started = emulator
        .call(
            "StartQuery",
            &json!({"QueryStatement": format!("SELECT eventName FROM {store_id}")}),
        )
        .await;
    let query_id = started
        .expect_ok("StartQuery")
        .get("QueryId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // No `EventDataStore` member on either call: the query id is what resolves.
    let selector = json!({"QueryId": query_id});
    let cancelled = emulator.call("CancelQuery", &selector).await;
    let body = cancelled.expect_ok("CancelQuery with no store member");
    assert_eq!(
        (
            body.get("QueryId").and_then(Value::as_str),
            body.get("QueryStatus").and_then(Value::as_str)
        ),
        (Some(query_id.as_str()), Some("CANCELLED")),
        "case CancelQuery with no store member: expected the cancelled run, got {body:?}"
    );

    let described = emulator.call("DescribeQuery", &selector).await;
    let reported = described.expect_ok("DescribeQuery with no store member");
    assert_eq!(
        reported.get("QueryStatus").and_then(Value::as_str),
        Some("CANCELLED"),
        "case DescribeQuery with no store member: the cancellation must persist, got \
         {reported:?}"
    );

    // A second cancellation of the same id reaches the run's own state rule, not a
    // store refusal.
    emulator
        .call("CancelQuery", &selector)
        .await
        .expect_error(
            "CancelQuery on a terminated run",
            CLIENT_ERROR_STATUS,
            "InactiveQueryException",
        );
}

/// RULE: `TagsLimitExceededException` states the RULE it refuses by — a resultant tag
/// set larger than a resource may carry — in the service's own fixed sentence, whatever
/// the request listed, and the refusal writes nothing.
///
/// The rows differ along the operation refusing, how the limit is passed (one request
/// over it, or a merge that would pass it) and how far over it the count is, so the
/// sentence cannot be built from the submitted count.
#[tokio::test]
async fn the_tag_limit_refusal_states_the_rule_it_refuses_by() {
    let emulator = Emulator::new();
    let limit = state::TAGS_PER_RESOURCE_LIMIT;
    let expected = format!("The resultant tag set must not have more than {limit} user tags.");
    let tags = |count: usize, offset: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"Key": format!("key-{}", index + offset), "Value": "v"}))
                .collect(),
        )
    };

    // A trail that already carries one tag, so the merge path has something to merge.
    let trail = emulator
        .call(
            "CreateTrail",
            &json!({"Name": "tag-limit-trail", "S3BucketName": "tag-bucket"}),
        )
        .await;
    let trail_arn = trail
        .expect_ok("CreateTrail")
        .get("TrailARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    emulator
        .call(
            "AddTags",
            &json!({"ResourceId": trail_arn, "TagsList": tags(1, 0)}),
        )
        .await
        .expect_ok("AddTags with one tag");

    // (case, operation, request)
    let rows: [(&str, &str, Value); 5] = [
        (
            "a merge one past the limit",
            "AddTags",
            json!({"ResourceId": trail_arn, "TagsList": tags(limit, 1)}),
        ),
        (
            "a request far past the limit",
            "AddTags",
            json!({"ResourceId": trail_arn, "TagsList": tags(limit + 100, 1)}),
        ),
        (
            "a create one past the limit",
            "CreateTrail",
            json!({
                "Name": "tag-limit-create",
                "S3BucketName": "tag-bucket",
                "TagsList": tags(limit + 1, 0),
            }),
        ),
        (
            "a create far past the limit",
            "CreateTrail",
            json!({
                "Name": "tag-limit-create-2",
                "S3BucketName": "tag-bucket",
                "TagsList": tags(limit + 149, 0),
            }),
        ),
        (
            "a channel create past the limit",
            "CreateChannel",
            json!({
                "Name": "tag-limit-channel",
                "Source": "Custom",
                "Destinations": [{
                    "Type": "EVENT_DATA_STORE",
                    "Location": "aaaaaaaa-1111-2222-3333-444444444444",
                }],
                "Tags": tags(limit + 1, 0),
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, CLIENT_ERROR_STATUS, "TagsLimitExceededException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.body
        );
    }

    // Every refusal above left the trail's own tag set as it was.
    let listed = emulator
        .call("ListTags", &json!({"ResourceIdList": [trail_arn]}))
        .await;
    let body = listed.expect_ok("ListTags after the refusals");
    let held = body
        .get("ResourceTagList")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|entry| entry.get("TagsList"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        held.len(),
        1,
        "case the refusals wrote nothing: expected the one tag the trail carried, got \
         {body:?}"
    );
}
