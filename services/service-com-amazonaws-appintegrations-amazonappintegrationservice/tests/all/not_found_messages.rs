// SPDX-License-Identifier: CC0-1.0

//! The wording AppIntegrations gives a resource the request named and this account
//! does not hold.
//!
//! The rule under test: a `ResourceNotFoundException` from this service names the
//! resource TYPE, how the request addressed it and the value the request sent —
//! `"Could not find <ResourceType> with <name|identifier>: <value>."` — so the text is
//! derived from the request and is one template across the service's resources, rather
//! than a sentence each handler invents.
//!
//! The table spans both resources that have an implemented reader, every operation that
//! can answer 404 for them, and several identifier forms per operation (including both
//! published bounds of the identifier's `@length`). The names the issue recorded are
//! only two rows.

use serde_json::{json, Value};

use super::harness::{encode_label, Emulator, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The template every not-found message of this service follows.
fn expected_message(resource_type: &str, addressed_by: &str, value: &str) -> String {
    format!("Could not find {resource_type} with {addressed_by}: {value}.")
}

/// A stored neighbour of each resource type, so a 404 is about the resource the request
/// named and not about an empty store.
async fn with_neighbours(emulator: &Emulator) {
    let created = emulator
        .send(
            "POST",
            "/eventIntegrations",
            REGION,
            Some(json!({
                "Name": "present-event-integration",
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "aws.partner/demo" },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the neighbour event integration must be created, got {} / {}",
        created.status, created.body
    );

    let kms_key = emulator.create_kms_key(REGION).await;
    let created = emulator
        .send(
            "POST",
            "/dataIntegrations",
            REGION,
            Some(json!({ "Name": "present-data-integration", "KmsKey": kms_key })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the neighbour data integration must be created, got {} / {}",
        created.status, created.body
    );
}

#[tokio::test]
async fn a_resource_this_account_does_not_hold_is_reported_with_the_not_found_template() {
    let emulator = Emulator::new();
    with_neighbours(&emulator).await;

    // `Name` on the event-integration operations is len 1..255, pattern
    // ^[a-zA-Z0-9\/\._\-]+$; `Identifier` on the data-integration ones is len 1..255,
    // pattern \S (model: `smithy_query(query="operation",
    // operation="GetEventIntegration,GetDataIntegration,DeleteDataIntegration")`).
    // Rows per resource differ in the identifier's form and length, at both published
    // bounds of that length.
    let event_names: Vec<String> = vec![
        "absent-integration".to_string(),
        "n".to_string(),
        "n".repeat(255),
        "nested/name.with_punctuation-1".to_string(),
    ];
    let data_identifiers: Vec<String> = vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "i".to_string(),
        "i".repeat(255),
        format!(
            "arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration/\
             11111111-2222-3333-4444-555555555555"
        ),
    ];

    for name in &event_names {
        let label = encode_label(name);
        for (operation, reply) in [
            (
                "GetEventIntegration",
                emulator
                    .send("GET", &format!("/eventIntegrations/{label}"), REGION, None)
                    .await,
            ),
            (
                "UpdateEventIntegration",
                emulator
                    .send(
                        "PATCH",
                        &format!("/eventIntegrations/{label}"),
                        REGION,
                        Some(json!({ "Description": "x" })),
                    )
                    .await,
            ),
            (
                "DeleteEventIntegration",
                emulator
                    .send(
                        "DELETE",
                        &format!("/eventIntegrations/{label}"),
                        REGION,
                        None,
                    )
                    .await,
            ),
        ] {
            assert_not_found(
                &format!("{operation} on {name:?}"),
                &reply,
                &expected_message("EventIntegration", "name", name),
            );
        }
    }

    for identifier in &data_identifiers {
        let label = encode_label(identifier);
        for (operation, reply) in [
            (
                "GetDataIntegration",
                emulator
                    .send("GET", &format!("/dataIntegrations/{label}"), REGION, None)
                    .await,
            ),
            (
                "DeleteDataIntegration",
                emulator
                    .send("DELETE", &format!("/dataIntegrations/{label}"), REGION, None)
                    .await,
            ),
        ] {
            assert_not_found(
                &format!("{operation} on {identifier:?}"),
                &reply,
                &expected_message("DataIntegration", "identifier", identifier),
            );
        }
    }
}

#[tokio::test]
async fn the_not_found_template_is_the_same_before_and_after_a_delete() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows: a resource of each type, read back after the delete that removed it. The
    // post-delete read is the path the recorded suite takes, and it must answer the
    // same template as a read of something that never existed.
    let name = "deleted.event-integration";
    emulator
        .send(
            "POST",
            "/eventIntegrations",
            REGION,
            Some(json!({
                "Name": name,
                "EventBridgeBus": "default",
                "EventFilter": { "Source": "aws.cases" },
            })),
        )
        .await;
    emulator
        .send("DELETE", &format!("/eventIntegrations/{name}"), REGION, None)
        .await;
    let read_back = emulator
        .send("GET", &format!("/eventIntegrations/{name}"), REGION, None)
        .await;
    assert_not_found(
        &format!("GetEventIntegration after delete on {name:?}"),
        &read_back,
        &expected_message("EventIntegration", "name", name),
    );

    let created = emulator
        .send(
            "POST",
            "/dataIntegrations",
            REGION,
            Some(json!({ "Name": "deleted-data-integration", "KmsKey": kms_key })),
        )
        .await;
    let id = created
        .string("Id")
        .expect("CreateDataIntegration reports the minted Id");
    emulator
        .send("DELETE", &format!("/dataIntegrations/{id}"), REGION, None)
        .await;
    let read_back = emulator
        .send("GET", &format!("/dataIntegrations/{id}"), REGION, None)
        .await;
    assert_not_found(
        &format!("GetDataIntegration after delete on {id:?}"),
        &read_back,
        &expected_message("DataIntegration", "identifier", &id),
    );
}

/// The status, the error code and the message a 404 of this service must carry.
fn assert_not_found(case: &str, reply: &super::harness::Reply, expected: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.message().as_deref(),
        Some(expected),
        "case {case:?}: expected {expected:?}, got {:?}",
        reply.body.get("message").unwrap_or(&Value::Null)
    );
}
