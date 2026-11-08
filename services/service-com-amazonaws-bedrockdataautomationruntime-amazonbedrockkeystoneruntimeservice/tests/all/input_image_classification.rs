// SPDX-License-Identifier: CC0-1.0

//! A job's terminal status is settled on what the service established about its INPUT
//! ASSET.
//!
//! The RULE asserted here: a job's work is to read the asset
//! `inputConfiguration.s3Uri` names and process it, so an asset it cannot process ends
//! the job in the modelled terminal failure status attributable to the request
//! (`ClientError`, `smithy_query` shape `AutomationJobStatus`) carrying the `errorType` of
//! that failure class, the fixed `errorMessage` for it, and `jobCompletionTime`; an asset
//! it can process ends the job in `Success` with neither error member. For an asset that
//! IS an image, "cannot process" means a format data automation does not read, bytes that
//! claim an image format and carry no readable header, or dimensions outside the pixel
//! bounds it processes — and the answer is `errorType` `InvalidImage` with the message
//! `Image format is invalid or not supported.`, both spelled out here rather than read
//! from the crate so a wrong constant cannot make this pass.
//!
//! The rows span what the rule generalises over: the asset's FORMAT (PNG, JPEG, a format
//! data automation does not read, a document that is no image at all), its SIZE on each
//! side (one pixel, each published bound, one past each bound, a non-square image with one
//! side out of bounds) and bytes that decode to no image at all. An asset of another
//! modality is the control: the image rule says nothing about it, so such a job completes.
//!
//! Amazon S3 is mounted beside this service on one engine — the way the assembled emulator
//! runs them — because the asset is an object of it: every row stages its asset with this
//! service's own object store, exactly as a caller would have uploaded it before
//! submitting.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`).
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for, and the region the staged bucket is created
/// in: the handler addresses the object store in the region it read off the credential
/// scope, so the two have to agree.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` whose account is AWS's own (`smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The data-automation project every submission names, arranged in the owning service's
/// store by [`crate::support::seed_project`].
const PROJECT_ARN: &str = "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/assets";

/// The bucket every asset below is staged in.
const ASSET_BUCKET: &str = "image-assets";

/// The `errorType` and `errorMessage` AWS answers for an image data automation cannot
/// process, as the behavioural corpus recorded them.
const INVALID_IMAGE_ERROR_TYPE: &str = "InvalidImage";
const INVALID_IMAGE_MESSAGE: &str = "Image format is invalid or not supported.";

/// The pixel bounds an image asset has to sit inside, each side. The floor is what the
/// corpus pins at its extreme (a one-pixel image is refused); both are the bounds this
/// emulator enforces for the class (`bda::state::input_image`).
const MIN_SIDE: u32 = 64;
const MAX_SIDE: u32 = 4_096;

/// Long enough for a job submitted during the test to have been owed its terminal
/// transition: both emulated spans, with room for a slow machine.
fn past_the_lifecycle() -> Duration {
    bda::state::CREATED_FOR + bda::state::IN_PROGRESS_FOR + Duration::from_millis(500)
}

/// What the terminal read of one row's job must say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The asset is processable, or nothing about it could be established.
    Success,
    /// The asset is an image this service cannot process.
    InvalidImage,
}

/// One row: the case, the object key its asset is staged at, the bytes staged there, and
/// the outcome its job must reach.
struct AssetCase {
    case: String,
    key: String,
    bytes: Vec<u8>,
    outcome: Outcome,
}

/// A PNG of `width` x `height`, encoded by the same crate that reads an asset's header —
/// a real image file, not a hand-rolled byte string.
fn png(width: u32, height: u32) -> Vec<u8> {
    encoded(width, height, ::image::ImageFormat::Png)
}

/// A JPEG of `width` x `height`.
fn jpeg(width: u32, height: u32) -> Vec<u8> {
    encoded(width, height, ::image::ImageFormat::Jpeg)
}

/// One opaque grey image of `width` x `height`, in `format`.
fn encoded(width: u32, height: u32, format: ::image::ImageFormat) -> Vec<u8> {
    let image = ::image::RgbImage::from_pixel(width, height, ::image::Rgb([128, 128, 128]));
    let mut bytes: Vec<u8> = Vec::new();
    ::image::DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap_or_else(|error| panic!("encode a {width}x{height} {format:?}: {error}"));
    bytes
}

/// The table, built here because the encoded images are not constants.
fn rows() -> Vec<AssetCase> {
    let mut rows = vec![
        AssetCase {
            case: "a one-pixel PNG".to_string(),
            key: "input/one-pixel.png".to_string(),
            bytes: png(1, 1),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "a PNG one pixel under the floor on both sides".to_string(),
            key: "input/under-floor.png".to_string(),
            bytes: png(MIN_SIDE - 1, MIN_SIDE - 1),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "a PNG exactly at the floor".to_string(),
            key: "input/at-floor.png".to_string(),
            bytes: png(MIN_SIDE, MIN_SIDE),
            outcome: Outcome::Success,
        },
        AssetCase {
            case: "a JPEG exactly at the floor".to_string(),
            key: "input/at-floor.jpg".to_string(),
            bytes: jpeg(MIN_SIDE, MIN_SIDE),
            outcome: Outcome::Success,
        },
        AssetCase {
            case: "a wide JPEG whose shorter side is under the floor".to_string(),
            key: "input/thin.jpg".to_string(),
            bytes: jpeg(640, MIN_SIDE - 32),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "a PNG comfortably inside both bounds".to_string(),
            key: "input/ordinary.png".to_string(),
            bytes: png(800, 600),
            outcome: Outcome::Success,
        },
        AssetCase {
            case: "a PNG one pixel over the ceiling on one side".to_string(),
            key: "input/over-ceiling.png".to_string(),
            bytes: png(MAX_SIDE + 1, MIN_SIDE),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "bytes carrying a PNG signature and no readable header".to_string(),
            key: "input/truncated.png".to_string(),
            // The PNG magic number, and then nothing a header can be read out of.
            bytes: b"\x89PNG\r\n\x1a\nnot-a-header".to_vec(),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "an image in a format data automation does not read".to_string(),
            key: "input/bitmap.bmp".to_string(),
            // A 1x1 Windows bitmap: `image` recognises the format from these magic
            // numbers, and it is not one of the two data automation reads.
            bytes: bitmap_1x1(),
            outcome: Outcome::InvalidImage,
        },
        AssetCase {
            case: "a document, which is no image at all".to_string(),
            key: "input/statement.pdf".to_string(),
            bytes: b"%PDF-1.7 emulator data automation asset".to_vec(),
            outcome: Outcome::Success,
        },
        AssetCase {
            case: "an asset whose bytes are in no format this emulator recognises".to_string(),
            key: "input/opaque.bin".to_string(),
            bytes: vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
            outcome: Outcome::Success,
        },
    ];
    // A PNG at the ceiling itself is encoded only once, because a 4096-pixel side is the
    // largest image in the table: it pins the published bound from the inside.
    rows.push(AssetCase {
        case: "a PNG exactly at the ceiling on one side".to_string(),
        key: "input/at-ceiling.png".to_string(),
        bytes: png(MAX_SIDE, MIN_SIDE),
        outcome: Outcome::Success,
    });
    rows
}

/// A 1x1 24-bit Windows bitmap: the `BM` signature, a 14-byte file header, a 40-byte
/// `BITMAPINFOHEADER` and one padded pixel.
fn bitmap_1x1() -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&58u32.to_le_bytes()); // file size
    bytes.extend_from_slice(&0u16.to_le_bytes()); // reserved
    bytes.extend_from_slice(&0u16.to_le_bytes()); // reserved
    bytes.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    bytes.extend_from_slice(&40u32.to_le_bytes()); // header size
    bytes.extend_from_slice(&1i32.to_le_bytes()); // width
    bytes.extend_from_slice(&1i32.to_le_bytes()); // height
    bytes.extend_from_slice(&1u16.to_le_bytes()); // planes
    bytes.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    bytes.extend_from_slice(&0u32.to_le_bytes()); // compression
    bytes.extend_from_slice(&4u32.to_le_bytes()); // image size
    bytes.extend_from_slice(&0i32.to_le_bytes()); // horizontal resolution
    bytes.extend_from_slice(&0i32.to_le_bytes()); // vertical resolution
    bytes.extend_from_slice(&0u32.to_le_bytes()); // palette colours
    bytes.extend_from_slice(&0u32.to_le_bytes()); // important colours
    bytes.extend_from_slice(&[0x80, 0x80, 0x80, 0x00]); // one padded pixel
    bytes
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// One emulator: this service and Amazon S3 on a single shared engine behind the
/// production router, with the referenced project arranged.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "image-classification-assets");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(s3::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Put one object at `key` in [`ASSET_BUCKET`], creating that bucket the first time.
fn stage_asset(engine: &Arc<engine::InMemoryEngine>, key: &str, bytes: Vec<u8>) {
    let state = s3::state::service_state(&**engine).expect("the object store on this engine");
    let mut store = state.lock();
    if store.bucket(ASSET_BUCKET).is_none() {
        assert_eq!(
            store.create_bucket(ASSET_BUCKET, REGION),
            s3::state::BucketCreation::Created,
            "the staging bucket {ASSET_BUCKET:?} must be created"
        );
    }
    let object = s3::state::Object::stored(bytes, None);
    assert_eq!(
        store.put_object(ASSET_BUCKET, key, object),
        Some(s3::state::ObjectWrite::Created),
        "the asset {key:?} must be staged in {ASSET_BUCKET:?}"
    );
}

/// Call one operation of this service the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Submit one job pointing at `key` in the staging bucket, and answer its ARN.
async fn submit(router: &MultiServiceRouter, case: &str, key: &str) -> String {
    let answer = call(
        router,
        "InvokeDataAutomationAsync",
        json!({
            "inputConfiguration": {"s3Uri": format!("s3://{ASSET_BUCKET}/{key}")},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/classified/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
        .to_string()
}

/// Read one job's status, asserting the call itself succeeded.
async fn read(router: &MultiServiceRouter, case: &str, invocation_arn: &str) -> Value {
    let answer = call(
        router,
        "GetDataAutomationStatus",
        json!({"invocationArn": invocation_arn}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn an_image_asset_the_service_cannot_process_fails_the_job_with_invalid_image() {
    let (router, engine) = emulator();
    let rows = rows();

    // Stage every asset, then submit every job: one wait covers all of them, because each
    // was submitted before it.
    for row in &rows {
        stage_asset(&engine, &row.key, row.bytes.clone());
    }
    let mut submitted: Vec<(&AssetCase, String)> = Vec::new();
    for row in &rows {
        let arn = submit(&router, &row.case, &row.key).await;
        submitted.push((row, arn));
    }
    tokio::time::sleep(past_the_lifecycle()).await;

    for (row, arn) in &submitted {
        let case = &row.case;
        let body = read(&router, case, arn).await;
        let want_status = match row.outcome {
            Outcome::Success => "Success",
            Outcome::InvalidImage => "ClientError",
        };
        assert_eq!(
            body["status"].as_str(),
            Some(want_status),
            "case {case:?}: expected {want_status:?}, got {body}"
        );
        // A job that finished reports when it finished, whichever way it went.
        assert!(
            !body["jobCompletionTime"].is_null(),
            "case {case:?}: a finished job must report a completion time, got {body}"
        );

        match row.outcome {
            Outcome::Success => assert!(
                body["errorType"].is_null() && body["errorMessage"].is_null(),
                "case {case:?}: a job that completed carries no error members, got {body}"
            ),
            Outcome::InvalidImage => {
                assert_eq!(
                    body["errorType"].as_str(),
                    Some(INVALID_IMAGE_ERROR_TYPE),
                    "case {case:?}: expected errorType {INVALID_IMAGE_ERROR_TYPE:?}, got {body}"
                );
                assert_eq!(
                    body["errorMessage"].as_str(),
                    Some(INVALID_IMAGE_MESSAGE),
                    "case {case:?}: expected the fixed message for this class, got {body}"
                );
            }
        }

        // The outcome is terminal: a later read answers the same way, so a read never
        // changes what a later caller sees.
        let again = read(&router, case, arn).await;
        assert_eq!(
            (again["status"].clone(), again["errorMessage"].clone()),
            (body["status"].clone(), body["errorMessage"].clone()),
            "case {case:?}: a later read must report the same outcome"
        );
    }
}

#[tokio::test]
async fn an_asset_replaced_by_a_processable_image_before_the_job_runs_completes() {
    // The classification happens when the job RUNS, not when the submission is accepted:
    // the asset the job reads is the asset the store holds then.
    let (router, engine) = emulator();
    let case = "an asset replaced before the job ran";
    let key = "input/replaced.png";
    stage_asset(&engine, key, png(1, 1));
    let arn = submit(&router, case, key).await;

    // Overwrite the one-pixel image with one the service can process, before the clock
    // owes the job its terminal transition.
    let state = s3::state::service_state(&*engine).expect("the object store on this engine");
    assert_eq!(
        state.lock().put_object(
            ASSET_BUCKET,
            key,
            s3::state::Object::stored(png(MIN_SIDE, MIN_SIDE), None),
        ),
        Some(s3::state::ObjectWrite::Replaced),
        "case {case:?}: the asset must be replaced before the job runs"
    );
    tokio::time::sleep(past_the_lifecycle()).await;

    let body = read(&router, case, &arn).await;
    assert_eq!(
        body["status"].as_str(),
        Some("Success"),
        "case {case:?}: the job must be settled on the asset it read, got {body}"
    );
    assert!(
        body["errorType"].is_null() && body["errorMessage"].is_null(),
        "case {case:?}: a job that completed carries no error members, got {body}"
    );
}
