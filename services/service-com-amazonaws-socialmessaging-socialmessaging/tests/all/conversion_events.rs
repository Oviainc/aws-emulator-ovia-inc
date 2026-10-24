// SPDX-License-Identifier: CC0-1.0

//! The Meta Conversions API surface: `CreateWhatsAppDataset` and
//! `SendWhatsAppConversionEvent`, end to end over `restJson1`.
//!
//! Both run on the persisted account record: the dataset a create mints is what the
//! account reports afterwards and the only dataset an event can be sent to, so a handler
//! that answered without consulting the store would fail here.

use aws_smithy_types::base64;
use serde_json::json;
use std::collections::BTreeSet;

use crate::harness::{call, emulator, link, query_escape, Answer, ACCOUNT_ID};

/// A Conversions API payload. Opaque to this service: it is the caller's bytes, recorded
/// as sent.
const EVENT_DATA: &str = r#"{"event_name":"Purchase","event_time":1735689600}"#;

async fn create_dataset(router: &smithy_server_runtime::MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "POST",
        "/v1/whatsapp/waba/dataset",
        Some(json!({ "id": id })),
    )
    .await
}

async fn send_event(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    dataset_id: &str,
    event_data: &str,
) -> Answer {
    call(
        router,
        "POST",
        "/v1/whatsapp/waba/dataset/events",
        Some(json!({
            "id": id,
            "datasetId": dataset_id,
            "eventData": base64::encode(event_data),
        })),
    )
    .await
}

async fn get_account(router: &smithy_server_runtime::MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(id));
    call(router, "GET", &uri, None).await
}

/// RULE: a dataset is MINTED for the account the request names, is what the account
/// reports from then on, and is the same dataset on a repeat call (`CreateWhatsAppDataset`
/// is `@idempotent`).
///
/// The id satisfies `WhatsAppDatasetId` — `len 10..20, pattern ^[0-9]+$`
/// (`smithy_query(query="shape", shape="WhatsAppDatasetId")`) — which is what a caller can
/// then send back as `datasetId`.
#[tokio::test]
async fn a_dataset_is_minted_once_per_account_and_read_back_on_it() {
    let router = emulator();
    let accounts = [
        link(&router, "waba-11111111111111111111111111111111", &["phone-a"]).await,
        link(&router, "waba-22222222222222222222222222222222", &["phone-b"]).await,
    ];

    let mut minted = BTreeSet::new();
    for account in &accounts {
        let created = create_dataset(&router, account).await;
        assert_eq!(
            created.status, 200,
            "case {account:?}: expected 200 from CreateWhatsAppDataset, got {} {:?}",
            created.status, created.body
        );
        let dataset_id = created.string_at("/datasetId");
        assert!(
            (10..=20).contains(&dataset_id.len())
                && dataset_id.bytes().all(|byte| byte.is_ascii_digit()),
            "case {account:?}: expected a 10..20 digit WhatsAppDatasetId, got {dataset_id:?}"
        );
        assert!(
            minted.insert(dataset_id.clone()),
            "case {account:?}: expected a dataset id of its own, got {dataset_id:?} twice"
        );

        // Read back through the account: the create wrote to the store the read serves.
        let read = get_account(&router, account).await;
        assert_eq!(
            read.string_at("/account/datasetId"),
            dataset_id,
            "case {account:?}: expected the account to report the dataset that was \
             created, got {:?}",
            read.at("/account/datasetId")
        );

        // `@idempotent`: an account has ONE dataset, so the repeat reports the same id
        // rather than minting a second.
        let again = create_dataset(&router, account).await;
        assert_eq!(
            (again.status, again.string_at("/datasetId")),
            (200, dataset_id.clone()),
            "case {account:?}: expected the repeat create to report {dataset_id:?}, got \
             {} {:?}",
            again.status,
            again.body
        );
    }
}

/// RULE: an event is accepted only for the dataset the named account HOLDS, and each
/// accepted call reports its own `requestId`.
///
/// The rows differ in the form the account is named by (short id, ARN) and in the payload
/// sent, which this service carries rather than interprets.
#[tokio::test]
async fn an_event_is_accepted_for_the_dataset_its_account_holds() {
    let router = emulator();
    let account = link(&router, "waba-33333333333333333333333333333333", &["phone-c"]).await;
    let dataset_id = create_dataset(&router, &account).await.string_at("/datasetId");
    let account_arn = get_account(&router, &account).await.string_at("/account/arn");
    assert!(
        account_arn.starts_with("arn:aws:social-messaging:"),
        "setup: expected the account's ARN, got {account_arn:?}"
    );

    let payloads = [
        EVENT_DATA,
        r#"{"event_name":"AddToCart"}"#,
        // The same payload twice: the request id is minted per CALL, not derived from
        // what was sent.
        EVENT_DATA,
    ];
    let mut request_ids = BTreeSet::new();
    for (index, payload) in payloads.iter().enumerate() {
        for named_by in [account.as_str(), account_arn.as_str()] {
            let answer = send_event(&router, named_by, &dataset_id, payload).await;
            assert_eq!(
                answer.status, 200,
                "case {index} as {named_by:?}: expected 200 from \
                 SendWhatsAppConversionEvent, got {} {:?}",
                answer.status, answer.body
            );
            let request_id = answer.string_at("/requestId");
            assert!(
                !request_id.is_empty(),
                "case {index} as {named_by:?}: expected a minted requestId, got \
                 {request_id:?}"
            );
            assert!(
                request_ids.insert(request_id.clone()),
                "case {index} as {named_by:?}: expected a new requestId per call, got \
                 {request_id:?} twice"
            );
        }
    }
    assert_eq!(
        request_ids.len(),
        payloads.len() * 2,
        "expected one distinct requestId per accepted call, got {request_ids:?}"
    );
}

/// RULE: a `datasetId` that is not the one the named account holds names no dataset this
/// service has, which is the modelled 404 — and an account with no dataset at all holds
/// none, so every well-formed id is a miss for it.
///
/// The rows span the ways a well-formed `datasetId` can fail to name the account's
/// dataset: another account's dataset, an id of the shortest and longest length the
/// member allows, and the account's own dataset asked for before it was created.
#[tokio::test]
async fn an_event_for_a_dataset_the_account_does_not_hold_is_the_modelled_not_found() {
    let router = emulator();
    let mine = link(&router, "waba-44444444444444444444444444444444", &["phone-d"]).await;
    let other = link(&router, "waba-55555555555555555555555555555555", &["phone-e"]).await;

    // Before any dataset exists, every well-formed id is a miss.
    let before = send_event(&router, &mine, "1234567890", EVENT_DATA).await;
    assert_eq!(
        (before.status, before.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case an account with no dataset: expected 404 ResourceNotFoundException, got {} \
         {:?} {:?}",
        before.status,
        before.error_type,
        before.body
    );

    let my_dataset = create_dataset(&router, &mine).await.string_at("/datasetId");
    let other_dataset = create_dataset(&router, &other).await.string_at("/datasetId");
    let misses = [
        ("another account's dataset", other_dataset.as_str()),
        ("the shortest id the member allows", "0000000000"),
        ("the longest id the member allows", "00000000000000000000"),
    ];
    for (case, dataset_id) in misses {
        let answer = send_event(&router, &mine, dataset_id, EVENT_DATA).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // And the account's own dataset is still accepted, so the refusals above are about
    // the dataset named and not about the operation.
    let accepted = send_event(&router, &mine, &my_dataset, EVENT_DATA).await;
    assert_eq!(
        accepted.status, 200,
        "case the account's own dataset: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// RULE: a `datasetId` outside the member's `10..20` bound is refused BEFORE the lookup,
/// as the modelled `ValidationException`, so a malformed id is never read as a miss.
#[tokio::test]
async fn a_dataset_id_outside_its_bound_is_refused_as_a_validation_exception() {
    let router = emulator();
    let account = format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/0000000000000000");
    let rows = [
        (
            "nine digits, one below the minimum",
            "123456789",
            "Member must have length greater than or equal to 10",
        ),
        (
            "twenty-one digits, one above the maximum",
            "123456789012345678901",
            "Member must have length less than or equal to 20",
        ),
    ];
    for (case, dataset_id, requirement) in rows {
        let answer = send_event(&router, &account, dataset_id, EVENT_DATA).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let expected = format!(
            "1 validation error detected: Value '{dataset_id}' at 'datasetId' failed to \
             satisfy constraint: {requirement}"
        );
        let actual = answer.message();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}
