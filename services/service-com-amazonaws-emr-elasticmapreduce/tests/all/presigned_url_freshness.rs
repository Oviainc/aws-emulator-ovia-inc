// SPDX-License-Identifier: CC0-1.0

//! The presigned URL `GetPersistentAppUIPresignedURL` hands back, end to end over the
//! wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns.
//!
//! THE RULE: each call PRESIGNS the URL afresh. The interface it opens is the stored user
//! interface's — the UI's own id under the regional application-UI domain, with the type
//! and, where the caller named one, the application as the path — but the session token
//! the URL carries is minted for that one call, so no two calls for one user interface
//! ever answer identical bytes and a URL that was cached or shared carries a credential of
//! its own. A token is never a function of the request either: two calls with the same
//! input differ.
//!
//! The rows span what the rule generalises over: which interface the UI publishes (each
//! `ProfilerType` the create accepts), whether the request names the type or leaves it to
//! the UI, whether it names an application, and how many times it is repeated.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;

use crate::job_flow_lifecycle::{at, kept_alive_request, Emr};
use crate::persistent_app_ui::opened_interface;

/// Longer than the service's clock-owed transition span (500 ms), so a UI that was
/// starting has been attached — which is what makes a presigned URL ready — by the time
/// we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How many times each row asks for a URL.
const REPEATS: usize = 3;

/// Every `ProfilerType` the create accepts, with the `PersistentAppUIType` the UI it
/// creates is addressed by. SOURCE: `smithy_query(query="shape",
/// shape="ProfilerType,PersistentAppUIType")` — `ProfilerType` is `SHS | TEZUI | YTS` and
/// `PersistentAppUIType` is `SHS | TEZ | YTS`.
const PROFILER_TO_UI_TYPE: [(&str, &str); 3] = [("SHS", "SHS"), ("TEZUI", "TEZ"), ("YTS", "YTS")];

/// One row: how the request selects the interface, and the application it names.
struct Case {
    label: &'static str,
    /// Whether the request names the `PersistentAppUIType` the UI publishes, or leaves the
    /// UI to answer for its own.
    name_the_type: bool,
    /// "The application ID associated with the presigned URL"
    /// (`GetPersistentAppUIPresignedURL$ApplicationId`), when the row names one.
    application_id: Option<&'static str>,
}

const CASES: [Case; 3] = [
    Case {
        label: "the UI's own interface",
        name_the_type: false,
        application_id: None,
    },
    Case {
        label: "the published type, named",
        name_the_type: true,
        application_id: None,
    },
    Case {
        label: "one application's own interface",
        name_the_type: false,
        application_id: Some("application_1700000000000_0001"),
    },
];

/// The token a presigned URL carries: everything after its query string's `=`.
fn token_of(case: &str, url: &str) -> String {
    let query = url
        .split_once('?')
        .unwrap_or_else(|| panic!("case {case:?}: expected a presigned URL with a token, got {url}"))
        .1;
    let (parameter, token) = query.split_once('=').unwrap_or_else(|| {
        panic!("case {case:?}: expected a token parameter, got {query:?} in {url}")
    });
    assert!(
        !parameter.is_empty() && !token.is_empty(),
        "case {case:?}: expected a named, non-empty token, got {query:?}"
    );
    // A token travels in a URL query as itself, so it is drawn from the unreserved
    // characters of RFC 3986 §2.3 and from nothing else.
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')),
        "case {case:?}: expected a URL-safe token, got {token:?}"
    );
    token.to_string()
}

#[tokio::test]
async fn every_call_presigns_the_url_afresh() {
    let emr = Emr::new();
    let cluster_arn = {
        let out = emr
            .ok("RunJobFlow", kept_alive_request("presigned-url-host"))
            .await;
        out["ClusterArn"]
            .as_str()
            .expect("RunJobFlow must answer a ClusterArn")
            .to_string()
    };

    // One UI per profiler the create accepts, all created before the wait below, so the
    // whole table is attached by the time the first URL is asked for.
    let mut uis: Vec<(&str, &str, String)> = Vec::new();
    for (profiler, ui_type) in PROFILER_TO_UI_TYPE {
        let out = emr
            .ok(
                "CreatePersistentAppUI",
                json!({ "TargetResourceArn": &cluster_arn, "ProfilerType": profiler }),
            )
            .await;
        let id = out["PersistentAppUIId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {profiler:?}: no PersistentAppUIId in {out}"))
            .to_string();
        uis.push((profiler, ui_type, id));
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    // Every token this test ever sees, so freshness is asserted across the whole table
    // and not only within one row.
    let mut every_token: BTreeSet<String> = BTreeSet::new();

    for (profiler, ui_type, id) in &uis {
        for case in &CASES {
            let label = format!("{profiler} / {}", case.label);
            let mut request = json!({ "PersistentAppUIId": id });
            if case.name_the_type {
                request["PersistentAppUIType"] = json!(ui_type);
            }
            if let Some(application_id) = case.application_id {
                request["ApplicationId"] = json!(application_id);
            }

            let mut interfaces: BTreeSet<String> = BTreeSet::new();
            let mut tokens: Vec<String> = Vec::new();
            for repeat in 0..REPEATS {
                let out = emr
                    .ok("GetPersistentAppUIPresignedURL", request.clone())
                    .await;
                assert_eq!(
                    at(&out, &["PresignedURLReady"]).as_bool(),
                    Some(true),
                    "case {label:?} (call {repeat}): an attached UI is ready, got {out}"
                );
                let url = at(&out, &["PresignedURL"])
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?} (call {repeat}): no URL in {out}"))
                    .to_string();
                let interface = opened_interface(&url).to_string();
                assert!(
                    interface.starts_with("https://")
                        && interface.contains(id.as_str())
                        && interface.contains(&ui_type.to_lowercase()),
                    "case {label:?}: the interface is the stored UI's own, got {url}"
                );
                match case.application_id {
                    Some(application_id) => assert!(
                        interface.contains(application_id),
                        "case {label:?}: the application is part of the interface, got {url}"
                    ),
                    None => assert!(
                        !interface.contains("history/"),
                        "case {label:?}: no application was named, got {url}"
                    ),
                }
                interfaces.insert(interface);
                tokens.push(token_of(&label, &url));
            }

            // The interface is stable across the repeats; the token is not.
            assert_eq!(
                interfaces.len(),
                1,
                "case {label:?}: every call opens the same interface, got {interfaces:?}"
            );
            let distinct: BTreeSet<&String> = tokens.iter().collect();
            assert_eq!(
                distinct.len(),
                tokens.len(),
                "case {label:?}: expected {REPEATS} distinct tokens, got {tokens:?}"
            );
            for token in tokens {
                assert!(
                    every_token.insert(token.clone()),
                    "case {label:?}: a token was handed out twice: {token:?}"
                );
            }
        }
    }
}

/// A UI the service has not attached to its target resource yet serves nothing, so it is
/// reported as not ready and no token is minted for it.
#[tokio::test]
async fn a_ui_that_is_not_attached_yet_is_answered_without_a_url() {
    let case = "a starting UI";
    let emr = Emr::new();
    let out = emr
        .ok("RunJobFlow", kept_alive_request("unattached-url-host"))
        .await;
    let cluster_arn = out["ClusterArn"]
        .as_str()
        .expect("RunJobFlow must answer a ClusterArn")
        .to_string();
    let created = emr
        .ok(
            "CreatePersistentAppUI",
            json!({ "TargetResourceArn": cluster_arn }),
        )
        .await;
    let id = created["PersistentAppUIId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no PersistentAppUIId in {created}"));

    let answer = emr
        .ok(
            "GetPersistentAppUIPresignedURL",
            json!({ "PersistentAppUIId": id }),
        )
        .await;
    assert_eq!(
        at(&answer, &["PresignedURLReady"]).as_bool(),
        Some(false),
        "case {case:?}: a UI that is not attached yet is not ready, got {answer}"
    );
    assert!(
        at(&answer, &["PresignedURL"]).is_null(),
        "case {case:?}: no URL is answered for a UI that serves nothing, got {answer}"
    );
}
