// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateFpgaImage`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. THE CALL READS THE DESIGN CHECKPOINT THE REQUEST NAMES, so an Amazon S3 location it
//!    cannot access is refused with 400 `InaccessibleStorageLocation` naming that bucket and
//!    key, and no AFI is minted. This emulator holds no Amazon S3 objects and no operation of
//!    this API puts a design checkpoint anywhere, so every location a request can name is one
//!    it cannot read — whichever side of Amazon S3's bucket naming rules the name falls, and
//!    whatever shape the key takes.
//! 2. A FACT OF THE REQUEST IS ANSWERED AHEAD OF THAT READ: a storage location that names no
//!    bucket or no key names no object at all (`MissingParameter`), a tag specification may
//!    only name the resource this call would create and a tag and a client token have
//!    published maximums (`InvalidParameterValue`), and `DryRun` asks whether the CALLER MAY
//!    make the request (412 `DryRunOperation`).
//! 3. THE REFUSAL PERSISTS NOTHING, whatever the request carried: no AFI, no `ClientToken`
//!    the store would answer a later call from, and not a byte charged against the store's
//!    budget — so a repeat of the same request is answered exactly as the first one was.
//!
//! Each table row is a distinct input along what the rules generalise over — which bucket
//! name, which key form, which location is named, which member is left out, which ceiling is
//! passed, which optional members the request carries — so no row is the specification. The
//! bucket table is [`super::fpga_image_tasks_and_volume_io::bucket_name_rows`], the one the
//! rest of the storage-location rule is written against.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS, MAX_TAG_KEY_CHARS,
    MAX_TAG_VALUE_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};
use super::fpga_image_tasks_and_volume_io::bucket_name_rows;
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The code EC2 answers when it cannot read the Amazon S3 location a request named.
const INACCESSIBLE: &str = "InaccessibleStorageLocation";

/// The HTTP status EC2 answers a `DryRun` request with: 412, carrying `DryRunOperation`.
const DRY_RUN_STATUS: u16 = 412;

/// A bucket name inside every one of Amazon S3's general-purpose naming rules, for the rows
/// that are about something other than the bucket.
const WELL_FORMED_BUCKET: &str = "my-design-checkpoints";

/// The key of the design checkpoint tarball those rows name in it.
const CHECKPOINT_KEY: &str = "dcp/checkpoint.tar";

async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The parameters of a `CreateFpgaImage` whose design checkpoint is at `bucket`/`key`.
fn create_params(bucket: &str, key: &str) -> Params {
    vec![
        p("InputStorageLocation.Bucket", bucket),
        p("InputStorageLocation.Key", key),
    ]
}

/// The message EC2 answers `InaccessibleStorageLocation` with: its fixed text, with the
/// bucket and the key taken from the request that could not be served.
fn inaccessible_message(bucket: &str, key: &str) -> String {
    format!(
        "Error accessing resource in S3. Check permissions, naming, and ensure the bucket \
         is within the same region as the API endpoint. Bucket: {bucket}, Key: {key}."
    )
}

/// Everything the store holds that a create would have added: the AFIs, the client tokens
/// that name them, and the caller-supplied bytes charged for them. Compared against the
/// BASELINE a fresh account's store starts at, which already holds the public AMIs it can
/// see, rather than against zero.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (
        guard.fpga_images.len(),
        guard.fpga_image_client_tokens.len(),
        guard.stored_bytes(),
    )
}

/// Assert that `body` is the refusal of the location at `bucket`/`key`, and that it reports no
/// AFI of any kind.
fn assert_refused(case: &str, status: u16, body: &str, bucket: &str, key: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(body),
        INACCESSIBLE,
        "case {case:?}: expected {INACCESSIBLE}, got {body}"
    );
    let message = unescaped(&element(body, "Message").unwrap_or_default());
    assert_eq!(
        message,
        inaccessible_message(bucket, key),
        "case {case:?}: expected the refusal to name the location it could not read, \
         got {message:?}"
    );
    assert!(
        element(body, "fpgaImageId").is_none() && element(body, "fpgaImageGlobalId").is_none(),
        "case {case:?}: a refused create reports no AFI, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 1: the design checkpoint is read, and this emulator holds no S3 object
// ---------------------------------------------------------------------------

/// Every design checkpoint location a request can name is refused for the object it names:
/// the bucket names run both sides of Amazon S3's naming rules, at each published bound and
/// one past it, and the keys run the forms an object key takes.
#[tokio::test]
async fn every_design_checkpoint_location_is_refused_for_the_object_it_names() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    // `(case, the bucket, the key)`: the bucket table, each row with the same checkpoint
    // key, and then one bucket with the key forms an S3 object key takes.
    let mut rows: Vec<(String, String, String)> = bucket_name_rows()
        .into_iter()
        .map(|(case, bucket, well_formed)| {
            (
                format!("{case} (a bucket name Amazon S3 would {}accept)", if well_formed { "" } else { "not " }),
                bucket,
                CHECKPOINT_KEY.to_string(),
            )
        })
        .collect();
    for (case, key) in [
        ("a key at the root of the bucket", "checkpoint.tar"),
        ("a key several prefixes deep", "teams/fpga/dcp/2024/checkpoint.tar"),
        ("a key of one character", "a"),
        ("a key with dots and hyphens", "dcp.v2-final.tar"),
        ("a key with a space in it", "my checkpoints/design.tar"),
    ] {
        rows.push((case.to_string(), WELL_FORMED_BUCKET.to_string(), key.to_string()));
    }

    for (case, bucket, key) in rows {
        let (status, body) = op(&router, "CreateFpgaImage", &create_params(&bucket, &key)).await;
        assert_refused(&case, status, &body, &bucket, &key);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "no refused create stores an AFI, a client token or a byte, got {:?} where the store \
         started at {baseline:?}",
        stored(&engine)
    );
}

/// The refusal names the INPUT location, the one the call reads, whether or not the request
/// also names a location for the output logs — and a logs location of its own does not get a
/// request past the checkpoint it cannot read.
#[tokio::test]
async fn the_refusal_names_the_location_the_call_reads() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    // `(case, the extra parameters beyond the input location)`.
    let rows: Vec<(&str, Params)> = vec![
        ("no logs location at all", Vec::new()),
        (
            "a logs location in another bucket",
            vec![
                p("LogsStorageLocation.Bucket", "my-afi-logs"),
                p("LogsStorageLocation.Key", "logs/afi.log"),
            ],
        ),
        (
            "a logs location in the same bucket as the checkpoint",
            vec![
                p("LogsStorageLocation.Bucket", WELL_FORMED_BUCKET),
                p("LogsStorageLocation.Key", "logs/afi.log"),
            ],
        ),
        (
            "a logs location whose bucket name breaks the naming rules",
            vec![
                p("LogsStorageLocation.Bucket", "Logs_Bucket"),
                p("LogsStorageLocation.Key", "logs/afi.log"),
            ],
        ),
    ];

    for (case, extra) in rows {
        let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
        params.extend(extra);
        let (status, body) = op(&router, "CreateFpgaImage", &params).await;
        assert_refused(case, status, &body, WELL_FORMED_BUCKET, CHECKPOINT_KEY);
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "no refused create stores an AFI, a client token or a byte, got {:?} where the store \
         started at {baseline:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a fact of the request is answered ahead of the read
// ---------------------------------------------------------------------------

/// A request fault is answered as itself, before the design checkpoint is reached for: a
/// location that names no object, a tag specification naming a resource this call does not
/// create, a tag past a published maximum, a client token past its maximum, and the `DryRun`
/// probe.
#[tokio::test]
async fn a_fact_of_the_request_is_answered_before_the_checkpoint_is_read() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    // `(case, the parameters, the status, the code)`.
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "an input location that names a key but no bucket",
            vec![p("InputStorageLocation.Key", CHECKPOINT_KEY)],
            400,
            "MissingParameter",
        ),
        (
            "an input location that names a bucket but no key",
            vec![p("InputStorageLocation.Bucket", WELL_FORMED_BUCKET)],
            400,
            "MissingParameter",
        ),
        (
            "an input location whose bucket is empty",
            vec![
                p("InputStorageLocation.Bucket", ""),
                p("InputStorageLocation.Key", CHECKPOINT_KEY),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a logs location that names a bucket but no key",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("LogsStorageLocation.Bucket", "my-afi-logs"));
                params
            },
            400,
            "MissingParameter",
        ),
        (
            "a tag specification naming a volume, which this call does not create",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("TagSpecification.1.ResourceType", "volume"));
                params.push(p("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(p("TagSpecification.1.Tag.1.Value", "built"));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming an instance, which this call does not create",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("TagSpecification.1.ResourceType", "instance"));
                params.push(p("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(p("TagSpecification.1.Tag.1.Value", "built"));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key one character past the published maximum",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("TagSpecification.1.ResourceType", "fpga-image"));
                params.push(p(
                    "TagSpecification.1.Tag.1.Key",
                    "k".repeat(MAX_TAG_KEY_CHARS + 1),
                ));
                params.push(p("TagSpecification.1.Tag.1.Value", "built"));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag value one character past the published maximum",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("TagSpecification.1.ResourceType", "fpga-image"));
                params.push(p("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(p(
                    "TagSpecification.1.Tag.1.Value",
                    "v".repeat(MAX_TAG_VALUE_CHARS + 1),
                ));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a client token one character past the accepted maximum",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("ClientToken", "c".repeat(MAX_CLIENT_TOKEN_CHARS + 1)));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a DryRun probe, which asks whether the caller may make the request",
            {
                let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
                params.push(p("DryRun", "true"));
                params
            },
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = op(&router, "CreateFpgaImage", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    assert_eq!(
        stored(&engine),
        baseline,
        "a request answered for its own fault stores nothing either, got {:?} where the store \
         started at {baseline:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the refusal persists nothing, and repeats itself
// ---------------------------------------------------------------------------

/// Whatever optional members the request carries — a name, a description, tags of the AFI it
/// would have created, a client token, a logs location — the call is refused for the same
/// location and leaves the store exactly as it found it, so the same request asked twice is
/// answered twice the same way.
#[tokio::test]
async fn a_refused_create_persists_nothing_however_the_request_is_shaped() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    // `(case, the parameters beyond the input location)`.
    let rows: Vec<(&str, Params)> = vec![
        ("nothing but the input location", Vec::new()),
        (
            "a name and a description for the AFI",
            vec![p("Name", "golden-bitstream"), p("Description", "the first build")],
        ),
        (
            "tags of the fpga-image it would have created",
            vec![
                p("TagSpecification.1.ResourceType", "fpga-image"),
                p("TagSpecification.1.Tag.1.Key", "project"),
                p("TagSpecification.1.Tag.1.Value", "fpga"),
                p("TagSpecification.1.Tag.2.Key", "owner"),
                p("TagSpecification.1.Tag.2.Value", "hardware"),
            ],
        ),
        (
            "a tag specification carrying no resource type",
            vec![
                p("TagSpecification.1.Tag.1.Key", "project"),
                p("TagSpecification.1.Tag.1.Value", "fpga"),
            ],
        ),
        (
            "a client token of one character",
            vec![p("ClientToken", "t")],
        ),
        (
            "a client token at the accepted maximum",
            vec![p("ClientToken", "c".repeat(MAX_CLIENT_TOKEN_CHARS))],
        ),
        (
            "every optional member at once",
            vec![
                p("Name", "everything"),
                p("Description", "a request carrying every member"),
                p("ClientToken", "every-member"),
                p("LogsStorageLocation.Bucket", "my-afi-logs"),
                p("LogsStorageLocation.Key", "logs/afi.log"),
                p("TagSpecification.1.ResourceType", "fpga-image"),
                p("TagSpecification.1.Tag.1.Key", "project"),
                p("TagSpecification.1.Tag.1.Value", "fpga"),
            ],
        ),
    ];

    for (case, extra) in rows {
        let mut params = create_params(WELL_FORMED_BUCKET, CHECKPOINT_KEY);
        params.extend(extra);

        let mut answers = Vec::new();
        for attempt in 0..2 {
            let (status, body) = op(&router, "CreateFpgaImage", &params).await;
            let attempted = format!("{case} (attempt {attempt})");
            assert_refused(&attempted, status, &body, WELL_FORMED_BUCKET, CHECKPOINT_KEY);
            answers.push(error_code(&body));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {case:?}: the same request is answered the same way twice, got {answers:?}"
        );
        assert_eq!(
            stored(&engine),
            baseline,
            "case {case:?}: the refusal leaves the store as it found it, got {:?} where it \
             started at {baseline:?}",
            stored(&engine)
        );
    }

    // Nothing was minted, so nothing can be reached: the id space is empty, and the
    // operations that resolve an AFI of this account answer for an id they hold nothing for.
    for (action, params) in [
        ("DeleteFpgaImage", vec![p("FpgaImageId", "afi-0123456789abcdef0")]),
        (
            "ResetFpgaImageAttribute",
            vec![
                p("FpgaImageId", "afi-0123456789abcdef0"),
                p("Attribute", "loadPermission"),
            ],
        ),
    ] {
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "{action} after the refused creates: expected HTTP 400, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFpgaImageID.NotFound",
            "{action} after the refused creates: expected InvalidFpgaImageID.NotFound, \
             got {body}"
        );
    }
}
