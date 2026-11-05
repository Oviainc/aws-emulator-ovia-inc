// SPDX-License-Identifier: CC0-1.0

//! WHICH conditional members of a delete each kind of bucket supports — and what a bucket
//! that does not support one answers.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour: status, error code, error message, and what the key holds afterwards.
//!
//! The rule under test: "the `If-Match` header is supported for both general purpose and
//! directory buckets. `IfMatchLastModifiedTime` and `IfMatchSize` is only supported for
//! directory buckets" (`DeleteObject`'s own documentation, and the same note on
//! `DeleteObjectRequest$IfMatchSize` and on `ObjectIdentifier$LastModifiedTime` /
//! `$Size`, which carry the same two conditions for a batch delete). A delete addressed to a
//! bucket whose kind does not support a member it carries asks for functionality the service
//! does not implement there, which is HTTP 501 `NotImplemented` — "a header you provided
//! implies functionality that is not implemented" — and nothing is deleted: a condition that
//! cannot be judged must not be dropped and the object removed anyway.
//!
//! The sentence that code carries names the header CLASS rather than the member, so the
//! document carries the `<Header>` naming the one that asked for it — the only part of the
//! refusal a client feature-detecting the member reads to learn which header to drop.
//!
//! The rows differ along what the rule generalises over: which of the two directory-only
//! members the request carries and how many at once, whether the value it carries HOLDS for
//! the stored object, whether `If-Match` (supported by both kinds) is carried beside it, and
//! which operation carries the condition — one-object `DeleteObject` and batch
//! `DeleteObjects` alike.

use service_com_amazonaws_s3_amazons3::state::{
    NOT_IMPLEMENTED_CODE, NOT_IMPLEMENTED_HEADER_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_conditional_deletes::delete_object;

/// The payload every object in this suite holds, so the size a row names either IS its
/// length or is not.
const PAYLOAD: &[u8] = b"conditional-delete-payload";

/// [`PAYLOAD`]'s own length, as a row names it in the header's value. Asserted against the
/// payload where it is used, so the two cannot drift apart.
const HELD_SIZE: &str = "26";

/// `POST /<bucket>?delete` — `DeleteObjects`, with the `Delete` container as the payload.
async fn delete_objects(router: &MultiServiceRouter, name: &str, payload: &str) -> Answer {
    send(router, "POST", &format!("/{name}?delete"), &[], payload).await
}

/// One `<Object>` identifier of a batch delete: its key, and the conditional members the row
/// makes the delete of that key conditional on.
fn identifier(key: &str, last_modified_time: Option<&str>, size: Option<usize>) -> String {
    let element = |name: &str, value: Option<String>| match value {
        Some(value) => format!("<{name}>{value}</{name}>"),
        None => String::new(),
    };
    format!(
        "<Object><Key>{key}</Key>{}{}</Object>",
        element("LastModifiedTime", last_modified_time.map(str::to_string)),
        element("Size", size.map(|size| size.to_string()))
    )
}

/// A modification time that is no object in this suite's — the clock is read at the write,
/// so a time in the past can match none of them.
///
/// In the spelling the framework's timestamp binding reads a value in, which is the spelling
/// the conditional-delete suite beside this one sends its own time member in.
const ANOTHER_TIME: &str = "2015-01-01T00:00:00Z";

/// One row: the case name, the conditional headers the delete carries, and the header the
/// refusal must NAME — the first directory-only member the request carries, in the model's
/// own member order (`IfMatchLastModifiedTime` before `IfMatchSize`).
struct Row {
    case: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    named: &'static str,
}

#[tokio::test]
async fn a_condition_only_directory_buckets_support_is_not_implemented_on_a_general_purpose_bucket()
{
    let router = router();
    let name = "general-purpose-conditions";
    bucket(&router, name).await;
    assert_eq!(
        PAYLOAD.len().to_string(),
        HELD_SIZE,
        "the size the rows name must be the payload's own length, which is {}",
        PAYLOAD.len()
    );

    // The rows: each directory-only member on its own with a value that HOLDS for the stored
    // object and with one that does not, both of them at once, and each beside the `If-Match`
    // both kinds of bucket support — none of which changes the answer, because the refusal is
    // about where the member is supported rather than about what it says.
    let held_size: &str = HELD_SIZE;
    let rows: Vec<Row> = vec![
        Row {
            case: "a size that is the object's",
            headers: vec![("x-amz-if-match-size", held_size)],
            named: "x-amz-if-match-size",
        },
        Row {
            case: "a size that is not the object's",
            headers: vec![("x-amz-if-match-size", "7")],
            named: "x-amz-if-match-size",
        },
        Row {
            case: "a modification time that is not the object's",
            headers: vec![("x-amz-if-match-last-modified-time", ANOTHER_TIME)],
            named: "x-amz-if-match-last-modified-time",
        },
        Row {
            case: "both directory-only members at once",
            headers: vec![
                ("x-amz-if-match-size", held_size),
                ("x-amz-if-match-last-modified-time", ANOTHER_TIME),
            ],
            named: "x-amz-if-match-last-modified-time",
        },
        Row {
            case: "a size beside the wildcard If-Match both kinds support",
            headers: vec![("if-match", "*"), ("x-amz-if-match-size", held_size)],
            named: "x-amz-if-match-size",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = format!("conditions/row-{index}.bin");
        let stored = put_object(&router, name, &key, PAYLOAD, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "case {case:?}: the fixture write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );

        let refused = delete_object(&router, name, &key, &row.headers).await;
        assert_eq!(
            refused.status,
            501,
            "case {case:?}: expected the published 501 for a header this bucket kind does \
             not support, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            NOT_IMPLEMENTED_CODE,
            "case {case:?}: expected the published code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            NOT_IMPLEMENTED_HEADER_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
        // The sentence names the header CLASS and not the member, so the `<Header>` element
        // is the only part of the refusal that says which header the caller must drop.
        assert_eq!(
            refused.element("Header").as_deref(),
            Some(row.named),
            "case {case:?}: the refusal must name the header that asked for it, got {:?} in \
             {:?}",
            refused.element("Header"),
            text(&refused)
        );

        // A refused delete took nothing away: the object is still there with its bytes.
        let after = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            after.status,
            200,
            "case {case:?}: an unserved condition must leave the object exactly as it was, \
             got {} with body {:?}",
            after.status,
            text(&after)
        );
        assert_eq!(
            after.body.as_ref(),
            PAYLOAD,
            "case {case:?}: the surviving object must hold the bytes it was written with"
        );
    }

    // The same two members in a BATCH delete, which carries them per identifier: the request
    // is refused whole, so neither the conditional key nor the one beside it is removed.
    let batch_keys = ["batch/conditioned.bin", "batch/plain.bin"];
    for key in batch_keys {
        let stored = put_object(&router, name, key, PAYLOAD, &[]).await;
        assert_eq!(
            stored.status, 200,
            "the batch fixture write of {key:?} must succeed, got {}",
            stored.status
        );
    }
    for (case, conditioned) in [
        (
            "a size in one identifier",
            identifier(batch_keys[0], None, Some(PAYLOAD.len())),
        ),
        (
            "a modification time in one identifier",
            identifier(batch_keys[0], Some(ANOTHER_TIME), None),
        ),
    ] {
        let payload = format!(
            "<Delete>{conditioned}{}</Delete>",
            identifier(batch_keys[1], None, None)
        );
        let refused = delete_objects(&router, name, &payload).await;
        assert_eq!(
            refused.status,
            501,
            "case {case:?}: expected the published 501 for a batch naming a header this \
             bucket kind does not support, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            NOT_IMPLEMENTED_CODE,
            "case {case:?}: expected the published code, got {:?}",
            refused.error_code()
        );
        // A batch carries its conditions as MEMBERS of the request document rather than as
        // headers, so there is no header name for the refusal to name.
        assert_eq!(
            refused.element("Header"),
            None,
            "case {case:?}: no header asked for it, so the refusal names none, got {:?}",
            refused.element("Header")
        );
        for key in batch_keys {
            let after = get_object(&router, name, key, "", &[]).await;
            assert_eq!(
                after.status, 200,
                "case {case:?}: the refused batch must leave {key:?} exactly as it was, got \
                 {}",
                after.status
            );
        }
    }
}

/// One row of the served-conditions table: the case name, the conditional headers the delete
/// carries, and the status it must answer.
type ServedRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16);

#[tokio::test]
async fn the_same_conditions_are_judged_on_a_directory_bucket() {
    let router = router();
    let name = "served-conditions--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    // The same members a general purpose bucket refuses are JUDGED here, against the object
    // the key really holds: a value that holds carries the delete out (204) and one that does
    // not refuses it (412), which is the condition doing its work rather than being dropped.
    let rows: Vec<ServedRow<'_>> = vec![
        (
            "a size that is the object's",
            vec![("x-amz-if-match-size", HELD_SIZE)],
            204,
        ),
        (
            "a size that is not the object's",
            vec![("x-amz-if-match-size", "7")],
            412,
        ),
        (
            "a modification time that is not the object's",
            vec![("x-amz-if-match-last-modified-time", ANOTHER_TIME)],
            412,
        ),
    ];
    assert_eq!(
        PAYLOAD.len().to_string(),
        HELD_SIZE,
        "the size the rows above name must be the payload's own length, which is {}",
        PAYLOAD.len()
    );

    for (index, (case, headers, status)) in rows.iter().enumerate() {
        let key = format!("served/row-{index}.bin");
        let stored = put_object(&router, name, &key, PAYLOAD, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "case {case:?}: the fixture write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        let answered = delete_object(&router, name, &key, headers).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answered.status,
            text(&answered)
        );
        let after = get_object(&router, name, &key, "", &[]).await;
        let expected_after = if *status == 204 { 404 } else { 200 };
        assert_eq!(
            after.status, expected_after,
            "case {case:?}: expected the key to answer {expected_after} after the delete, \
             got {}",
            after.status
        );
    }
}
