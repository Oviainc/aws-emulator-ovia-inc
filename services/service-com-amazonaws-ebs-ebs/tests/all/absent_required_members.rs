// SPDX-License-Identifier: CC0-1.0

//! A request that OMITS a member this service's model requires.
//!
//! RULE: a request whose method and path match an operation's modelled HTTP binding is
//! that operation's request, and a header or query member the model marks `@required` and
//! the request omitted is refused as a CLIENT INPUT error — HTTP 400 with this service's
//! modelled `ValidationException`, carrying `Reason` `INVALID_PARAMETER_VALUE` and a
//! `Message` naming what was missing — never as an operation this emulator does not serve
//! (HTTP 501 `NotImplemented`), which says the server has nothing to answer with and
//! which a client treats as non-retryable infrastructure breakage rather than as "fix your
//! request".
//!
//! The members the rule covers are every `@required` `@httpHeader` / `@httpQuery` member
//! this model declares
//! (`smithy_query(query="operation",operation="PutSnapshotBlock,GetSnapshotBlock,CompleteSnapshot")`):
//! `PutSnapshotBlock`'s `Checksum`, `ChecksumAlgorithm` and `DataLength`,
//! `GetSnapshotBlock`'s `BlockToken` and `CompleteSnapshot`'s `ChangedBlocksCount`. The
//! other three operations declare none, so there is no request of theirs to omit one from.
//!
//! Each row of the table below is one (operation, omitted member) pair, and every row
//! asserts the whole refusal: the status, the error code, the `Reason` and the `Message`.
//! The same requests WITH every member present are the positive control: they are served,
//! so the guard refuses the absent member and nothing else.

use crate::snapshot_block_loop::{
    block_bytes, checksum, list_blocks, listed_blocks, member, router, send, start_snapshot,
    write_snapshot, Reply, BLOCK_SIZE,
};

/// The `Reason` an absent input member is reported under, a member of
/// `ValidationExceptionReason`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationExceptionReason")`).
const ABSENT_MEMBER_REASON: &str = "INVALID_PARAMETER_VALUE";

/// The sentence this service answers with when the `x-amz-Checksum` header — the binding
/// between the unsigned block payload and the request — is absent.
const MISSING_CHECKSUM: &str = "Missing checksum for request payload";

/// The envelope an absent required member is otherwise reported with, as the request
/// boundary words it: the member's name lower-camelled, and the requirement it failed.
fn absent_member_envelope(lower_camel_member: &str) -> String {
    format!(
        "1 validation error detected: Value null at '{lower_camel_member}' failed to satisfy \
         constraint: Member must not be null"
    )
}

/// One omitted-member case: the request to send, which member it omits, and the sentence
/// the refusal must carry.
struct Case {
    case: &'static str,
    method: &'static str,
    /// Built from the fixture snapshot's id, since every one of these requests names a
    /// snapshot in its path.
    uri: fn(&str) -> String,
    /// The headers the request DOES carry — the omitted one is simply not here.
    headers: fn() -> Vec<(&'static str, String)>,
    /// Whether the request carries the 524288-byte block payload.
    payload: bool,
    message: fn() -> String,
}

/// Every header a complete `PutSnapshotBlock` carries, with the checksum of the payload
/// the table sends.
fn put_headers() -> Vec<(&'static str, String)> {
    vec![
        ("content-type", "application/octet-stream".to_string()),
        ("x-amz-Data-Length", BLOCK_SIZE.to_string()),
        ("x-amz-Checksum", checksum(&block_bytes(41))),
        ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
    ]
}

/// `put_headers` without `name`.
fn put_headers_without(name: &str) -> Vec<(&'static str, String)> {
    put_headers()
        .into_iter()
        .filter(|(header, _)| !header.eq_ignore_ascii_case(name))
        .collect()
}

#[tokio::test]
async fn a_request_omitting_a_required_member_is_a_client_input_error() {
    let cases: &[Case] = &[
        Case {
            case: "PutSnapshotBlock without x-amz-Checksum",
            method: "PUT",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0"),
            headers: || put_headers_without("x-amz-Checksum"),
            payload: true,
            message: || MISSING_CHECKSUM.to_string(),
        },
        Case {
            case: "PutSnapshotBlock without x-amz-Checksum-Algorithm",
            method: "PUT",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0"),
            headers: || put_headers_without("x-amz-Checksum-Algorithm"),
            payload: true,
            message: || absent_member_envelope("checksumAlgorithm"),
        },
        Case {
            case: "PutSnapshotBlock without x-amz-Data-Length",
            method: "PUT",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0"),
            headers: || put_headers_without("x-amz-Data-Length"),
            payload: true,
            message: || absent_member_envelope("dataLength"),
        },
        Case {
            // Several at once: the checksum binds the payload to the request, so its
            // absence is what the refusal names however many others are missing.
            case: "PutSnapshotBlock with none of the three headers",
            method: "PUT",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0"),
            headers: || vec![("content-type", "application/octet-stream".to_string())],
            payload: true,
            message: || MISSING_CHECKSUM.to_string(),
        },
        Case {
            case: "GetSnapshotBlock without the blockToken query",
            method: "GET",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0"),
            headers: Vec::new,
            payload: false,
            message: || absent_member_envelope("blockToken"),
        },
        Case {
            case: "GetSnapshotBlock with a query that is not blockToken",
            method: "GET",
            uri: |snapshot_id| format!("/snapshots/{snapshot_id}/blocks/0?other=1"),
            headers: Vec::new,
            payload: false,
            message: || absent_member_envelope("blockToken"),
        },
        Case {
            case: "CompleteSnapshot without x-amz-ChangedBlocksCount",
            method: "POST",
            uri: |snapshot_id| format!("/snapshots/completion/{snapshot_id}"),
            headers: Vec::new,
            payload: false,
            message: || absent_member_envelope("changedBlocksCount"),
        },
        Case {
            case: "CompleteSnapshot carrying only the optional checksum headers",
            method: "POST",
            uri: |snapshot_id| format!("/snapshots/completion/{snapshot_id}"),
            headers: || {
                vec![
                    ("x-amz-Checksum-Algorithm", "SHA256".to_string()),
                    ("x-amz-Checksum-Aggregation-Method", "LINEAR".to_string()),
                ]
            },
            payload: false,
            message: || absent_member_envelope("changedBlocksCount"),
        },
    ];

    for Case {
        case,
        method,
        uri,
        headers,
        payload,
        message,
    } in cases
    {
        // A fresh store per row, and a snapshot that EXISTS and is pending: the refusal
        // must be about the omitted member, not about the snapshot.
        let router = router();
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = started
            .json()
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        let body = if *payload { block_bytes(41) } else { Vec::new() };
        let reply = send(&router, method, &(uri)(&snapshot_id), &(headers)(), body).await;

        assert_eq!(
            reply.status, 400,
            "case {case:?}: an omitted required member must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Reason"),
            ABSENT_MEMBER_REASON,
            "case {case:?}: expected Reason {ABSENT_MEMBER_REASON}, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            (message)(),
            "case {case:?}: expected the sentence for the omitted member, got {}",
            reply.json()
        );
    }
}

/// The positive control for every row above: the SAME requests with every required member
/// present are served, so the guard refuses the absent member and nothing else — and the
/// write it admits is the one a read observes.
#[tokio::test]
async fn the_same_requests_with_every_required_member_are_served() {
    let router = router();
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let snapshot_id = started
        .json()
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();

    let data = block_bytes(41);
    let written = send(
        &router,
        "PUT",
        &format!("/snapshots/{snapshot_id}/blocks/0"),
        &put_headers(),
        data.clone(),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "a PutSnapshotBlock carrying every required header must answer 201, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );

    let sealed = send(
        &router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &[("x-amz-ChangedBlocksCount", "1".to_string())],
        Vec::new(),
    )
    .await;
    assert_eq!(
        sealed.status, 202,
        "a CompleteSnapshot carrying the count must answer 202, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );

    let listed = list_blocks(&router, &snapshot_id, "").await;
    let blocks = listed_blocks(&listed);
    let token = blocks
        .first()
        .map(|(_, token)| token.clone())
        .expect("the sealed snapshot lists the block that was written");
    let fetched = send(
        &router,
        "GET",
        &format!("/snapshots/{snapshot_id}/blocks/0?blockToken={token}"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        fetched.status, 200,
        "a GetSnapshotBlock carrying the token must answer 200, got {} with body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert!(
        fetched.body == data,
        "the block admitted by the guard must read back byte-identical; got {} bytes with \
         checksum {:?}",
        fetched.body.len(),
        checksum(&fetched.body)
    );
}

/// A request for a method and path this service's model binds NO route for is still the
/// operation-not-served answer: guarding the routes the model does bind must not make the
/// service claim to serve everything under its signing name.
#[tokio::test]
async fn a_path_the_model_does_not_bind_is_not_served() {
    let router = router();
    let snapshot_id = write_snapshot(&router, "unbound paths", 1, &[(0, 7)]).await;

    // Rows: a method no route uses on a bound path, a segment no route has, and a path
    // one segment too long for the route it resembles.
    let cases: &[(&str, &str, String)] = &[
        (
            "DELETE on a path only GET and PUT bind",
            "DELETE",
            format!("/snapshots/{snapshot_id}/blocks/0"),
        ),
        (
            "a segment no route binds",
            "GET",
            format!("/snapshots/{snapshot_id}/sectors"),
        ),
        (
            "one segment too many",
            "GET",
            format!("/snapshots/{snapshot_id}/blocks/0/bytes"),
        ),
    ];

    for (case, method, uri) in cases {
        let reply: Reply = send(&router, method, uri, &[], Vec::new()).await;
        assert_eq!(
            reply.status, 501,
            "case {case:?}: a request for no modelled route must answer 501, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}
