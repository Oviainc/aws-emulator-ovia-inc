// SPDX-License-Identifier: CC0-1.0

//! The FILTER stage of a pipe, end to end over the wire: which polled records a pipe's
//! `FilterCriteria` admit and what the target receives for them.
//!
//! Pipes and SQS are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! what a filter decided is asserted where a client sees it: on the target queue, and on
//! the source the pipe drained.
//!
//! The rule asserted over a TABLE: a source record's message body travels as the text its
//! producer sent, so the filter reads it in the FORMAT that text is in — a body spelling a
//! JSON document is matched as the document it parses to, one spelling none as the text it
//! carries — and a pattern's `body` clause, written either as a nested document or as a
//! list of candidate values, admits only a record whose body is in its own format,
//! whatever the values would compare to. A pattern naming no `body` clause constrains
//! neither format. Within one pattern the clauses are ANDed and the entries of
//! `FilterCriteria.Filters` are ORed, and what the target RECEIVES is the record envelope
//! with the body exactly as sent.
//!
//! The rows differ along what the rule generalises over: the format of the body, the form
//! of the `body` clause, the document a JSON body spells (object or list) against the
//! scalars that spell none, the matching operators the clause uses, how many patterns are
//! ORed, whether the pattern reads the body at all, and the batch size the records are
//! polled under.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, emulator, given_queue, sqs, wait_for_visible, LOOK_AGAIN,
    QUIET,
};
use crate::pipe_lifecycle::{call, describe, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-filter";

/// How long a wait for work the pipe has to do is given before it is a failure. Well
/// above the data plane's poll interval and the span a pipe takes to reach RUNNING, so a
/// row fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(20);

/// The message attribute a row's sends carry, for the pattern that reads the record's
/// METADATA instead of its body. The envelope reports a message attribute's value under
/// `stringValue`, beside the `dataType` it was sent with.
const TIER_ATTRIBUTE: &str = "tier";

/// The `eventSource` every Amazon SQS record envelope reports, which is what says the
/// delivered payload is the record and not something the test wrote.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// One message a row sends to its source queue.
struct Sent {
    /// The `MessageBody` as the producer sends it.
    body: &'static str,
    /// The `tier` message attribute the send carries, when the row gives it one.
    tier: Option<&'static str>,
    /// Whether the pipe's filter must admit it, so it reaches the target.
    admitted: bool,
}

/// One row of the filter table.
struct Row {
    /// The case, which is also the pipe's and the queues' name.
    case: &'static str,
    /// The patterns `FilterCriteria.Filters` carries, in order.
    patterns: &'static [&'static str],
    /// `BatchSize`, when the row configures one.
    batch_size: Option<i64>,
    /// What the row sends to the source, in send order.
    messages: &'static [Sent],
}

/// RULE: a pipe's filter reads a record's message body in the format the producer sent it
/// in — a body spelling a JSON document as that document, one spelling none as its text —
/// and a `body` clause written for the other format admits the record no more than one
/// whose values disagreed; a pattern with no `body` clause reads neither format; clauses
/// within one pattern are ANDed and the entries of `Filters` are ORed; and the record the
/// target receives carries the body as sent.
#[tokio::test]
async fn a_pipe_filter_reads_a_record_body_in_the_format_it_was_sent_in() {
    let rows = [
        Row {
            case: "a-document-clause-reads-a-json-body",
            patterns: &[r#"{"body": {"kind": ["alpha"]}}"#],
            batch_size: Some(1),
            messages: &[
                Sent {
                    body: r#"{"kind": "alpha", "marker": "one"}"#,
                    tier: None,
                    admitted: true,
                },
                // Valid JSON, read as the document it spells: its `kind` is another one.
                Sent {
                    body: r#"{"kind": "beta", "marker": "two"}"#,
                    tier: None,
                    admitted: false,
                },
                // Not JSON, so the body is text and a document clause reads nothing in
                // it, however much of the clause the text spells.
                Sent {
                    body: "kind=alpha but not json",
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "a-json-array-body-is-the-document-it-parses-to",
            patterns: &[r#"{"body": {"kind": ["alpha"]}}"#],
            batch_size: Some(10),
            messages: &[
                // A body spelling a LIST of documents is one too: a clause matches when
                // any element of the list does.
                Sent {
                    body: r#"[{"kind": "alpha", "marker": "in-a-list"}]"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"[{"kind": "beta"}, {"kind": "gamma"}]"#,
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "the-clauses-of-one-pattern-are-anded",
            patterns: &[
                r#"{"body": {"severity": [{"numeric": [">=", 500]}], "stage": [{"prefix": "prod-"}], "team": [{"anything-but": ["ignore-me"]}], "suppress": [{"exists": false}]}}"#,
            ],
            batch_size: Some(10),
            messages: &[
                // Every clause satisfied, the numeric one exactly at its bound.
                Sent {
                    body: r#"{"severity": 500, "stage": "prod-west", "team": "payments"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"severity": 1000.5, "stage": "prod-", "team": "others"}"#,
                    tier: None,
                    admitted: true,
                },
                // One clause short, in turn: under the bound, the wrong prefix, an
                // excluded value, and the field the pattern requires to be absent.
                Sent {
                    body: r#"{"severity": 499, "stage": "prod-west", "team": "payments"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: r#"{"severity": 900, "stage": "staging-west", "team": "payments"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: r#"{"severity": 900, "stage": "prod-east", "team": "ignore-me"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: r#"{"severity": 900, "stage": "prod-east", "team": "payments", "suppress": true}"#,
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "the-entries-of-filters-are-ored",
            patterns: &[
                r#"{"body": {"kind": ["alpha"]}}"#,
                r#"{"body": {"kind": ["gamma"]}}"#,
            ],
            batch_size: Some(1),
            messages: &[
                Sent {
                    body: r#"{"kind": "alpha"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "gamma"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "delta"}"#,
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "a-values-clause-reads-the-text-a-body-carries",
            patterns: &[r#"{"body": ["PLAINMATCH-plain", "{\"k\": \"json\"}"]}"#],
            batch_size: Some(1),
            messages: &[
                Sent {
                    body: "PLAINMATCH-plain",
                    tier: None,
                    admitted: true,
                },
                // Byte-identical to the second literal the clause lists, and still
                // dropped: the body is valid JSON and the clause reads text.
                Sent {
                    body: r#"{"k": "json"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "PLAINMATCH-other",
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "a-values-clause-drops-a-json-body-whatever-it-spells",
            patterns: &[r#"{"body": [{"exists": true}]}"#],
            batch_size: Some(10),
            messages: &[
                // The clause asks only that the body be there, which the text one is.
                Sent {
                    body: "a plain text body",
                    tier: None,
                    admitted: true,
                },
                // A document — an object or a list — is read as one, and a clause reading
                // text reads nothing in it however little it asks.
                Sent {
                    body: r#"{"k": "v"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "[1, 2, 3]",
                    tier: None,
                    admitted: false,
                },
                // The other side of that boundary: a body spelling a bare scalar names
                // nothing to descend into, so it is the text it is.
                Sent {
                    body: "17",
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: "\"a quoted json string\"",
                    tier: None,
                    admitted: true,
                },
            ],
        },
        Row {
            case: "operators-under-a-document-clause",
            patterns: &[
                r#"{"body": {"kind": [{"prefix": "ord"}], "count": [{"numeric": ["<", 10]}]}}"#,
            ],
            batch_size: Some(2),
            messages: &[
                Sent {
                    body: r#"{"kind": "order", "count": 3}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "refund", "count": 3}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: r#"{"kind": "order", "count": 11}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "ord but text",
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "a-pattern-with-no-body-clause-reads-both-formats",
            patterns: &[r#"{"messageAttributes": {"tier": {"stringValue": ["gold"]}}}"#],
            batch_size: Some(10),
            messages: &[
                Sent {
                    body: "PLAIN-not-json",
                    tier: Some("gold"),
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "order"}"#,
                    tier: Some("gold"),
                    admitted: true,
                },
                // The metadata the pattern reads says otherwise, whatever the body is.
                Sent {
                    body: r#"{"kind": "order", "tier": "gold"}"#,
                    tier: Some("silver"),
                    admitted: false,
                },
                Sent {
                    body: "PLAIN-no-attribute",
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Row {
            case: "each-branch-of-or-carries-its-own-body-format",
            patterns: &[
                r#"{"$or": [{"body": {"kind": ["alpha"]}}, {"body": ["PLAINMATCH-branch"]}]}"#,
            ],
            batch_size: Some(1),
            messages: &[
                Sent {
                    body: r#"{"kind": "alpha"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: "PLAINMATCH-branch",
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "beta"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "PLAINMATCH-other",
                    tier: None,
                    admitted: false,
                },
            ],
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// Keep the table honest: every row is driven by this one body.
async fn run_row(row: &Row) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;

    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": source_parameters(row.batch_size, row.patterns),
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    for message in row.messages {
        send(&router, &source_url, case, message).await;
    }

    let wanted: Vec<String> = row
        .messages
        .iter()
        .filter(|message| message.admitted)
        .map(|message| message.body.to_string())
        .collect();

    // Wait for everything the filter must admit, then keep watching: a record the filter
    // must DROP would arrive late, so the quiet window is what makes its absence an
    // answer rather than a race.
    let mut delivered = collect_at_least(&router, &target_url, wanted.len(), BUDGET).await;
    delivered.extend(collect_during(&router, &target_url, QUIET).await);

    let bodies: Vec<String> = delivered
        .iter()
        .map(|payload| body_of(case, payload))
        .collect();
    let seen: BTreeSet<String> = bodies.iter().cloned().collect();
    let expected: BTreeSet<String> = wanted.iter().cloned().collect();
    assert_eq!(
        seen, expected,
        "case {case:?}: expected the filter to admit exactly {expected:?}, got {bodies:?}"
    );
    assert_eq!(
        bodies.len(),
        wanted.len(),
        "case {case:?}: expected {} deliveries, got {bodies:?}",
        wanted.len()
    );

    // A dropped record is still CONSUMED: it entered the pipe and the pipe decided about
    // it, so nothing the filter rejected is left waiting on the source.
    let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
    assert_eq!(
        left, 0,
        "case {case:?}: expected every polled record consumed, {left} are still visible \
         on the source"
    );
}

/// The `SourceParameters` a pipe is created or updated with: the batch size it polls
/// under, when one is configured, and the patterns `FilterCriteria.Filters` carries.
fn source_parameters(batch_size: Option<i64>, patterns: &[&str]) -> Value {
    let mut queue_parameters = serde_json::Map::new();
    if let Some(size) = batch_size {
        queue_parameters.insert("BatchSize".to_string(), json!(size));
    }
    let filters: Vec<Value> = patterns
        .iter()
        .map(|pattern| json!({ "Pattern": pattern }))
        .collect();
    json!({
        "SqsQueueParameters": Value::Object(queue_parameters),
        "FilterCriteria": { "Filters": filters },
    })
}

/// Send one of a row's messages to the queue at `url`.
async fn send(router: &MultiServiceRouter, url: &str, case: &str, message: &Sent) {
    let mut request = json!({ "QueueUrl": url, "MessageBody": message.body });
    if let Some(tier) = message.tier {
        request["MessageAttributes"] = json!({
            TIER_ATTRIBUTE: { "DataType": "String", "StringValue": tier },
        });
    }
    let (status, answer) = sqs(router, "SendMessage", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage of {:?} answered {status} {answer}",
        message.body
    );
}

/// The message body one delivery carries, read out of the record envelope the target
/// received.
///
/// The envelope is what a pipe hands its target, and its `body` is the message AS SENT —
/// the filter reading a JSON body as a document is the FILTER's reading and not the
/// payload's, so this both reads the delivery and asserts that.
fn body_of(case: &str, payload: &str) -> String {
    let envelope: Value = serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("case {case:?}: a delivered body is not JSON ({error}): {payload:?}")
    });
    assert_eq!(
        envelope["eventSource"].as_str(),
        Some(SQS_EVENT_SOURCE),
        "case {case:?}: expected the target to receive the Amazon SQS record envelope, \
         got {envelope}"
    );
    envelope["body"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the delivered record to carry the message body as \
                 sent, got {envelope}"
            )
        })
        .to_string()
}

/// The criteria in force for one leg of the update test, and what they must decide.
struct Leg {
    case: &'static str,
    /// The patterns `FilterCriteria.Filters` carries while this leg runs.
    patterns: &'static [&'static str],
    /// What the leg sends while they are in force. Each leg's bodies are its own, so a
    /// delivery is attributed to the leg that sent it.
    messages: &'static [Sent],
}

/// The criteria a pipe is created with, and the ones an update puts in their place: one
/// reading the body as a document, one that also lists a plain-text candidate.
const CREATED_CRITERIA: &[&str] = &[r#"{"body": {"kind": ["alpha", "beta"]}}"#];
const UPDATED_CRITERIA: &[&str] = &[
    r#"{"body": {"kind": ["gamma"]}}"#,
    r#"{"body": ["PLAIN-v2-admitted"]}"#,
];

/// RULE: the filter a record is decided by is the one the pipe's record carries WHEN it is
/// polled, so criteria an `UpdatePipe` installs on a running pipe decide every record
/// polled after it and criteria it replaces decide none — whichever format each pattern's
/// `body` clause is written for.
#[tokio::test]
async fn the_criteria_in_force_are_the_ones_the_pipe_carries_when_a_record_is_polled() {
    let name = "filter-criteria-updated";
    let legs = [
        Leg {
            case: "the created criteria",
            patterns: CREATED_CRITERIA,
            messages: &[
                Sent {
                    body: r#"{"kind": "alpha", "leg": "v1"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "gamma", "leg": "v1"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "PLAIN-v1-admitted",
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the updated criteria",
            patterns: UPDATED_CRITERIA,
            messages: &[
                Sent {
                    body: r#"{"kind": "gamma", "leg": "v2"}"#,
                    tier: None,
                    admitted: true,
                },
                // The second updated pattern reads the body as text, and this body is
                // text, so it is the one record that pattern admits.
                Sent {
                    body: "PLAIN-v2-admitted",
                    tier: None,
                    admitted: true,
                },
                // What the replaced criteria admitted, which these do not.
                Sent {
                    body: r#"{"kind": "alpha", "leg": "v2"}"#,
                    tier: None,
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the criteria reverted",
            patterns: CREATED_CRITERIA,
            messages: &[
                Sent {
                    body: r#"{"kind": "beta", "leg": "v3"}"#,
                    tier: None,
                    admitted: true,
                },
                Sent {
                    body: r#"{"kind": "gamma", "leg": "v3"}"#,
                    tier: None,
                    admitted: false,
                },
                Sent {
                    body: "PLAIN-v2-admitted-no-more",
                    tier: None,
                    admitted: false,
                },
            ],
        },
    ];

    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{name}-tgt")).await;

    for (index, leg) in legs.iter().enumerate() {
        let case = leg.case;
        let answer = if index == 0 {
            call(
                &router,
                "POST",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "Source": source_arn,
                    "Target": target_arn,
                    "RoleArn": ROLE,
                    "DesiredState": "RUNNING",
                    "SourceParameters": source_parameters(Some(1), leg.patterns),
                })),
                REGION,
            )
            .await
        } else {
            call(
                &router,
                "PUT",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "RoleArn": ROLE,
                    "DesiredState": "RUNNING",
                    "SourceParameters": source_parameters(Some(1), leg.patterns),
                })),
                REGION,
            )
            .await
        };
        assert_eq!(
            answer.status, 200,
            "case {case:?}: installing the criteria answered {} {}",
            answer.status, answer.body
        );
        // The criteria are in force once the pipe rests in RUNNING again, which is when it
        // polls under them.
        let state = wait_for_running(&router, name, case).await;
        assert_eq!(
            state, "RUNNING",
            "case {case:?}: expected the pipe running under the new criteria, got {state:?}"
        );

        for message in leg.messages {
            send(&router, &source_url, case, message).await;
        }
        let wanted: BTreeSet<String> = leg
            .messages
            .iter()
            .filter(|message| message.admitted)
            .map(|message| message.body.to_string())
            .collect();
        let sent: BTreeSet<String> = leg
            .messages
            .iter()
            .map(|message| message.body.to_string())
            .collect();

        let mut delivered = collect_at_least(&router, &target_url, wanted.len(), BUDGET).await;
        delivered.extend(collect_during(&router, &target_url, QUIET).await);
        // Only this leg's own records: an earlier leg's delivery is its own assertion.
        let seen: BTreeSet<String> = delivered
            .iter()
            .map(|payload| body_of(case, payload))
            .filter(|body| sent.contains(body))
            .collect();
        assert_eq!(
            seen, wanted,
            "case {case:?}: expected the criteria in force to admit exactly {wanted:?}, got \
             {seen:?}"
        );

        let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected every polled record consumed, {left} are still visible \
             on the source"
        );
    }
}

/// Wait until the pipe `name` reports `CurrentState` RUNNING, answering what it reported
/// last.
async fn wait_for_running(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = describe(router, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribePipe answered {} {}",
            described.status, described.body
        );
        let state = described.string("CurrentState");
        if state == "RUNNING" || Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
