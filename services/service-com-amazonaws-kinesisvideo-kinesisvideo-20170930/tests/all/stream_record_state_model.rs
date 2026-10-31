// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for two rules of the stream RECORD's state model.
//!
//! RULE 1 (the delete path): a resource whose creation the service has acknowledged is
//! addressable by every call that does not itself need the provisioning to have
//! finished, and `DeleteStream` only "marks the stream for deletion", so the delete a
//! caller sends to the `StreamARN` its create returned is SERVED from that moment,
//! inside the creation window as much as after it; the in-use refusal is reserved for a
//! removal that is already under way.
//!
//! RULE 2 (the `Version` token): `Version` (`@length` 1..64, `@pattern
//! ^[a-zA-Z0-9]+$`) is an opaque per-record identifier whose only job is to compare
//! equal to the one the caller read, so each record's token is drawn rather than
//! composed: it carries no store-wide ordering, no mint-time clock, and nothing two
//! records minted in one run can share.
//!
//! Every case drives the real request walk (route -> deserialize -> validate -> handle
//! -> serialize) through [`MultiServiceRouter::dispatch`], so a response here is the
//! same bytes a client on the socket would see. The harness and the request helpers are
//! the ones the sibling `stream_data_retention` suite owns, and the clock waits are
//! `stream_lifecycle`'s — both are modules of this crate's single `tests/all.rs`
//! target, so they are shared rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::stream_data_retention::{call, error_code, harness};
use crate::stream_lifecycle::{wait_for_activation, wait_for_deletion};

/// The `Message` of a failure body.
fn error_message(body: &serde_json::Value) -> String {
    body.get("Message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `DescribeStream` for `name`, as (status, `StreamInfo`-or-body).
async fn describe(router: &MultiServiceRouter, name: &str) -> (u16, serde_json::Value) {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    let projected = body.get("StreamInfo").cloned().unwrap_or(body);
    (status, projected)
}

/// One member of a `StreamInfo`/`ChannelInfo` projection, as a string.
fn text(info: &serde_json::Value, member: &str) -> String {
    info.get(member)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("no {member} in {info:?}"))
        .to_string()
}

/// How a row's delete quotes the stream's version.
#[derive(Debug, Clone, Copy)]
enum Guard {
    /// No `CurrentVersion` at all — an unconditional delete.
    Absent,
    /// The version the create's own record carries.
    Stored,
}

/// A stream `CreateStream` has acknowledged is deletable straight away, with no wait:
/// the delete answers HTTP 200 with the empty `DeleteStreamOutput` body, the record
/// moves to `DELETING` keeping every other member, the activation the clock still owed
/// never undoes that mark, and the removal falls due on the deletion span.
///
/// The rows differ along what the rule generalises over: which optional members the
/// stream carries and the size of its retention, how the delete quotes the version
/// (not at all, or the stored token), and how many calls the client makes between the
/// create and the delete (none, one read, several reads) — the acceptance is a function
/// of the request sequence, not of how much wall-clock time the client spent.
#[tokio::test]
async fn a_stream_the_create_acknowledged_is_deletable_inside_its_creation_window() {
    let router = harness();

    // (case, the create request, the version guard, how many reads intervene)
    let rows: Vec<(&str, serde_json::Value, Guard, usize)> = vec![
        (
            "only the required name, deleted with no intervening call",
            serde_json::json!({ "StreamName": "window-bare" }),
            Guard::Absent,
            0,
        ),
        (
            "device and media type, one read in between",
            serde_json::json!({
                "StreamName": "window.read", "DeviceName": "device_01",
                "MediaType": "video/h264"
            }),
            Guard::Absent,
            1,
        ),
        (
            "the stored version quoted, after several reads",
            serde_json::json!({
                "StreamName": "window-guarded", "DataRetentionInHours": 25,
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234"
            }),
            Guard::Stored,
            4,
        ),
        (
            "every optional member, retention at the published maximum",
            serde_json::json!({
                "StreamName": "window-full", "DeviceName": "d",
                "MediaType": "video/h264,audio/aac",
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/ffff-9999",
                "DataRetentionInHours": 87_600
            }),
            Guard::Stored,
            2,
        ),
    ];

    for (case, request, guard, reads) in &rows {
        let name = text(request, "StreamName");

        let (status, body) = call(&router, "/createStream", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStream: expected HTTP 200, got {status} with {body:?}"
        );
        let arn = text(&body, "StreamARN");

        // The record as it stands inside the creation window, plus whatever reads this
        // row makes before deleting.
        let (status, created) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStream: expected HTTP 200, got {status} with {created:?}"
        );
        assert_eq!(
            created.get("Status").and_then(serde_json::Value::as_str),
            Some("CREATING"),
            "case {case:?}: a stream inside its creation window reports CREATING, got \
             {created:?}"
        );
        for _ in 0..*reads {
            let (status, polled) = describe(&router, &name).await;
            assert_eq!(
                (status, &polled),
                (200, &created),
                "case {case:?}: a read must change nothing, expected {created:?}, got {polled:?}"
            );
        }

        let mut delete = serde_json::json!({ "StreamARN": arn.clone() });
        if matches!(guard, Guard::Stored) {
            delete["CurrentVersion"] = serde_json::json!(text(&created, "Version"));
        }
        let (status, body) = call(&router, "/deleteStream", delete).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStream of a stream the create acknowledged: expected \
             HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            body,
            serde_json::json!({}),
            "case {case:?}: DeleteStreamOutput has no members, so the body is the empty \
             object, got {body:?}"
        );

        // The accepted delete MARKED the record: only `Status` moved.
        let mut expected = created.clone();
        expected["Status"] = serde_json::json!("DELETING");
        let (status, after) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a marked stream stays describable, got {status} with {after:?}"
        );
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?}, got {after:?}"
        );
    }

    // The activation the clock owed each of these records falls due INSIDE the deletion
    // span: a pending promotion must not pull a marked stream back to `ACTIVE`.
    wait_for_activation();
    for (case, request, _, _) in &rows {
        let name = text(request, "StreamName");
        let (status, after) = describe(&router, &name).await;
        assert_eq!(
            (status, after.get("Status").and_then(serde_json::Value::as_str)),
            (200, Some("DELETING")),
            "case {case:?}: a marked stream stays DELETING once its activation falls due, \
             got {status} with {after:?}"
        );

        // Still the in-use refusal, since the removal is already under way.
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": text(&after, "StreamARN") }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (
                400,
                "ResourceInUseException",
                "The requested stream is not active."
            ),
            "case {case:?}: a second delete: got {status} with {body:?}"
        );
    }

    // And the removal the first delete owed does fall due.
    wait_for_deletion();
    for (case, request, _, _) in &rows {
        let name = text(request, "StreamName");
        let (status, body) = describe(&router, &name).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a removed stream must not describe, got {status} with {body:?}"
        );
    }
}

/// The version guard is still reached inside the creation window: a `CurrentVersion`
/// that is not the stored token refuses the delete (HTTP 400,
/// `VersionMismatchException`) and leaves the record exactly as it was, and the stored
/// token then deletes it.
///
/// The rows are stale tokens of several shapes, each inside `Version`'s modeled
/// `@length` 1..64 and `^[a-zA-Z0-9]+$` pattern, so the boundary validator passes them
/// through to the handler.
#[tokio::test]
async fn a_delete_inside_the_creation_window_still_honours_the_version_guard() {
    let router = harness();

    let long = "z".repeat(64);
    let rows = ["bogus1", "A", "0", long.as_str()];

    for (index, case) in rows.iter().enumerate() {
        let name = format!("window-guard-{index}");
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStream: expected HTTP 200, got {status} with {body:?}"
        );
        let arn = text(&body, "StreamARN");

        let (_, created) = describe(&router, &name).await;
        let stored = text(&created, "Version");
        assert_ne!(
            &stored.as_str(),
            case,
            "case {case:?}: the row must differ from the stored token"
        );

        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn.clone(), "CurrentVersion": case }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "VersionMismatchException"),
            "case {case:?}: a stale CurrentVersion: got {status} with {body:?}"
        );
        let (status, after) = describe(&router, &name).await;
        assert_eq!(
            (status, &after),
            (200, &created),
            "case {case:?}: a refused delete must leave the record as it was, expected \
             {created:?}, got {after:?}"
        );

        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn, "CurrentVersion": stored }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored token must delete, got {status} with {body:?}"
        );
    }
}

/// Every `Version` token this service mints — for a stream or a signaling channel, at
/// creation or on a later mutation — is an opaque per-record identifier.
///
/// The invariants asserted over the whole collected set, rather than over one literal
/// token: the modeled charset and length, one fixed width, pairwise distinctness,
/// pairwise difference across most of the token (so no two share a run-local prefix or
/// suffix, and no token is one edit away from another's), the full alphanumeric
/// alphabet in use rather than a lowercase subset, and no mint-order ordering a caller
/// could read. The round trip is asserted too: each token works as the
/// `CurrentVersion` the record's own mutation requires, and that mutation re-mints it.
#[tokio::test]
async fn minted_version_tokens_are_opaque_per_record_identifiers() {
    /// `Version`'s modeled `@length` bounds, from
    /// `smithy_query(query="shape", shape="Version")`.
    const MIN_LENGTH: usize = 1;
    /// See [`MIN_LENGTH`].
    const MAX_LENGTH: usize = 64;

    let router = harness();

    // (what minted it, the token). Six streams and four channels, each sampled at
    // creation and again after a mutation that re-mints the token: twenty tokens drawn
    // in one store's lifetime, which is exactly where a composed token would show its
    // shared prefix or suffix.
    let mut minted: Vec<(String, String)> = Vec::new();

    for index in 0..6_i32 {
        let name = format!("token-stream-{index}");
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": name.clone(), "DataRetentionInHours": index + 1 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateStream {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
    }
    let mut channel_arns: Vec<(String, String)> = Vec::new();
    for index in 0..4_i32 {
        let name = format!("token-channel-{index}");
        let (status, body) = call(
            &router,
            "/createSignalingChannel",
            serde_json::json!({ "ChannelName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateSignalingChannel {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        channel_arns.push((name, text(&body, "ChannelARN")));
    }
    wait_for_activation();

    for index in 0..6_i32 {
        let name = format!("token-stream-{index}");
        let (_, info) = describe(&router, &name).await;
        let created = text(&info, "Version");
        minted.push((format!("{name} at creation"), created.clone()));

        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": name.clone(),
                "CurrentVersion": created.clone(),
                "Operation": "INCREASE_DATA_RETENTION",
                "DataRetentionChangeInHours": index + 1
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the reported token must work as CurrentVersion, got {status} \
             with {body:?}"
        );
        let (_, info) = describe(&router, &name).await;
        let reminted = text(&info, "Version");
        assert_ne!(
            reminted, created,
            "case {name:?}: a mutation must re-mint the token, still {reminted:?}"
        );
        minted.push((format!("{name} after its update"), reminted));
    }

    for (index, (name, arn)) in channel_arns.iter().enumerate() {
        let (status, body) = call(
            &router,
            "/describeSignalingChannel",
            serde_json::json!({ "ChannelName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeSignalingChannel {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let info = body
            .get("ChannelInfo")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: no ChannelInfo in {body:?}"));
        let created = text(&info, "Version");
        minted.push((format!("{name} at creation"), created.clone()));

        // `MessageTtlSeconds` stays inside its modeled `@range` 5..120.
        let ttl = 30 + i64::try_from(index).unwrap_or(0);
        let (status, body) = call(
            &router,
            "/updateSignalingChannel",
            serde_json::json!({
                "ChannelARN": arn.clone(),
                "CurrentVersion": created.clone(),
                "SingleMasterConfiguration": { "MessageTtlSeconds": ttl }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the reported token must work as CurrentVersion, got {status} \
             with {body:?}"
        );
        let (status, body) = call(
            &router,
            "/describeSignalingChannel",
            serde_json::json!({ "ChannelName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeSignalingChannel {name:?} after its update: got {status} with {body:?}"
        );
        let reminted = text(
            &body
                .get("ChannelInfo")
                .cloned()
                .unwrap_or_else(|| panic!("case {name:?}: no ChannelInfo in {body:?}")),
            "Version",
        );
        assert_ne!(
            reminted, created,
            "case {name:?}: a mutation must re-mint the token, still {reminted:?}"
        );
        minted.push((format!("{name} after its update"), reminted));
    }

    // The modeled constraints, token by token.
    let width = minted[0].1.chars().count();
    for (case, token) in &minted {
        assert!(
            token.chars().all(|symbol| symbol.is_ascii_alphanumeric()),
            "case {case:?}: a token must satisfy ^[a-zA-Z0-9]+$, got {token:?}"
        );
        let length = token.chars().count();
        assert!(
            (MIN_LENGTH..=MAX_LENGTH).contains(&length),
            "case {case:?}: expected a length within {MIN_LENGTH}..={MAX_LENGTH}, got \
             {length} in {token:?}"
        );
        assert_eq!(
            length, width,
            "case {case:?}: every token carries one fixed width, expected {width}, got \
             {length} in {token:?}"
        );
    }

    // Opaqueness, over the pairs: two tokens of one run must not be variations of one
    // another. Half the width is a floor no composed token clears — a shared mint-time
    // clock or a shared counter prefix leaves the two agreeing almost everywhere —
    // while two drawn tokens of this width agree in about one position out of sixty.
    for (left_index, (left_case, left)) in minted.iter().enumerate() {
        for (right_case, right) in minted.iter().skip(left_index + 1) {
            assert_ne!(
                left, right,
                "case {left_case:?} vs {right_case:?}: tokens must be distinct, both {left:?}"
            );
            let shared = left
                .chars()
                .zip(right.chars())
                .filter(|(one, other)| one == other)
                .count();
            assert!(
                shared <= width / 2,
                "case {left_case:?} vs {right_case:?}: two tokens must differ across most \
                 of their width, but {left:?} and {right:?} agree in {shared} of {width} \
                 positions"
            );
        }
    }

    // The alphabet is the whole alphanumeric set the pattern admits, not a lowercase
    // base-N subset: over this many drawn symbols each class appears.
    let symbols: String = minted.iter().map(|(_, token)| token.as_str()).collect();
    for (class, present) in [
        ("an upper-case letter", symbols.chars().any(char::is_uppercase)),
        ("a lower-case letter", symbols.chars().any(char::is_lowercase)),
        (
            "a digit",
            symbols.chars().any(|symbol| symbol.is_ascii_digit()),
        ),
    ] {
        assert!(
            present,
            "the minted tokens must draw on the whole alphanumeric alphabet, but \
             {class} appears in none of {minted:?}"
        );
    }

    // And nothing a caller can order: the tokens do not come out in mint order, so no
    // client can read the store's mutation sequence out of them or guess the next.
    let ordered: Vec<&String> = minted.iter().map(|(_, token)| token).collect();
    let mut sorted = ordered.clone();
    sorted.sort();
    assert_ne!(
        ordered, sorted,
        "the mint order must not be a readable ordering of the tokens, got {minted:?}"
    );
}
