// SPDX-License-Identifier: CC0-1.0

//! Which create a MediaLive input `RequestId` is a replay OF.
//!
//! `CreateInput` and `CreatePartnerInput` both create inputs and both carry an
//! `@idempotencyToken` `RequestId` (`smithy_query(query="operation",
//! operation="CreateInput,CreatePartnerInput")`), so a token answers a replay
//! only for the create it was spent on: the operation, and — for a partner —
//! the input the partner is created FOR. A token spent on any other create
//! names a resource this request did not ask for, so the request goes ahead and
//! creates the input it describes.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one row is one way a token can have been spent, never
//! the whole rule.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::resolved_input_body;
use crate::wire::{call, medialive};

/// The collection `CreateInput` and `ListInputs` are bound to
/// (`smithy_query(query="operation", operation="CreateInput,ListInputs")`).
const INPUTS: &str = "/prod/inputs";

/// `CreateInput`, pointed at resources this account really holds.
async fn create_input(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let body = resolved_input_body(router, body).await;
    call(router, "POST", INPUTS, Some(body)).await
}

/// `CreateInput` that has to succeed, answering the created input.
async fn created_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = create_input(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateInput to answer 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// `CreatePartnerInput`: `POST /prod/inputs/{InputId}/partners`
/// (`smithy_query(query="operation", operation="CreatePartnerInput")`).
async fn create_partner(
    router: &MultiServiceRouter,
    input_id: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("{INPUTS}/{input_id}/partners"),
        body,
    )
    .await
}

/// `CreatePartnerInput` that has to succeed, answering the created partner.
async fn created_partner(
    router: &MultiServiceRouter,
    case: &str,
    input_id: &str,
    token: &str,
) -> Value {
    let (status, response) =
        create_partner(router, input_id, Some(json!({ "requestId": token }))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreatePartnerInput to answer 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{INPUTS}/{id}"), None).await
}

/// Every input id this account holds, in the order `ListInputs` reports them.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, response) = call(router, "GET", INPUTS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListInputs to answer 200, got {status}: {response}"
    );
    response["inputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|input| input["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `inputPartnerIds` an input reports, whatever its length.
fn partner_ids(input: &Value) -> Vec<String> {
    input["inputPartnerIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The configuration the primary of each row is created with: the family a
/// partner input exists for, so a partner inherits THIS and nothing a row's own
/// `CreateInput` describes.
fn primary_body(name: &str) -> Value {
    json!({
        "name": name,
        "type": "AWS_CDI",
        "destinations": [{ "streamName": "cdi-a" }],
    })
}

/// The `CreateInput` bodies the first table describes, each a different family
/// from the partner's inherited one. Source for the type names and the
/// per-family member each row fills: the `CreateInput` input signature and the
/// `InputType` enum (`smithy_query(query="operation", operation="CreateInput")`).
fn described_input_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "url pull with its own sources",
            json!({
                "name": "brand-new-pull",
                "type": "URL_PULL",
                "sources": [
                    { "url": "https://example.com/a.m3u8" },
                    { "url": "https://example.com/b.m3u8" },
                ],
            }),
        ),
        (
            "udp push with one destination",
            json!({
                "name": "brand-new-udp",
                "type": "UDP_PUSH",
                "destinations": [{ "streamName": "udp-a" }],
            }),
        ),
        (
            "rtmp push with two destinations",
            json!({
                "name": "brand-new-rtmp",
                "type": "RTMP_PUSH",
                "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
            }),
        ),
        (
            "mp4 file with its own sources",
            json!({
                "name": "brand-new-mp4",
                "type": "MP4_FILE",
                "sources": [
                    { "url": "https://example.com/a.mp4" },
                    { "url": "https://example.com/b.mp4" },
                ],
            }),
        ),
        (
            "mediaconnect with its own flow",
            json!({
                "name": "brand-new-mediaconnect",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
        ),
    ]
}

/// Rule: a `RequestId` spent creating somebody's partner input is not a replay
/// of a standalone `CreateInput`, so that create stores and answers with the
/// input it describes rather than the partner.
#[tokio::test]
async fn a_request_id_spent_on_a_partner_create_still_creates_the_described_input() {
    for (case, body) in described_input_table() {
        let router = medialive();
        let primary = created_input(&router, case, primary_body("token-primary")).await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        // The token's first and only use before the create under test.
        let token = "shared-token";
        let partner = created_partner(&router, case, &primary_id, token).await;
        let partner_id = partner["id"].as_str().unwrap_or_default().to_string();

        let mut replayed = body.clone();
        replayed["requestId"] = json!(token);
        let created = created_input(&router, case, replayed).await;
        let created_id = created["id"].as_str().unwrap_or_default().to_string();

        assert_ne!(
            created_id, partner_id,
            "case {case:?}: expected an input of its own, got the partner input {partner_id:?}"
        );
        assert_eq!(
            created["type"], body["type"],
            "case {case:?}: expected type {:?}, got {:?}",
            body["type"], created["type"]
        );
        assert_eq!(
            created["name"], body["name"],
            "case {case:?}: expected name {:?}, got {:?}",
            body["name"], created["name"]
        );
        assert!(
            partner_ids(&created).is_empty(),
            "case {case:?}: expected no partner relationship, got {:?}",
            partner_ids(&created)
        );

        // The input really was stored: it reads back, and the account holds all
        // three inputs rather than just the two the first two calls created.
        let (status, described) = describe_input(&router, &created_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeInput to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described["type"], body["type"],
            "case {case:?}: expected the stored input to report type {:?}, got {:?}",
            body["type"], described["type"]
        );
        let mut held = listed_ids(&router, case).await;
        held.sort();
        let mut expected = vec![primary_id, partner_id, created_id];
        expected.sort();
        assert_eq!(
            held, expected,
            "case {case:?}: expected ListInputs to hold {expected:?}, got {held:?}"
        );
    }
}

/// Which create a token was spent on, as a row of the table below names it.
#[derive(Clone, Copy, Debug)]
enum Create {
    /// `CreateInput`: an input that is nobody's partner.
    Standalone,
    /// `CreatePartnerInput` for the primary at this index of the two the test
    /// creates.
    PartnerOf(usize),
}

/// Spend `token` on one create and answer the id of the input it created.
async fn spend(
    router: &MultiServiceRouter,
    case: &str,
    primaries: &[String],
    create: Create,
    token: &str,
) -> String {
    let created = match create {
        Create::Standalone => {
            let mut body = json!({
                "name": "standalone",
                "type": "URL_PULL",
                "sources": [
                    { "url": "https://example.com/a.m3u8" },
                    { "url": "https://example.com/b.m3u8" },
                ],
            });
            body["requestId"] = json!(token);
            created_input(router, case, body).await
        }
        Create::PartnerOf(index) => created_partner(router, case, &primaries[index], token).await,
    };
    created["id"].as_str().unwrap_or_default().to_string()
}

/// Rule: the same `RequestId` presented to the create it was spent on answers
/// with that create's input, and presented to any OTHER create answers with an
/// input of that create's own.
#[tokio::test]
async fn a_replay_is_a_replay_only_of_the_create_that_spent_the_token() {
    // (case, the create that spent the token, the create replaying it, whether
    //  the second answers with the first one's input)
    let table: [(&str, Create, Create, bool); 6] = [
        (
            "one standalone create twice",
            Create::Standalone,
            Create::Standalone,
            true,
        ),
        (
            "a standalone create then a partner create",
            Create::Standalone,
            Create::PartnerOf(0),
            false,
        ),
        (
            "a partner create then a standalone create",
            Create::PartnerOf(0),
            Create::Standalone,
            false,
        ),
        (
            "one partner create of the same input twice",
            Create::PartnerOf(0),
            Create::PartnerOf(0),
            true,
        ),
        (
            "partner creates of two different inputs",
            Create::PartnerOf(0),
            Create::PartnerOf(1),
            false,
        ),
        (
            "partner creates of two different inputs, the other way round",
            Create::PartnerOf(1),
            Create::PartnerOf(0),
            false,
        ),
    ];

    for (case, first, second, same) in table {
        let router = medialive();
        let mut primaries = Vec::new();
        for name in ["primary-a", "primary-b"] {
            let primary = created_input(&router, case, primary_body(name)).await;
            primaries.push(primary["id"].as_str().unwrap_or_default().to_string());
        }

        let token = "shared-token";
        let first_id = spend(&router, case, &primaries, first, token).await;
        let second_id = spend(&router, case, &primaries, second, token).await;

        assert_eq!(
            first_id == second_id,
            same,
            "case {case:?}: expected same_input={same} for {first:?} then {second:?}, \
             got ids {first_id:?} and {second_id:?}"
        );
        for id in [&first_id, &second_id] {
            let (status, described) = describe_input(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: every id a create answered must describe, got {status}: {described}"
            );
        }
        let held = listed_ids(&router, case).await;
        let expected = if same {
            primaries.len() + 1
        } else {
            primaries.len() + 2
        };
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected {expected} inputs to exist, got {held:?}"
        );
    }
}
