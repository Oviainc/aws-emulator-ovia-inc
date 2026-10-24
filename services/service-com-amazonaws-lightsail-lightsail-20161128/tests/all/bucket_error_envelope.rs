// SPDX-License-Identifier: CC0-1.0

//! Lightsail's rejection envelope for a BUCKET request that names something the
//! account does not hold, driven over the wire through the same front door a
//! client uses.
//!
//! The rule under test is the service-wide one `instance_error_envelope.rs` pins
//! for instances, applied to the bucket surface: when Lightsail cannot find what
//! a request names it answers **HTTP 400** with the wire error
//! `NotFoundException`, the body member `code` set to `DoesNotExist` and a
//! `message` built from the request — `The <ResourceType> does not exist:
//! <name>` for a resource of the account. The model declares `@httpError(404)`
//! on `NotFoundException`, but a Lightsail SDK client maps the failure from the
//! body's `code`, and the recorded endpoint answers 400 — so 400 is the wire
//! status, and it is the same envelope on every path, reporting or mutating, for
//! every resource type the service serves.
//!
//! So every case is a TABLE: one row per bucket operation that resolves an
//! identifier, crossed with names that differ in length (the modelled minimum,
//! the modelled maximum and lengths between), digits and separators; plus the
//! identifier a bucket holds INSIDE it (an access key id), the Region dimension
//! a bucket's name is only reachable through, and the deleted-then-read path.
//! Refusals are also asserted to change nothing a later caller sees.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The wire error code (awsJson `__type`) of that rejection: the modelled shape's
/// own name.
const NOT_FOUND: &str = "NotFoundException";

/// The wire error code the centralized constraint walk refuses a request with,
/// before any handler runs — a different failure from the one under test.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The modelled `ResourceType` member a bucket is reported under, which is what
/// the not-found message names.
const BUCKET_RESOURCE_TYPE: &str = "Bucket";

/// A published bucket bundle id, so a create in these cases is accepted for
/// reasons that have nothing to do with the rule under test.
const BUNDLE_ID: &str = "small_1_0";

/// An access key id shaped like one the service mints, used where the case needs
/// a syntactically acceptable `accessKeyId` that names no key.
const ABSENT_ACCESS_KEY_ID: &str = "EMULATORAKAAAAAAAAAA";

/// Bucket names nothing created, differing along what the rule generalises over:
/// the length at each modelled bound, the digits and the separators.
///
/// SOURCE: the modelled `BucketName` constraints —
/// `smithy_query(query="operation",operation="GetBuckets")` reports
/// `length 3..54, pattern ^[a-z0-9][a-z0-9-]{1,52}[a-z0-9]$` — so each row is
/// inside them and the centralized walk passes it through to the handler. The
/// issue's own recorded name is one row, not the case.
fn missing_names() -> Vec<String> {
    vec![
        // The modelled minimum length.
        "ab0".to_string(),
        "never-created".to_string(),
        "bucket-with-digits-0123456789".to_string(),
        // The modelled maximum length: 1 + 52 + 1 characters.
        format!("a{}z", "b".repeat(52)),
        "0-starts-and-ends-with-a-digit-9".to_string(),
    ]
}

/// Bucket names just OUTSIDE the modelled `BucketName` length bounds, which the
/// centralized constraint walk refuses before any handler runs — so they are
/// asserted to be a validation refusal and not the not-found envelope.
fn names_outside_the_bounds() -> Vec<String> {
    vec![
        // One short of the modelled minimum of 3.
        "ab".to_string(),
        // One past the modelled maximum of 54.
        format!("a{}z", "b".repeat(53)),
    ]
}

/// The bucket operations that resolve `bucketName` against the store, as
/// `(operation, the rest of the request body)` — every implemented one,
/// reporting and mutating alike.
fn bucket_operations() -> Vec<(&'static str, Value)> {
    vec![
        ("GetBuckets", json!({})),
        ("GetBucketAccessKeys", json!({})),
        ("CreateBucketAccessKey", json!({})),
        (
            "DeleteBucketAccessKey",
            json!({"accessKeyId": ABSENT_ACCESS_KEY_ID}),
        ),
        ("DeleteBucket", json!({})),
    ]
}

/// The request body for `operation` addressing the bucket `name`.
fn request(name: &str, extra: &Value) -> Value {
    let mut body = json!({"bucketName": name});
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Assert `status`/`body` is Lightsail's `DoesNotExist` envelope carrying
/// `message`.
fn assert_does_not_exist(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        NOT_FOUND,
        "case {case}: expected the wire error {NOT_FOUND}, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {body:?}"
    );
}

/// Create the bucket `name` in `region`, requiring success.
async fn create_bucket(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUNDLE_ID}),
        )
        .await;
}

/// Every bucket name `GetBuckets` reports in `region`, walking every page.
async fn listed_names(emulator: &Emulator, region: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok_at(None, region, "GetBuckets", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok_at(None, region, "GetBuckets", json!({})).await,
        };
        for bucket in body["buckets"].as_array().unwrap_or_else(|| {
            panic!("case listing buckets: expected a buckets array, got {body:?}")
        }) {
            names.push(
                bucket["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every listed bucket carries a name: {bucket:?}"))
                    .to_string(),
            );
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            names.len() < 1_000,
            "case listing buckets: the walk did not terminate, got {names:?}"
        );
    }
    names
}

// ---------------------------------------------------------------------------
// The bucket name
// ---------------------------------------------------------------------------

/// RULE: every bucket operation that resolves `bucketName` against the store
/// answers the same `DoesNotExist` envelope when nothing of that name is stored,
/// with a message naming the resource's `ResourceType` and the requested name.
#[tokio::test]
async fn every_unresolvable_bucket_name_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    // One bucket that DOES exist, so the refusals below are about the name the
    // request carried and not about an empty store.
    create_bucket(&emulator, None, "surviving-bucket").await;

    for (operation, extra) in bucket_operations() {
        for name in missing_names() {
            let case = format!("{operation} on {name:?}");
            let (status, body) = emulator.call(operation, request(&name, &extra)).await;
            assert_does_not_exist(
                &case,
                status,
                &body,
                &format!("The {BUCKET_RESOURCE_TYPE} does not exist: {name}"),
            );
        }
    }

    // Every refusal changed nothing a later caller sees: the one stored bucket is
    // still the only one, and still readable.
    assert_eq!(
        listed_names(&emulator, None).await,
        vec!["surviving-bucket".to_string()],
        "case after every unresolvable bucket call: the store must be unchanged"
    );
    let read = emulator
        .ok("GetBuckets", json!({"bucketName": "surviving-bucket"}))
        .await;
    assert_eq!(
        read["buckets"][0]["name"].as_str(),
        Some("surviving-bucket"),
        "case after every unresolvable bucket call: the stored bucket must still read back, \
         got {read:?}"
    );
    let keys = emulator
        .ok(
            "GetBucketAccessKeys",
            json!({"bucketName": "surviving-bucket"}),
        )
        .await;
    assert_eq!(
        keys["accessKeys"].as_array().map(Vec::len),
        Some(0),
        "case after every unresolvable bucket call: no refused mint may have stored a key, \
         got {keys:?}"
    );
}

/// RULE: the envelope is the HANDLER's answer for a name the store does not hold,
/// so a name outside the modelled `BucketName` bounds is a different failure —
/// the centralized constraint walk refuses it before the handler runs with its
/// own 400 `ValidationException`, and never as `DoesNotExist`.
#[tokio::test]
async fn a_name_outside_the_modelled_bounds_is_a_validation_refusal() {
    let emulator = Emulator::new();

    for (operation, extra) in bucket_operations() {
        for name in names_outside_the_bounds() {
            let case = format!("{operation} on the out-of-bounds name {name:?}");
            let (status, body) = emulator.call(operation, request(&name, &extra)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                VALIDATION_EXCEPTION,
                "case {case}: expected the boundary walk's {VALIDATION_EXCEPTION}, got {body:?}"
            );
            assert_ne!(
                body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: a constraint refusal is not the not-found envelope, got {body:?}"
            );
        }
    }
}

/// RULE: a bucket that existed and was deleted is refused exactly like one that
/// never existed — the envelope is a function of what the store holds NOW.
#[tokio::test]
async fn a_deleted_bucket_is_refused_like_one_that_never_existed() {
    let emulator = Emulator::new();
    let name = "short-lived-bucket";
    create_bucket(&emulator, None, name).await;
    emulator.ok("DeleteBucket", json!({"bucketName": name})).await;

    for (operation, extra) in bucket_operations() {
        let case = format!("{operation} after the delete");
        let (status, body) = emulator.call(operation, request(name, &extra)).await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!("The {BUCKET_RESOURCE_TYPE} does not exist: {name}"),
        );
    }
}

/// RULE: a bucket record answers only in the Region the request is made in, so
/// the same name is unresolvable — and carries the same envelope — from any other
/// Region.
#[tokio::test]
async fn a_bucket_of_another_region_answers_the_same_envelope() {
    let emulator = Emulator::new();
    let name = "regional-only-bucket";
    create_bucket(&emulator, Some("us-east-2"), name).await;

    // Reporting operations only, so the bucket the case depends on survives every
    // row; the mutating paths are covered by the table above.
    let reporting: &[(&str, Value)] = &[
        ("GetBuckets", json!({})),
        ("GetBucketAccessKeys", json!({})),
    ];
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        for (operation, extra) in reporting {
            let case = format!("{operation} in {region}");
            let (status, body) = emulator
                .call_in(Some(region), operation, request(name, extra))
                .await;
            assert_does_not_exist(
                &case,
                status,
                &body,
                &format!("The {BUCKET_RESOURCE_TYPE} does not exist: {name}"),
            );
        }
    }

    // The Region that holds it still does.
    let read = emulator
        .ok_at(None, Some("us-east-2"), "GetBuckets", json!({"bucketName": name}))
        .await;
    assert_eq!(
        read["buckets"][0]["name"].as_str(),
        Some(name),
        "case the holding Region: the bucket must still read back, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// An identifier INSIDE a bucket
// ---------------------------------------------------------------------------

/// RULE: the same envelope answers an identifier the service resolves INSIDE a
/// resource it did find — an access key id the bucket does not hold — with the
/// wording for a thing that is not a resource of the modelled `ResourceType`
/// enum, and the bucket keeps the keys it has.
#[tokio::test]
async fn an_access_key_the_bucket_does_not_hold_answers_the_same_envelope() {
    let emulator = Emulator::new();
    let bucket = "keyed-bucket";
    create_bucket(&emulator, None, bucket).await;
    let minted = emulator
        .ok("CreateBucketAccessKey", json!({"bucketName": bucket}))
        .await["accessKey"]["accessKeyId"]
        .as_str()
        .expect("a minted key carries an accessKeyId")
        .to_string();

    // Ids differing along what the rule generalises over: a plausible unminted
    // id, the minted id with its last character changed, a lower-case form and a
    // single non-space character (the whole of what the modelled
    // `NonEmptyString` pattern `\S` requires).
    let mut rows = vec![
        ABSENT_ACCESS_KEY_ID.to_string(),
        format!("{}!", &minted[..minted.len() - 1]),
        minted.to_lowercase(),
        "x".to_string(),
    ];
    rows.retain(|id| id != &minted);

    for id in &rows {
        let case = format!("DeleteBucketAccessKey on {id:?}");
        let (status, body) = emulator
            .call(
                "DeleteBucketAccessKey",
                json!({"bucketName": bucket, "accessKeyId": id}),
            )
            .await;
        assert_does_not_exist(
            &case,
            status,
            &body,
            &format!(
                "The bucket access key does not exist: {id} is not an access key of the \
                 Bucket {bucket}"
            ),
        );
    }

    // Every refusal left the key the bucket does hold.
    let keys = emulator
        .ok("GetBucketAccessKeys", json!({"bucketName": bucket}))
        .await;
    assert_eq!(
        keys["accessKeys"]
            .as_array()
            .map(|held| held
                .iter()
                .filter_map(|key| key["accessKeyId"].as_str())
                .collect::<Vec<_>>()),
        Some(vec![minted.as_str()]),
        "case after every unresolvable key: the bucket must still hold its key, got {keys:?}"
    );

    // And the one id that DOES name a key is served, not refused — the envelope
    // is about what the bucket holds, not about the shape of the id.
    emulator
        .ok(
            "DeleteBucketAccessKey",
            json!({"bucketName": bucket, "accessKeyId": minted}),
        )
        .await;
}
