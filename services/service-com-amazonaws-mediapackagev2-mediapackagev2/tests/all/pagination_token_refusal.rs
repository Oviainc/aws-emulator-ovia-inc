// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every paged list this service serves: a continuation token the service
//! did not issue is refused with the same modeled 400 — `ValidationException`, the
//! `INVALID_PAGINATION_TOKEN` discriminator, and the crate's single validation sentence
//! pathed at the member the token arrived in — whichever collection is being paged, while
//! a token the same list DID issue resumes it.
//!
//! The table spans the class in both directions: every paged list the model declares
//! (`smithy_query(query="digest")` — the four operations that bind `NextToken` as
//! `httpQuery("nextToken")` and page an `Items` list), crossed with tokens that differ in
//! FORM — prose, non-hex characters, an odd-length hex run, something base64-shaped, and
//! an even-length hex run that decodes to no text at all. The accepted rows are asserted
//! too: a refusal is only a rule if the tokens it does not cover still page.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The hierarchy every row is paged through. Two of each level, so a page of one item
/// always leaves something over and therefore yields a real token.
const GROUP_A: &str = "ptr-grp-a";
const GROUP_B: &str = "ptr-grp-b";
const CHANNEL_1: &str = "ptr-chan-1";
const CHANNEL_2: &str = "ptr-chan-2";
const ENDPOINT_1: &str = "ptr-ep-1";
const ENDPOINT_2: &str = "ptr-ep-2";

/// The whole body a paged list answers an unissued token with: the crate's validation
/// envelope (`Validation error encountered at request.<member>: <rule>`) pathed at the
/// wire name of the continuation member, then the rule about the token.
const EXPECTED_MESSAGE: &str = "Validation error encountered at request.nextToken: You've \
     provided an invalid pagination token in your request. Check your token and then try \
     again";

/// The `Message` member of an error envelope. A handler's modeled `ValidationException`
/// renders `Message`; the request boundary's synthetic shape renders `message`.
fn message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// One paged list: the operation that serves it and the path it is served at.
struct PagedList {
    operation: &'static str,
    path: String,
}

/// Every paged list of this service, over a hierarchy [`seed`] has populated.
fn paged_lists() -> Vec<PagedList> {
    vec![
        PagedList {
            operation: "ListChannelGroups",
            path: "/channelGroup".to_string(),
        },
        PagedList {
            operation: "ListChannels",
            path: format!("/channelGroup/{GROUP_A}/channel"),
        },
        PagedList {
            operation: "ListOriginEndpoints",
            path: format!("/channelGroup/{GROUP_A}/channel/{CHANNEL_1}/originEndpoint"),
        },
        PagedList {
            operation: "ListHarvestJobs",
            path: format!("/channelGroup/{GROUP_A}/harvestJob"),
        },
    ]
}

/// Build two channel groups, two channels, two origin endpoints and two harvest jobs, so
/// every list in [`paged_lists`] walks at least two items.
async fn seed() -> Emulator {
    let emulator = Emulator::new();
    for group in [GROUP_A, GROUP_B] {
        let (status, body) = emulator.create_group(group, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannelGroup {group} expected 200, got {status} {body}"
        );
    }
    for channel in [CHANNEL_1, CHANNEL_2] {
        let (status, body) = emulator.create_channel(GROUP_A, channel, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannel {channel} expected 200, got {status} {body}"
        );
    }
    for endpoint in [ENDPOINT_1, ENDPOINT_2] {
        let (status, body) = emulator
            .create_endpoint(GROUP_A, CHANNEL_1, endpoint, minimal_endpoint("TS"))
            .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateOriginEndpoint {endpoint} expected 200, got {status} {body}"
        );
    }
    let start = epoch_seconds_from_now(600.0);
    for job in ["ptr-job-1", "ptr-job-2"] {
        let mut request = harvest_body(start, start + 3600.0, &["index"]);
        request["HarvestJobName"] = json!(job);
        let (status, body) = emulator
            .call(
                "POST",
                &format!(
                    "/channelGroup/{GROUP_A}/channel/{CHANNEL_1}\
                     /originEndpoint/{ENDPOINT_1}/harvestJob"
                ),
                Some(request),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateHarvestJob {job} expected 200, got {status} {body}"
        );
    }
    emulator
}

#[tokio::test]
async fn every_paged_list_refuses_a_token_it_did_not_issue_in_one_form() {
    let emulator = seed().await;
    let lists = paged_lists();

    // Tokens no list issued, differing in form: prose, characters a token never holds, an
    // odd-length hex run, a base64-shaped string, and an even-length hex run that decodes
    // to no text.
    for token in [
        "not-a-real-pagination-token",
        "zzzz",
        "abc",
        "eyJrZXkiOiAidmFsdWUifQ==",
        "ff",
    ] {
        for list in &lists {
            let operation = list.operation;
            let case = format!("{operation} with token {token:?}");
            let (status, body) = emulator
                .call("GET", &format!("{}?nextToken={token}", list.path), None)
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                validation_type(&body),
                "INVALID_PAGINATION_TOKEN",
                "case {case:?}: expected ValidationExceptionType INVALID_PAGINATION_TOKEN, \
                 got {body}"
            );
            assert_eq!(
                message(&body),
                EXPECTED_MESSAGE,
                "case {case:?}: every paged list words this refusal identically, got {:?}",
                message(&body)
            );
            assert!(
                !message(&body).contains(token),
                "case {case:?}: the message states the rule, not the caller's token, got {:?}",
                message(&body)
            );
        }
    }
}

#[tokio::test]
async fn every_paged_list_resumes_from_a_token_it_did_issue() {
    let emulator = seed().await;

    for list in &paged_lists() {
        let operation = list.operation;
        let (status, first) = emulator
            .call("GET", &format!("{}?maxResults=1", list.path), None)
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a first page of one expected 200, got {status} {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {operation:?}: a collection of two items must leave a token after a page \
             of one, got {first}"
        );
        let (status, second) = emulator
            .call(
                "GET",
                &format!("{}?maxResults=1&nextToken={token}", list.path),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the token this list issued must page it, got {status} {second}"
        );
        let first_items = first["Items"].as_array().cloned().unwrap_or_default();
        let second_items = second["Items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            (first_items.len(), second_items.len()),
            (1, 1),
            "case {operation:?}: two pages of one expected, got {first} then {second}"
        );
        assert_ne!(
            first_items[0], second_items[0],
            "case {operation:?}: the resumed page must start beyond the first, got \
             {first} then {second}"
        );
    }
}
