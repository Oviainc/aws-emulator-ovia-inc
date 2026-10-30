// SPDX-License-Identifier: CC0-1.0

//! `CreateFileSystem`'s creation token is single-use, end to end over HTTP.
//!
//! The rule under test: `CreateFileSystemRequest$clientToken` targets `CreationToken`,
//! so a token is CONSUMED by the create that succeeds under it — every later
//! `CreateFileSystem` carrying that same token, whatever else the request says, is
//! refused with the modelled `ConflictException` (409) naming the file system that
//! already holds the token, and no second file system is created. The token becomes
//! free again only when the file system holding it is gone.
//!
//! The rows span what the rule generalises over: the token's length (both `@length(1,
//! 64)` bounds and in between) and character content (`^(.+)$` accepts anything
//! non-empty), and how much of the REST of the repeat request differs from the create
//! that consumed the token — identical, a different bucket, or a different bucket,
//! role, prefix, KMS key and tag set. None of that changes the answer: the token alone
//! decides.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots, including the restJson1 error envelope
//! and its `x-amzn-errortype` header.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, create_file_system, new_router, Answer, ROLE_ARN,
};

/// The `resourceType` a conflict over a file system reports: the type name S3 Files
/// spells on the wire, stated here as the literal the service has to answer rather than
/// read back out of the implementation's own constant.
const RESOURCE_TYPE: &str = "FileSystem";

/// A token at `CreationToken`'s `@length` maximum of 64 characters.
fn longest_token() -> String {
    "t".repeat(64)
}

/// The table the rule is asserted over: the token the first create consumes, and the
/// members of the repeat that carries it again.
fn reuse_table() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "a one-character token, repeated with identical members",
            // `CreationToken` is @length(1, 64): one character is the minimum.
            "x".to_string(),
            json!({}),
        ),
        (
            "an ordinary token, repeated on a different bucket",
            "token-two".to_string(),
            json!({ "bucket": "arn:aws:s3:::s3files-token-other-bucket" }),
        ),
        (
            "a 64-character token, repeated with every member different",
            longest_token(),
            json!({
                "bucket": "arn:aws-cn:s3:::s3files-token-other-everything",
                "roleArn": "arn:aws-cn:iam::210987654321:role/path/OtherRole",
                "prefix": "elsewhere/",
                "kmsKeyId": "alias/s3files-other",
                "tags": [{ "key": "Name", "value": "other" }],
                "acceptBucketWarning": true,
            }),
        ),
        (
            "a token of spaces and punctuation",
            // `^(.+)$` constrains nothing but emptiness.
            "token with spaces, / and : ".to_string(),
            json!({ "prefix": "punctuated/" }),
        ),
        (
            "a token that looks like a file system id",
            "fs-0123456789abcdef0".to_string(),
            json!({}),
        ),
        (
            "a non-ASCII token",
            "jeton-éùà-☃".to_string(),
            json!({}),
        ),
    ]
}

/// The body of a `CreateFileSystem` on `bucket` under `token`, with `extra`'s members
/// overriding.
fn create_body(bucket: &str, token: &str, extra: &Value) -> Value {
    let mut body = json!({ "bucket": bucket, "roleArn": ROLE_ARN, "clientToken": token });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    body
}

/// How many file systems this router holds.
async fn held_count(router: &MultiServiceRouter) -> usize {
    let listed = call(router, "GET", "/file-systems", None).await;
    listed
        .field("fileSystems")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The modelled 409 for a creation token that is already held by `owner`.
fn assert_token_conflict(case: &str, answer: &Answer, owner: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected the modelled 409, got {} / {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ConflictException",
        "case {case:?}: expected the modelled ConflictException on the wire, got {:?} / {:?}",
        answer.error_type, answer.body
    );
    assert_eq!(
        answer.text("errorCode"),
        "ConflictException",
        "case {case:?}: errorCode is @required on ConflictException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.text("resourceId"),
        owner,
        "case {case:?}: resourceId must name the file system holding the token, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.text("resourceType"),
        RESOURCE_TYPE,
        "case {case:?}: resourceType must name the conflicting resource's type, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.text("message"),
        format!("File system '{owner}' already exists with this clientToken."),
        "case {case:?}: the message names the file system that holds the token, got {:?}",
        answer.body
    );
    assert!(
        answer.field("fileSystemId").is_null(),
        "case {case:?}: a refused create answers no file system, got {:?}",
        answer.body
    );
}

/// The rule: a consumed creation token refuses every later create carrying it, and
/// creates nothing.
#[tokio::test]
async fn a_consumed_creation_token_refuses_every_later_create() {
    let router = new_router();
    let table = reuse_table();

    for (index, (case, token, extra)) in table.iter().enumerate() {
        let bucket = format!("arn:aws:s3:::s3files-token-{index}");
        let created = create_file_system(&router, create_body(&bucket, token, &json!({}))).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the first create under the token must be accepted, got {} / {:?}",
            created.status, created.body
        );
        let owner = created.text("fileSystemId");
        let held = held_count(&router).await;

        // The repeat the row describes, and then an EXACT repeat of the accepted
        // create: a creation token is not an idempotent replay key, so both are
        // refused the same way.
        for (variant, body) in [
            ("the row's repeat", create_body(&bucket, token, extra)),
            ("an exact repeat", create_body(&bucket, token, &json!({}))),
        ] {
            let refused = create_file_system(&router, body).await;
            assert_token_conflict(&format!("{case} ({variant})"), &refused, &owner);
            assert_eq!(
                held_count(&router).await,
                held,
                "case {case:?} ({variant}): a refused create must persist nothing, \
                 expected {held} file system(s)"
            );
        }

        // The file system the token holds is still exactly the one the first create
        // answered, readable and unchanged by the refusals.
        let read = call(&router, "GET", &format!("/file-systems/{owner}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the owning file system must still be readable, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("clientToken"),
            token.as_str(),
            "case {case:?}: the owning file system reports the token it consumed, got {:?}",
            read.body
        );
    }

    assert_eq!(
        held_count(&router).await,
        table.len(),
        "each row must have created exactly one file system"
    );
}

/// A token names a conflict only while the file system holding it exists: once that file
/// system is deleted the token is free, and a create carrying it is accepted again.
#[tokio::test]
async fn a_token_is_free_again_once_its_file_system_is_gone() {
    let router = new_router();
    let bucket = "arn:aws:s3:::s3files-token-released";

    for (case, token) in [
        ("a short token", "released-1"),
        ("a 64-character token", longest_token().as_str()),
    ] {
        let first = create_file_system(&router, create_body(bucket, token, &json!({}))).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: the first create must be accepted, got {} / {:?}",
            first.status, first.body
        );
        let owner = first.text("fileSystemId");

        let refused = create_file_system(&router, create_body(bucket, token, &json!({}))).await;
        assert_token_conflict(&format!("{case} (while the file system is held)"), &refused, &owner);

        let deleted = call(&router, "DELETE", &format!("/file-systems/{owner}"), None).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected DeleteFileSystem to succeed, got {} / {:?}",
            deleted.status, deleted.body
        );

        let reused = create_file_system(&router, create_body(bucket, token, &json!({}))).await;
        assert_eq!(
            reused.status, 201,
            "case {case:?}: a token whose file system is gone must be accepted again, \
             got {} / {:?}",
            reused.status, reused.body
        );
        assert_ne!(
            reused.text("fileSystemId"),
            owner,
            "case {case:?}: the accepted create must mint a new file system, not revive \
             the deleted one"
        );
    }
}

/// Distinct tokens never conflict with each other, however close they are, and a create
/// that names no token is recorded under a token minted for it alone — so it can collide
/// with nothing.
#[tokio::test]
async fn distinct_tokens_and_untokened_creates_never_conflict() {
    let router = new_router();
    let bucket = "arn:aws:s3:::s3files-token-distinct";
    let longest = longest_token();
    // Pairs that differ only just enough to be different tokens: case, a trailing
    // character, the 64th character (`CreationToken` is case-sensitive per the member's
    // documentation).
    let tokens = [
        ("lower case", "neighbour".to_string()),
        ("upper case", "NEIGHBOUR".to_string()),
        ("one character longer", "neighbour2".to_string()),
        ("64 characters", longest.clone()),
        ("63 characters", longest[..63].to_string()),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, token) in &tokens {
        let created = create_file_system(&router, create_body(bucket, token, &json!({}))).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: a token no create has consumed must be accepted, got {} / {:?}",
            created.status, created.body
        );
        let id = created.text("fileSystemId");
        assert!(
            !minted.contains(&id),
            "case {case:?}: a distinct token must mint a distinct file system, got {id} again"
        );
        minted.push(id);
    }

    // Two creates carrying no token at all are two creates: the model documents the SDK
    // as generating a token per call, so nothing ties them together and neither can be
    // the conflict of the other.
    let mut untokened = Vec::new();
    for attempt in 1..=2 {
        let created = create_file_system(
            &router,
            json!({ "bucket": bucket, "roleArn": ROLE_ARN }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case \"no clientToken\" (attempt {attempt}): expected 201, got {} / {:?}",
            created.status, created.body
        );
        untokened.push(created.text("fileSystemId"));
    }
    assert_ne!(
        untokened[0], untokened[1],
        "case \"no clientToken\": each untokened create is its own file system, got {} twice",
        untokened[0]
    );

    assert_eq!(
        held_count(&router).await,
        tokens.len() + untokened.len(),
        "every accepted create must have persisted exactly one file system"
    );
}
