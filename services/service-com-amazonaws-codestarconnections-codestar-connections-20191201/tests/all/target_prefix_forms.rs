// SPDX-License-Identifier: CC0-1.0

//! The ANSWER an operation gives does not depend on which `X-Amz-Target` prefix form
//! named it.
//!
//! `sdk_target_dispatch.rs` pins REACHABILITY: every modelled operation is reached behind
//! every prefix form that names this service. This suite pins the complementary half —
//! AGREEMENT. A request dispatched behind any of those forms reaches the same handler and
//! the same store, so it must come back with the same status and the same error code,
//! whatever kind of answer the operation gives: a success, a modelled refusal, the request
//! walk refusing an input the model constrains, or the scaffold channel's own 501 for an
//! operation nobody has written yet.
//!
//! That matters because the framework resolves an awsJson routing key as everything after
//! the target's FIRST dot, so for a prefix carrying dots of its own (this service
//! publishes the fully qualified Coral name) the forms are exactly where the answers used
//! to diverge: one form reached the handler and the rest were refused as an unknown
//! operation. The rows below therefore span all four kinds of answer, not just the
//! success path.
//!
//! Every call goes over the wire through the real `MultiServiceRouter`, reusing the
//! `connection_lifecycle` suite's harness.

use serde_json::{json, Value};

use super::connection_lifecycle::{error_code, Emulator, REGION, TARGET_PREFIX};

/// The prefix forms a client of this service may write, each `(case, prefix)`.
///
/// The service shape is `com.amazonaws.codestarconnections#CodeStar_connections_20191201`
/// (`smithy_query(query="service_info")`), so its local name is the last label of every
/// row and the rows differ only in what stands in front of it — none, one namespace
/// label, the SDK's dotted Coral namespace, and the absolute shape id with `#` written as
/// `.`.
const PREFIXES: &[(&str, &str)] = &[
    ("the bare service shape name", TARGET_PREFIX),
    (
        "the fully qualified Coral name an AWS SDK sends",
        "com.amazonaws.codestar.connections.CodeStar_connections_20191201",
    ),
    (
        "one namespace label before the shape name",
        "codestarconnections.CodeStar_connections_20191201",
    ),
    (
        "the absolute shape id",
        "com.amazonaws.codestarconnections.CodeStar_connections_20191201",
    ),
];

/// RULE: one request, one answer — every prefix form that names this service answers a
/// given request with the same status and the same error code.
///
/// A FRESH emulator per form, so the agreement is in the dispatch and not in state one
/// form left behind for the next.
#[tokio::test]
async fn the_same_request_gets_the_same_answer_behind_every_prefix_form() {
    // (case, operation, body) — one row per kind of answer this service can give.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a create that succeeds",
            "CreateConnection",
            json!({ "ConnectionName": "prefix-agreement", "ProviderType": "GitHub" }),
        ),
        (
            "a list that succeeds",
            "ListConnections",
            json!({ "MaxResults": 10 }),
        ),
        (
            "a modelled refusal: an ARN naming no connection",
            "GetConnection",
            json!({
                "ConnectionArn": format!(
                    "arn:aws:codestar-connections:{REGION}:000000000000:connection/00000000-0000-0000-0000-000000000000"
                )
            }),
        ),
        (
            "a modelled refusal: a delete of what is not there",
            "DeleteConnection",
            json!({
                "ConnectionArn": format!(
                    "arn:aws:codestar-connections:{REGION}:000000000000:connection/11111111-2222-3333-4444-555555555555"
                )
            }),
        ),
        (
            "the request walk refusing an absent required member",
            "GetConnection",
            json!({}),
        ),
        (
            "the request walk refusing a value outside a modelled bound",
            "CreateConnection",
            json!({ "ConnectionName": "", "ProviderType": "GitHub" }),
        ),
        (
            "an operation nobody has written yet",
            "ListSyncConfigurations",
            json!({ "RepositoryLinkId": "00000000-0000-0000-0000-000000000000", "SyncType": "CFN_STACK_SYNC" }),
        ),
    ];

    for (case, operation, body) in rows {
        let mut answers: Vec<(&str, u16, String)> = Vec::new();
        for (form, prefix) in PREFIXES {
            let emulator = Emulator::new();
            let (status, response) = emulator
                .call_target(&format!("{prefix}.{operation}"), body.clone())
                .await;
            answers.push((form, status, error_code(&response)));
        }

        let (first_form, first_status, first_code) = answers[0].clone();
        for (form, status, code) in &answers[1..] {
            assert_eq!(
                (*status, code.as_str()),
                (first_status, first_code.as_str()),
                "case {case:?} ({operation}): behind {form:?} the answer was \
                 {status} {code:?}, but behind {first_form:?} it was {first_status} \
                 {first_code:?}"
            );
        }
        // An answer that is the ROUTER refusing a key it could not resolve would agree
        // with itself across the forms while serving nobody, so the agreed answer is
        // also checked to be one this service's own walk produces.
        assert!(
            matches!(first_status, 200 | 400 | 403 | 501),
            "case {case:?} ({operation}): expected a status this service's own answer \
             uses, got {first_status} {first_code:?}"
        );
        if first_code == "NotImplemented" {
            assert_eq!(
                (first_status, operation),
                (501, "ListSyncConfigurations"),
                "case {case:?}: only a scaffolded operation may answer NotImplemented, \
                 got {first_status} {first_code:?}"
            );
        }
    }
}

/// RULE: the form that WROTE a connection does not own it — a connection created behind
/// one prefix form is deleted behind another, and after that delete every form reports it
/// absent.
///
/// One emulator for the whole test: what is asserted is that the forms share one store,
/// which is the other half of "the same request gets the same answer".
#[tokio::test]
async fn a_connection_created_behind_one_prefix_form_is_deleted_behind_another() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .call_target(
            &format!("{}.CreateConnection", PREFIXES[1].1),
            json!({ "ConnectionName": "written-by-the-sdk-form", "ProviderType": "GitLab" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected CreateConnection behind the SDK form to answer 200, got {status} {created}"
    );
    let arn = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn in {created}"))
        .to_string();

    // Deleted behind the LAST form, then read behind every form.
    let (status, deleted) = emulator
        .call_target(
            &format!("{}.DeleteConnection", PREFIXES[PREFIXES.len() - 1].1),
            json!({ "ConnectionArn": arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteConnection behind another form to answer 200, got {status} {deleted}"
    );

    for (form, prefix) in PREFIXES {
        let (status, response) = emulator
            .call_target(
                &format!("{prefix}.GetConnection"),
                json!({ "ConnectionArn": arn }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {form:?}: a connection deleted behind another form must read as absent, \
             got {status} {response}"
        );
    }
}
