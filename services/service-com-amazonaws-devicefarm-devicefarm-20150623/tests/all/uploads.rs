// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the Device Farm **upload** operations and the content plane
//! they publish: `CreateUpload`, the `PUT` to the locator it hands back (`Upload$url`), and
//! `GetUpload`.
//!
//! Every case drives the real wire path through the shared harness in [`crate::projects`]
//! — `POST /` with `X-Amz-Target: DeviceFarm_20150623.<Operation>` for the modelled
//! operations, and a plain unsigned `PUT` to the published URL for the content — so what is
//! asserted is what an SDK client plus an HTTP client see.
//!
//! The rule the tables span: an upload is registered INITIALIZED with a locator of its
//! own, the content stored against that locator is read as the package format the upload's
//! TYPE names, and the outcome of that read becomes the upload's terminal status — so an
//! upload of a type whose content the service does not parse accepts arbitrary bytes, while
//! an app-package type whose content is not that platform's package fails with the
//! `errorCode` of the processing step that could not read it. The rows differ along what
//! the rule generalises over: the upload type (each kind the `UploadType` enum carries),
//! the form of the content (the package, an archive that is not it, bytes that are no
//! archive at all, nothing at all), and the step of the processing that fails.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_devicefarm_devicefarm_20150623::state::UPLOAD_PROCESSING_SPAN;

use crate::projects::{arn_of, error_code, foreign_arn, Emulator, EMULATOR_HOST, REGION};

/// How often a case asks again for an upload whose processing is still running.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A zip container carrying `entries`, written by the same crate the service reads it back
/// with.
fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("the entry name is a plain relative path");
        std::io::Write::write_all(&mut writer, body).expect("writing to a Vec");
    }
    writer
        .finish()
        .expect("finishing a zip in memory")
        .into_inner()
}

/// A file in Android's binary-XML form, which is what an application package carries its
/// manifest as: the platform's resource-chunk framing (chunk type 0x0003 for an XML
/// document, an 8-byte header, then the file size) followed by `body`.
fn android_binary_xml(body: &[u8]) -> Vec<u8> {
    let size = u32::try_from(8 + body.len()).expect("the test manifest is small");
    let mut manifest = vec![0x03, 0x00, 0x08, 0x00];
    manifest.extend_from_slice(&size.to_le_bytes());
    manifest.extend_from_slice(body);
    manifest
}

/// An Android application package: a zip container whose `AndroidManifest.xml` is in the
/// binary-XML form `aapt debug badging` reads.
fn apk() -> Vec<u8> {
    zip_of(&[
        ("AndroidManifest.xml", android_binary_xml(b"com.example.app")),
        ("classes.dex", b"dex\n035\0".to_vec()),
    ])
}

/// An iOS application archive: a zip container carrying one app bundle under `Payload/`.
fn ipa() -> Vec<u8> {
    zip_of(&[
        ("Payload/Demo.app/Info.plist", b"bplist00".to_vec()),
        ("Payload/Demo.app/Demo", b"\xcf\xfa\xed\xfe".to_vec()),
    ])
}

/// A zip container that is no platform package: the shape a test package has.
fn test_package() -> Vec<u8> {
    zip_of(&[
        ("tests/test_smoke.py", b"def test_smoke():\n    assert True\n".to_vec()),
        ("requirements.txt", b"pytest\n".to_vec()),
    ])
}

/// A zip container whose `AndroidManifest.xml` is a TEXT document rather than Android's
/// binary XML — a file of the right name that is not a manifest.
fn zip_with_text_manifest() -> Vec<u8> {
    zip_of(&[(
        "AndroidManifest.xml",
        br#"<?xml version="1.0"?><manifest package="com.example.app"/>"#.to_vec(),
    )])
}

/// Bytes that are no archive at all.
fn not_an_archive() -> Vec<u8> {
    b"this is not a zip container, it is just some bytes".to_vec()
}

/// The `upload` object of a successful `CreateUpload`/`GetUpload` answer.
async fn upload_of(emu: &Emulator, op: &str, request: &Value) -> Value {
    let (status, response) = emu.call(op, REGION, request).await;
    assert_eq!(
        status, 200,
        "case {op} {request}: expected 200, got {status:?} with {response}"
    );
    response["upload"].clone()
}

/// One row of the processing table: what is uploaded, and what the service reports once it
/// has read it.
struct Row {
    case: &'static str,
    upload_type: &'static str,
    name: &'static str,
    content: Vec<u8>,
    /// The terminal status the content puts the upload in.
    status: &'static str,
    /// The `errorCode` the metadata document carries, for a row that fails.
    error_code: Option<String>,
}

/// CreateUpload registers an upload with a locator of its own, the content stored against
/// that locator is read as the package format the upload's type names, and GetUpload
/// reports the outcome of that read — the whole published workflow, end to end.
#[tokio::test]
async fn uploaded_content_is_processed_against_the_format_its_type_names() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "upload-processing" })).await);

    let rows: Vec<Row> = vec![
        Row {
            case: "an APK uploaded as an Android app",
            upload_type: "ANDROID_APP",
            name: "demo.apk",
            content: apk(),
            status: "SUCCEEDED",
            error_code: None,
        },
        Row {
            case: "a zip carrying no manifest uploaded as an Android app",
            upload_type: "ANDROID_APP",
            name: "demo.apk",
            content: test_package(),
            status: "FAILED",
            error_code: Some("ANDROID_APP_AAPT_DEBUG_BADGING_FAILED".to_string()),
        },
        Row {
            case: "a zip whose manifest is text, not binary XML, as an Android app",
            upload_type: "ANDROID_APP",
            name: "demo.apk",
            content: zip_with_text_manifest(),
            status: "FAILED",
            error_code: Some("ANDROID_APP_AAPT_DEBUG_BADGING_FAILED".to_string()),
        },
        Row {
            case: "bytes that are no archive uploaded as an Android app",
            upload_type: "ANDROID_APP",
            name: "demo.apk",
            content: not_an_archive(),
            status: "FAILED",
            error_code: Some("ANDROID_APP_UNZIP_FAILED".to_string()),
        },
        Row {
            case: "an IPA uploaded as an iOS app",
            upload_type: "IOS_APP",
            name: "demo.ipa",
            content: ipa(),
            status: "SUCCEEDED",
            error_code: None,
        },
        Row {
            case: "an APK uploaded as an iOS app",
            upload_type: "IOS_APP",
            name: "demo.ipa",
            content: apk(),
            status: "FAILED",
            error_code: Some("IOS_APP_PLIST_FILE_NOT_FOUND".to_string()),
        },
        Row {
            case: "bytes that are no archive uploaded as an iOS app",
            upload_type: "IOS_APP",
            name: "demo.ipa",
            content: not_an_archive(),
            status: "FAILED",
            error_code: Some("IOS_APP_UNZIP_FAILED".to_string()),
        },
        Row {
            case: "a zip archive uploaded as an Appium test package",
            upload_type: "APPIUM_PYTHON_TEST_PACKAGE",
            name: "tests.zip",
            content: test_package(),
            status: "SUCCEEDED",
            error_code: None,
        },
        Row {
            case: "bytes that are no archive uploaded as an Appium test package",
            upload_type: "APPIUM_PYTHON_TEST_PACKAGE",
            name: "tests.zip",
            content: not_an_archive(),
            status: "FAILED",
            error_code: Some("APPIUM_PYTHON_TEST_PACKAGE_UNZIP_FAILED".to_string()),
        },
        Row {
            case: "bytes that are no archive uploaded as an instrumentation test package",
            upload_type: "INSTRUMENTATION_TEST_PACKAGE",
            name: "tests.zip",
            content: not_an_archive(),
            status: "FAILED",
            error_code: Some("INSTRUMENTATION_TEST_PACKAGE_UNZIP_FAILED".to_string()),
        },
        Row {
            case: "arbitrary data uploaded as external data",
            upload_type: "EXTERNAL_DATA",
            name: "fixtures.zip",
            content: not_an_archive(),
            status: "SUCCEEDED",
            error_code: None,
        },
        Row {
            case: "no content at all uploaded as external data",
            upload_type: "EXTERNAL_DATA",
            name: "fixtures.zip",
            content: Vec::new(),
            status: "SUCCEEDED",
            error_code: None,
        },
        Row {
            case: "a test spec document",
            upload_type: "APPIUM_PYTHON_TEST_SPEC",
            name: "spec.yml",
            content: b"version: 0.1\nphases:\n  test:\n    commands:\n      - pytest\n".to_vec(),
            status: "SUCCEEDED",
            error_code: None,
        },
    ];

    // PHASE 1 — every upload is registered INITIALIZED, with its own ARN and locator, and
    // reads back identically.
    let mut registered: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let created = upload_of(
            &emu,
            "CreateUpload",
            &json!({
                "projectArn": project_arn,
                "name": row.name,
                "type": row.upload_type,
                "contentType": "application/octet-stream"
            }),
        )
        .await;
        let arn = arn_of(&created);
        assert_eq!(
            created["status"],
            json!("INITIALIZED"),
            "case {:?}: a registered upload has no content yet, got {}",
            row.case,
            created["status"]
        );
        for (member, expected) in [
            ("name", json!(row.name)),
            ("type", json!(row.upload_type)),
            ("contentType", json!("application/octet-stream")),
            ("category", json!("PRIVATE")),
            ("metadata", Value::Null),
            ("message", Value::Null),
        ] {
            assert_eq!(
                created[member], expected,
                "case {:?}: expected {member} {expected}, got {}",
                row.case, created[member]
            );
        }
        assert!(
            created["created"].is_number(),
            "case {:?}: expected a created timestamp, got {}",
            row.case,
            created["created"]
        );
        let url = created["url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a content URL, got {created}", row.case))
            .to_string();
        assert!(
            url.starts_with(&format!("https://{EMULATOR_HOST}/_aws/")),
            "case {:?}: expected a locator on the endpoint the request reached, under the \
             emulator's reserved data-plane path namespace, got {url:?}",
            row.case
        );
        // The round trip: GetUpload reports the persisted record, byte for byte.
        let read_back = upload_of(&emu, "GetUpload", &json!({ "arn": arn })).await;
        assert_eq!(
            read_back, created,
            "case {:?}: expected GetUpload to return {created}, got {read_back}",
            row.case
        );
        registered.push((arn, url));
    }

    // Two uploads of the same name in the same project are different resources.
    let arns: std::collections::BTreeSet<&String> =
        registered.iter().map(|(arn, _url)| arn).collect();
    assert_eq!(
        arns.len(),
        registered.len(),
        "expected one fresh ARN per upload, got {} for {} uploads",
        arns.len(),
        registered.len()
    );
    let urls: std::collections::BTreeSet<&String> =
        registered.iter().map(|(_arn, url)| url).collect();
    assert_eq!(
        urls.len(),
        registered.len(),
        "expected one locator per upload, got {} for {} uploads",
        urls.len(),
        registered.len()
    );

    // PHASE 2 — the content is stored against each locator, which takes the upload into
    // processing. Asserted for every row before any of them is polled, so the span is
    // spent once for the whole table.
    for (row, (arn, url)) in rows.iter().zip(&registered) {
        let (status, body) = emu.request_url("PUT", url, row.content.clone(), None).await;
        assert_eq!(
            status,
            200,
            "case {:?}: expected the published locator to accept the content with 200, got \
             {status:?} with {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
        let processing = upload_of(&emu, "GetUpload", &json!({ "arn": arn })).await;
        assert_eq!(
            processing["status"],
            json!("PROCESSING"),
            "case {:?}: an upload whose content has just been stored is being read, got {}",
            row.case,
            processing["status"]
        );
    }

    // PHASE 3 — the outcome of reading the content is what the upload reports.
    for (row, (arn, _url)) in rows.iter().zip(&registered) {
        let settled = settled_upload(&emu, arn, row.case).await;
        assert_eq!(
            settled["status"],
            json!(row.status),
            "case {:?}: expected status {}, got {} with {settled}",
            row.case,
            row.status,
            settled["status"]
        );
        match &row.error_code {
            Some(expected) => {
                let metadata = settled["metadata"].as_str().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a metadata document on a failed upload, got \
                         {settled}",
                        row.case
                    )
                });
                let document: Value = serde_json::from_str(metadata).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: expected the metadata to be a JSON document, got \
                         {metadata:?} ({error})",
                        row.case
                    )
                });
                assert_eq!(
                    document["errorCode"],
                    json!(expected),
                    "case {:?}: expected errorCode {expected}, got {}",
                    row.case,
                    document["errorCode"]
                );
                assert!(
                    settled["message"].as_str().is_some_and(|message| !message.is_empty()),
                    "case {:?}: expected a message about the result, got {}",
                    row.case,
                    settled["message"]
                );
            }
            None => {
                for member in ["metadata", "message"] {
                    assert_eq!(
                        settled[member],
                        Value::Null,
                        "case {:?}: an upload that processed cleanly reports no {member}, got \
                         {}",
                        row.case,
                        settled[member]
                    );
                }
            }
        }
        // A read does not change what a later caller sees.
        let again = upload_of(&emu, "GetUpload", &json!({ "arn": arn })).await;
        assert_eq!(
            again, settled,
            "case {:?}: expected a second read to report {settled}, got {again}",
            row.case
        );
    }
}

/// The upload `arn` once its processing has reached a terminal status, waited for rather
/// than slept through.
async fn settled_upload(emu: &Emulator, arn: &str, case: &str) -> Value {
    let deadline = std::time::Instant::now() + UPLOAD_PROCESSING_SPAN * 20;
    loop {
        let upload = upload_of(emu, "GetUpload", &json!({ "arn": arn })).await;
        let status = upload["status"].as_str().unwrap_or_default().to_string();
        if status == "SUCCEEDED" || status == "FAILED" {
            return upload;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the processing of the content to reach a terminal \
             status within {:?}, still {status:?}",
            UPLOAD_PROCESSING_SPAN * 20
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Storing content again is a second upload of the same resource: the upload is read
/// afresh, so content that failed the first time can be replaced by content that passes.
#[tokio::test]
async fn storing_content_again_reprocesses_the_upload() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "upload-replacement" })).await);
    let created = upload_of(
        &emu,
        "CreateUpload",
        &json!({ "projectArn": project_arn, "name": "demo.apk", "type": "ANDROID_APP" }),
    )
    .await;
    let arn = arn_of(&created);
    let url = created["url"].as_str().expect("a content URL").to_string();

    // (case, content, expected status)
    let attempts: Vec<(&str, Vec<u8>, &str)> = vec![
        ("the first store, of content that is no package", not_an_archive(), "FAILED"),
        ("the second store, of the package itself", apk(), "SUCCEEDED"),
        ("the third store, of a package again", apk(), "SUCCEEDED"),
        ("the fourth store, of a zip that is no package", test_package(), "FAILED"),
    ];
    for (case, content, expected) in attempts {
        let (status, body) = emu.request_url("PUT", &url, content, None).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: expected 200 from the locator, got {status:?} with {:?}",
            String::from_utf8_lossy(&body)
        );
        let settled = settled_upload(&emu, &arn, case).await;
        assert_eq!(
            settled["status"],
            json!(expected),
            "case {case:?}: expected status {expected}, got {settled}"
        );
    }
}

/// Every way an upload request can name something that is not there, or a file name the
/// type cannot carry, is refused with the modelled HTTP 400 — never a 501 and never a 200.
#[tokio::test]
async fn upload_requests_the_service_cannot_serve_are_refused() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "upload-refusals" })).await);
    let upload_arn = arn_of(
        &upload_of(
            &emu,
            "CreateUpload",
            &json!({ "projectArn": project_arn, "name": "present.zip", "type": "EXTERNAL_DATA" }),
        )
        .await,
    );

    // (case, operation, request, expected error code)
    let cases: Vec<(String, &str, Value, &str)> = vec![
        (
            "CreateUpload for a project ARN no project has".to_string(),
            "CreateUpload",
            json!({
                "projectArn": foreign_arn("project"),
                "name": "orphan.zip",
                "type": "EXTERNAL_DATA"
            }),
            "NotFoundException",
        ),
        (
            "CreateUpload given an upload ARN as the project".to_string(),
            "CreateUpload",
            json!({ "projectArn": upload_arn.clone(), "name": "wrong.zip", "type": "EXTERNAL_DATA" }),
            "ArgumentException",
        ),
        (
            "an Android app whose file name is not an .apk".to_string(),
            "CreateUpload",
            json!({ "projectArn": project_arn, "name": "demo.zip", "type": "ANDROID_APP" }),
            "ArgumentException",
        ),
        (
            "an Android app whose file name has no extension at all".to_string(),
            "CreateUpload",
            json!({ "projectArn": project_arn, "name": "demo", "type": "ANDROID_APP" }),
            "ArgumentException",
        ),
        (
            "an iOS app whose file name is an .apk".to_string(),
            "CreateUpload",
            json!({ "projectArn": project_arn, "name": "demo.apk", "type": "IOS_APP" }),
            "ArgumentException",
        ),
        (
            "a test spec whose file name is neither .yaml nor .yml".to_string(),
            "CreateUpload",
            json!({
                "projectArn": project_arn,
                "name": "spec.zip",
                "type": "APPIUM_PYTHON_TEST_SPEC"
            }),
            "ArgumentException",
        ),
        (
            "a file name carrying a path separator".to_string(),
            "CreateUpload",
            json!({ "projectArn": project_arn, "name": "dir/demo.zip", "type": "EXTERNAL_DATA" }),
            "ArgumentException",
        ),
        (
            "GetUpload for an upload ARN no upload has".to_string(),
            "GetUpload",
            json!({ "arn": foreign_arn("upload") }),
            "NotFoundException",
        ),
        (
            "GetUpload given the owning project's ARN".to_string(),
            "GetUpload",
            json!({ "arn": project_arn }),
            "ArgumentException",
        ),
        (
            "GetUpload given a device-pool ARN".to_string(),
            "GetUpload",
            json!({ "arn": foreign_arn("devicepool") }),
            "ArgumentException",
        ),
    ];

    for (case, op, request, expected_code) in &cases {
        let (status, response) = emu.call(op, REGION, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {response}"
        );
    }

    // The accept half of the same rule: each documented extension, and the types whose
    // file name the service does not constrain (which the recording shows AWS serving) —
    // so the refusals above are a per-type rule and not a blanket one.
    for (case, upload_type, name) in [
        ("an Android app under its .apk name", "ANDROID_APP", "demo.apk"),
        ("an iOS app under its .ipa name", "IOS_APP", "demo.ipa"),
        ("a test spec under a .yml name", "APPIUM_PYTHON_TEST_SPEC", "spec.yml"),
        ("a test spec under a .yaml name", "APPIUM_PYTHON_TEST_SPEC", "spec.yaml"),
        (
            "a test package under a .zip name",
            "APPIUM_PYTHON_TEST_PACKAGE",
            "tests.zip",
        ),
        (
            "a test package under a name that is not a .zip",
            "APPIUM_PYTHON_TEST_PACKAGE",
            "tests.tar",
        ),
        ("external data under a .zip name", "EXTERNAL_DATA", "fixtures.zip"),
        (
            "external data under a name that is not a .zip",
            "EXTERNAL_DATA",
            "fixtures.bin",
        ),
        ("a web app under a name that is not a .zip", "WEB_APP", "site.tar"),
    ] {
        let created = upload_of(
            &emu,
            "CreateUpload",
            &json!({ "projectArn": project_arn, "name": name, "type": upload_type }),
        )
        .await;
        assert_eq!(
            created["name"],
            json!(name),
            "case {case:?}: expected the name {name} to be registered, got {created}"
        );
    }

    // The refused calls changed nothing: the one upload created first still reads back.
    let present = upload_of(&emu, "GetUpload", &json!({ "arn": upload_arn })).await;
    assert_eq!(
        present["name"],
        json!("present.zip"),
        "case the surviving upload after every refusal: expected name \"present.zip\", got \
         {present}"
    );
}

/// The published locator serves the write it documents, and nothing else: a locator of an
/// upload this emulator never minted is not served at all, and a method the locator does
/// not describe is refused.
#[tokio::test]
async fn the_published_locator_serves_only_the_write_it_documents() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "locator-scope" })).await);
    let created = upload_of(
        &emu,
        "CreateUpload",
        &json!({ "projectArn": project_arn, "name": "fixtures.zip", "type": "EXTERNAL_DATA" }),
    )
    .await;
    let arn = arn_of(&created);
    let url = created["url"].as_str().expect("a content URL").to_string();

    // (case, method, url, declared content length, expected status)
    let cases: Vec<(&str, &str, String, Option<u64>, u16)> = vec![
        ("a read of the locator", "GET", url.clone(), None, 405),
        ("a delete of the locator", "DELETE", url.clone(), None, 405),
        (
            // `can_handle` claims a locator only for an upload the store HOLDS, so this
            // request is never routed into the plane at all: the 404 is the framework's own
            // answer for a request no service claimed. The plane's own `NoSuchKey` refusal
            // covers the narrow race where the upload goes away between the claim and the
            // serve, which no caller can drive from outside.
            "a write to a locator of an upload that does not exist",
            "PUT",
            format!("https://{EMULATOR_HOST}/_aws/devicefarm-uploads/0000/0000"),
            None,
            404,
        ),
        (
            // The path is not a locator at all, so the content plane does not serve it:
            // what answers is the framework's own refusal of a request that reached this
            // service and named no operation.
            "a write to a path that is no locator",
            "PUT",
            format!("https://{EMULATOR_HOST}/_aws/devicefarm-uploads/"),
            None,
            400,
        ),
        (
            "a write declaring more content than one store accepts",
            "PUT",
            url.clone(),
            Some(64 * 1024 * 1024 + 1),
            400,
        ),
    ];
    for (case, method, target, declared, expected) in &cases {
        let (status, body) = emu
            .request_url(method, target, b"content".to_vec(), *declared)
            .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    // None of those touched the upload: it is still waiting for its content.
    let untouched = upload_of(&emu, "GetUpload", &json!({ "arn": arn })).await;
    assert_eq!(
        untouched["status"],
        json!("INITIALIZED"),
        "case the upload after five refused requests to its locator: expected INITIALIZED, \
         got {untouched}"
    );
}
