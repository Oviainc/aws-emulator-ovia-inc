// SPDX-License-Identifier: CC0-1.0

//! The Managed Blockchain accessor, end to end over HTTP: `CreateAccessor` persists an
//! accessor, `GetAccessor` answers from that one record, and `DeleteAccessor` moves that
//! record from `AVAILABLE` to `PENDING_DELETION` and, on the clock, to `DELETED`.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures.
//!
//! The tables below span the classes the rules generalise over: every `NetworkType` the
//! model declares plus its omission, tag counts from none to the published maximum,
//! `ClientRequestToken`s from one character to the published maximum length, every
//! id-shaped lookup that names nothing stored, and each way a create request can be
//! malformed.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::ACCESSOR_DELETION_SPAN;
use smithy_server_runtime::MultiServiceRouter;

// The harness (a router over a fresh engine, a signed call, and the answer it returns)
// is shared with the network/node suite rather than copied.
use crate::network_and_node_lifecycle::{
    call, is_date_time, is_resource_id, new_router, Answer, ACCOUNT, PAST_TRANSITION, REGION,
};

/// The only `AccessorType` the model declares (smithy_query: shape AccessorType =
/// BILLING_TOKEN).
const BILLING_TOKEN: &str = "BILLING_TOKEN";

/// `AccessorBillingTokenString`'s `@length(42, 42)`.
const BILLING_TOKEN_LEN: usize = 42;

/// `InputTagMap`'s published maximum: "an overall maximum of 50 tags allowed per
/// resource" (`CreateAccessor$Tags`, `@length(0, 50)`).
const MAX_INPUT_TAGS: usize = 50;

/// A tag map with `count` entries.
fn tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|i| (format!("key{i}"), Value::String(format!("value{i}"))))
            .collect(),
    )
}

/// Create one accessor and return what the emulator answered.
///
/// `pub(crate)` so a sibling suite in this target drives a create through this one helper
/// rather than a second copy of the request.
pub(crate) async fn create_accessor(
    router: &MultiServiceRouter,
    token: &str,
    network_type: Option<&str>,
    tag_count: usize,
) -> Answer {
    let mut body = json!({
        "ClientRequestToken": token,
        "AccessorType": BILLING_TOKEN,
        "Tags": tags(tag_count),
    });
    if let Some(network_type) = network_type {
        body["NetworkType"] = Value::String(network_type.to_string());
    }
    call(router, "POST", "/accessors", Some(body)).await
}

/// One row of the accessor-create table: what the request names, and the `NetworkType`
/// the record must report for it.
#[derive(Debug)]
struct AccessorCase {
    token: &'static str,
    network_type: Option<&'static str>,
    expected_network_type: &'static str,
    tag_count: usize,
}

/// `CreateAccessor` persists an accessor and `GetAccessor` answers from that record —
/// over a table spanning every `NetworkType` the model declares, its omission, and tag
/// counts from none to the published maximum.
#[tokio::test]
async fn create_persists_the_accessor_and_get_answers_from_it() {
    let router = new_router();

    // The network types are the whole enum (smithy_query: shape AccessorNetworkType =
    // ETHEREUM_GOERLI | ETHEREUM_MAINNET | ETHEREUM_MAINNET_AND_GOERLI | POLYGON_MAINNET
    // | POLYGON_MUMBAI), plus a row that names none: AWS's CreateAccessor reference
    // states the default value of ETHEREUM_MAINNET_AND_GOERLI is applied when the action
    // does not set a networkType.
    let cases: &[AccessorCase] = &[
        AccessorCase {
            token: "accessor-mainnet",
            network_type: Some("ETHEREUM_MAINNET"),
            expected_network_type: "ETHEREUM_MAINNET",
            tag_count: 0,
        },
        AccessorCase {
            token: "accessor-goerli",
            network_type: Some("ETHEREUM_GOERLI"),
            expected_network_type: "ETHEREUM_GOERLI",
            tag_count: 1,
        },
        AccessorCase {
            token: "accessor-mainnet-and-goerli",
            network_type: Some("ETHEREUM_MAINNET_AND_GOERLI"),
            expected_network_type: "ETHEREUM_MAINNET_AND_GOERLI",
            tag_count: 3,
        },
        AccessorCase {
            token: "accessor-polygon-mainnet",
            network_type: Some("POLYGON_MAINNET"),
            expected_network_type: "POLYGON_MAINNET",
            tag_count: MAX_INPUT_TAGS,
        },
        AccessorCase {
            token: "accessor-polygon-mumbai",
            network_type: Some("POLYGON_MUMBAI"),
            expected_network_type: "POLYGON_MUMBAI",
            tag_count: 2,
        },
        AccessorCase {
            token: "accessor-default-network-type",
            network_type: None,
            expected_network_type: "ETHEREUM_MAINNET_AND_GOERLI",
            tag_count: 0,
        },
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    let mut seen_tokens: Vec<String> = Vec::new();
    for case in cases {
        let &AccessorCase {
            token,
            network_type,
            expected_network_type,
            tag_count,
        } = case;
        let answer = create_accessor(&router, token, network_type, tag_count).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAccessor 200, got {} {:?}",
            answer.status, answer.body
        );

        let accessor_id = answer.text(&["AccessorId"]);
        assert!(
            is_resource_id(&accessor_id, "ac"),
            "case {case:?}: expected an `ac-`-prefixed resource id, got {accessor_id:?}"
        );
        let billing_token = answer.text(&["BillingToken"]);
        assert_eq!(
            billing_token.chars().count(),
            BILLING_TOKEN_LEN,
            "case {case:?}: expected a {BILLING_TOKEN_LEN}-character BillingToken, got \
             {billing_token:?}"
        );
        assert!(
            billing_token.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected an alphanumeric BillingToken, got {billing_token:?}"
        );
        assert_eq!(
            answer.text(&["NetworkType"]),
            expected_network_type,
            "case {case:?}: expected CreateAccessor to report NetworkType \
             {expected_network_type}, got {:?}",
            answer.body
        );
        assert!(
            !seen_ids.contains(&accessor_id),
            "case {case:?}: expected a fresh accessor id, got the already-minted \
             {accessor_id:?}"
        );
        assert!(
            !seen_tokens.contains(&billing_token),
            "case {case:?}: expected a fresh BillingToken, got the already-minted \
             {billing_token:?}"
        );
        seen_ids.push(accessor_id.clone());
        seen_tokens.push(billing_token.clone());

        // The create's record reads back through `GetAccessor`.
        let answer = call(&router, "GET", &format!("/accessors/{accessor_id}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetAccessor 200, got {} {:?}",
            answer.status, answer.body
        );
        let accessor = &answer.body["Accessor"];
        assert_eq!(
            accessor["Id"].as_str(),
            Some(accessor_id.as_str()),
            "case {case:?}: expected Accessor.Id {accessor_id}, got {accessor:?}"
        );
        assert_eq!(
            accessor["Type"].as_str(),
            Some(BILLING_TOKEN),
            "case {case:?}: expected Accessor.Type {BILLING_TOKEN}, got {accessor:?}"
        );
        // `AccessorStatus` declares no creating state, so a created accessor is ready at
        // once (smithy_query: shape AccessorStatus = AVAILABLE | PENDING_DELETION |
        // DELETED).
        assert_eq!(
            accessor["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected Accessor.Status AVAILABLE, got {accessor:?}"
        );
        assert_eq!(
            accessor["BillingToken"].as_str(),
            Some(billing_token.as_str()),
            "case {case:?}: expected the created BillingToken {billing_token} back, got \
             {accessor:?}"
        );
        assert_eq!(
            accessor["NetworkType"].as_str(),
            Some(expected_network_type),
            "case {case:?}: expected Accessor.NetworkType {expected_network_type}, got \
             {accessor:?}"
        );
        assert_eq!(
            accessor["Arn"].as_str(),
            Some(
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:accessors/{accessor_id}")
                    .as_str()
            ),
            "case {case:?}: expected the accessor ARN for the signed region, got {accessor:?}"
        );
        assert!(
            is_date_time(&accessor["CreationDate"]),
            "case {case:?}: expected an ISO-8601 Accessor.CreationDate, got {accessor:?}"
        );
        assert_eq!(
            accessor["Tags"].as_object().map(serde_json::Map::len),
            Some(tag_count),
            "case {case:?}: expected {tag_count} tags back, got {accessor:?}"
        );
        if tag_count > 0 {
            assert_eq!(
                accessor["Tags"]["key0"].as_str(),
                Some("value0"),
                "case {case:?}: expected the tag values back, got {accessor:?}"
            );
        }

        // A read changes nothing a later caller sees: the second read is the first.
        let again = call(&router, "GET", &format!("/accessors/{accessor_id}"), None).await;
        assert_eq!(
            again.body, answer.body,
            "case {case:?}: expected GetAccessor to answer identically twice, got {:?} then {:?}",
            answer.body, again.body
        );
    }
}

/// `ClientRequestToken` is an idempotency token: "An idempotent operation completes no
/// more than once", so a repeated token answers with the accessor the first call created
/// and mints nothing new — over a table of tokens spanning the published length bounds.
#[tokio::test]
async fn a_repeated_client_request_token_replays_its_first_accessor() {
    let router = new_router();

    // `ClientRequestTokenString` is `@length(1, 64)`: both published bounds are rows,
    // and so is an ordinary token in between.
    let tokens: &[String] = &[
        "a".to_string(),
        "retry-token-2".to_string(),
        "t".repeat(64),
    ];

    let mut first_ids: Vec<String> = Vec::new();
    for token in tokens {
        let first = create_accessor(&router, token, Some("ETHEREUM_MAINNET"), 1).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected CreateAccessor 200, got {} {:?}",
            first.status, first.body
        );
        // Deliberately a DIFFERENT request body under the same token: the token, not the
        // body, decides whether a second accessor is created.
        let second = create_accessor(&router, token, Some("POLYGON_MAINNET"), 3).await;
        assert_eq!(
            second.status, 200,
            "case {token:?}: expected the replayed CreateAccessor 200, got {} {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.text(&["AccessorId"]),
            first.text(&["AccessorId"]),
            "case {token:?}: expected the replay to answer with the first accessor id {:?}, got {:?}",
            first.text(&["AccessorId"]),
            second.body
        );
        assert_eq!(
            second.text(&["BillingToken"]),
            first.text(&["BillingToken"]),
            "case {token:?}: expected the replay to answer with the first BillingToken, got {:?}",
            second.body
        );
        assert_eq!(
            second.text(&["NetworkType"]),
            first.text(&["NetworkType"]),
            "case {token:?}: expected the replay to report the stored NetworkType, got {:?}",
            second.body
        );

        let accessor_id = first.text(&["AccessorId"]);
        let read = call(&router, "GET", &format!("/accessors/{accessor_id}"), None).await;
        assert_eq!(
            read.body["Accessor"]["NetworkType"].as_str(),
            Some("ETHEREUM_MAINNET"),
            "case {token:?}: expected the replay to leave the stored accessor untouched, got {:?}",
            read.body
        );

        assert!(
            !first_ids.contains(&accessor_id),
            "case {token:?}: expected a distinct token to mint a distinct accessor, got the \
             already-minted {accessor_id:?}"
        );
        first_ids.push(accessor_id);
    }
}

/// An accessor id IN the accessor-id space that nothing minted is not found:
/// `GetAccessor` models `ResourceNotFoundException` (HTTP 404) for "a requested resource
/// doesn't exist". An id OUTSIDE that space is a different answer, pinned by
/// [`crate::not_found_and_malformed_ids`].
#[tokio::test]
async fn get_accessor_refuses_an_id_nothing_holds() {
    let router = new_router();

    // One accessor exists, so these are refused for naming nothing stored rather than
    // because the store is empty.
    let answer = create_accessor(&router, "the-one-accessor", Some("ETHEREUM_MAINNET"), 0).await;
    let existing = answer.text(&["AccessorId"]);

    // Rows span the well-formed accessor ids a caller can name that nothing minted: the
    // first and last id of the space (`ac-` plus 26 characters of the base-32 alphabet
    // `A`-`Z`, `2`-`7`), one in the middle of it, and the stored id with one suffix
    // character changed.
    let unknown_ids: &[String] = &[
        "ac-AAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
        "ac-77777777777777777777777777".to_string(),
        "ac-ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string(),
        // The stored id with its first suffix character changed to something else names
        // nothing stored either, whatever that character happened to be.
        format!(
            "ac-{}{}",
            if existing[3..4] == *"A" { "B" } else { "A" },
            &existing[4..]
        ),
    ];

    for id in unknown_ids {
        let answer = call(&router, "GET", &format!("/accessors/{id}"), None).await;
        assert_eq!(
            answer.status, 404,
            "case {id:?}: expected GetAccessor 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {:?} {:?}",
            answer.error_type, answer.body
        );
    }

    // The accessor that does exist is still found.
    let answer = call(&router, "GET", &format!("/accessors/{existing}"), None).await;
    assert_eq!(
        answer.status, 200,
        "expected the stored accessor {existing} to still be found, got {} {:?}",
        answer.status, answer.body
    );
}

/// A `CreateAccessor` request that breaks one of the model's own rules is refused with
/// HTTP 400, and nothing is created for it.
#[tokio::test]
async fn create_accessor_refuses_a_request_the_model_rejects() {
    let router = new_router();

    // Rows: a value outside `AccessorType`'s enum, each `@required` member absent, a
    // `ClientRequestToken` under its `@length(1, 64)` minimum and one over its maximum,
    // and a `NetworkType` outside `AccessorNetworkType`.
    let cases: &[(&str, Value)] = &[
        (
            "AccessorType outside the enum",
            json!({ "ClientRequestToken": "bad-type", "AccessorType": "NOT_A_TYPE" }),
        ),
        (
            "AccessorType absent",
            json!({ "ClientRequestToken": "no-type" }),
        ),
        (
            "ClientRequestToken absent",
            json!({ "AccessorType": BILLING_TOKEN }),
        ),
        (
            "ClientRequestToken under its minimum length",
            json!({ "ClientRequestToken": "", "AccessorType": BILLING_TOKEN }),
        ),
        (
            "ClientRequestToken over its maximum length",
            json!({ "ClientRequestToken": "t".repeat(65), "AccessorType": BILLING_TOKEN }),
        ),
        (
            "NetworkType outside the enum",
            json!({
                "ClientRequestToken": "bad-network",
                "AccessorType": BILLING_TOKEN,
                "NetworkType": "ETHEREUM_SEPOLIA"
            }),
        ),
    ];

    for (case, body) in cases {
        let answer = call(&router, "POST", "/accessors", Some(body.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected CreateAccessor 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.text(&["AccessorId"]).is_empty(),
            "case {case:?}: expected a refusal to create no accessor, got {:?}",
            answer.body
        );
    }
}

/// Read one accessor back.
async fn get_accessor(router: &MultiServiceRouter, accessor_id: &str) -> Answer {
    call(router, "GET", &format!("/accessors/{accessor_id}"), None).await
}

/// Delete one accessor.
async fn delete_accessor(router: &MultiServiceRouter, accessor_id: &str) -> Answer {
    call(router, "DELETE", &format!("/accessors/{accessor_id}"), None).await
}

/// `DeleteAccessor` moves the stored accessor to `PENDING_DELETION` — "After an accessor
/// is deleted, the status of the accessor changes from AVAILABLE to PENDING_DELETION" —
/// leaves every other property of the record exactly as the create persisted it, and the
/// clock then owes the accessor `DELETED`.
///
/// The table spans the accessor class the rule generalises over: every `NetworkType` the
/// model declares plus its omission, and tag counts from none to the published maximum.
#[tokio::test]
async fn delete_moves_the_accessor_to_pending_deletion_then_the_clock_owes_deleted() {
    let router = new_router();

    let cases: &[AccessorCase] = &[
        AccessorCase {
            token: "delete-mainnet",
            network_type: Some("ETHEREUM_MAINNET"),
            expected_network_type: "ETHEREUM_MAINNET",
            tag_count: 0,
        },
        AccessorCase {
            token: "delete-goerli",
            network_type: Some("ETHEREUM_GOERLI"),
            expected_network_type: "ETHEREUM_GOERLI",
            tag_count: 2,
        },
        AccessorCase {
            token: "delete-mainnet-and-goerli",
            network_type: Some("ETHEREUM_MAINNET_AND_GOERLI"),
            expected_network_type: "ETHEREUM_MAINNET_AND_GOERLI",
            tag_count: 1,
        },
        AccessorCase {
            token: "delete-polygon-mainnet",
            network_type: Some("POLYGON_MAINNET"),
            expected_network_type: "POLYGON_MAINNET",
            tag_count: MAX_INPUT_TAGS,
        },
        AccessorCase {
            token: "delete-polygon-mumbai",
            network_type: Some("POLYGON_MUMBAI"),
            expected_network_type: "POLYGON_MUMBAI",
            tag_count: 3,
        },
        AccessorCase {
            token: "delete-default-network-type",
            network_type: None,
            expected_network_type: "ETHEREUM_MAINNET_AND_GOERLI",
            tag_count: 0,
        },
    ];

    let mut deleted_ids: Vec<String> = Vec::new();
    for case in cases {
        let &AccessorCase {
            token,
            network_type,
            tag_count,
            expected_network_type,
        } = case;
        let created = create_accessor(&router, token, network_type, tag_count).await;
        let accessor_id = created.text(&["AccessorId"]);
        let before = get_accessor(&router, &accessor_id).await;
        assert_eq!(
            before.body["Accessor"]["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected the accessor to start AVAILABLE, got {:?}",
            before.body
        );

        let started = Instant::now();
        let answer = delete_accessor(&router, &accessor_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteAccessor 200, got {} {:?}",
            answer.status, answer.body
        );

        // `PENDING_DELETION` is where the delete leaves the accessor, and the record is
        // still readable: `AccessorStatus` has a value for each stage of the deletion
        // (smithy_query: shape AccessorStatus = AVAILABLE | PENDING_DELETION | DELETED).
        let after = get_accessor(&router, &accessor_id).await;
        assert!(
            started.elapsed() < ACCESSOR_DELETION_SPAN,
            "case {case:?}: this row must read the accessor back inside the deletion span \
             ({ACCESSOR_DELETION_SPAN:?}), but {:?} had already passed",
            started.elapsed()
        );
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected GetAccessor 200 for the deleted accessor, got {} {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.body["Accessor"]["Status"].as_str(),
            Some("PENDING_DELETION"),
            "case {case:?}: expected Accessor.Status PENDING_DELETION after the delete, got {:?}",
            after.body
        );
        assert_eq!(
            after.body["Accessor"]["NetworkType"].as_str(),
            Some(expected_network_type),
            "case {case:?}: expected the stored NetworkType {expected_network_type} to survive \
             the delete, got {:?}",
            after.body
        );

        // The delete changes the STATUS and nothing else: every other property is the one
        // the create persisted.
        let mut expected = before.body["Accessor"].clone();
        expected["Status"] = Value::String("PENDING_DELETION".to_string());
        assert_eq!(
            after.body["Accessor"], expected,
            "case {case:?}: expected only Accessor.Status to change, got {:?} from {:?}",
            after.body, before.body
        );

        // A second delete inside the span is a success that moves nothing: the accessor
        // is already on its way out, and the operation models no state-conflict error.
        let again = delete_accessor(&router, &accessor_id).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a repeated DeleteAccessor 200, got {} {:?}",
            again.status, again.body
        );

        deleted_ids.push(accessor_id);
    }

    // Past the deletion span the clock owes every one of them `DELETED`.
    tokio::time::sleep(PAST_TRANSITION).await;
    for (case, accessor_id) in cases.iter().zip(&deleted_ids) {
        let settled = get_accessor(&router, accessor_id).await;
        assert_eq!(
            settled.body["Accessor"]["Status"].as_str(),
            Some("DELETED"),
            "case {case:?}: expected Accessor.Status DELETED once the deletion came due, got {:?}",
            settled.body
        );
        // `DELETED` is terminal, and a read of it changes nothing a later caller sees.
        let again = get_accessor(&router, accessor_id).await;
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: expected GetAccessor to answer identically twice, got {:?} then {:?}",
            settled.body, again.body
        );
    }
}

/// Deleting an accessor whose deletion has already finished is a success that leaves it
/// in its terminal `DELETED` status: `DeleteAccessor` starts a deletion once, so a repeat
/// neither moves the status back nor re-arms the transition.
#[tokio::test]
async fn a_repeated_delete_leaves_the_accessor_deleted() {
    let router = new_router();

    // Rows: how many times the accessor is deleted after it has already settled.
    let repeats: &[usize] = &[1, 2, 5];

    for repeat in repeats {
        let created = create_accessor(
            &router,
            &format!("repeat-delete-{repeat}"),
            Some("ETHEREUM_MAINNET"),
            1,
        )
        .await;
        let accessor_id = created.text(&["AccessorId"]);

        let answer = delete_accessor(&router, &accessor_id).await;
        assert_eq!(
            answer.status, 200,
            "case {repeat:?}: expected the first DeleteAccessor 200, got {} {:?}",
            answer.status, answer.body
        );
        tokio::time::sleep(PAST_TRANSITION).await;
        let settled = get_accessor(&router, &accessor_id).await;
        assert_eq!(
            settled.body["Accessor"]["Status"].as_str(),
            Some("DELETED"),
            "case {repeat:?}: expected the accessor to settle DELETED, got {:?}",
            settled.body
        );

        for attempt in 0..*repeat {
            let answer = delete_accessor(&router, &accessor_id).await;
            assert_eq!(
                answer.status, 200,
                "case {repeat:?} attempt {attempt}: expected a repeated DeleteAccessor 200, got \
                 {} {:?}",
                answer.status, answer.body
            );
            let read = get_accessor(&router, &accessor_id).await;
            assert_eq!(
                read.body, settled.body,
                "case {repeat:?} attempt {attempt}: expected the repeat to leave the record as \
                 it was, got {:?} from {:?}",
                read.body, settled.body
            );
        }
    }
}

/// An accessor id nothing minted cannot be deleted: `DeleteAccessor` models
/// `ResourceNotFoundException` (HTTP 404) for "a requested resource doesn't exist", and
/// the refusal changes no stored accessor.
#[tokio::test]
async fn delete_accessor_refuses_an_id_nothing_holds() {
    let router = new_router();

    // One accessor exists, so these are refused for naming nothing stored rather than
    // because the store is empty.
    let created = create_accessor(&router, "the-kept-accessor", Some("POLYGON_MUMBAI"), 2).await;
    let existing = created.text(&["AccessorId"]);
    let before = get_accessor(&router, &existing).await;

    // Rows span the id shapes a caller can name: an unminted accessor id, another
    // resource family's id, a one-character id, `ResourceIdString`'s 32-character
    // maximum, and the stored id with one suffix character changed.
    let unknown_ids: &[String] = &[
        "ac-ZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        format!("nd-{}", "Y".repeat(26)),
        "q".to_string(),
        "z".repeat(32),
        format!(
            "ac-{}{}",
            if existing[3..4] == *"A" { "B" } else { "A" },
            &existing[4..]
        ),
    ];

    for id in unknown_ids {
        let answer = delete_accessor(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {id:?}: expected DeleteAccessor 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {:?} {:?}",
            answer.error_type, answer.body
        );
    }

    // The accessor that does exist is untouched by the refusals.
    let after = get_accessor(&router, &existing).await;
    assert_eq!(
        after.body, before.body,
        "expected the stored accessor {existing} to be untouched, got {:?} from {:?}",
        after.body, before.body
    );
    assert_eq!(
        after.body["Accessor"]["Status"].as_str(),
        Some("AVAILABLE"),
        "expected the stored accessor {existing} to still be AVAILABLE, got {:?}",
        after.body
    );
}
