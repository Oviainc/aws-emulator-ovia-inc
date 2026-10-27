// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateRestoreImageTask`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. THE BUCKET NAME IS READ AS A REQUEST PARAMETER FIRST: a value outside Amazon S3's
//!    published general-purpose bucket naming rules is not a bucket name at all, so the call
//!    is refused 400 `InvalidRequest` — a permanent fault of the request — with the one
//!    message EC2 answers such a name with, and nothing is looked for;
//! 2. A NAME THAT COULD BE A BUCKET IS REFUSED FOR THE OBJECT IT NAMES: 400
//!    `InaccessibleStorageLocation`, whose message names the bucket and the key the request
//!    gave. "Restores an AMI from an Amazon S3 object that was previously created by using
//!    CreateStoreImageTask" (`CreateRestoreImageTask`): the object is the INPUT of the
//!    restore, and this emulator holds no Amazon S3 object for any location to name;
//! 3. A FACT OF THE REQUEST IS ANSWERED AHEAD OF THAT READ: a key that names no object is
//!    `MissingParameter`, and a tag specification may only name the AMI and the snapshots
//!    this call would tag (`InvalidParameterValue`);
//! 4. NO AMI IS MINTED and nothing is stored, whatever the request carried: an AMI restored
//!    from an object nothing read would be an image this emulator invented.
//!
//! Each table row is a distinct input along what the rules generalise over — which naming
//! rule the bucket name breaks and at which published bound, which form the object key takes,
//! which resource type the tags name — so no row is the specification. The bucket table is
//! [`super::fpga_image_tasks_and_volume_io::bucket_name_rows`], the one every
//! storage-location rule in this crate is written against; `CreateFpgaImage` runs it too
//! ([`super::create_fpga_image`]) and answers the unreadable-location refusal for a malformed
//! name, which is that operation's own recorded behaviour and not this one's.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};
use super::fpga_image_tasks_and_volume_io::bucket_name_rows;
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The code EC2 answers for a request that is invalid in a way no member-specific code of its
/// own covers — what a malformed bucket name is reported as.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The code EC2 answers when it cannot read the Amazon S3 location a request named.
const INACCESSIBLE: &str = "InaccessibleStorageLocation";

/// The message EC2 answers a bucket name that is no bucket name with, spelled out here as the
/// recorded response gives it rather than read back out of the handler, so a wording
/// regression fails this suite. Fixed text: it names nothing from the request.
const MALFORMED_BUCKET_MESSAGE: &str = "The S3 bucket name contains invalid characters.";

/// A bucket name inside every one of Amazon S3's general-purpose naming rules, for the rows
/// that are about something other than the bucket.
const WELL_FORMED_BUCKET: &str = "my-stored-amis";

/// The key of the stored AMI object those rows name in it.
const STORED_OBJECT_KEY: &str = "ami/stored.bin";

async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The parameters of a `CreateRestoreImageTask` that restores the object at `bucket`/`key`.
fn restore_params(bucket: &str, key: &str) -> Params {
    vec![p("Bucket", bucket), p("ObjectKey", key)]
}

/// The message EC2 answers `InaccessibleStorageLocation` with: its fixed text, with the bucket
/// and the key taken from the request that could not be served.
fn inaccessible_message(bucket: &str, key: &str) -> String {
    format!(
        "Error accessing resource in S3. Check permissions, naming, and ensure the bucket \
         is within the same region as the API endpoint. Bucket: {bucket}, Key: {key}."
    )
}

/// What the store holds that a restore would have added: the AMIs it can see and the
/// caller-supplied bytes charged against its budget. Compared against the BASELINE a fresh
/// account's store starts at, which already holds the public AMIs, rather than against zero.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (guard.images.len(), guard.stored_bytes())
}

/// Assert that `body` reports no restored AMI of any kind.
fn assert_no_image(case: &str, body: &str) {
    assert!(
        element(body, "imageId").is_none(),
        "case {case:?}: a refused restore reports no AMI, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 1: the bucket name is a request parameter, read before anything else
// ---------------------------------------------------------------------------

/// A bucket name outside Amazon S3's naming rules is a malformed parameter (`InvalidRequest`),
/// while one inside them gets as far as the object and is refused for the location
/// (`InaccessibleStorageLocation`): the two refusals are what a caller branches on, so which
/// one a name gets follows only from whether it is a bucket name at all.
#[tokio::test]
async fn a_bucket_name_outside_the_naming_rules_is_a_malformed_parameter() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    for (case, bucket, well_formed) in bucket_name_rows() {
        let (status, body) = op(
            &router,
            "CreateRestoreImageTask",
            &restore_params(&bucket, STORED_OBJECT_KEY),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for bucket {bucket:?}, got {status} with \
             body {body}"
        );
        let (expected_code, expected_message) = if well_formed {
            (
                INACCESSIBLE,
                inaccessible_message(&bucket, STORED_OBJECT_KEY),
            )
        } else {
            (INVALID_REQUEST, MALFORMED_BUCKET_MESSAGE.to_string())
        };
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code} for bucket {bucket:?}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?} for bucket {bucket:?}, \
             got {message:?}"
        );
        assert_no_image(case, &body);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "no refused restore stores an AMI or a byte, got {:?} where the store started at \
         {baseline:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a readable bucket name is refused for the object it names
// ---------------------------------------------------------------------------

/// Every stored-AMI object a request can name is refused for the object: the keys run the
/// forms an Amazon S3 object key takes, and each refusal names the location it could not read.
#[tokio::test]
async fn every_stored_object_the_request_can_name_is_refused_for_the_object() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    // The rows differ along the FORM of the object key, which is what the rule generalises
    // over: `ObjectKey` is a plain `String` in the model, so a key is any object name.
    for (case, key) in [
        ("a key at the root of the bucket", "stored.bin"),
        ("a key several prefixes deep", "teams/ami/2026/stored.bin"),
        ("a key of one character", "a"),
        ("a key with dots and hyphens", "ami.v2-final.bin"),
        ("a key with a space in it", "my amis/stored.bin"),
    ] {
        let (status, body) = op(
            &router,
            "CreateRestoreImageTask",
            &restore_params(WELL_FORMED_BUCKET, key),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INACCESSIBLE,
            "case {case:?}: expected {INACCESSIBLE}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            inaccessible_message(WELL_FORMED_BUCKET, key),
            "case {case:?}: expected the refusal to name the location it could not read, \
             got {message:?}"
        );
        assert_no_image(case, &body);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "no refused restore stores an AMI or a byte, got {:?} where the store started at \
         {baseline:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a fact of the request is answered ahead of the read
// ---------------------------------------------------------------------------

/// A key that names no object, and a tag specification naming a resource this call does not
/// tag, are faults of the request: each is answered with its own code rather than reported as
/// a location that could not be read.
#[tokio::test]
async fn a_request_fault_is_answered_with_its_own_code() {
    let (router, _engine) = emulator();

    // `(case, the parameters, the code)`: the rows differ along WHICH member is at fault and
    // which resource type the tags name.
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            "an object key that names no object",
            restore_params(WELL_FORMED_BUCKET, ""),
            "MissingParameter",
        ),
        (
            "tags for the AMI it restores",
            tagged_params("image"),
            INACCESSIBLE,
        ),
        (
            "tags for the snapshots it restores",
            tagged_params("snapshot"),
            INACCESSIBLE,
        ),
        (
            "tags for a volume, which this call does not restore",
            tagged_params("volume"),
            "InvalidParameterValue",
        ),
        (
            "tags for an instance, which this call does not restore",
            tagged_params("instance"),
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in rows {
        let (status, body) = op(&router, "CreateRestoreImageTask", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_no_image(case, &body);
    }
}

/// The parameters of a restore whose one tag specification names `resource_type`.
fn tagged_params(resource_type: &str) -> Params {
    let mut params = restore_params(WELL_FORMED_BUCKET, STORED_OBJECT_KEY);
    params.extend([
        p("TagSpecification.1.ResourceType", resource_type),
        p("TagSpecification.1.Tag.1.Key", "Name"),
        p("TagSpecification.1.Tag.1.Value", "restored"),
    ]);
    params
}
