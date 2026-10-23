// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListRecordingConfigurations`, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! dispatch harness itself (`Ivs`, `text`, `hex_of`) is the one
//! [`crate::channel_lifecycle`] already owns, so the two suites cannot drift apart
//! in how they talk to the emulator.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The list answers every persisted recording configuration, in ONE stable order
//!    (ARN), cut into pages of at most `maxResults`, with a `nextToken` that resumes
//!    exactly where the previous page stopped.
//! 2. A `nextToken` this list did not issue is refused with a 400
//!    `ValidationException`, never honoured as another list's boundary and never
//!    answered as a silently truncated page.
//! 3. A listed entry is the `RecordingConfigurationSummary` projection of the stored
//!    resource: the members that shape declares, read off state, and none of the
//!    detail members only `GetRecordingConfiguration` answers.
//! 4. Listing is a report: it answers the same state a read answers (including the
//!    `CREATING` → `ACTIVE` move the clock owes) and repeating it changes nothing.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
    RECORDING_CONFIGURATION_ACTIVATION, RECORDING_CONFIGURATION_ACTIVE,
    RECORDING_CONFIGURATION_CREATING,
};

use crate::channel_lifecycle::{hex_of, text, Ivs};

/// The ARN prefix every recording configuration this emulator mints carries, from
/// the modelled `RecordingConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:recording-configuration/[a-zA-Z0-9-]+$` plus the
/// canonical emulator account id.
const ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:recording-configuration/";

/// Create a recording configuration whose destination names `bucket`, and return the
/// resource the create answered.
async fn create_in(ivs: &Ivs, bucket: &str) -> Value {
    ivs.ok(
        "CreateRecordingConfiguration",
        json!({ "destinationConfiguration": { "s3": { "bucketName": bucket } } }),
    )
    .await["recordingConfiguration"]
        .clone()
}

/// `GetRecordingConfiguration` for `arn`.
async fn get(ivs: &Ivs, arn: &str) -> Value {
    ivs.ok("GetRecordingConfiguration", json!({ "arn": arn }))
        .await["recordingConfiguration"]
        .clone()
}

/// Every persisted configuration, walked page by page through the `nextToken` the
/// list issues.
pub(crate) async fn list_all(ivs: &Ivs) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token = Value::Null;
    loop {
        let mut request = json!({});
        if let Some(next) = token.as_str() {
            request["nextToken"] = json!(next);
        }
        let page = ivs.ok("ListRecordingConfigurations", request).await;
        let listed = page["recordingConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        all.extend(listed);
        token = page.get("nextToken").cloned().unwrap_or(Value::Null);
        if token.as_str().is_none() {
            break;
        }
    }
    all
}

/// The listed summary of `arn`, or `None` when the list does not answer it.
async fn listed(ivs: &Ivs, arn: &str) -> Option<Value> {
    list_all(ivs)
        .await
        .into_iter()
        .find(|summary| text(summary, "arn") == arn)
}

/// Rule 1: every page size, including both published bounds of `maxResults` and a
/// size past the number of configurations, returns the WHOLE set exactly once and in
/// one order; a size outside the modelled 1..100 range is refused.
#[tokio::test]
async fn list_recording_configurations_pages_the_whole_set_without_duplicates_or_omissions() {
    let ivs = Ivs::new();
    let mut all = BTreeSet::new();
    for index in 0..7 {
        let created = create_in(&ivs, &format!("recordings-{index}")).await;
        all.insert(text(&created, "arn"));
    }

    // Rows: the page size, spanning 1 (the modelled minimum), sizes that divide the
    // set unevenly, the set's own size, and 100 (the modelled maximum).
    for page_size in [1, 2, 3, 7, 100] {
        let mut seen = Vec::new();
        let mut token = Value::Null;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(next) = token.as_str() {
                request["nextToken"] = json!(next);
            }
            let page = ivs.ok("ListRecordingConfigurations", request).await;
            let listed = page["recordingConfigurations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                listed.len() <= page_size,
                "case maxResults {page_size}: a page holds at most maxResults entries, got {}",
                listed.len()
            );
            seen.extend(listed.iter().map(|summary| text(summary, "arn")));
            token = page.get("nextToken").cloned().unwrap_or(Value::Null);
            if token.as_str().is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            all.len(),
            "case maxResults {page_size}: expected each of the {} configurations exactly once, \
             got {seen:?}",
            all.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            all,
            "case maxResults {page_size}: the pages together are the whole set"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case maxResults {page_size}: the pages come in one stable (ARN) order, got {seen:?}"
        );
    }

    // One past each published bound is refused by the modelled range, before any
    // handler runs.
    for max_results in [0, 101] {
        let reply = ivs
            .call(
                "ListRecordingConfigurations",
                &json!({ "maxResults": max_results }),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults {max_results}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 2: a `nextToken` this list did not issue is a 400 `ValidationException`,
/// never a silently truncated or empty page.
#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let ivs = Ivs::new();
    // A token the CHANNEL list issues: valid hex, valid codec, wrong list.
    for index in 0..3 {
        ivs.ok("CreateChannel", json!({ "name": format!("channel-{index}") }))
            .await;
    }
    let channel_token = text(
        &ivs.ok("ListChannels", json!({ "maxResults": 1 })).await,
        "nextToken",
    );
    assert!(
        !channel_token.is_empty(),
        "the channel list must issue a token for this case to be meaningful"
    );
    for index in 0..3 {
        create_in(&ivs, &format!("tokens-{index}")).await;
    }

    let forged: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        (
            "hex of a plain ARN, unstamped",
            hex_of(&format!("{ARN_PREFIX}abc")),
        ),
        (
            "the right stamp, a boundary that is no ARN",
            hex_of("ivs:1:recording-configuration:nothing"),
        ),
        (
            "another list's stamp around this list's boundary",
            hex_of(&format!("ivs:1:channel:{ARN_PREFIX}abc")),
        ),
        ("a token the channel list issued", channel_token),
    ];

    for (case, token) in forged {
        let reply = ivs
            .call(
                "ListRecordingConfigurations",
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case forged nextToken ({case}): expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // An EMPTY token is not a forged one: it names no boundary, so it is the first
    // page.
    let page = ivs
        .ok("ListRecordingConfigurations", json!({ "nextToken": "" }))
        .await;
    assert_eq!(
        page["recordingConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        3,
        "an empty nextToken is the first page, got {page}"
    );
}

/// Rules 3 and 4: each listed entry is the summary projection of the stored
/// resource, it reports whatever state a read of the same resource reports — before
/// and after the clock's owed transition — and repeating the list answers the same
/// thing.
#[tokio::test]
async fn a_listed_entry_is_the_summary_projection_of_the_stored_resource() {
    // Rows differ in which members were sent, so the projection is exercised over
    // present and absent names, tags and nested configurations.
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required destination",
            json!({ "destinationConfiguration": { "s3": { "bucketName": "projection-one" } } }),
        ),
        (
            "named, tagged, with both nested configurations",
            json!({
                "name": "projection_two",
                "destinationConfiguration": { "s3": { "bucketName": "projection.two" } },
                "recordingReconnectWindowSeconds": 42,
                "tags": { "env": "test", "team": "video" },
                "thumbnailConfiguration": { "recordingMode": "INTERVAL", "targetIntervalSeconds": 1 },
                "renditionConfiguration": { "renditionSelection": "CUSTOM", "renditions": ["HD"] },
            }),
        ),
        (
            "thumbnails disabled and no renditions recorded",
            json!({
                "name": "projection-three",
                "destinationConfiguration": { "s3": { "bucketName": "projection-three" } },
                "thumbnailConfiguration": { "recordingMode": "DISABLED" },
                "renditionConfiguration": { "renditionSelection": "NONE" },
            }),
        ),
    ];

    let ivs = Ivs::new();
    let mut arns = Vec::new();
    for (case, request) in &cases {
        let created = ivs.ok("CreateRecordingConfiguration", request.clone()).await
            ["recordingConfiguration"]
            .clone();
        let arn = text(&created, "arn");
        let summary = listed(&ivs, &arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created configuration must be listed"));

        // The members `RecordingConfigurationSummary` declares, read off state.
        for member in ["arn", "name", "destinationConfiguration", "tags"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                created.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: the listed {member} must be the stored value, got {summary}"
            );
        }
        // `state` is the one member the clock may have advanced between the create
        // and this list, so what the rule fixes is that the list reports the same
        // state the read reports, and that it is one the model declares.
        let listed_state = text(&summary, "state");
        assert!(
            listed_state == RECORDING_CONFIGURATION_CREATING
                || listed_state == RECORDING_CONFIGURATION_ACTIVE,
            "case {case:?}: the listed state is one the model declares, got {listed_state:?}"
        );
        assert_eq!(
            listed_state,
            text(&get(&ivs, &arn).await, "state"),
            "case {case:?}: the list and the read report one state"
        );
        // And none of the detail members, which only the read answers.
        for member in [
            "thumbnailConfiguration",
            "renditionConfiguration",
            "recordingReconnectWindowSeconds",
        ] {
            assert!(
                summary.get(member).is_none(),
                "case {case:?}: {member} is not a RecordingConfigurationSummary member, \
                 got {summary}"
            );
        }
        arns.push((*case, arn));
    }

    tokio::time::sleep(RECORDING_CONFIGURATION_ACTIVATION + Duration::from_millis(250)).await;

    for (case, arn) in &arns {
        // Rule 4: the list reports the clock's owed move, agrees with the read, and
        // repeating either answers the same thing.
        for round in 1..=3 {
            let summary = listed(&ivs, arn)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: it must still be listed"));
            assert_eq!(
                text(&summary, "state"),
                RECORDING_CONFIGURATION_ACTIVE,
                "case {case:?} round {round}: expected ACTIVE once the activation span was due, \
                 got {summary}"
            );
            assert_eq!(
                text(&summary, "state"),
                text(&get(&ivs, arn).await, "state"),
                "case {case:?} round {round}: the list and the read report one state"
            );
        }
    }
}

/// An empty store lists as an empty page with no `nextToken` — the modelled
/// `recordingConfigurations` member is required, so it is present and empty rather
/// than absent.
#[tokio::test]
async fn an_empty_store_lists_as_an_empty_page() {
    let ivs = Ivs::new();
    // A channel exists, so the answer is empty because no RECORDING CONFIGURATION
    // was created, not because the store was never touched.
    ivs.ok("CreateChannel", json!({ "name": "unrelated" })).await;

    for request in [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": 100 }),
    ] {
        let page = ivs
            .ok("ListRecordingConfigurations", request.clone())
            .await;
        assert_eq!(
            page["recordingConfigurations"],
            json!([]),
            "case {request}: expected an empty list, got {page}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {request}: an answer with no further page carries no nextToken, got {page}"
        );
    }
}
