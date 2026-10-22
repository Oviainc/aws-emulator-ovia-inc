// SPDX-License-Identifier: CC0-1.0

//! What X-Ray judges about a request, and WHEN — end to end over the wire (the
//! shared harness in `xray_wire`), so every assertion is about an HTTP response
//! rather than about internal state.
//!
//! Four rules, each checked over a table whose rows differ along what the rule
//! generalises over:
//!
//! * an operation whose model documents two members as ALTERNATIVE selectors —
//!   each "required if" the other "isn't provided", as
//!   `smithy_query(query="shape",shape="GetInsightSummariesRequest")` documents
//!   `GroupARN` and `GroupName` — requires exactly one of them: a request naming
//!   neither is refused with the operation's `InvalidRequestException` at HTTP
//!   400, never answered with an unscoped result;
//! * a group selector narrows a derived read to the traffic the group's own
//!   `FilterExpression` selects, so a well-formed read over an existing group is
//!   ANSWERED with the aggregate (or the graph) of the matching data — empty
//!   when nothing matches — and a filter expression is never itself a reason to
//!   refuse;
//! * a service-owned resource no API can create — the account's one indexing
//!   rule — reports a name other than its own as an invalid request (400), not
//!   as a resource that happens to be missing (404);
//! * request validation PRECEDES resource resolution: an absent required member
//!   and a token that is not of the form the service issues are refused with
//!   `InvalidRequestException` (400) before the store is consulted, and HTTP 404
//!   `ResourceNotFoundException` is reserved for a well-formed token whose
//!   retrieval is not there.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::update_indexing_rule::ONLY_DEFAULT_RULE_IS_UPDATABLE;
use service_com_amazonaws_xray_awsxray::state::{
    CONFLICTING_GROUP_SELECTORS_MESSAGE, RETRIEVAL_TOKEN_HEX_DIGITS, TRACE_FORMAT_XRAY,
};

use crate::xray_wire::{array, root_document, trace_id, Answer, Xray};

/// A 60-second-aligned epoch instant, so every seeded request below falls in
/// ONE aggregation period: `1_699_999_980 == 60 * 28_333_333`.
const ALIGNED: f64 = 1_699_999_980.0;

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// Create a group with the given filter expression, answering the ARN the
/// service minted. `CreateGroup` requires an expression, so every group here
/// carries one.
async fn create_group(xray: &Xray, name: &str, filter: &str) -> String {
    let created = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": name, "FilterExpression": filter}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating group {name:?} with {filter:?} must succeed, got {created:?}"
    );
    created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// =============================================================================
// 1. Exactly one group selector, where the model documents the pair as
//    alternatives
// =============================================================================

#[tokio::test]
async fn get_insight_summaries_requires_exactly_one_group_selector() {
    let xray = Xray::new();
    let arn = create_group(&xray, "scoped", "fault = true").await;

    // Rows differ in everything BUT the selector: the width and age of the
    // window, whether `States` narrows the listing, whether a page is bounded,
    // and whether an empty `States` list was sent — none of which makes a
    // request that names no group answerable.
    let without_selector: Vec<(&str, Value)> = vec![
        (
            "the window alone",
            json!({"StartTime": now() - 3_600.0, "EndTime": now()}),
        ),
        (
            "a one-second window",
            json!({"StartTime": now() - 1.0, "EndTime": now()}),
        ),
        (
            "a window a day wide",
            json!({"StartTime": now() - 86_400.0, "EndTime": now()}),
        ),
        (
            "a zero-width window",
            json!({"StartTime": now() - 60.0, "EndTime": now() - 60.0}),
        ),
        (
            "with States",
            json!({"StartTime": now() - 3_600.0, "EndTime": now(), "States": ["ACTIVE"]}),
        ),
        (
            "with an empty States list",
            json!({"StartTime": now() - 3_600.0, "EndTime": now(), "States": []}),
        ),
        (
            "with MaxResults at the model's minimum",
            json!({"StartTime": now() - 3_600.0, "EndTime": now(), "MaxResults": 1}),
        ),
        (
            "with MaxResults at the model's maximum",
            json!({"StartTime": now() - 3_600.0, "EndTime": now(), "MaxResults": 100}),
        ),
    ];

    for (case, body) in &without_selector {
        let answer = xray.call("/InsightSummaries", body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: a request naming neither GroupName nor GroupARN must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(CONFLICTING_GROUP_SELECTORS_MESSAGE),
            "case {case:?}: the refusal must state the rule, got {answer:?}"
        );
        assert!(
            answer.get("InsightSummaries").is_null(),
            "case {case:?}: a refused request must carry no listing, got {answer:?}"
        );
    }

    // And naming BOTH is the same refusal, with the same sentence: the rule is
    // exactly one selector, not at least one.
    let both = xray
        .call(
            "/InsightSummaries",
            &json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "GroupName": "scoped",
                "GroupARN": arn.clone(),
            }),
        )
        .await;
    assert_eq!(
        (both.status, both.error_type.as_deref(), both.message()),
        (
            400,
            Some("InvalidRequestException"),
            Some(CONFLICTING_GROUP_SELECTORS_MESSAGE)
        ),
        "both selectors together must be refused with the same sentence, got {both:?}"
    );

    // The positive control: each selector ON ITS OWN is serviced, including the
    // reserved group every account holds.
    for (case, selector) in [
        ("GroupName", json!({"GroupName": "scoped"})),
        ("GroupARN", json!({"GroupARN": arn})),
        ("the reserved Default group", json!({"GroupName": "Default"})),
    ] {
        let mut body = json!({"StartTime": now() - 3_600.0, "EndTime": now()});
        for (member, value) in selector.as_object().expect("the selector is an object") {
            body[member] = value.clone();
        }
        let answer = xray.call("/InsightSummaries", &body).await;
        assert_eq!(
            (answer.status, array(&answer.body, "InsightSummaries")),
            (200, &[] as &[Value]),
            "case {case:?}: one selector must be serviced, and a fresh account holds no insight, got {answer:?}"
        );
    }
}

// =============================================================================
// 2. A group's FilterExpression narrows a derived read; it never refuses one
// =============================================================================

/// One seeded request: the root segment's service name, its recorded status, the
/// instant it started and how long it took, plus any annotations.
struct Seeded {
    label: &'static str,
    service: &'static str,
    status: i64,
    start: f64,
    duration: f64,
    annotations: Option<Value>,
}

/// The traffic every row of the filter table is evaluated over: four requests in
/// one aggregation period, differing in service, recorded status, response time
/// and annotations — the axes a filter expression selects along.
fn seeded_requests() -> Vec<Seeded> {
    vec![
        Seeded {
            label: "alpha, 200, 1s",
            service: "alpha",
            status: 200,
            start: ALIGNED + 1.0,
            duration: 1.0,
            annotations: None,
        },
        Seeded {
            label: "beta, 503, 1s",
            service: "beta",
            status: 503,
            start: ALIGNED + 3.0,
            duration: 1.0,
            annotations: None,
        },
        Seeded {
            label: "gamma, 429, 3s",
            service: "gamma",
            status: 429,
            start: ALIGNED + 5.0,
            duration: 3.0,
            annotations: None,
        },
        Seeded {
            label: "delta, 201, 1s, annotated",
            service: "delta",
            status: 201,
            start: ALIGNED + 9.0,
            duration: 1.0,
            annotations: Some(json!({"stage": "prod"})),
        },
    ]
}

/// Ingest the seeded requests, one trace each, through `PutTraceSegments`.
async fn seed_traffic(xray: &Xray) {
    let mut documents = Vec::new();
    for (index, request) in seeded_requests().iter().enumerate() {
        let trace = trace_id(600 + u32::try_from(index).expect("the index fits"));
        let mut document = root_document(
            &format!("{:016x}", 0x6000 + index),
            &trace,
            request.service,
            request.start,
            Some(request.start + request.duration),
        );
        document["http"] = json!({
            "request": {"method": "GET", "url": "http://localhost/"},
            "response": {"status": request.status},
        });
        if let Some(annotations) = &request.annotations {
            document["annotations"] = annotations.clone();
        }
        documents.push(document.to_string());
    }
    let ingested = xray
        .call(
            "/TraceSegments",
            &json!({"TraceSegmentDocuments": documents}),
        )
        .await;
    assert_eq!(
        ingested.status, 200,
        "seeding the traffic must succeed, got {ingested:?}"
    );
    assert!(
        array(&ingested.body, "UnprocessedTraceSegments").is_empty(),
        "every seeded document must be stored, got {ingested:?}"
    );
}

/// `EdgeSummaryStatistics$TotalCount` of one aggregation period.
fn total_count(period: &Value) -> i64 {
    period
        .get("EdgeSummaryStatistics")
        .and_then(|edge| edge.get("TotalCount"))
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// The `Service$Name`s a graph answer carries.
fn graph_names(answer: &Answer) -> BTreeSet<String> {
    array(&answer.body, "Services")
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Every `Service$Name` a `GetServiceGraph` call answers, walking the pages: the
/// graph of several services exceeds one page, and the rule under test is about
/// WHICH services the group selects, not about where a page ends.
async fn every_graph_name(xray: &Xray, body: &Value, case: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut token: Option<String> = None;
    for page in 0..8 {
        let mut body = body.clone();
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let answer = xray.call("/ServiceGraph", &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {page} of the graph of a filtered group must be derived, not refused, got {answer:?}"
        );
        names.extend(graph_names(&answer));
        match answer.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case {case:?}: the graph pagination must terminate")
}

#[tokio::test]
async fn a_groups_filter_expression_narrows_the_statistics_and_the_graph() {
    let xray = Xray::new();
    seed_traffic(&xray).await;

    // Rows differ in the KIND of expression the group carries — a service
    // selector, a flag, an ordered comparison, an indexed http member, an
    // annotation, a disjunction of two — and in how much of the seeded traffic
    // each selects, from all four requests down to none.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("service(\"alpha\")", vec!["alpha"]),
        ("service(\"beta\")", vec!["beta"]),
        ("fault = true", vec!["beta"]),
        ("throttle = true", vec!["gamma"]),
        ("responsetime > 2", vec!["gamma"]),
        ("http.status = 200", vec!["alpha"]),
        ("annotation.stage = \"prod\"", vec!["delta"]),
        (
            "fault = true OR service(\"alpha\")",
            vec!["alpha", "beta"],
        ),
        ("duration >= 0", vec!["alpha", "beta", "gamma", "delta"]),
        ("service(\"nothing-recorded\")", vec![]),
        ("http.status = 418", vec![]),
    ];

    let window = json!({"StartTime": ALIGNED, "EndTime": ALIGNED + 60.0});
    for (index, (expression, expected)) in rows.iter().enumerate() {
        let name = format!("filter{index}");
        create_group(&xray, &name, expression).await;

        // The aggregation: one period holding exactly the selected requests, or
        // no period at all when the expression selects none of them — an empty
        // series, never a client error.
        let mut statistics_body = window.clone();
        statistics_body["GroupName"] = json!(&name);
        let statistics = xray
            .call("/TimeSeriesServiceStatistics", &statistics_body)
            .await;
        assert_eq!(
            statistics.status, 200,
            "case {expression:?}: a group carrying a FilterExpression must be aggregated, not refused, got {statistics:?}"
        );
        assert_eq!(
            statistics.get("ContainsOldGroupVersions"),
            &json!(false),
            "case {expression:?}: the aggregation is recomputed from stored segments, so it mixes no older group version, got {statistics:?}"
        );
        let periods = array(&statistics.body, "TimeSeriesServiceStatistics");
        let counted: i64 = periods.iter().map(total_count).sum();
        assert_eq!(
            (periods.len(), counted),
            (
                usize::from(!expected.is_empty()),
                i64::try_from(expected.len()).expect("the row is small")
            ),
            "case {expression:?}: expected the {} selected request(s) in one period, got {statistics:?}",
            expected.len()
        );

        // And the graph derived for the same group names exactly the selected
        // services, so the expression narrows every derived read alike.
        let mut graph_body = window.clone();
        graph_body["GroupName"] = json!(&name);
        let named = every_graph_name(&xray, &graph_body, expression).await;
        for service in expected {
            assert!(
                named.contains(*service),
                "case {expression:?}: the graph must carry the selected service {service:?}, got {named:?}"
            );
        }
        for request in seeded_requests() {
            if expected.contains(&request.service) {
                continue;
            }
            assert!(
                !named.contains(request.service),
                "case {expression:?}: {} is not selected, so {:?} must not be in the graph, got {named:?}",
                request.label,
                request.service
            );
        }
    }

    // An expression the compiler cannot evaluate is the ONE filter refusal left,
    // and it names what is at fault rather than claiming the group is missing.
    create_group(&xray, "unevaluated", "rootcause.responsetime.service.name = \"alpha\"").await;
    let mut body = window.clone();
    body["GroupName"] = json!("unevaluated");
    let refused = xray.call("/TimeSeriesServiceStatistics", &body).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "an expression this emulator cannot evaluate must be refused, got {refused:?}"
    );
    assert!(
        refused
            .message()
            .is_some_and(|message| message.contains("FilterExpression")),
        "the refusal must name the group's FilterExpression, got {refused:?}"
    );
}

// =============================================================================
// 3. The account's one indexing rule: another name is a bad request, not a 404
// =============================================================================

#[tokio::test]
async fn only_the_default_indexing_rule_is_updatable_and_another_name_is_a_bad_request() {
    // Rows differ in how the name misses the one updatable rule: case, trailing
    // space, the empty string, a plausible other rule, and a name of another
    // shape entirely.
    let other_names = [
        "default",
        "DEFAULT",
        "Default ",
        " Default",
        "",
        "SamplingRule",
        "Probabilistic",
        "arn:aws:xray:us-east-1:123456789012:indexing-rule/Default",
    ];

    for name in other_names {
        let xray = Xray::new();
        let answer = xray.update_rule(name, 42.0).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {name:?}: a name other than the default rule must be an invalid request, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(ONLY_DEFAULT_RULE_IS_UPDATABLE),
            "case {name:?}: the refusal must state the restriction, got {answer:?}"
        );

        // Nothing was created, and the one rule the account holds is untouched.
        let listed = xray.get_rules(None).await;
        let rules = array(&listed.body, "IndexingRules");
        assert_eq!(
            rules.len(),
            1,
            "case {name:?}: a refused update must not add a rule, got {listed:?}"
        );
        assert_eq!(
            rules[0].get("Name").and_then(Value::as_str),
            Some("Default"),
            "case {name:?}: the account still holds only its default rule, got {listed:?}"
        );
    }

    // The positive control: the default rule IS updatable, and the update reads
    // back through `GetIndexingRules`.
    let xray = Xray::new();
    for percentage in [0.0_f64, 7.5, 100.0] {
        let updated = xray.update_rule("Default", percentage).await;
        assert_eq!(
            updated.status, 200,
            "case {percentage}: updating the default rule must succeed, got {updated:?}"
        );
        let listed = xray.get_rules(None).await;
        let rules = array(&listed.body, "IndexingRules");
        let stored = rules
            .first()
            .and_then(|rule| rule.get("Rule"))
            .and_then(|rule| rule.get("Probabilistic"))
            .and_then(|probabilistic| probabilistic.get("DesiredSamplingPercentage"))
            .and_then(Value::as_f64);
        assert_eq!(
            stored,
            Some(percentage),
            "case {percentage}: the stored rule must report what was written, got {listed:?}"
        );
    }
}

// =============================================================================
// 4. Validation precedes resolution in the long-term retrieval reads
// =============================================================================

/// The operations that take a `RetrievalToken`, and the body each one needs
/// beside it (`TraceFormat` is a member of `ListRetrievedTraces` alone).
fn retrieval_reads(token: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "/ListRetrievedTraces",
            json!({"RetrievalToken": token, "TraceFormat": TRACE_FORMAT_XRAY}),
        ),
        (
            "/GetRetrievedTracesGraph",
            json!({"RetrievalToken": token}),
        ),
        ("/CancelTraceRetrieval", json!({"RetrievalToken": token})),
    ]
}

#[tokio::test]
async fn a_malformed_retrieval_token_is_refused_before_the_store_is_consulted() {
    let xray = Xray::new();
    let base = (now() - 600.0).trunc();
    let trace = trace_id(650);
    let ingested = xray
        .put_segments(&[
            root_document("6500000000000000", &trace, "alpha", base + 1.0, Some(base + 2.0))
                .to_string(),
        ])
        .await;
    assert_eq!(
        ingested.status, 200,
        "the segment must be ingested, got {ingested:?}"
    );
    let started = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [&trace], "StartTime": base, "EndTime": base + 300.0}),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "starting a retrieval must succeed, got {started:?}"
    );
    let real = started
        .get("RetrievalToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        real.len(),
        RETRIEVAL_TOKEN_HEX_DIGITS,
        "a minted RetrievalToken is {RETRIEVAL_TOKEN_HEX_DIGITS} hexadecimal digits, got {real:?}"
    );

    // Rows differ in HOW the token fails to be one this service could have
    // issued: its length either side of the minted width, a character outside
    // hexadecimal, the UUID shape, surrounding space, and the empty string.
    // `RetrievalToken`'s modelled `@length` is 0..1020, so every row reaches the
    // handler rather than the request layer.
    let malformed = [
        ("the empty token", String::new()),
        ("one digit short", "a".repeat(RETRIEVAL_TOKEN_HEX_DIGITS - 1)),
        ("one digit past", "a".repeat(RETRIEVAL_TOKEN_HEX_DIGITS + 1)),
        ("the longest token the model allows", "a".repeat(1_020)),
        ("a character outside hexadecimal", format!("{}z", "a".repeat(31))),
        (
            "the UUID shape",
            "59adda79-1d1f-4b1e-9f3a-2c7d5e8a9b01".to_string(),
        ),
        ("a real token with surrounding space", format!(" {real} ")),
    ];
    // The same length and alphabet the mint uses, so these COULD have been
    // issued — and were not.
    let well_formed = [
        ("all zeroes", "0".repeat(RETRIEVAL_TOKEN_HEX_DIGITS)),
        ("all f", "f".repeat(RETRIEVAL_TOKEN_HEX_DIGITS)),
        ("upper-case hexadecimal", "0123456789ABCDEF".repeat(2)),
    ];

    for (case, token) in &malformed {
        for (path, body) in retrieval_reads(token) {
            let answer = xray.call(path, &body).await;
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {path}: a token that could never have been issued is a bad request, got {answer:?}"
            );
            assert!(
                answer
                    .message()
                    .is_some_and(|message| message.contains(token.as_str())),
                "case {case:?} on {path}: the refusal must name the token it could not read, got {answer:?}"
            );
        }
    }

    for (case, token) in &well_formed {
        for (path, body) in retrieval_reads(token) {
            let answer = xray.call(path, &body).await;
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} on {path}: a well-formed token whose retrieval is not there is a 404, got {answer:?}"
            );
        }
    }

    // And the real retrieval is still readable: nothing a refusal did touched
    // the store.
    for (path, body) in retrieval_reads(&real) {
        let answer = xray.call(path, &body).await;
        assert_eq!(
            answer.status, 200,
            "{path}: the real retrieval must still be served, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn list_retrieved_traces_judges_trace_format_before_it_resolves_the_token() {
    let xray = Xray::new();
    let base = (now() - 600.0).trunc();
    let trace = trace_id(660);
    let ingested = xray
        .put_segments(&[
            root_document("6600000000000000", &trace, "alpha", base + 1.0, Some(base + 2.0))
                .to_string(),
        ])
        .await;
    assert_eq!(
        ingested.status, 200,
        "the segment must be ingested, got {ingested:?}"
    );
    let started = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [&trace], "StartTime": base, "EndTime": base + 300.0}),
        )
        .await;
    let real = started
        .get("RetrievalToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !real.is_empty(),
        "starting a retrieval must answer a token, got {started:?}"
    );

    // Rows differ in what the token would resolve to — a live retrieval, a
    // well-formed token naming none, a token of the wrong form — and all three
    // answer the SAME refusal, because the absent member is judged first.
    let tokens = [
        ("a live retrieval", real.clone()),
        (
            "a well-formed token naming no retrieval",
            "0".repeat(RETRIEVAL_TOKEN_HEX_DIGITS),
        ),
        ("a token of the wrong form", "not-a-token".to_string()),
    ];
    for (case, token) in &tokens {
        let answer = xray
            .call("/ListRetrievedTraces", &json!({"RetrievalToken": token}))
            .await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: an absent TraceFormat must be refused before the token is resolved, got {answer:?}"
        );
        let message = answer.message().unwrap_or_default();
        assert!(
            message.starts_with("TraceFormat cannot be empty"),
            "case {case:?}: the refusal must name the absent member, got {answer:?}"
        );
        for value in ["XRAY", "OTEL"] {
            assert!(
                message.contains(value),
                "case {case:?}: the refusal must offer the modelled value {value:?}, got {answer:?}"
            );
        }
    }

    // With the member supplied, the same tokens are judged on what they name
    // again: the live retrieval is served and the others are refused apart.
    let served = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": &real, "TraceFormat": TRACE_FORMAT_XRAY}),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "the live retrieval must be served once TraceFormat is supplied, got {served:?}"
    );
}

// =============================================================================
// 5. A document nested to the ingest bound is READ, not aborted
// =============================================================================

#[tokio::test]
async fn a_document_nested_to_the_bound_is_read_by_every_derivation() {
    use service_com_amazonaws_xray_awsxray::state::MAX_SUBSEGMENT_DEPTH;

    // The depth of a submitted `subsegments` tree is caller-driven work on every
    // read path that walks it — the root cause a summary names, the entity index
    // a graph resolves a `parent_id` through, the annotations a filter
    // expression reads, the segments inferred for remote calls. The ingest
    // refuses a document nested deeper than MAX_SUBSEGMENT_DEPTH
    // (`trace_data_plane_rules.rs` owns that rule) and every one of those walks
    // bounds itself by that same depth or carries its own worklist, so a
    // document nested right AT the bound must be answered by each of them.
    let xray = Xray::new();
    let trace = trace_id(670);
    let deepest = format!("level-{}", MAX_SUBSEGMENT_DEPTH - 1);

    // Built bottom up: the deepest subsegment records the fault and carries the
    // annotation, and a remote sibling sits beside it at that same deepest
    // level, so one document exercises the root-cause walk, the annotation index
    // and the inferred-segment walk at full depth.
    let mut nested = json!([]);
    for level in (0..MAX_SUBSEGMENT_DEPTH).rev() {
        let mut subsegment = json!({
            "id": format!("{:016x}", 0x6700 + level),
            "name": format!("level-{level}"),
            "start_time": ALIGNED + 1.1_f64,
            "end_time": ALIGNED + 1.4_f64,
        });
        if level == MAX_SUBSEGMENT_DEPTH - 1 {
            subsegment["fault"] = json!(true);
            subsegment["annotations"] = json!({"deep": "yes"});
        }
        if !nested.as_array().is_some_and(Vec::is_empty) {
            subsegment["subsegments"] = nested;
        }
        nested = json!([subsegment]);
        if level == MAX_SUBSEGMENT_DEPTH - 1 {
            nested
                .as_array_mut()
                .expect("the level is an array")
                .push(json!({
                    "id": "67ff000000000000",
                    "name": "downstream",
                    "namespace": "remote",
                    "start_time": ALIGNED + 1.2_f64,
                    "end_time": ALIGNED + 1.3_f64,
                }));
        }
    }
    let mut document = root_document(
        "6700000000000000",
        &trace,
        "entry",
        ALIGNED + 1.0,
        Some(ALIGNED + 2.0),
    );
    document["subsegments"] = nested;

    let put = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(
        put.status, 200,
        "ingesting a document nested to the bound must succeed, got {put:?}"
    );
    assert!(
        array(&put.body, "UnprocessedTraceSegments").is_empty(),
        "nesting AT the bound must be stored, got {put:?}"
    );

    let window = json!({"StartTime": ALIGNED, "EndTime": ALIGNED + 60.0});

    // The root cause walk reaches the deepest entity: the path is the whole
    // chain, segment first, with the faulted subsegment last.
    let summaries = xray.call("/TraceSummaries", &window).await;
    assert_eq!(
        summaries.status, 200,
        "GetTraceSummaries must answer over a document nested to the bound, got {summaries:?}"
    );
    let summary = array(&summaries.body, "TraceSummaries")
        .first()
        .cloned()
        .unwrap_or(Value::Null);
    let entity_path = summary
        .get("FaultRootCauses")
        .and_then(Value::as_array)
        .and_then(|causes| causes.first())
        .and_then(|cause| cause.get("Services"))
        .and_then(Value::as_array)
        .and_then(|services| services.first())
        .and_then(|service| service.get("EntityPath"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names: Vec<&str> = entity_path
        .iter()
        .filter_map(|entity| entity.get("Name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names.last().copied(),
        Some(deepest.as_str()),
        "the root cause path must reach the deepest entity, got {names:?}"
    );

    // The annotation index reaches it too, so a filter expression on the
    // deepest annotation selects the trace.
    let filtered = xray
        .filtered(ALIGNED, ALIGNED + 60.0, "annotation.deep = \"yes\"")
        .await;
    assert_eq!(
        (filtered.status, array(&filtered.body, "TraceSummaries").len()),
        (200, 1),
        "an annotation at the deepest level must be indexed and selectable, got {filtered:?}"
    );

    // And every other derivation over that tree answers: the per-trace graph and
    // the window's service graph (which resolve `parent_id`s through the entity
    // index and synthesise the inferred segment for the remote call at the
    // deepest level), and the whole-trace read.
    for (case, path, body) in [
        ("GetTraceGraph", "/TraceGraph", {
            let mut body = window.clone();
            body["TraceIds"] = json!([&trace]);
            body
        }),
        ("GetServiceGraph", "/ServiceGraph", window.clone()),
        (
            "BatchGetTraces",
            "/Traces",
            json!({"TraceIds": [&trace]}),
        ),
    ] {
        let answer = xray.call(path, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: a document nested to the bound must be read, got {answer:?}"
        );
        if case == "BatchGetTraces" {
            continue;
        }
        // The remote call at the deepest level is reached, so the downstream
        // service it names is a node of the graph.
        let named = graph_names(&answer);
        assert!(
            named.contains("downstream"),
            "case {case}: the remote call at the deepest level must contribute its node, got {named:?}"
        );
    }
}
