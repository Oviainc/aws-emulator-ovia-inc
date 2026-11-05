// SPDX-License-Identifier: CC0-1.0

//! `ListClusters`' creation-date window, end to end over the wire.
//!
//! THE RULE: the window `CreatedAfter`/`CreatedBefore` bound a listing with is settled
//! BEFORE the filter is applied. A request that bounds the window at BOTH ends with a
//! beginning strictly after its end has asked for a window no cluster can fall inside, so
//! Amazon EMR refuses it — HTTP 400 with its modelled client-input error
//! `InvalidRequestException`, carrying the discriminator
//! `INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE` beside the entirely FIXED sentence
//! `createdAfter date cannot be after createdBefore date.` — instead of answering an empty
//! success a caller reads as "no clusters in this window". Every other window is filtered:
//! one bounded at a single end is open on the other, a window whose two bounds are the
//! same instant is the single-instant window, and a window the right way round reports
//! exactly the clusters created inside it.
//!
//! The rows span what the rule generalises over: which bounds the request carries, how far
//! apart they are and in which order (a day, a second, a fraction of a second), whether
//! the instants are before or after the epoch, and whether another filter accompanies the
//! window — never only the recorded day-apart pair. Each case drives the real HTTP path
//! through the router's `dispatch`, and a cluster is only ever observed through
//! `ListClusters`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, epoch_secs, kept_alive_request, Emr};
use std::time::SystemTime;

/// EMR's own client-input error: "This exception occurs when there is something wrong
/// with user input" (HTTP 400), which `ListClusters` declares
/// (`smithy_query(query="operation", operation="ListClusters")`).
const SERVICE_ERROR: &str = "InvalidRequestException";

/// The discriminator EMR answers an inverted creation-date window with.
const INVERTED_WINDOW_CODE: &str = "INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE";

/// The sentence EMR answers it with, which interpolates nothing from the request.
const INVERTED_WINDOW_MESSAGE: &str = "createdAfter date cannot be after createdBefore date.";

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &Value) -> String {
    at(payload, &["__type"])
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A `ListClusters` request carrying the bounds a row names, and anything else it adds.
fn listing(after: Option<f64>, before: Option<f64>, extra: Value) -> Value {
    let mut body = match extra {
        Value::Object(_) => extra,
        _ => json!({}),
    };
    if let Some(after) = after {
        body["CreatedAfter"] = json!(after);
    }
    if let Some(before) = before {
        body["CreatedBefore"] = json!(before);
    }
    body
}

/// What a row expects of its listing.
enum Expected {
    /// The window is filtered, and these cluster names are the ones inside it.
    Reports(Vec<&'static str>),
    /// The window is inverted, so the request is refused.
    Refused,
}

/// One row: the window a request bounds the listing with, and what it is answered with.
struct Case {
    case: &'static str,
    after: Option<f64>,
    before: Option<f64>,
    /// Anything the request carries beside the window.
    extra: Value,
    expected: Expected,
}

/// Every cluster name a listing reports, sorted so a row can compare a set.
fn reported_names(payload: &Value) -> Vec<String> {
    let mut names: Vec<String> = at(payload, &["Clusters"])
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| at(cluster, &["Name"]).as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn an_inverted_creation_window_is_refused_and_every_other_one_is_filtered() {
    let emr = Emr::new();
    // Two clusters, both created between `before_launch` and `after_launch`, so a row can
    // bound a window that holds them, one that holds neither, and one that is inverted.
    let before_launch = epoch_secs(SystemTime::now()) - 1.0;
    emr.run_job_flow(kept_alive_request("first")).await;
    emr.run_job_flow(kept_alive_request("second")).await;
    let after_launch = epoch_secs(SystemTime::now()) + 1.0;
    let both = vec!["first", "second"];

    let cases = vec![
        Case {
            case: "no window at all",
            after: None,
            before: None,
            extra: Value::Null,
            expected: Expected::Reports(both.clone()),
        },
        Case {
            case: "only the beginning, which leaves the end open",
            after: Some(before_launch),
            before: None,
            extra: Value::Null,
            expected: Expected::Reports(both.clone()),
        },
        Case {
            case: "only the end, which leaves the beginning open",
            after: None,
            before: Some(after_launch),
            extra: Value::Null,
            expected: Expected::Reports(both.clone()),
        },
        Case {
            case: "only a beginning after every cluster was created",
            after: Some(after_launch),
            before: None,
            extra: Value::Null,
            expected: Expected::Reports(Vec::new()),
        },
        Case {
            case: "a window the right way round, holding both clusters",
            after: Some(before_launch),
            before: Some(after_launch),
            extra: Value::Null,
            expected: Expected::Reports(both.clone()),
        },
        Case {
            case: "a window the right way round, holding neither cluster",
            after: Some(after_launch),
            before: Some(after_launch + 3600.0),
            extra: Value::Null,
            expected: Expected::Reports(Vec::new()),
        },
        Case {
            // Equal bounds are the single-instant window: satisfiable, so filtered.
            case: "both bounds at the same instant",
            after: Some(after_launch),
            before: Some(after_launch),
            extra: Value::Null,
            expected: Expected::Reports(Vec::new()),
        },
        Case {
            case: "both bounds at the epoch itself",
            after: Some(0.0),
            before: Some(0.0),
            extra: Value::Null,
            expected: Expected::Reports(Vec::new()),
        },
        Case {
            case: "a beginning a day after the end",
            after: Some(after_launch),
            before: Some(after_launch - 86_400.0),
            extra: Value::Null,
            expected: Expected::Refused,
        },
        Case {
            case: "a beginning one second after the end",
            after: Some(after_launch),
            before: Some(after_launch - 1.0),
            extra: Value::Null,
            expected: Expected::Refused,
        },
        Case {
            // `Date` is a timestamp, which awsJson1_1 carries as epoch SECONDS, so a
            // fraction of a second still inverts the window.
            case: "a beginning a millisecond after the end",
            after: Some(after_launch),
            before: Some(after_launch - 0.001),
            extra: Value::Null,
            expected: Expected::Refused,
        },
        Case {
            case: "an inverted window entirely before the epoch",
            after: Some(-86_400.0),
            before: Some(-172_800.0),
            extra: Value::Null,
            expected: Expected::Refused,
        },
        Case {
            // The window is settled BEFORE the filter, so an accompanying filter that
            // would itself have matched nothing does not change the answer.
            case: "an inverted window beside a ClusterStates filter",
            after: Some(after_launch),
            before: Some(before_launch),
            extra: json!({ "ClusterStates": ["TERMINATED"] }),
            expected: Expected::Refused,
        },
        Case {
            case: "an inverted window beside a ClusterStates filter that would match",
            after: Some(after_launch),
            before: Some(before_launch),
            extra: json!({ "ClusterStates": ["STARTING", "RUNNING", "WAITING"] }),
            expected: Expected::Refused,
        },
    ];

    for case in cases {
        let label = case.case;
        let (status, payload) = emr
            .call("ListClusters", listing(case.after, case.before, case.extra))
            .await;
        match case.expected {
            Expected::Reports(expected) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the window to be filtered, got {status} with \
                     {payload}"
                );
                let reported = reported_names(&payload);
                let mut expected: Vec<String> =
                    expected.iter().map(|name| (*name).to_string()).collect();
                expected.sort();
                assert_eq!(
                    reported, expected,
                    "case {label:?}: expected the clusters inside the window, got \
                     {reported:?}"
                );
            }
            Expected::Refused => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, SERVICE_ERROR),
                    "case {label:?}: expected 400 {SERVICE_ERROR}, got {status} with \
                     {payload}"
                );
                assert_eq!(
                    at(&payload, &["ErrorCode"]).as_str(),
                    Some(INVERTED_WINDOW_CODE),
                    "case {label:?}: expected the inverted-window discriminator, got \
                     {payload}"
                );
                assert_eq!(
                    at(&payload, &["Message"]).as_str(),
                    Some(INVERTED_WINDOW_MESSAGE),
                    "case {label:?}: expected the inverted-window sentence, got {payload}"
                );
            }
        }
    }

    // A refused listing changed nothing: the clusters are still reported by a listing
    // with no window at all.
    let listed = emr.ok("ListClusters", json!({})).await;
    assert_eq!(
        reported_names(&listed),
        vec!["first".to_string(), "second".to_string()],
        "a refused listing must leave every cluster reportable, got {listed}"
    );
}
