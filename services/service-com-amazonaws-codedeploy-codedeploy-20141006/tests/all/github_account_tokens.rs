// SPDX-License-Identifier: CC0-1.0

//! `DeleteGitHubAccountToken` over the wire.
//!
//! RULE: `DeleteGitHubAccountToken` removes the connection the request names from
//! this service's stored GitHub account connections and answers that name; a name
//! the store does not hold is a success that changes nothing (the delete is
//! idempotent), and a call that names no connection at all is refused with
//! `GitHubAccountTokenNameRequiredException`.
//!
//! The harness — the router, the `X-Amz-Target` request and the error-code helper —
//! is the one `codedeploy_lifecycle` drives the rest of this service with; only the
//! cases and the two connection helpers here are new.
//!
//! CodeDeploy exposes no API that CREATES a connection — one is made by the
//! console's GitHub OAuth flow — and `ListGitHubAccountTokenNames` is still a
//! scaffold with its own issue, so these cases seed and then inspect the shared
//! store directly to observe what the handler persisted. That is the same store
//! the handler writes through (`state::service_state`), never a private one.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// Link `names` as stored GitHub account connections, as the console's OAuth flow
/// would.
fn link(emulator: &Emulator, names: &[&str]) {
    let state = codedeploy::state::service_state(&*emulator.engine)
        .expect("the shared in-memory store is reachable");
    let mut guard = state.lock();
    for name in names {
        guard.record_git_hub_account_token(name);
    }
}

/// The connection names the store holds, in linking order.
fn linked(emulator: &Emulator) -> Vec<String> {
    codedeploy::state::service_state(&*emulator.engine)
        .expect("the shared in-memory store is reachable")
        .lock()
        .git_hub_account_token_names
        .clone()
}

/// RULE: the call answers the name it was given and the store no longer holds it,
/// whether or not it held it before, and the connections it did not name are left
/// exactly where they were.
#[tokio::test]
async fn deleting_a_connection_removes_only_that_name_and_answers_it() {
    // Rows differ along the FORM of the name (the model's `GitHubAccountTokenName`
    // is a bare string: no pattern, no length) and along whether the store holds a
    // connection under it.
    let long_name = "n".repeat(256);
    let cases: &[(&str, bool)] = &[
        ("github-connection", true),
        ("My.Org_Account-42", true),
        ("a", true),
        ("connection with spaces", true),
        ("ascii+symbols=ok,here@now", true),
        ("héllo-gîthub", true),
        (long_name.as_str(), true),
        ("never-linked", false),
        ("GITHUB-CONNECTION", false), // names are matched exactly, not case-folded
    ];

    for (name, linked_first) in cases {
        let emulator = Emulator::new();
        // A second connection that no case names, so each case also shows the
        // delete touches nothing else.
        let bystander = "untouched-connection";
        if *linked_first {
            link(&emulator, &[name, bystander]);
        } else {
            link(&emulator, &[bystander]);
        }

        let (status, body) = emulator
            .call("DeleteGitHubAccountToken", json!({"tokenName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["tokenName"].as_str(),
            Some(*name),
            "case {name:?}: expected the deleted connection name back, got {body:?}"
        );

        let remaining = linked(&emulator);
        assert_eq!(
            remaining,
            vec![bystander.to_string()],
            "case {name:?}: expected only {bystander:?} still linked, got {remaining:?}"
        );
    }
}

/// RULE (idempotence): the delete is a no-op on a name the store does not hold, so
/// repeating it answers the same success and leaves the store alone.
#[tokio::test]
async fn deleting_the_same_connection_twice_succeeds_both_times() {
    let emulator = Emulator::new();
    let name = "repeatedly-deleted";
    link(&emulator, &[name]);

    for attempt in 1..=3 {
        let (status, body) = emulator
            .call("DeleteGitHubAccountToken", json!({"tokenName": name}))
            .await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["tokenName"].as_str(),
            Some(name),
            "attempt {attempt}: expected the deleted connection name back, got {body:?}"
        );
        let remaining = linked(&emulator);
        assert!(
            remaining.is_empty(),
            "attempt {attempt}: expected no connection left, got {remaining:?}"
        );
    }
}

/// RULE: a call that names no connection — the member absent, or blank — is
/// refused with `GitHubAccountTokenNameRequiredException` (HTTP 400) and deletes
/// nothing.
#[tokio::test]
async fn a_call_naming_no_connection_is_refused() {
    // Rows differ along HOW the name is missing: absent member, empty string, and
    // whitespace of each kind. A JSON `null` is not a row: the protocol boundary
    // refuses it as a type error before any handler runs.
    let cases: &[(&str, Value)] = &[
        ("absent", json!({})),
        ("empty", json!({"tokenName": ""})),
        ("spaces", json!({"tokenName": "   "})),
        ("tab", json!({"tokenName": "\t"})),
        ("newline", json!({"tokenName": "\n"})),
    ];

    for (case, request) in cases {
        let emulator = Emulator::new();
        link(&emulator, &["kept-connection"]);

        let (status, body) = emulator
            .call("DeleteGitHubAccountToken", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "GitHubAccountTokenNameRequiredException",
            "case {case:?}: expected GitHubAccountTokenNameRequiredException, got {body:?}"
        );
        let remaining = linked(&emulator);
        assert_eq!(
            remaining,
            vec!["kept-connection".to_string()],
            "case {case:?}: a refused call must delete nothing, got {remaining:?}"
        );
    }
}

/// RULE (persistence): what one request deleted is gone for the NEXT request, not
/// just for the response it answered — the handler writes through the shared store.
#[tokio::test]
async fn a_deleted_connection_stays_deleted_for_later_requests() {
    let emulator = Emulator::new();
    link(&emulator, &["first", "second", "third"]);

    for name in ["second", "first"] {
        let (status, body) = emulator
            .call("DeleteGitHubAccountToken", json!({"tokenName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
    }

    let remaining = linked(&emulator);
    assert_eq!(
        remaining,
        vec!["third".to_string()],
        "expected only the connection nobody deleted to survive, got {remaining:?}"
    );

    // And a re-link after the deletes goes to the end: the store kept linking
    // order rather than re-using the deleted slots.
    link(&emulator, &["first"]);
    let relinked = linked(&emulator);
    assert_eq!(
        relinked,
        vec!["third".to_string(), "first".to_string()],
        "expected the re-linked connection at the end, got {relinked:?}"
    );
}
