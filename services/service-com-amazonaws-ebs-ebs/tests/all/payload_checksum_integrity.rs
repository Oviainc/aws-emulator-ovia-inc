// SPDX-License-Identifier: CC0-1.0

//! The checksum header that binds a block payload to its request.
//!
//! RULE: `PutSnapshotBlock` carries its block as an UNSIGNED payload (the operation is
//! `@unsignedPayload` and `BlockData` is the `@httpPayload`
//! (`smithy_query(query="operation",operation="PutSnapshotBlock")`)), so the
//! `x-amz-Checksum` header is what binds those bytes to the signed request. A payload
//! whose checksum is not the checksum of the bytes that arrived therefore fails the
//! request's INTEGRITY, which is the signing layer's refusal — HTTP 403
//! `InvalidSignatureException`, whose message states both the checksum computed over the
//! received bytes and the one the request supplied — and not a flat input error. The
//! distinction is what a client branches on: retry and credential-refresh logic keyed on
//! the signing class never fires for a 400, and a caller cannot tell a corrupted upload
//! from a malformed request.
//!
//! Asserted over a table whose rows differ in WHY the two checksums disagree and in the
//! payload sent — including payloads that do not deliver the length the request DECLARED,
//! which are still the integrity refusal: the checksum is computed over the bytes that
//! arrived, so a payload the checksum does not name is refused before its length is
//! judged at all (the length refusal is in `sealed_status_and_fixed_refusal_text.rs`).
//! The matching checksum is the positive control.

use crate::snapshot_block_loop::{
    block_bytes, checksum, list_blocks, listed_blocks, put_block, router, start_snapshot,
    BLOCK_SIZE,
};

/// How the refusal states the two checksums: the one computed over the received payload,
/// then the one the request presented.
pub(crate) fn mismatch_message(computed: &str, supplied: &str) -> String {
    format!(
        "The value passed in as x-amz-Checksum does not match the computed checksum. \
         Computed checksum: {computed} expected checksum: {supplied}"
    )
}

/// The `message` member of a refusal body. The signing layer's class is not one of this
/// service's modelled shapes, so its message travels in the lower-case member every AWS
/// framework error carries.
pub(crate) fn lowercase_message(reply: &crate::snapshot_block_loop::Reply) -> String {
    reply
        .json()
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One mismatch case: the case name, the payload the request carries and the checksum it
/// presents for it.
struct Case {
    case: &'static str,
    payload: fn() -> Vec<u8>,
    supplied: fn() -> String,
}

#[tokio::test]
async fn a_payload_that_is_not_the_one_the_checksum_names_fails_the_requests_integrity() {
    // Rows: the checksum of DIFFERENT bytes of the same length, the checksum of the empty
    // payload, the checksum of a truncated copy, a well-formed value of the right form
    // that is no checksum of anything, and two payloads that do not deliver the declared
    // 524288 bytes at all — the length the request declared is not what decides this
    // refusal.
    let cases: &[Case] = &[
        Case {
            case: "the checksum of other bytes",
            payload: || block_bytes(51),
            supplied: || checksum(&block_bytes(52)),
        },
        Case {
            case: "the checksum of the empty payload",
            payload: || block_bytes(53),
            supplied: || checksum(&Vec::new()),
        },
        Case {
            case: "the checksum of a truncated copy of the payload",
            payload: || block_bytes(54),
            supplied: || checksum(&block_bytes(54)[..1024]),
        },
        Case {
            // 32 bytes of base64, inside the modelled `^[A-Za-z0-9+/=]+$` and length
            // 0..64, so the value is accepted as a checksum and only then compared.
            case: "a well-formed value that is no checksum of these bytes",
            payload: || block_bytes(55),
            supplied: || aws_smithy_types::base64::encode([7_u8; 32]),
        },
        Case {
            case: "no payload at all against the checksum of a full block",
            payload: Vec::new,
            supplied: || checksum(&block_bytes(56)),
        },
        Case {
            case: "a payload truncated in flight",
            payload: || block_bytes(57)[..4096].to_vec(),
            supplied: || checksum(&block_bytes(57)),
        },
    ];

    for Case {
        case,
        payload,
        supplied,
    } in cases
    {
        let router = router();
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = started
            .json()
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        let data = (payload)();
        let supplied = (supplied)();
        let reply = put_block(&router, &snapshot_id, 0, data.clone(), BLOCK_SIZE, &supplied).await;

        assert_eq!(
            reply.status, 403,
            "case {case:?}: a payload the checksum does not name must be refused 403, got {} \
             with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("InvalidSignatureException"),
            "case {case:?}: expected the InvalidSignatureException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            lowercase_message(&reply),
            mismatch_message(&checksum(&data), &supplied),
            "case {case:?}: the refusal must state the checksum computed over the bytes that \
             arrived and the one supplied, got {}",
            reply.json()
        );

        // Nothing was stored: the request never reached the snapshot. Proven by reading
        // the snapshot back once it is sealed (sealing is what makes it readable), where
        // the listing reports no block at all.
        let sealed = crate::snapshot_block_loop::complete_snapshot(&router, &snapshot_id, 0).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: the snapshot the refused write targeted must still seal; got {} \
             with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        let listing = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the sealed snapshot must be listable, got {} with body {}",
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        let listed: Vec<i32> = listed_blocks(&listing)
            .iter()
            .map(|(index, _)| *index)
            .collect();
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused write must store nothing, so the listing must hold no \
             block; got {listed:?}"
        );
    }
}

/// The positive control: the SAME payloads with their own checksum are written, and the
/// write reports the checksum computed over the bytes it stored.
#[tokio::test]
async fn a_payload_its_checksum_names_is_written() {
    let seeds: &[u8] = &[51, 53, 54, 55];

    for seed in seeds {
        let router = router();
        let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
        let snapshot_id = started
            .json()
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        let data = block_bytes(*seed);
        let expected = checksum(&data);
        let reply = put_block(&router, &snapshot_id, 0, data, BLOCK_SIZE, &expected).await;
        assert_eq!(
            reply.status, 201,
            "case \"seed {seed}\": a payload its checksum names must answer 201, got {} with \
             body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.header("x-amz-Checksum"),
            expected,
            "case \"seed {seed}\": the write must report the checksum of the stored bytes, got {:?}",
            reply.header("x-amz-Checksum")
        );
    }
}
