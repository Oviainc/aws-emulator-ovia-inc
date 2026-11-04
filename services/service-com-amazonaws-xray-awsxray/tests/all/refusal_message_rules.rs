// SPDX-License-Identifier: CC0-1.0

//! What X-Ray's refusals SAY, end to end over the wire (the `Xray` harness in
//! [`super::xray_wire`]), for the refusal classes whose message is produced by a
//! rule rather than composed per call.
//!
//! Every test is a table whose rows differ along what its rule generalises over
//! — which operation is called, the size of an inverted window, where in an
//! expression the fault is, the shape of a malformed id, how far past a limit a
//! list reaches, the form of a token, the value of a pattern-checked member —
//! because the rules are:
//!
//! * a window whose `EndTime` is before its `StartTime` is refused with one
//!   FIXED text that interpolates neither bound, on every operation that takes a
//!   window, and the operations that state a refused parameter under their own
//!   preamble put that fixed text after it;
//! * an expression the filter-expression language cannot read is refused with
//!   the parser's own template — the line and the one-based column the parse gave
//!   up at, and the input it had read there — on every surface that takes a
//!   `FilterExpression`;
//! * a trace id that is not of the documented `1-<8 hex>-<24 hex>` form is
//!   refused, by every reader that takes one, with a template naming the
//!   offending value and the rule it broke, before any lookup;
//! * a read naming more trace ids than the service accepts per call is refused
//!   with a fixed phrase that states that limit, which the model does not
//!   publish;
//! * a `SamplingRule$ResourceARN` is judged on ARN SYNTAX, so a value that
//!   constrains no resource is accepted and one that is not ARN syntax is
//!   refused with a template naming the value and the syntax rule it broke;
//! * a `RetrievalToken` of a form the service never mints is refused, on every
//!   operation that takes one, with a two-clause template that names the
//!   rejected value in EACH clause — the operation's parameter clause and the
//!   execution-id decoder's own clause — while a well-formed token nothing
//!   stands behind is a different class (`ResourceNotFoundException`, 404); and
//! * a member outside its modelled `@pattern` is refused with AWS's validation
//!   envelope quoting that pattern WITHOUT the anchors wrapping it, so the
//!   expression the refusal reports is the one the client can re-apply.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{
    retrieval_token_check, INVALID_INPUT_PARAMETERS_PREAMBLE, INVERTED_WINDOW_MESSAGE,
    MALFORMED_RETRIEVAL_TOKEN_CAUSE, MAX_TRACE_IDS_PER_CALL,
};

use super::xray_wire::{array, trace_id, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

// =============================================================================
// 1. An inverted [StartTime, EndTime] window is refused with the fixed text
// =============================================================================

/// The operations that take a required window, each with the other members it
/// needs — `smithy_query(query="operation",operation="GetServiceGraph,
/// GetTraceSummaries,GetTimeSeriesServiceStatistics,GetInsightSummaries,
/// StartTraceRetrieval")` — and the preamble, if any, this operation states a
/// refused request parameter under.
fn window_operations() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        ("GetServiceGraph", "/ServiceGraph", json!({}), ""),
        ("GetTraceSummaries", "/TraceSummaries", json!({}), ""),
        (
            "GetTimeSeriesServiceStatistics",
            "/TimeSeriesServiceStatistics",
            json!({}),
            "",
        ),
        // `GetInsightSummaries` resolves a group, so it is asked about the
        // reserved account-level `Default` group every account already holds.
        (
            "GetInsightSummaries",
            "/InsightSummaries",
            json!({"GroupName": "Default"}),
            "",
        ),
        (
            "StartTraceRetrieval",
            "/StartTraceRetrieval",
            json!({"TraceIds": [trace_id(1)]}),
            INVALID_INPUT_PARAMETERS_PREAMBLE,
        ),
    ]
}

#[tokio::test]
async fn an_inverted_window_is_refused_with_the_services_fixed_text() {
    // The fixed text interpolates nothing, so it cannot vary from run to run.
    assert_eq!(
        INVERTED_WINDOW_MESSAGE, "End time cannot be earlier than start time",
        "the fixed text must be the service's own, got {INVERTED_WINDOW_MESSAGE:?}"
    );

    // Rows differ in HOW inverted the window is — by a nanosecond-scale sliver,
    // by a second, by hours — because the refusal is the same whatever the gap:
    // none of it reaches the message.
    let inversions: [(&str, f64); 4] = [
        ("inverted by a sliver", 0.000_001),
        ("inverted by a second", 1.0),
        ("inverted by an hour", 3_600.0),
        ("inverted by two hours", 7_200.0),
    ];

    for (operation, path, extra, preamble) in window_operations() {
        let xray = Xray::new();
        let expected = if preamble.is_empty() {
            INVERTED_WINDOW_MESSAGE.to_string()
        } else {
            format!("{preamble} {INVERTED_WINDOW_MESSAGE}")
        };
        for (label, gap) in inversions {
            let start = now() - 600.0;
            let mut body = extra.clone();
            body["StartTime"] = json!(start);
            body["EndTime"] = json!(start - gap);
            let refused = xray.call(path, &body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "{operation} with a window {label}: expected 400 InvalidRequestException, got \
                 {refused:?}"
            );
            assert_eq!(
                said(&refused),
                expected,
                "{operation} with a window {label}: expected {expected:?}, got {:?}",
                said(&refused)
            );
        }

        // The boundary: a window of zero width runs forwards, so it is served
        // rather than refused — the refusal is about the ORDER of the bounds.
        let start = now() - 600.0;
        let mut body = extra.clone();
        body["StartTime"] = json!(start);
        body["EndTime"] = json!(start);
        let served = xray.call(path, &body).await;
        assert_eq!(
            served.status, 200,
            "{operation} with a zero-width window: a window that runs forwards must be served, \
             got {served:?}"
        );
    }
}

// =============================================================================
// 2. A filter expression that does not parse is refused with the parser's
//    location template
// =============================================================================

/// One expression the language cannot read, and the location template its
/// refusal must carry.
struct FilterCase {
    case: &'static str,
    expression: &'static str,
    line: usize,
    position: usize,
    symbol: &'static str,
}

/// The template a parse failure is stated with.
fn error_location(case: &FilterCase) -> String {
    format!(
        "Error Location: line {}, position {} : Invalid input symbol: '{}'",
        case.line, case.position, case.symbol
    )
}

/// Rows differ along WHERE the parse gives up and on WHAT: at the end of the
/// input (a group nothing closes), on the first character (a word no keyword of
/// the language begins — `metadata.<key>` is the ordinary case, since a
/// segment's metadata is never indexed), inside the input (a string nothing
/// terminates, a character of no token, a bracket nothing opened, an item the
/// grammar cannot take there with more input after it), and on a later LINE, so
/// the line and the column are both derived rather than assumed.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "a group nothing closes, so the parse reaches the end of the input",
            expression: "((service(\"x\")",
            line: 1,
            position: 15,
            symbol: "((service(\"x\")",
        },
        FilterCase {
            case: "an operator the grammar cannot take, with input after it",
            expression: "annotation.foo ==> \"bar\"",
            line: 1,
            position: 17,
            symbol: "=",
        },
        FilterCase {
            case: "a second keyword where the grammar expects none",
            expression: "ok fault AND ok",
            line: 1,
            position: 4,
            symbol: "fault",
        },
        FilterCase {
            case: "a word no keyword of the language begins",
            expression: "metadata.probe_key = \"v\"",
            line: 1,
            position: 1,
            symbol: "met",
        },
        FilterCase {
            case: "a string nothing terminates",
            expression: "service(\"unclosed",
            line: 1,
            position: 9,
            symbol: "\"unclosed",
        },
        FilterCase {
            case: "a character no token can begin with",
            expression: "fault = true & ok",
            line: 1,
            position: 14,
            symbol: "&",
        },
        FilterCase {
            case: "a bracket nothing opened",
            expression: "fault = true)",
            line: 1,
            position: 13,
            symbol: ")",
        },
        FilterCase {
            // A word the language does not admit is reported by its first three
            // characters, whether or not it begins one of the keywords: `wha`
            // here, `met` for the `metadata.<key>` rows.
            case: "a word that shares no prefix with any keyword",
            expression: "whatever = 1",
            line: 1,
            position: 1,
            symbol: "wha",
        },
        FilterCase {
            // Shorter than that, so the whole word is what there is to report.
            case: "a two-character word that shares no prefix with any keyword",
            expression: "zz = 1",
            line: 1,
            position: 1,
            symbol: "zz",
        },
        FilterCase {
            // A word that BEGINS a keyword and then leaves it is reported up to
            // the character that left it: `si` begins `service`, `sim` does not.
            case: "a word that begins a keyword and then leaves it",
            expression: "fault AND simulated = 1",
            line: 1,
            position: 11,
            symbol: "si",
        },
        FilterCase {
            case: "a fault on the second line",
            expression: "ok AND\nmetadata.k = 1",
            line: 2,
            position: 1,
            symbol: "met",
        },
        FilterCase {
            case: "a string nothing terminates on the second line",
            expression: "service(\"a\") AND\nuser = \"oops",
            line: 2,
            position: 8,
            symbol: "\"oops",
        },
        FilterCase {
            case: "a fault after leading whitespace, which is part of the submitted text",
            expression: "  metadata.k",
            line: 1,
            position: 3,
            symbol: "met",
        },
    ]
}

#[tokio::test]
async fn an_unreadable_filter_expression_is_refused_with_its_location_in_the_expression() {
    for case in filter_cases() {
        let xray = Xray::new();
        let expected = error_location(&case);

        // CreateGroup: the surface that only STORES an expression.
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": "probe", "FilterExpression": case.expression}),
            )
            .await;
        assert_eq!(
            (created.status, created.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {:?} on CreateGroup: expected 400 InvalidRequestException, got {created:?}",
            case.case
        );
        assert_eq!(
            said(&created),
            expected,
            "case {:?} on CreateGroup: expected {expected:?}, got {:?}",
            case.case,
            said(&created)
        );

        // GetTraceSummaries: the surface that EVALUATES one.
        let start = now() - 600.0;
        let queried = xray
            .call(
                "/TraceSummaries",
                &json!({
                    "StartTime": start,
                    "EndTime": start + 300.0,
                    "FilterExpression": case.expression,
                }),
            )
            .await;
        assert_eq!(
            (queried.status, queried.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {:?} on GetTraceSummaries: expected 400 InvalidRequestException, got {queried:?}",
            case.case
        );
        assert_eq!(
            said(&queried),
            expected,
            "case {:?} on GetTraceSummaries: expected {expected:?}, got {:?}",
            case.case,
            said(&queried)
        );

        // UpdateGroup: the same refusal, and the stored expression is left
        // exactly as it was — read back through the implemented reader.
        let kept = "fault = true";
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": "stored", "FilterExpression": kept}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the group to update must be created, got {created:?}",
            case.case
        );
        let updated = xray
            .call(
                "/UpdateGroup",
                &json!({"GroupName": "stored", "FilterExpression": case.expression}),
            )
            .await;
        assert_eq!(
            (updated.status, updated.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {:?} on UpdateGroup: expected 400 InvalidRequestException, got {updated:?}",
            case.case
        );
        assert_eq!(
            said(&updated),
            expected,
            "case {:?} on UpdateGroup: expected {expected:?}, got {:?}",
            case.case,
            said(&updated)
        );
        let read = xray.call("/GetGroup", &json!({"GroupName": "stored"})).await;
        assert_eq!(
            read.get("Group").get("FilterExpression").and_then(Value::as_str),
            Some(kept),
            "case {:?}: a refused UpdateGroup must leave the stored expression alone, got {read:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_expression_the_language_reads_is_not_refused() {
    // The counter-rows: each of these addresses a keyword root the catalog
    // admits (AWS's X-Ray "Using filter expressions" keyword reference), so the
    // location template above never answers for them. `annotation.<key>` and
    // `rootcause.*` are entries the recorded cases did not cite.
    let accepted = [
        ("a bare boolean keyword", "ok"),
        ("a caller-chosen annotation key", "annotation.probe_key = \"v\""),
        ("a root-cause path", "rootcause.fault.service { name = \"svc\" }"),
        ("a complex keyword with a block", "service(\"svc\") { fault }"),
        ("a labelled argument", "id(name: \"svc\", type: \"AWS::EC2::Instance\")"),
        ("a composition over two lines", "ok AND\nhttp.status >= 400"),
    ];
    let xray = Xray::new();
    for (index, (label, expression)) in accepted.iter().enumerate() {
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": format!("ok-{index:02}"), "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: {expression:?} must be accepted, got {created:?}"
        );
    }
}

// =============================================================================
// 3. A malformed trace id is refused with the service's template
// =============================================================================

/// Rows differ in HOW the id departs from `1-<8 hex>-<24 hex>`: not an id at
/// all, each part of the wrong width, a non-hexadecimal digit, another version
/// number, a missing part, and separators of the wrong kind.
fn malformed_trace_ids() -> Vec<(&'static str, String)> {
    vec![
        ("a string that is no id at all", "not-a-trace-id".to_string()),
        (
            "a well-formed prefix with too few trailing digits",
            "1-6add71cc-3c58f4ad1a56877a3a77".to_string(),
        ),
        (
            "too many trailing digits",
            "1-6add71cc-3c58f4ad1a56877a3a7700".to_string(),
        ),
        (
            "a non-hexadecimal digit in the unique part",
            "1-6add71cc-3c58f4ad1a56877a3a77zzzz".to_string(),
        ),
        (
            "an epoch part of the wrong width",
            "1-6add71c-3c58f4ad1a56877a3a7700".to_string(),
        ),
        (
            "a version other than 1",
            "2-6add71cc-3c58f4ad1a56877a3a7700".to_string(),
        ),
        ("only the version", "1".to_string()),
        (
            "parts joined by something other than a dash",
            "1_6add71cc_3c58f4ad1a56877a3a7700".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_malformed_trace_id_is_refused_with_the_services_template() {
    let xray = Xray::new();
    let stored = trace_id(42);

    for (label, malformed) in malformed_trace_ids() {
        let expected = format!("Invalid trace id {malformed}: incorrect id format");
        // The refusal names the OFFENDING id, so a list whose malformed id sits
        // beside a well-formed one reports the malformed one.
        for (where_in_list, ids) in [
            ("alone", vec![malformed.clone()]),
            (
                "after a well-formed id",
                vec![stored.clone(), malformed.clone()],
            ),
        ] {
            for (operation, path) in [("BatchGetTraces", "/Traces"), ("GetTraceGraph", "/TraceGraph")]
            {
                let refused = xray.call(path, &json!({"TraceIds": ids})).await;
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {label:?} {where_in_list} on {operation}: expected 400 \
                     InvalidRequestException, got {refused:?}"
                );
                assert_eq!(
                    said(&refused),
                    expected,
                    "case {label:?} {where_in_list} on {operation}: expected {expected:?}, got \
                     {:?}",
                    said(&refused)
                );
            }
        }
    }

    // The boundary: an id OF the form is not refused for its form — it is
    // simply a trace that is not there.
    let answer = xray.call("/Traces", &json!({"TraceIds": [stored]})).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed id must not be refused for its form, got {answer:?}"
    );
}

// =============================================================================
// 4. More trace ids than one call accepts is refused with the limit's own
//    phrase
// =============================================================================

#[tokio::test]
async fn a_read_past_the_trace_id_limit_is_refused_with_the_phrase_that_states_it() {
    let xray = Xray::new();
    let expected = format!("Exceeding maximum query size: {MAX_TRACE_IDS_PER_CALL}");

    // Ids the store actually holds, so the row AT the limit is answered rather
    // than refused for a reason of its own.
    let ids: Vec<String> = (0..12).map(|index| trace_id(300 + index)).collect();
    let documents: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(index, trace)| {
            super::xray_wire::root_document(
                &format!("ab0000000000{index:04}"),
                trace,
                "svc",
                now() - 60.0,
                Some(now() - 59.0),
            )
            .to_string()
        })
        .collect();
    let put = xray.put_segments(&documents).await;
    assert_eq!(put.status, 200, "the seed must be ingested, got {put:?}");

    // Rows differ in how far past the limit the list reaches, plus the two
    // boundary rows at and below it.
    for count in [1, MAX_TRACE_IDS_PER_CALL - 1, MAX_TRACE_IDS_PER_CALL] {
        for (operation, path) in [("BatchGetTraces", "/Traces"), ("GetTraceGraph", "/TraceGraph")] {
            let answer = xray
                .call(path, &json!({"TraceIds": ids[..count]}))
                .await;
            assert_eq!(
                answer.status, 200,
                "{operation} with {count} id(s): a list at or inside the limit must be served, \
                 got {answer:?}"
            );
        }
    }
    for count in [
        MAX_TRACE_IDS_PER_CALL + 1,
        MAX_TRACE_IDS_PER_CALL + 2,
        ids.len(),
    ] {
        for (operation, path) in [("BatchGetTraces", "/Traces"), ("GetTraceGraph", "/TraceGraph")] {
            let refused = xray
                .call(path, &json!({"TraceIds": ids[..count]}))
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "{operation} with {count} ids: expected 400 InvalidRequestException, got \
                 {refused:?}"
            );
            assert_eq!(
                said(&refused),
                expected,
                "{operation} with {count} ids: expected {expected:?}, got {:?}",
                said(&refused)
            );
        }
    }
}

// =============================================================================
// 5. A ResourceARN is judged on ARN syntax
// =============================================================================

/// A `SamplingRule` body carrying `resource_arn`, with every other member a
/// value the model accepts.
fn rule_with_resource_arn(name: &str, resource_arn: &str) -> Value {
    json!({
        "SamplingRule": {
            "RuleName": name,
            "ResourceARN": resource_arn,
            "Priority": 100,
            "FixedRate": 0.1,
            "ReservoirSize": 5,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            "Version": 1,
        }
    })
}

#[tokio::test]
async fn a_resource_arn_is_refused_only_for_its_arn_syntax() {
    let xray = Xray::new();

    // Accepted: the wildcard, a value that constrains no resource at all, and
    // concrete ARNs of several services — AWS judges the SYNTAX, not whether
    // the value is a wildcard.
    let accepted = [
        ("the wildcard", "*"),
        ("a value constraining no resource", ""),
        (
            "a concrete function ARN",
            "arn:aws:lambda:us-east-1:123456789012:function:checkout",
        ),
        (
            "a concrete instance ARN",
            "arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0",
        ),
        ("an ARN in another partition", "arn:aws-cn:ec2:cn-north-1:123456789012:instance/*"),
    ];
    for (index, (label, resource_arn)) in accepted.iter().enumerate() {
        let body = rule_with_resource_arn(&format!("accepted-{index:02}"), resource_arn);
        let created = xray.call("/CreateSamplingRule", &body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: {resource_arn:?} must be accepted, got {created:?}"
        );
        // Round-tripped through the implemented reader: the stored rule, not
        // the request, is what answers.
        let listed = xray.call("/GetSamplingRules", &json!({})).await;
        let found = array(&listed.body, "SamplingRuleRecords").iter().any(|record| {
            record
                .get("SamplingRule")
                .and_then(|rule| rule.get("ResourceARN"))
                .and_then(Value::as_str)
                == Some(*resource_arn)
                && record
                    .get("SamplingRule")
                    .and_then(|rule| rule.get("RuleName"))
                    .and_then(Value::as_str)
                    == Some(format!("accepted-{index:02}").as_str())
        });
        assert!(
            found,
            "case {label:?}: the stored rule must read back carrying {resource_arn:?}, got \
             {listed:?}"
        );
    }

    // Refused: values that are not ARN syntax. Rows differ in how they miss the
    // prefix — a bare name, a name with separators, a dropped scheme, another
    // case — and the message names the value and the rule it broke.
    let refused = [
        ("a bare word", "not-an-arn"),
        ("a resource name", "checkout-function"),
        ("an ARN with its scheme dropped", "aws:lambda:us-east-1:123456789012:function:checkout"),
        ("an upper-case scheme", "ARN:aws:lambda:us-east-1:123456789012:function:checkout"),
        ("a path", "/aws/lambda/checkout"),
    ];
    for (index, (label, resource_arn)) in refused.iter().enumerate() {
        let expected =
            format!("Invalid resource ARN: {resource_arn}. Malformed ARN - doesn't start with 'arn:'");
        let name = format!("refused-{index:02}");
        let answer = xray
            .call("/CreateSamplingRule", &rule_with_resource_arn(&name, resource_arn))
            .await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: expected 400 InvalidRequestException, got {answer:?}"
        );
        assert_eq!(
            said(&answer),
            expected,
            "case {label:?}: expected {expected:?}, got {:?}",
            said(&answer)
        );

        // The same judgement, in the same words, where an UPDATE would carry a
        // rule to that value.
        let created = xray
            .call(
                "/CreateSamplingRule",
                &rule_with_resource_arn(&format!("update-{index:02}"), "*"),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the rule to update must be created, got {created:?}"
        );
        let updated = xray
            .call(
                "/UpdateSamplingRule",
                &json!({"SamplingRuleUpdate": {
                    "RuleName": format!("update-{index:02}"),
                    "ResourceARN": resource_arn,
                }}),
            )
            .await;
        assert_eq!(
            (updated.status, updated.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?} on UpdateSamplingRule: expected 400 InvalidRequestException, got \
             {updated:?}"
        );
        assert_eq!(
            said(&updated),
            expected,
            "case {label:?} on UpdateSamplingRule: expected {expected:?}, got {:?}",
            said(&updated)
        );
    }
}

// =============================================================================
// 6. A retrieval token of a form the service never mints is refused by name
// =============================================================================

#[tokio::test]
async fn a_token_of_no_minted_form_is_refused_with_the_services_template() {
    let xray = Xray::new();

    // Rows differ in how the token misses the minted FORM: prose, one digit
    // short, one digit long, a non-hexadecimal digit, and a value of another
    // service's token shape. A value that is of the minted form but whose check
    // half does not verify was decoded and rejected rather than refused at the
    // decoder's entry, so it carries the other cause clause and is asserted in
    // `super::malformed_retrieval_token_message`.
    let tokens = [
        ("prose", "sim2real-behav-not-a-real-token".to_string()),
        ("one digit short", "a".repeat(31)),
        ("one digit long", "a".repeat(33)),
        ("a non-hexadecimal digit", format!("{}g", "a".repeat(31))),
        ("a dashed identifier", "11111111-2222-3333-4444-555555".to_string()),
    ];
    for (label, token) in tokens {
        // Every operation that takes a `RetrievalToken`
        // (`smithy_query(query="operation",operation="CancelTraceRetrieval,
        // ListRetrievedTraces,GetRetrievedTracesGraph")`), with the leading
        // clause it names the member by. The cause clause after `passed: ` is
        // the same fixed phrase on all of them, whatever the operation named the
        // member and whatever the value was.
        for (operation, path, clause, body) in [
            (
                "ListRetrievedTraces",
                "/ListRetrievedTraces",
                "execution id",
                json!({"RetrievalToken": token, "TraceFormat": "XRAY"}),
            ),
            (
                "GetRetrievedTracesGraph",
                "/GetRetrievedTracesGraph",
                "execution id",
                json!({"RetrievalToken": token}),
            ),
            (
                "CancelTraceRetrieval",
                "/CancelTraceRetrieval",
                "retrieval token",
                json!({"RetrievalToken": token}),
            ),
        ] {
            let expected = format!(
                "Invalid {clause} {token} passed: {MALFORMED_RETRIEVAL_TOKEN_CAUSE}"
            );
            let refused = xray.call(path, &body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {label:?} on {operation}: expected 400 InvalidRequestException, got \
                 {refused:?}"
            );
            assert_eq!(
                said(&refused),
                expected,
                "case {label:?} on {operation}: expected {expected:?}, got {:?}",
                said(&refused)
            );
            // The outer clause names the rejected value, once: the cause clause
            // states why it could not be read and names no value.
            assert_eq!(
                said(&refused).matches(token.as_str()).count(),
                1,
                "case {label:?} on {operation}: the rejected value must be named once, \
                 got {:?}",
                said(&refused)
            );
        }
    }

    // The boundary: a token the service COULD have issued — its check half
    // derived from its identifying half, as the mint derives it — that nothing
    // stands behind is a different class: the resource is not there, not the
    // request wrong.
    let identity = "0123456789abcdef";
    let well_formed = format!("{identity}{}", retrieval_token_check(identity));
    let missing = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": well_formed, "TraceFormat": "XRAY"}),
        )
        .await;
    assert_eq!(
        (missing.status, missing.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "an intact token nothing stands behind must be 404 ResourceNotFoundException, got \
         {missing:?}"
    );
    assert!(
        !said(&missing).contains(MALFORMED_RETRIEVAL_TOKEN_CAUSE),
        "a token that is not there must not be stated as a malformed one, got {:?}",
        said(&missing)
    );
}

// =============================================================================
// 7. A pattern requirement is quoted as AWS quotes it: without the anchors
// =============================================================================

/// `InsightId`'s modelled pattern as the refusal must QUOTE it: the model's
/// `@pattern` (`smithy_query(query="shape",shape="InsightId")`) with the `^` and
/// `$` that wrap it dropped, which is the expression a client can re-apply.
const INSIGHT_ID_PATTERN: &str = "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-\
                                  [89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}";

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_anchors_that_wrap_it() {
    let xray = Xray::new();
    let start = now() - 600.0;

    // Rows differ in how the value misses the pattern: a variant nibble outside
    // the set, a version digit outside the range, no dashes at all, too few
    // groups, and prose.
    let values = [
        ("a variant nibble outside the set", "11111111-2222-3333-4444-555555555555"),
        ("a version digit outside the range", "11111111-2222-6333-8444-555555555555"),
        ("no separators", "11111111222233338444555555555555"),
        ("too few groups", "11111111-2222-3333-8444"),
        ("prose", "not-a-guid"),
    ];
    // Every operation that takes an `InsightId`, so the rendering is the request
    // layer's and not one handler's.
    let operations = [
        ("GetInsight", "/Insight", json!({})),
        ("GetInsightEvents", "/InsightEvents", json!({})),
        (
            "GetInsightImpactGraph",
            "/InsightImpactGraph",
            json!({"StartTime": start, "EndTime": start + 300.0}),
        ),
    ];

    for (label, value) in values {
        for (operation, path, extra) in &operations {
            let mut body = extra.clone();
            body["InsightId"] = json!(value);
            let refused = xray.call(path, &body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {label:?} on {operation}: expected 400 ValidationException, got {refused:?}"
            );
            let expected = format!(
                "1 validation error detected: Value '{value}' at 'insightId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {INSIGHT_ID_PATTERN}"
            );
            assert_eq!(
                said(&refused),
                expected,
                "case {label:?} on {operation}: expected {expected:?}, got {:?}",
                said(&refused)
            );
            let quoted = said(&refused)
                .rsplit("regular expression pattern: ")
                .next()
                .unwrap_or_default();
            assert!(
                !quoted.starts_with('^') && !quoted.ends_with('$'),
                "case {label:?} on {operation}: the quoted expression must carry no anchors, got \
                 {quoted:?}"
            );
        }
    }

    // The boundary: a value the pattern admits is not refused by the request
    // layer at all — it reaches the handler, which answers for the insight.
    let admitted = "11111111-2222-3333-8444-555555555555";
    let answer = xray
        .call("/Insight", &json!({"InsightId": admitted}))
        .await;
    assert!(
        answer.error_type.as_deref() != Some("ValidationException"),
        "a value the pattern admits must not be refused by the request layer, got {answer:?}"
    );
}
