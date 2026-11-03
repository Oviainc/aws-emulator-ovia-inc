// SPDX-License-Identifier: CC0-1.0

//! What this service's `ResourceNotFoundException` SAYS.
//!
//! Each not-found PATH states its own constant: the text does not vary with what was
//! looked up, so a client matching on it does not depend on what it asked for and the
//! lookup key never reaches an error the service keeps opaque, while two paths that word
//! the same miss differently stay distinguishable. The rows below span the not-found paths
//! the service has — a read, a delete, a dataset create, a conversion event, a send — and,
//! for each, several ids that name no resource, including the ARN form and ids of different
//! lengths: each path answers its own constant every time.

use aws_smithy_types::base64;
use serde_json::json;

use crate::harness::{call, emulator, link, query_escape, ACCOUNT_ID};

/// The message the account READ answers an id that names no linked account with
/// (`RESOURCE_NOT_FOUND_MESSAGE` in `src/state.rs`).
const NOT_FOUND: &str = "Requested resource not found.";

/// The message the `id`-scoped lookups answer their own not-found with
/// (`ID_NOT_FOUND_FOR_ACCOUNT_MESSAGE` in `src/state.rs`): a constant too, and the one
/// those paths — the dataset create and the conversion event — were recorded stating.
const DATASET_NOT_FOUND: &str = "Id not found for account.";

/// The message the DISASSOCIATION answers an id that names no linked account with
/// (`DISASSOCIATION_NOT_FOUND_MESSAGE` in `src/state.rs`): a constant too, and the one
/// that path was recorded stating — different text from the read's and the dataset's,
/// which is how a caller tells which lookup failed.
const DISASSOCIATION_NOT_FOUND: &str = "Resource requested for disassociation does not exist.";

/// The message the PHONE-NUMBER lookups answer an unregistered number with
/// (`ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE` in `src/state.rs`): a constant too, and
/// the one that lookup was recorded stating.
const ORIGINATION_NOT_FOUND: &str = "Origination phone number id not found for account.";

#[tokio::test]
async fn every_not_found_states_the_services_constant_message() {
    let router = emulator();
    // One account exists, so a miss is a miss on the id and not on an empty store.
    let linked = link(
        &router,
        "waba-55555555555555555555555555555555",
        &["phone-number-id-aaaa"],
    )
    .await;
    assert!(
        linked.starts_with("waba-"),
        "setup: expected a linked account id, got {linked:?}"
    );

    // Ids that name no resource: the short form, the ARN form, the shortest id the
    // `1..115` bound allows, and the longest — the message does not vary with any of it.
    let unknown_wabas = [
        "waba-00000000000000000000000000000000".to_string(),
        format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/0000000000000000"),
        "waba-".to_string(),
        format!("waba-{}", "0".repeat(110)),
    ];
    let unknown_phones = [
        "phone-number-id-00000000000000000000000000000000".to_string(),
        format!(
            "arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:phone-number-id/0000000000000000"
        ),
        "phone-number-id-".to_string(),
    ];

    for id in &unknown_wabas {
        let reads = [
            (
                "GetLinkedWhatsAppBusinessAccount",
                "GET",
                format!("/v1/whatsapp/waba/details?id={}", query_escape(id)),
                None,
                NOT_FOUND,
            ),
            (
                "DisassociateWhatsAppBusinessAccount",
                "DELETE",
                format!("/v1/whatsapp/waba/disassociate?id={}", query_escape(id)),
                None,
                DISASSOCIATION_NOT_FOUND,
            ),
            (
                "CreateWhatsAppDataset",
                "POST",
                "/v1/whatsapp/waba/dataset".to_string(),
                Some(json!({ "id": id })),
                DATASET_NOT_FOUND,
            ),
            (
                "SendWhatsAppConversionEvent",
                "POST",
                "/v1/whatsapp/waba/dataset/events".to_string(),
                Some(json!({
                    "id": id,
                    "datasetId": "1234567890",
                    "eventData": base64::encode(r#"{"event_name":"Purchase"}"#),
                })),
                DATASET_NOT_FOUND,
            ),
        ];
        for (operation, method, uri, body, expected) in reads {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {operation} {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
                answer.status,
                answer.error_type,
                answer.body
            );
            let actual = answer.message();
            assert_eq!(
                actual, expected,
                "case {operation} {id:?}: expected the message {expected:?}, got {actual:?}"
            );
        }
    }

    for id in &unknown_phones {
        let reads = [
            (
                "GetLinkedWhatsAppBusinessAccountPhoneNumber",
                "GET",
                format!("/v1/whatsapp/waba/phone/details?id={}", query_escape(id)),
                None,
                ORIGINATION_NOT_FOUND,
            ),
            (
                "SendWhatsAppMessage",
                "POST",
                "/v1/whatsapp/send".to_string(),
                Some(json!({
                    "originationPhoneNumberId": id,
                    "message": base64::encode("hello"),
                    "metaApiVersion": "v20.0",
                })),
                ORIGINATION_NOT_FOUND,
            ),
        ];
        for (operation, method, uri, body, expected) in reads {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {operation} {id:?}: expected 404 ResourceNotFoundException, got {} {:?} {:?}",
                answer.status,
                answer.error_type,
                answer.body
            );
            let actual = answer.message();
            assert_eq!(
                actual, expected,
                "case {operation} {id:?}: expected the message {expected:?}, got {actual:?}"
            );
        }
    }
}
