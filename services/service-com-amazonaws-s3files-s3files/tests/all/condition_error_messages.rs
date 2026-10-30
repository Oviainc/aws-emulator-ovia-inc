// SPDX-License-Identifier: CC0-1.0

//! The `message` S3 Files answers a failed request with, end to end over HTTP.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots: routing, the restJson1 wire shapes and
//! the modelled error envelopes are all exercised.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A `ResourceNotFoundException` states its CONDITION in one fixed sentence naming the
//!    KIND of resource that is missing, carrying no value from the request or from the
//!    store: one missing resource kind therefore reads as the same string from every
//!    operation that resolves that kind, for every id, in every id form, and whether the
//!    resource was never created or has just been deleted.
//! 2. A `ConflictException` raised because `latestVersionNumber` is not the version the
//!    next write must name is one fixed remediation sentence — re-read the version and
//!    retry — carrying neither the submitted number nor the stored one, so it is the same
//!    string however the named version is wrong and whatever version the store holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, create_access_point, created_file_system_id, encoded, file_system_arn_prefix, new_router,
    Answer,
};

/// The sentences this service answers each not-found condition with, and the one it
/// answers a stale `latestVersionNumber` with, taken from the service's own constants so
/// every operation is asserted against ONE statement of the wording.
const ACCESS_POINT_NOT_FOUND: &str =
    service_com_amazonaws_s3files_s3files::state::ACCESS_POINT_NOT_FOUND_MESSAGE;
const FILE_SYSTEM_NOT_FOUND: &str =
    service_com_amazonaws_s3files_s3files::state::FILE_SYSTEM_NOT_FOUND_MESSAGE;
const INVALID_LATEST_VERSION_NUMBER: &str =
    service_com_amazonaws_s3files_s3files::state::INVALID_LATEST_VERSION_NUMBER_MESSAGE;

/// The version number the first accepted configuration write records.
const FIRST_VERSION: i32 =
    service_com_amazonaws_s3files_s3files::state::FIRST_SYNCHRONIZATION_VERSION;

/// Well-formed ids no create ever minted, at both ends of the hex run the model's
/// patterns admit: `FileSystemId` is `^(…|fs-[0-9a-f]{17,40})$` and `AccessPointId` is
/// `^(…|fsap-[0-9a-f]{17,40})$` (smithy_query: shape FileSystemId / AccessPointId), so
/// each of these passes the boundary validator and reaches the handler's own lookup.
const UNMINTED_FS_SHORTEST: &str = "fs-0123456789abcdef0";
const UNMINTED_FS_LONGEST: &str = "fs-0123456789abcdef0123456789abcdef01234567";
const UNMINTED_AP_SHORTEST: &str = "fsap-0123456789abcdef0";
const UNMINTED_AP_LONGEST: &str = "fsap-0123456789abcdef0123456789abcdef01234567";

/// A policy document that satisfies `PutFileSystemPolicyRequest$policy` ("a valid JSON
/// document that follows IAM policy syntax"), so a put addressed to a file system this
/// service does not hold is refused for the file system rather than for its document.
fn valid_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": ["s3files:GetObject"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A configuration body for `PutSynchronizationConfiguration`: the two `@required` rule
/// lists, plus `latestVersionNumber` when the caller names one.
fn configuration(prefix: &str, version: Option<i32>) -> Value {
    let mut body = json!({
        "importDataRules": [{ "prefix": prefix, "trigger": "ON_FILE_ACCESS", "sizeLessThan": 1_048_576 }],
        "expirationDataRules": [{ "daysAfterLastAccess": 30 }],
    });
    if let Some(version) = version {
        body["latestVersionNumber"] = json!(version);
    }
    body
}

/// Assert `answer` is the modelled 404 carrying exactly `expected`, and that the sentence
/// kept nothing of the request: neither the label the call addressed nor any id prefix.
fn assert_not_found(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected the modelled 404, got {} / {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException on the wire, got {:?} / {:?}",
        answer.error_type, answer.body
    );
    assert_eq!(
        answer.text("errorCode"),
        "ResourceNotFoundException",
        "case {case:?}: errorCode is @required on ResourceNotFoundException, got {:?}",
        answer.body
    );
    let message = answer.text("message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected the fixed condition sentence {expected:?}, got {message:?}"
    );
    for fragment in ["fs-", "fsap-", "arn:"] {
        assert!(
            !message.contains(fragment),
            "case {case:?}: the sentence carries no value from the request, but it holds \
             {fragment:?}: {message:?}"
        );
    }
}

/// Assert `answer` is the modelled 409 carrying the fixed remediation sentence, which
/// holds no version number from the request or the store — so no digit at all.
fn assert_version_conflict(case: &str, answer: &Answer) {
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
    let message = answer.text("message");
    assert_eq!(
        message, INVALID_LATEST_VERSION_NUMBER,
        "case {case:?}: expected the fixed remediation sentence \
         {INVALID_LATEST_VERSION_NUMBER:?}, got {message:?}"
    );
    assert!(
        !message.chars().any(|character| character.is_ascii_digit()),
        "case {case:?}: the sentence names neither the submitted nor the stored version, \
         but it carries a number: {message:?}"
    );
}

/// The wording itself, pinned once: these are the sentences the service answers these two
/// conditions with, so a later edit that re-words one fails here rather than silently on
/// a client.
#[test]
fn each_condition_sentence_is_the_wording_the_service_answers() {
    for (case, actual, expected) in [
        (
            "an access point that does not exist",
            ACCESS_POINT_NOT_FOUND,
            "Access point does not exist.",
        ),
        (
            "a file system that does not exist",
            FILE_SYSTEM_NOT_FOUND,
            "File system does not exist.",
        ),
        (
            "a latestVersionNumber that is not the current one",
            INVALID_LATEST_VERSION_NUMBER,
            "Invalid latestVersionNumber. Retrieve the most recent latestVersionNumber and retry.",
        ),
    ] {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the sentence {expected:?}, got {actual:?}"
        );
    }
}

/// Rule 1 for the access point: every operation that resolves an `accessPointId` answers
/// the one fixed sentence, over every way the id can name no access point.
#[tokio::test]
async fn an_absent_access_point_answers_one_fixed_sentence() {
    let router = new_router();
    let file_system_id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-absent-access-point",
        Some("absent-access-point"),
    )
    .await;

    // An access point created and then deleted: the id was real a moment ago, which is
    // the condition the delete -> get -> re-delete flow turns on.
    let created = create_access_point(
        &router,
        &file_system_id,
        json!({ "posixUser": { "uid": 1000, "gid": 1000 }, "rootDirectory": { "path": "/" } }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected the modelled 200 from CreateAccessPoint, got {} / {:?}",
        created.status, created.body
    );
    let deleted_id = created.text("accessPointId");
    let deleted_arn = created.text("accessPointArn");
    let dropped = call(
        &router,
        "DELETE",
        &format!("/access-points/{deleted_id}"),
        None,
    )
    .await;
    assert_eq!(
        dropped.status, 204,
        "expected 204 from DeleteAccessPoint, got {} / {:?}",
        dropped.status, dropped.body
    );

    // An ARN naming a `fsap-` no create ever minted, under a `fs-` no create ever minted:
    // the longer `AccessPointId` form, which resolves to the same absent record.
    let unminted_arn = format!(
        "{}{UNMINTED_FS_SHORTEST}/access-point/{UNMINTED_AP_SHORTEST}",
        file_system_arn_prefix()
    );

    // The rows differ along everything the rule generalises over: how long the id's hex
    // run is, which form the request addresses it in, and whether the id was never minted
    // or has just been released by a delete.
    let labels = [
        (
            "an id no create minted, at the shortest hex the pattern admits",
            UNMINTED_AP_SHORTEST.to_string(),
        ),
        (
            "an id no create minted, at the longest hex the pattern admits",
            UNMINTED_AP_LONGEST.to_string(),
        ),
        (
            "the ARN form of an id no create minted",
            encoded(&unminted_arn),
        ),
        ("an id a delete has just released", deleted_id.clone()),
        (
            "the ARN form of an id a delete has just released",
            encoded(&deleted_arn),
        ),
    ];
    // Every row above rides in a path segment or a query parameter, so each is already in
    // the form an SDK would put there.

    let mut seen: Vec<String> = Vec::new();
    for (label_case, label) in labels {
        for (operation, answer) in [
            (
                "GetAccessPoint",
                call(&router, "GET", &format!("/access-points/{label}"), None).await,
            ),
            (
                "DeleteAccessPoint",
                call(&router, "DELETE", &format!("/access-points/{label}"), None).await,
            ),
            (
                "DeleteAccessPoint, repeated",
                call(&router, "DELETE", &format!("/access-points/{label}"), None).await,
            ),
            (
                "TagResource",
                call(
                    &router,
                    "POST",
                    &format!("/resource-tags/{label}"),
                    Some(json!({ "tags": [{ "key": "Name", "value": "absent" }] })),
                )
                .await,
            ),
            (
                "ListTagsForResource",
                call(&router, "GET", &format!("/resource-tags/{label}"), None).await,
            ),
            (
                "UntagResource",
                call(
                    &router,
                    "DELETE",
                    &format!("/resource-tags/{label}?tagKeys=Name"),
                    None,
                )
                .await,
            ),
        ] {
            let case = format!("{operation} on {label_case}");
            assert_not_found(&case, &answer, ACCESS_POINT_NOT_FOUND);
            seen.push(answer.text("message"));
        }
    }

    // Sorted first, so the count below is the number of DISTINCT strings rather than the
    // number of runs of equal ones.
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the condition answers ONE string whatever the id, its form and the operation, \
         got {seen:?}"
    );
}

/// Rule 1 for the file system: every operation that resolves a `fileSystemId` answers the
/// one fixed sentence, over every way the id can name no file system.
#[tokio::test]
async fn an_absent_file_system_answers_one_fixed_sentence() {
    let router = new_router();

    // A file system created and then deleted, so one row's id was real a moment ago.
    let deleted_id = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-absent-file-system",
        Some("absent-file-system"),
    )
    .await;
    let deleted_arn = format!("{}{deleted_id}", file_system_arn_prefix());
    let dropped = call(&router, "DELETE", &format!("/file-systems/{deleted_id}"), None).await;
    assert_eq!(
        dropped.status, 204,
        "expected 204 from DeleteFileSystem, got {} / {:?}",
        dropped.status, dropped.body
    );

    let unminted_arn = format!("{}{UNMINTED_FS_LONGEST}", file_system_arn_prefix());
    // `(case, the value as a request carries it in a member, the value in a path segment
    // or query parameter)`: a `@httpLabel` or `@httpQuery` carries an ARN
    // percent-encoded, a JSON body member carries it as it is.
    let labels = [
        (
            "an id no create minted, at the shortest hex the pattern admits",
            UNMINTED_FS_SHORTEST.to_string(),
            UNMINTED_FS_SHORTEST.to_string(),
        ),
        (
            "an id no create minted, at the longest hex the pattern admits",
            UNMINTED_FS_LONGEST.to_string(),
            UNMINTED_FS_LONGEST.to_string(),
        ),
        (
            "the ARN form of an id no create minted",
            unminted_arn.clone(),
            encoded(&unminted_arn),
        ),
        (
            "an id a delete has just released",
            deleted_id.clone(),
            deleted_id.clone(),
        ),
        (
            "the ARN form of an id a delete has just released",
            deleted_arn.clone(),
            encoded(&deleted_arn),
        ),
    ];

    let policy = valid_policy();
    let mut seen: Vec<String> = Vec::new();
    for (label_case, raw, label) in labels {
        for (operation, answer) in [
            (
                "GetFileSystem",
                call(&router, "GET", &format!("/file-systems/{label}"), None).await,
            ),
            (
                "DeleteFileSystem",
                call(&router, "DELETE", &format!("/file-systems/{label}"), None).await,
            ),
            (
                "GetFileSystemPolicy",
                call(
                    &router,
                    "GET",
                    &format!("/file-systems/{label}/policy"),
                    None,
                )
                .await,
            ),
            (
                "PutFileSystemPolicy",
                call(
                    &router,
                    "PUT",
                    &format!("/file-systems/{label}/policy"),
                    Some(json!({ "policy": policy })),
                )
                .await,
            ),
            (
                "DeleteFileSystemPolicy",
                call(
                    &router,
                    "DELETE",
                    &format!("/file-systems/{label}/policy"),
                    None,
                )
                .await,
            ),
            (
                "GetSynchronizationConfiguration",
                call(
                    &router,
                    "GET",
                    &format!("/file-systems/{label}/synchronization-configuration"),
                    None,
                )
                .await,
            ),
            (
                "PutSynchronizationConfiguration",
                call(
                    &router,
                    "PUT",
                    &format!("/file-systems/{label}/synchronization-configuration"),
                    Some(configuration("absent/", None)),
                )
                .await,
            ),
            (
                "ListAccessPoints",
                call(
                    &router,
                    "GET",
                    &format!("/access-points?fileSystemId={label}"),
                    None,
                )
                .await,
            ),
            (
                "CreateAccessPoint",
                create_access_point(&router, &raw, json!({})).await,
            ),
            (
                "ListTagsForResource",
                call(&router, "GET", &format!("/resource-tags/{label}"), None).await,
            ),
            (
                "TagResource",
                call(
                    &router,
                    "POST",
                    &format!("/resource-tags/{label}"),
                    Some(json!({ "tags": [{ "key": "Name", "value": "absent" }] })),
                )
                .await,
            ),
            (
                "UntagResource",
                call(
                    &router,
                    "DELETE",
                    &format!("/resource-tags/{label}?tagKeys=Name"),
                    None,
                )
                .await,
            ),
        ] {
            let case = format!("{operation} on {label_case}");
            assert_not_found(&case, &answer, FILE_SYSTEM_NOT_FOUND);
            seen.push(answer.text("message"));
        }
    }

    // Sorted first, so the count below is the number of DISTINCT strings rather than the
    // number of runs of equal ones.
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the condition answers ONE string whatever the id, its form and the operation, \
         got {seen:?}"
    );
}

/// Rule 2: a write naming anything other than the version the store currently holds is
/// refused with the one fixed remediation sentence — over every way the named version can
/// be wrong, and at two different stored versions, so neither the submitted nor the
/// stored number can be hiding in the string.
#[tokio::test]
async fn a_stale_latest_version_number_answers_one_fixed_sentence() {
    let router = new_router();

    // Two file systems, taken to DIFFERENT stored versions: the same condition on each
    // has to read the same way.
    let fresh = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-version-conflict-fresh",
        Some("version-conflict-fresh"),
    )
    .await;
    let advanced = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-version-conflict-advanced",
        Some("version-conflict-advanced"),
    )
    .await;

    let mut seen: Vec<String> = Vec::new();

    // A file system holding NO configuration: the next write must name no version at all,
    // so naming any is the condition.
    for named in [FIRST_VERSION, FIRST_VERSION - 1, FIRST_VERSION + 7, 0, -1] {
        let refused = put_sync(&router, &fresh, configuration("first/", Some(named))).await;
        let case = format!("creating a configuration while naming version {named}");
        assert_version_conflict(&case, &refused);
        seen.push(refused.text("message"));
        assert_eq!(
            get_sync(&router, &fresh).await.status,
            404,
            "case {case:?}: a refused create must have stored nothing"
        );
    }

    // Take each file system to a stored version: `fresh` to the first, `advanced` three
    // accepted writes on.
    for (label, writes) in [(fresh.as_str(), 1), (advanced.as_str(), 3)] {
        for write in 0..writes {
            let named = (write > 0).then(|| FIRST_VERSION + write - 1);
            let accepted = put_sync(
                &router,
                label,
                configuration(&format!("accepted{write}/"), named),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "expected write {write} on {label} to be accepted, got {} / {:?}",
                accepted.status, accepted.body
            );
        }
    }

    // The rows differ along every way a named version can fail to be the stored one:
    // absent, behind, ahead, far ahead, out of range below, and the largest `Integer`.
    for (label, current) in [
        (fresh.as_str(), FIRST_VERSION),
        (advanced.as_str(), FIRST_VERSION + 2),
    ] {
        for (row, named) in [
            ("no version at all on an update", None),
            ("the version before the stored one", Some(current - 1)),
            ("one version ahead of the stored one", Some(current + 1)),
            ("a far-ahead version", Some(current + 1_000)),
            ("zero", Some(0)),
            ("a negative version", Some(-7)),
            ("the largest Integer", Some(i32::MAX)),
        ] {
            let refused = put_sync(&router, label, configuration("rejected/", named)).await;
            let case = format!("{row} at stored version {current}");
            assert_version_conflict(&case, &refused);
            seen.push(refused.text("message"));

            // The refusal changed nothing: the stored configuration is still the one the
            // last accepted write left, at its version.
            let read = get_sync(&router, label).await;
            assert_eq!(
                read.field("latestVersionNumber").as_i64(),
                Some(i64::from(current)),
                "case {case:?}: a refused write leaves the stored version where it was, \
                 got {:?}",
                read.field("latestVersionNumber")
            );
        }
    }

    // Sorted first, so the count below is the number of DISTINCT strings rather than the
    // number of runs of equal ones.
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the condition answers ONE string whatever version was named and whatever version \
         the store holds, got {seen:?}"
    );
}

async fn put_sync(router: &MultiServiceRouter, label: &str, body: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/file-systems/{label}/synchronization-configuration"),
        Some(body),
    )
    .await
}

async fn get_sync(router: &MultiServiceRouter, label: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/file-systems/{label}/synchronization-configuration"),
        None,
    )
    .await
}
