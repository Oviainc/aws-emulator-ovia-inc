// SPDX-License-Identifier: CC0-1.0

//! What the EBS direct APIs put in a response BODY: which members a success carries, and
//! how a refusal is worded.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production
//! router uses, over the harness `snapshot_block_loop` owns, and asserts a RULE over a
//! table of inputs rather than one recorded request:
//!
//! 1. An optional response member the stored snapshot does not have is ABSENT, never a
//!    placeholder (`SseType`, `Tags`).
//! 2. `CompleteSnapshot` is an acknowledgement: its 202 reports the snapshot `pending`,
//!    the snapshot is sealed from that instant, and `completed` is observed afterwards.
//! 3. A `blockToken` is judged against the expression the SERVICE enforces, and the
//!    refusal reports that expression.
//! 4. A checksum algorithm the service does not support is refused through its own
//!    modelled `ValidationException`, carrying both `Message` and `Reason`.
//! 5. A `ClientToken` conflict carries the service's fixed message and echoes nothing.
//! 6. A `VolumeSize` the service will not accept is refused with its request-derived
//!    template, naming only the rejected value.
//! 7. Every "that snapshot is not stored" refusal uses ONE template, in every operation
//!    that resolves a snapshot id.

use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, complete_snapshot_with_algorithm, get_block,
    list_blocks, listed_blocks, put_block, put_block_with_algorithm, put_block_with_progress,
    router, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// The expression the service enforces on a `blockToken`, which is also the one its
/// refusal reports. Mirrors `crate::state::BLOCK_TOKEN_PATTERN`; a test that quoted a
/// different expression would assert nothing about the refusal a caller reads.
const BLOCK_TOKEN_PATTERN: &str = "^[A-Za-z0-9+/=_-]+$";

/// The only checksum algorithm the modelled `ChecksumAlgorithm` enum declares
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ChecksumAlgorithm")`).
const SUPPORTED_ALGORITHM: &str = "SHA256";

/// The widths of the hex suffix of a snapshot id the service accepts, mirroring
/// `crate::state::SNAPSHOT_ID_HEX_WIDTHS`.
const SNAPSHOT_ID_HEX_WIDTHS: [usize; 2] = [8, 17];

/// A member of a modelled error body (`Message`, `Reason`), or the empty string when the
/// body does not carry it.
fn member(reply: &Reply, name: &str) -> String {
    reply
        .json()
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// How long a test waits for a transition the clock owes. Generously longer than
/// `crate::state::SNAPSHOT_COMPLETION_DELAY` so a loaded machine cannot fail the test,
/// while a transition that never happens still fails it.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(5);

/// A snapshot id a request may name that no `StartSnapshot` ever minted, WELL-FORMED so
/// the refusal is about the snapshot and not about the id's shape: `snap-` plus 8 or 17
/// lowercase hex digits, the widths `crate::state::SNAPSHOT_ID_HEX_WIDTHS` names. Each
/// row's width is asserted in the test, so a miscounted literal fails loudly instead of
/// quietly testing the other rule.
const UNSTORED_IDS: &[&str] = &[
    "snap-deadbeef",
    "snap-0123456789abcdef0",
    "snap-00000000000000001",
];

/// RULE 1: an optional member of the response is populated only when the stored snapshot
/// HAS it; otherwise it is absent from the body. `SseType` and `Tags` are optional with
/// no `@default`, so an unencrypted snapshot reports no SSE type and an untagged one
/// reports no tag list — a placeholder (`"none"`, `[]`) would make a presence-based
/// client read "the service reported this" where the service reported nothing.
#[tokio::test]
async fn optional_response_members_are_absent_when_the_snapshot_has_none() {
    let router = router();

    // Rows differ along both members the rule covers: whether the snapshot is encrypted
    // (and how), and how many tags it carries.
    let cases: &[(&str, serde_json::Value, Option<&str>, usize)] = &[
        (
            "unencrypted and untagged",
            serde_json::json!({ "VolumeSize": 1 }),
            None,
            0,
        ),
        (
            "unencrypted with one tag",
            serde_json::json!({
                "VolumeSize": 2,
                "Tags": [{ "Key": "only", "Value": "one" }],
            }),
            None,
            1,
        ),
        (
            "unencrypted with two tags",
            serde_json::json!({
                "VolumeSize": 3,
                "Tags": [
                    { "Key": "a", "Value": "1" },
                    { "Key": "b", "Value": "2" },
                ],
            }),
            None,
            2,
        ),
        (
            "EBS-managed encryption, untagged",
            serde_json::json!({ "VolumeSize": 4, "Encrypted": true }),
            Some("sse-ebs"),
            0,
        ),
        (
            "KMS encryption with a tag",
            serde_json::json!({
                "VolumeSize": 5,
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "Tags": [{ "Key": "kms", "Value": "yes" }],
            }),
            Some("sse-kms"),
            1,
        ),
    ];

    for (case, request, expected_sse, tag_count) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json();

        match expected_sse {
            Some(sse) => assert_eq!(
                body.get("SseType").and_then(serde_json::Value::as_str),
                Some(*sse),
                "case {case:?}: an encrypted snapshot must report its SseType, got {body}"
            ),
            None => assert!(
                body.get("SseType").is_none(),
                "case {case:?}: a snapshot with no SSE type must omit SseType, got {body}"
            ),
        }

        let reported_tags = body.get("Tags").and_then(serde_json::Value::as_array);
        if *tag_count == 0 {
            assert!(
                body.get("Tags").is_none(),
                "case {case:?}: an untagged snapshot must omit Tags, got {body}"
            );
        } else {
            assert_eq!(
                reported_tags.map(Vec::len),
                Some(*tag_count),
                "case {case:?}: the stored tags must be reported, got {body}"
            );
        }
    }
}

/// RULE 2: `CompleteSnapshot` ACKNOWLEDGES the seal. Its 202 reports the snapshot as
/// `pending`, the snapshot accepts no further block from that instant, and the terminal
/// `completed` becomes observable afterwards — so a client that waits for
/// pending -> completed has something to wait for.
#[tokio::test]
async fn completing_a_snapshot_is_acknowledged_pending_and_completes_from_the_clock() {
    let router = router();

    // Rows differ along how many blocks were written before the seal, including none.
    let cases: &[(&str, &[(i32, u8)])] = &[
        ("no blocks written", &[]),
        ("one block written", &[(0, 21)]),
        ("three blocks written", &[(0, 22), (1, 23), (7, 24)]),
    ];

    for (case, blocks) in cases {
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = member(&started, "SnapshotId");
        assert!(
            !snapshot_id.is_empty(),
            "case {case:?}: StartSnapshot must report a SnapshotId, got {}",
            started.json()
        );
        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &snapshot_id, *block_index, data, BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: PutSnapshotBlock at {block_index} must answer 201, got {} with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        let count = i32::try_from(blocks.len()).expect("a small block count");
        let accepted = complete_snapshot(&router, &snapshot_id, count).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        assert_eq!(
            member(&accepted, "Status"),
            "pending",
            "case {case:?}: the acknowledgement must report the snapshot as pending, got {}",
            accepted.json()
        );

        // Sealed from the instant the completion was accepted: "You cannot write new
        // blocks to a snapshot after it has been completed".
        let data = block_bytes(99);
        let refused = put_block(
            &router,
            &snapshot_id,
            1_000,
            data.clone(),
            BLOCK_SIZE,
            &checksum(&data),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a write to a sealed snapshot must be refused, got {} with body {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            refused.error_code()
        );

        // The transition the clock owes becomes observable, without this test knowing
        // how it is implemented: it reads the status back through the API. `tokio`'s
        // `time` feature is not enabled in this workspace, so the wait is a plain
        // thread sleep between requests.
        let deadline = Instant::now() + TRANSITION_DEADLINE;
        let mut observed = String::new();
        while Instant::now() < deadline {
            let read_back = complete_snapshot(&router, &snapshot_id, count).await;
            observed = member(&read_back, "Status");
            if observed == "completed" {
                break;
            }
            sleep(Duration::from_millis(25));
        }
        assert_eq!(
            observed, "completed",
            "case {case:?}: the snapshot must reach the completed status, last observed {observed:?}"
        );

        // A completed snapshot still serves everything that was written into it.
        let listed = listed_blocks(&list_blocks(&router, &snapshot_id, "").await);
        let indexes: Vec<i32> = listed.iter().map(|(index, _)| *index).collect();
        let expected_indexes: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            indexes, expected_indexes,
            "case {case:?}: a completed snapshot must still list the blocks written, got {indexes:?}"
        );
    }
}

/// RULE 3: a `blockToken` is judged against the expression the SERVICE enforces —
/// `^[A-Za-z0-9+/=_-]+$`, the URL-safe base64 alphabet — and a value outside it is
/// refused with the framework's constraint report naming the member and THAT expression.
/// A value inside it is never a constraint violation: it goes on to be resolved, and
/// fails (or succeeds) on whether it names the block.
#[tokio::test]
async fn block_token_is_judged_against_the_pattern_the_service_enforces() {
    let router = router();
    let snapshot_id = write_snapshot(&router, "block-token", 1, &[(0, 31)]).await;
    let real_token = listed_blocks(&list_blocks(&router, &snapshot_id, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the snapshot lists its block");

    // Rows OUTSIDE the enforced expression, differing along which character or bound
    // puts them there. The requirement each clause must state is the third column.
    let over_long = "A".repeat(257);
    let refused: &[(&str, String, String)] = &[
        (
            "empty",
            String::new(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            "a space",
            "tok en".to_string(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            "a character outside the alphabet",
            "token!".to_string(),
            format!("Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"),
        ),
        (
            // The modelled `BlockToken` `@length` is 0..256.
            "one character past the modelled maximum length",
            over_long,
            "Member must have length less than or equal to 256".to_string(),
        ),
    ];

    for (case, token, requirement) in refused {
        let reply = get_block(&router, &snapshot_id, 0, token).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token outside the enforced expression must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        let message = member(&reply, "message");
        assert!(
            message.starts_with("1 validation error detected: Value '"),
            "case {case:?}: the refusal must be the constraint report, got {message:?}"
        );
        assert!(
            message.contains(&format!("at 'blockToken' failed to satisfy constraint: {requirement}")),
            "case {case:?}: the report must name the member and the requirement {requirement:?}, got {message:?}"
        );
    }

    // The recorded instance, asserted whole: an empty token names the member, echoes the
    // value and quotes the expression the service enforces.
    let empty = get_block(&router, &snapshot_id, 0, "").await;
    assert_eq!(
        member(&empty, "message"),
        format!(
            "1 validation error detected: Value '' at 'blockToken' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {BLOCK_TOKEN_PATTERN}"
        ),
        "case \"empty\": the whole constraint report must be the service's, got {}",
        empty.json()
    );

    // Rows INSIDE the enforced expression: each gets past validation and is answered by
    // token resolution — the modelled `ValidationException` carrying `Reason` and the
    // service's request-derived template naming the rejected token. `_` and `-` are the
    // characters the model's narrower pattern would have refused.
    let other = write_snapshot(&router, "another snapshot", 1, &[(0, 32)]).await;
    let other_token = listed_blocks(&list_blocks(&router, &other, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the other snapshot lists its block");
    let accepted: Vec<(&str, String, i32)> = vec![
        ("URL-safe base64 alphabet", "AAA_BBB-CCC=".to_string(), 0),
        (
            "standard base64 alphabet",
            "YW5vdGhlci10b2tlbg==".to_string(),
            0,
        ),
        ("bare alphanumerics", "token0".to_string(), 0),
        ("a token issued for another snapshot", other_token, 0),
        ("the issued token at an index with no block", real_token.clone(), 3),
    ];

    for (case, token, block_index) in &accepted {
        let reply = get_block(&router, &snapshot_id, *block_index, token).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token that names no block must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_BLOCK_TOKEN",
            "case {case:?}: a token inside the enforced expression must be answered by token resolution, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("The block token {token} is invalid."),
            "case {case:?}: the refusal must name the rejected token, got {}",
            reply.json()
        );
        assert!(
            member(&reply, "message").is_empty(),
            "case {case:?}: a token inside the enforced expression must not be a constraint violation, got {}",
            reply.json()
        );
    }

    // And the token the service really issued still reads its block.
    let fetched = get_block(&router, &snapshot_id, 0, &real_token).await;
    assert_eq!(
        fetched.status, 200,
        "case \"the issued token\": GetSnapshotBlock must answer 200, got {} with body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
}

/// RULE 4: a checksum algorithm the service does not support is refused through the
/// service's OWN modelled `ValidationException` — HTTP 400 with `Message` naming the
/// rejected algorithm and the supported set, and `Reason` carrying the
/// `ValidationExceptionReason` a client branches on.
#[tokio::test]
async fn an_unsupported_checksum_algorithm_is_refused_with_message_and_reason() {
    let router = router();
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let snapshot_id = member(&started, "SnapshotId");
    let data = block_bytes(41);
    let data_checksum = checksum(&data);

    // Rows differ along which unsupported algorithm was asked for, including one that
    // differs from the supported value only in case.
    let unsupported: &[&str] = &["MD5", "SHA1", "CRC32C", "sha256"];

    for algorithm in unsupported {
        let case = format!("PutSnapshotBlock with {algorithm}");
        let reply = put_block_with_algorithm(
            &router,
            &snapshot_id,
            0,
            data.clone(),
            BLOCK_SIZE,
            &data_checksum,
            algorithm,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unsupported algorithm must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!(
                "Unsupported Checksum Algorithm '{algorithm}', supported algorithms are \
                 {SUPPORTED_ALGORITHM}"
            ),
            "case {case:?}: the refusal must name the rejected algorithm and the supported set, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the matching ValidationExceptionReason, got {}",
            reply.json()
        );

        // The same rule in the other operation that carries the member.
        let case = format!("CompleteSnapshot with {algorithm}");
        let completed = complete_snapshot_with_algorithm(&router, &snapshot_id, 0, algorithm).await;
        assert_eq!(
            completed.status, 400,
            "case {case:?}: an unsupported algorithm must be refused 400, got {} with body {}",
            completed.status,
            String::from_utf8_lossy(&completed.body)
        );
        assert_eq!(
            member(&completed, "Message"),
            format!(
                "Unsupported Checksum Algorithm '{algorithm}', supported algorithms are \
                 {SUPPORTED_ALGORITHM}"
            ),
            "case {case:?}: the refusal must name the rejected algorithm and the supported set, got {}",
            completed.json()
        );
        assert_eq!(
            member(&completed, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the matching ValidationExceptionReason, got {}",
            completed.json()
        );
    }

    // The supported algorithm is still accepted, so the refusal above is about the value
    // and not about the member.
    let written = put_block_with_algorithm(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        &data_checksum,
        SUPPORTED_ALGORITHM,
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"the supported algorithm\": PutSnapshotBlock must answer 201, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );

    // The same rule covers the other header members the service judges: a `Checksum`
    // outside its modelled form and a `Progress` outside its modelled range are refused
    // through the modelled error too, each carrying a Message and a Reason.
    let malformed_checksum = put_block_with_algorithm(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        "not*a*checksum",
        SUPPORTED_ALGORITHM,
    )
    .await;
    assert_eq!(
        malformed_checksum.status, 400,
        "case \"a Checksum outside its modelled form\": expected HTTP 400, got {} with body {}",
        malformed_checksum.status,
        String::from_utf8_lossy(&malformed_checksum.body)
    );
    assert!(
        member(&malformed_checksum, "Message").contains("at 'checksum' failed to satisfy constraint"),
        "case \"a Checksum outside its modelled form\": the refusal must name the member, got {}",
        malformed_checksum.json()
    );
    assert_eq!(
        member(&malformed_checksum, "Reason"),
        "INVALID_PARAMETER_VALUE",
        "case \"a Checksum outside its modelled form\": the refusal must carry a Reason, got {}",
        malformed_checksum.json()
    );

    // `Progress` is modelled `@range` 0..100; one past the maximum is the smallest input
    // outside it.
    let over_progress = put_block_with_progress(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        &data_checksum,
        "101",
    )
    .await;
    assert_eq!(
        over_progress.status, 400,
        "case \"a Progress past the modelled maximum\": expected HTTP 400, got {} with body {}",
        over_progress.status,
        String::from_utf8_lossy(&over_progress.body)
    );
    assert!(
        member(&over_progress, "Message").contains("at 'progress' failed to satisfy constraint"),
        "case \"a Progress past the modelled maximum\": the refusal must name the member, got {}",
        over_progress.json()
    );
    assert_eq!(
        member(&over_progress, "Reason"),
        "INVALID_PARAMETER_VALUE",
        "case \"a Progress past the modelled maximum\": the refusal must carry a Reason, got {}",
        over_progress.json()
    );

    // A `Progress` inside the range is accepted, so the refusal is about the value.
    let in_progress = put_block_with_progress(
        &router,
        &snapshot_id,
        0,
        data,
        BLOCK_SIZE,
        &data_checksum,
        "100",
    )
    .await;
    assert_eq!(
        in_progress.status, 201,
        "case \"a Progress at the modelled maximum\": expected HTTP 201, got {} with body {}",
        in_progress.status,
        String::from_utf8_lossy(&in_progress.body)
    );

    let sealed = complete_snapshot_with_algorithm(&router, &snapshot_id, 1, SUPPORTED_ALGORITHM).await;
    assert_eq!(
        sealed.status, 202,
        "case \"the supported algorithm\": CompleteSnapshot must answer 202, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
}

/// RULE 5: the `ClientToken` conflict carries a FIXED, request-independent message. It
/// is the same sentence whatever the token and whatever changed, and it echoes neither
/// the caller's token nor the operation.
#[tokio::test]
async fn a_client_token_conflict_carries_the_fixed_message() {
    let router = router();
    let expected = "Current request does not match request stored with this token.";

    // Rows differ along the token's text and along WHICH member of the repeat request
    // differs from the stored one.
    let cases: &[(&str, serde_json::Value, serde_json::Value)] = &[
        (
            "a changed volume size",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-volume" }),
            serde_json::json!({ "VolumeSize": 2, "ClientToken": "conflict-volume" }),
        ),
        (
            "a changed description",
            serde_json::json!({
                "VolumeSize": 1,
                "Description": "first",
                "ClientToken": "snap-055f871ca34010e74",
            }),
            serde_json::json!({
                "VolumeSize": 1,
                "Description": "second",
                "ClientToken": "snap-055f871ca34010e74",
            }),
        ),
        (
            "a tag added",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-tags" }),
            serde_json::json!({
                "VolumeSize": 1,
                "Tags": [{ "Key": "k", "Value": "v" }],
                "ClientToken": "conflict-tags",
            }),
        ),
        (
            "encryption added",
            serde_json::json!({ "VolumeSize": 1, "ClientToken": "conflict-encrypted" }),
            serde_json::json!({ "VolumeSize": 1, "Encrypted": true, "ClientToken": "conflict-encrypted" }),
        ),
    ];

    for (case, first, second) in cases {
        let created = start_snapshot(&router, first).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the first StartSnapshot must answer 201, got {} with body {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let conflict = start_snapshot(&router, second).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: a changed request under the same token must answer 409, got {} with body {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert!(
            conflict.error_code().ends_with("ConflictException"),
            "case {case:?}: expected the ConflictException error code, got {:?}",
            conflict.error_code()
        );
        assert_eq!(
            member(&conflict, "Message"),
            expected,
            "case {case:?}: the conflict message is fixed, got {}",
            conflict.json()
        );
        let token = first
            .get("ClientToken")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            !member(&conflict, "Message").contains(token),
            "case {case:?}: the refusal must not echo the caller's token {token:?}, got {}",
            conflict.json()
        );
    }
}

/// RULE 6: a `VolumeSize` the service will not accept is refused with its request-derived
/// template, which names only the rejected value — no bound, no member prose — and
/// carries the `INVALID_VOLUME_SIZE` reason.
#[tokio::test]
async fn an_out_of_range_volume_size_is_refused_with_the_service_template() {
    let router = router();

    // `StartSnapshotRequest$VolumeSize`: "The maximum size is 65536 GiB (64 TiB)". Rows
    // differ along how far past that bound they are, the first being one past it.
    let refused: &[i64] = &[65_537, 99_999, 1_048_576, i64::from(i32::MAX)];

    for volume_size in refused {
        let case = format!("VolumeSize {volume_size}");
        let reply = start_snapshot(&router, &serde_json::json!({ "VolumeSize": volume_size })).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an out-of-range volume size must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("Invalid volume size: {volume_size}"),
            "case {case:?}: the refusal must name only the rejected value, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_VOLUME_SIZE",
            "case {case:?}: the refusal must carry the INVALID_VOLUME_SIZE reason, got {}",
            reply.json()
        );
    }

    // The published bound itself is accepted, so the refusal is about being past it.
    let accepted = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 65_536 })).await;
    assert_eq!(
        accepted.status, 201,
        "case \"the published maximum\": StartSnapshot must answer 201, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// RULE 7: every operation that resolves a snapshot id words "that snapshot is not
/// stored" the same way — `The snapshot '<id>' does not exist.`, HTTP 404
/// `ResourceNotFoundException` with reason `SNAPSHOT_NOT_FOUND` — and says nothing about
/// a snapshot existing in some other state, which is a different refusal.
#[tokio::test]
async fn every_snapshot_not_found_refusal_uses_one_template() {
    let router = router();
    let data = block_bytes(51);
    let data_checksum = checksum(&data);
    // Inside the enforced `blockToken` expression, so the request reaches the lookup.
    let token = "QUJDREVG";

    for id in UNSTORED_IDS {
        // Every row must be WELL-FORMED, or it would be exercising the malformed-id rule
        // instead; the width is asserted rather than counted by eye.
        let hex = id.strip_prefix("snap-").unwrap_or_default();
        assert!(
            matches!(hex.len(), 8 | 17),
            "case {id:?}: a not-found row needs 8 or 17 hex digits, got {}",
            hex.len()
        );

        // Rows: every operation that resolves a snapshot id, over every unstored id.
        let replies: Vec<(String, Reply)> = vec![
            (
                format!("ListSnapshotBlocks on {id}"),
                list_blocks(&router, id, "").await,
            ),
            (
                format!("GetSnapshotBlock on {id}"),
                get_block(&router, id, 0, token).await,
            ),
            (
                format!("PutSnapshotBlock on {id}"),
                put_block(&router, id, 0, data.clone(), BLOCK_SIZE, &data_checksum).await,
            ),
            (
                format!("CompleteSnapshot on {id}"),
                complete_snapshot(&router, id, 0).await,
            ),
            (
                format!("StartSnapshot with ParentSnapshotId {id}"),
                start_snapshot(
                    &router,
                    &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": id }),
                )
                .await,
            ),
        ];

        for (case, reply) in replies {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an unstored snapshot must be refused 404, got {} with body {}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_code().ends_with("ResourceNotFoundException"),
                "case {case:?}: expected the ResourceNotFoundException error code, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                member(&reply, "Message"),
                format!("The snapshot '{id}' does not exist."),
                "case {case:?}: every not-found refusal shares one template, got {}",
                reply.json()
            );
            assert_eq!(
                member(&reply, "Reason"),
                "SNAPSHOT_NOT_FOUND",
                "case {case:?}: the refusal must carry the SNAPSHOT_NOT_FOUND reason, got {}",
                reply.json()
            );
        }
    }

    // A snapshot that IS stored but is still being written is a different condition, and
    // must not borrow the not-found template: it is refused as a validation failure
    // naming the state, not as an absent resource.
    let pending = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let pending_id = member(&pending, "SnapshotId");
    let child = start_snapshot(
        &router,
        &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": pending_id }),
    )
    .await;
    assert_eq!(
        child.status, 400,
        "case \"a parent that is still pending\": expected HTTP 400, got {} with body {}",
        child.status,
        String::from_utf8_lossy(&child.body)
    );
    assert!(
        child.error_code().ends_with("ValidationException"),
        "case \"a parent that is still pending\": expected the ValidationException error code, got {:?}",
        child.error_code()
    );
    assert!(
        !member(&child, "Message").contains("does not exist"),
        "case \"a parent that is still pending\": the refusal must not claim the snapshot is absent, got {}",
        child.json()
    );
}

/// RULE 8 (the other half of rule 7): a value that is not even SHAPED like a snapshot id
/// is a malformed parameter, not a snapshot the service does not hold. It is refused
/// HTTP 400 `ValidationException` with the service's request-derived template naming the
/// rejected value and the parameter it arrived in — `Value (<v>) for parameter <p> is
/// invalid. Expected: 'snap-...'.` — and the `INVALID_SNAPSHOT_ID` reason.
///
/// The widths a snapshot id has are 8 and 17 hex digits
/// (`crate::state::SNAPSHOT_ID_HEX_WIDTHS`); the modelled `SnapshotId` pattern
/// `^snap-[0-9a-f]+$` admits any of them, so every row below is a value the boundary
/// lets through.
#[tokio::test]
async fn a_snapshot_id_of_the_wrong_shape_is_a_malformed_parameter() {
    let router = router();
    let data = block_bytes(61);
    let data_checksum = checksum(&data);
    let token = "QUJDREVG";

    // Rows differ along how far the hex width is from the two the service accepts —
    // built from a repeat count rather than written out, so no row can be miscounted —
    // plus an all-zero id of each accepted width, whose suffix is not a resource number.
    // Every row satisfies the MODELLED pattern `^snap-[0-9a-f]+$`, so every row reaches
    // the handler: a value outside that pattern is the boundary's to refuse, asserted
    // separately below.
    let widths: &[usize] = &[1, 7, 9, 16, 18, 32];
    let mut malformed: Vec<String> = widths
        .iter()
        .map(|width| format!("snap-{}", "a".repeat(*width)))
        .collect();
    for width in SNAPSHOT_ID_HEX_WIDTHS {
        malformed.push(format!("snap-{}", "0".repeat(width)));
    }

    for id in &malformed {
        let replies: Vec<(String, Reply, &str)> = vec![
            (
                format!("ListSnapshotBlocks on {id}"),
                list_blocks(&router, id, "").await,
                "snapshotId",
            ),
            (
                format!("GetSnapshotBlock on {id}"),
                get_block(&router, id, 0, token).await,
                "snapshotId",
            ),
            (
                format!("PutSnapshotBlock on {id}"),
                put_block(&router, id, 0, data.clone(), BLOCK_SIZE, &data_checksum).await,
                "snapshotId",
            ),
            (
                format!("CompleteSnapshot on {id}"),
                complete_snapshot(&router, id, 0).await,
                "snapshotId",
            ),
            (
                format!("StartSnapshot with ParentSnapshotId {id}"),
                start_snapshot(
                    &router,
                    &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": id }),
                )
                .await,
                "parentSnapshotId",
            ),
        ];

        for (case, reply, parameter) in replies {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a malformed snapshot id must be refused 400, got {} with body {}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_code().ends_with("ValidationException"),
                "case {case:?}: expected the ValidationException error code, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                member(&reply, "Message"),
                format!("Value ({id}) for parameter {parameter} is invalid. Expected: 'snap-...'."),
                "case {case:?}: the refusal must name the value and the parameter, got {}",
                reply.json()
            );
            assert_eq!(
                member(&reply, "Reason"),
                "INVALID_SNAPSHOT_ID",
                "case {case:?}: the refusal must carry the INVALID_SNAPSHOT_ID reason, got {}",
                reply.json()
            );
        }
    }

    // And a well-formed id of each accepted width is NOT refused for its shape: it is
    // answered as a snapshot the service does not hold.
    for width in [8_usize, 17] {
        let id = format!("snap-{}", "a".repeat(width));
        let reply = list_blocks(&router, &id, "").await;
        assert_eq!(
            reply.status, 404,
            "case \"a well-formed {width}-digit id\": expected HTTP 404, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("The snapshot '{id}' does not exist."),
            "case \"a well-formed {width}-digit id\": expected the not-found template, got {}",
            reply.json()
        );
    }

    // The other half of the split: a value outside the MODELLED `SnapshotId` pattern is
    // refused by the boundary before any handler runs, with the framework's constraint
    // report — the same layer, and the same wording, that judges every other member the
    // model constrains.
    for outside in ["snap-", "not-an-id", "SNAP-DEADBEEF"] {
        let reply = list_blocks(&router, outside, "").await;
        assert_eq!(
            reply.status, 400,
            "case {outside:?}: a value outside the modelled pattern must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            member(&reply, "message").starts_with("1 validation error detected: Value '"),
            "case {outside:?}: expected the boundary's constraint report, got {}",
            reply.json()
        );
    }
}
