// SPDX-License-Identifier: CC0-1.0

//! Reading a function's delivered log through the group-wide SEARCH: `FilterLogEvents` on
//! `/aws/lambda/<function name>`, which is the read a client polls to find what a run wrote.
//!
//! RULE: the events an invocation's log delivery stores are ordinary CloudWatch Logs events of
//! the function's log group, so the group-wide search returns them — every line the run wrote,
//! in the stream that invocation took, matching the group's `filterPattern` the way any other
//! ingested event does — and a group no invocation has created yet is not a group at all, so
//! the search answers CloudWatch Logs' own `ResourceNotFoundException` (HTTP 400) until the
//! first delivery creates it.
//!
//! This is the half of the delivery a client depends on and that a stream-by-stream read does
//! not state: a client that has just invoked a function knows the group but not the stream, so
//! it searches the group. [`crate::invocation_log_delivery`] pins WHAT is delivered (its
//! harness is reused here rather than copied); this suite pins that a client can FIND it.
//!
//! Every row drives real HTTP through the production `MultiServiceRouter` with Lambda,
//! CloudWatch Logs and IAM mounted on ONE engine, and the rows differ along what the rule
//! generalises over: what the handler wrote (one line, several), which runtime ran it, which
//! group the configuration sends it to (the default, one the configuration names), and which
//! filter pattern the search carries (none, one the written line matches, one nothing
//! matches).
//!
//! Every test here RUNS handlers, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use serde_json::json;

use crate::event_source_mapping_role_access::{allow, given_role};
use crate::invocation_log_delivery::{
    create_function, emulator, invoke, logs, Created, LOGS, LOG_WRITES, NODE_RUNTIME,
    PYTHON_RUNTIME,
};

/// One row: a handler that writes, and what the group-wide search must then return.
struct Row {
    case: &'static str,
    /// The handler's source. It writes [`Row::written`], so what the search finds is what the
    /// run wrote.
    source: &'static str,
    /// The runtime that runs it.
    runtime: &'static str,
    /// The lines the handler itself wrote, each of which the search must return as an event
    /// of its own.
    written: &'static [&'static str],
    /// The group the configuration sends the log to, or `None` for the default one.
    log_group: Option<&'static str>,
    /// A `filterPattern` the written lines match, which must return them, and one nothing
    /// matches, which must return none. Each is a `"…"` string term, as the syntax requires
    /// of a term carrying a character outside its bare-word set (a hyphen here).
    matching: &'static str,
    non_matching: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a python handler printing one line",
            source: "def lambda_handler(event, context):\n    \
                     print('out-search-one')\n    return {'ok': True}\n",
            runtime: PYTHON_RUNTIME,
            written: &["out-search-one"],
            log_group: None,
            matching: "\"out-search-one\"",
            non_matching: "\"nothing-wrote-this\"",
        },
        Row {
            case: "a python handler printing three lines",
            source: "def lambda_handler(event, context):\n    \
                     for index in range(3):\n        \
                     print('line-%d-search-many' % index)\n    return {'ok': True}\n",
            runtime: PYTHON_RUNTIME,
            written: &[
                "line-0-search-many",
                "line-1-search-many",
                "line-2-search-many",
            ],
            log_group: None,
            matching: "\"search-many\"",
            non_matching: "\"line-9-search-many\"",
        },
        Row {
            case: "a node handler logging one line",
            source: "exports.handler = async () => {\n  \
                     console.log('out-search-node');\n  return { ok: true };\n};\n",
            runtime: NODE_RUNTIME,
            written: &["out-search-node"],
            log_group: None,
            matching: "\"out-search-node\"",
            non_matching: "\"out-search-nodejs-not-written\"",
        },
        Row {
            case: "a function whose LoggingConfig names a group of its own",
            source: "def lambda_handler(event, context):\n    \
                     print('out-search-vended')\n    return {'ok': True}\n",
            runtime: PYTHON_RUNTIME,
            written: &["out-search-vended"],
            log_group: Some("/aws/vendedlogs/lambda-search-suite"),
            matching: "\"out-search-vended\"",
            non_matching: "\"out-search-elsewhere\"",
        },
    ]
}

/// The `(logStreamName, message)` pairs `FilterLogEvents` reports for `group`, with the
/// filter `pattern` when the row carries one.
async fn searched(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    group: &str,
    pattern: Option<&str>,
) -> Vec<(String, String)> {
    let mut request = json!({ "logGroupName": group });
    if let Some(pattern) = pattern {
        request["filterPattern"] = json!(pattern);
    }
    let answered = logs(router, case, "FilterLogEvents", &request).await;
    answered["events"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .map(|event| {
                    (
                        event["logStreamName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        event["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The status and the error code CloudWatch Logs answered a search of `group` with — how a
/// group nothing has created yet reads.
async fn refused(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    group: &str,
) -> (u16, String) {
    let (status, answered) = crate::invocation_log_delivery::logs_answer(
        router,
        case,
        "FilterLogEvents",
        &json!({ "logGroupName": group }),
    )
    .await;
    let code = answered["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code)
}

/// A delivered run's lines are what a group-wide search returns, and the group does not exist
/// until the first delivery creates it.
#[tokio::test]
async fn a_delivered_run_is_found_by_a_search_of_the_functions_log_group() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let role = given_role(
            &router,
            case,
            &format!("log-searcher-{index}"),
            Some(json!([allow(LOGS, &LOG_WRITES, "*")])),
        )
        .await;
        let name = format!("log-search-{index}");
        let default_group = format!("/aws/lambda/{name}");
        let group = row.log_group.unwrap_or(&default_group).to_string();

        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: row.source,
                runtime: row.runtime,
                role: &role,
                log_group: row.log_group,
            },
        )
        .await;
        // Nothing has been delivered yet, so there is no group to search.
        let answered = refused(&router, case, &group).await;
        assert_eq!(
            answered,
            (400, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected a search of {group:?} to be refused before the first \
             invocation, got {answered:?}"
        );

        let (reported, _payload) = invoke(&router, case, &name).await;
        let found = searched(&router, case, &group, None).await;
        assert!(
            !found.is_empty(),
            "case {case:?}: expected a search of {group:?} to find the run's log, got none \
             (the run itself reported {reported:?})"
        );
        for written in row.written {
            assert!(
                found.iter().any(|(_, message)| message.contains(written)),
                "case {case:?}: expected a searched event carrying {written:?}, got {found:?}"
            );
        }
        // Every event the search returned belongs to ONE stream: the invocation's own, which
        // is how a client that searched the group then narrows to the run.
        let streams: Vec<&String> = found
            .iter()
            .map(|(stream, _)| stream)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            streams.len(),
            1,
            "case {case:?}: expected one invocation's lines in one stream, got {streams:?}"
        );
        // The search is a search: the group's events are filtered by the pattern exactly as
        // any other ingested event is.
        let matched = searched(&router, case, &group, Some(row.matching)).await;
        for written in row.written {
            assert!(
                matched.iter().any(|(_, message)| message.contains(written)),
                "case {case:?}: expected the pattern {:?} to match {written:?}, got {matched:?}",
                row.matching
            );
        }
        let unmatched = searched(&router, case, &group, Some(row.non_matching)).await;
        assert!(
            unmatched.is_empty(),
            "case {case:?}: expected the pattern {:?} to match nothing the run wrote, got \
             {unmatched:?}",
            row.non_matching
        );
        // A function that logs elsewhere leaves the default group uncreated, so a search of
        // it is still the refusal.
        if row.log_group.is_some() {
            let answered = refused(&router, case, &default_group).await;
            assert_eq!(
                answered,
                (400, "ResourceNotFoundException".to_string()),
                "case {case:?}: expected {default_group:?} to stay uncreated, got {answered:?}"
            );
        }
    }
}
