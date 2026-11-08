// SPDX-License-Identifier: CC0-1.0

//! A DICOM import job of AWS HealthImaging: what a job that reaches a terminal status
//! leaves behind, across three services on one engine.
//!
//! RULE: `StartDICOMImportJob` "imports DICOM P10 files … found in the S3 prefix specified
//! by the `inputS3Uri` parameter", so a job that reaches `COMPLETED` leaves an image set
//! in the target data store for every DICOM series the files under that prefix describe —
//! `imageSetState` `ACTIVE`, `imageSetWorkflowStatus` `IMPORTED`, discoverable by
//! `SearchImageSets` on the study's `DICOMStudyInstanceUID`, carrying the patient, study
//! and series attributes those files carry, and serving one image frame per imported
//! instance that held pixel data. An object the prefix holds that is not a DICOM P10 file
//! describes no image set, and an image set exists only where a file really was read.
//! `dataAccessRoleArn` is the role the import runs as, so a role the account does not
//! hold — or one whose trust policy does not admit this service — is refused 403
//! `AccessDeniedException` and creates no job at all.
//!
//! Only a cross-service suite can pin this: the objects are Amazon S3's, the role and its
//! trust policy are IAM's, and the data store, the job and the image sets are AWS
//! HealthImaging's, so the rule is observable only where all three are mounted on one
//! engine. The medical-imaging crate's own tests reach exactly one of them.
//!
//! The rows vary what the rule generalises over: how many DICOM files the prefix holds,
//! how they are spread over studies and series (one series, two series of one study, two
//! studies), the transfer syntax each file is encoded in (explicit and implicit VR little
//! endian), which optional attributes a file carries, whether a file holds pixel data at
//! all, the shape of the object keys (flat and under a nested prefix), and whether the
//! prefix also holds objects that are not readable DICOM — a file of another format, one
//! cut off part-way through its dataset, and one whose last element declares a value
//! length a thousand times larger than this emulator reads of a whole object, which is
//! what an object built to make a reader allocate from the length field inside it looks
//! like.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::MAX_IMPORTED_OBJECT_BYTES;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is what these calls are signed with.
/// The signature itself is never checked: this router runs with request authentication
/// off, as every suite in this crate runs it.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The bucket the imported objects are staged in.
const BUCKET: &str = "healthimaging-import-shelf";

/// A trust policy that lets AWS HealthImaging assume the role it is attached to. It
/// carries none of the characters that are significant in an
/// `application/x-www-form-urlencoded` body.
const HEALTHIMAGING_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"medical-imaging.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The same policy for ANOTHER service's principal: a role this service cannot assume,
/// which is the only thing that differs between it and the row above.
const LAMBDA_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// How long one poll of a clock-owed transition waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll loop waits for a clock-owed transition before giving up. Far past
/// every span this emulator gives one, so a slow machine does not fail a case the clock
/// would have settled.
const POLL_BUDGET: Duration = Duration::from_secs(30);

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Answer {
    /// One response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body as text, for a JSON or XML answer.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is neither JSON nor present.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The body as JSON after decoding the compression the answer declares — what a client
    /// of `GetImageSetMetadata` does with its blob, which AWS HealthImaging returns
    /// gzip-compressed (`contentEncoding: gzip`).
    ///
    /// Decoded through the medical-imaging crate's own inverse of the compression it
    /// applied, so this suite reads the blob the way the handler wrote it rather than
    /// stating the format a second time.
    fn decoded_json(&self, case: &str) -> Value {
        let encoding = self.header("content-encoding").unwrap_or_default();
        assert_eq!(
            encoding, "gzip",
            "case {case:?}: expected the metadata blob to declare the compression it is in, \
             got {encoding:?}"
        );
        let decoded = service_com_amazonaws_medicalimaging_ahigatewayservice::ops::get_image_set_metadata::decoded_metadata_blob(&self.body)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a gzip-compressed document, got {} bytes that do \
                     not decode",
                    self.body.len()
                )
            });
        serde_json::from_slice(&decoded).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a JSON document in the blob, got {error}")
        })
    }

    /// The member at `path` of a JSON body, as text.
    fn string(&self, path: &[&str]) -> String {
        let body = self.json();
        let mut cursor = &body;
        for key in path {
            cursor = &cursor[*key];
        }
        cursor.as_str().unwrap_or_default().to_string()
    }
}

/// The three services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_medicalimaging_ahigatewayservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One AWS HealthImaging call.
async fn imaging(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("medical-imaging.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("medical-imaging")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One AWS HealthImaging call that has to succeed.
async fn imaging_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let answer = imaging(router, method, path, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer
}

/// One Amazon S3 REST call, path-style as the emulator serves it.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut with_credential: Vec<(&str, &str)> = headers.to_vec();
    let credential = authorization("s3");
    with_credential.push(("authorization", &credential));
    dispatch(
        router,
        method,
        &format!("s3.{REGION}.amazonaws.com"),
        path,
        &with_credential,
        body,
    )
    .await
}

/// One IAM awsQuery call that has to succeed, answering its body.
async fn iam_ok(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let answer = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup {case:?}: expected HTTP 200 from IAM, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.text()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One IAM role created with `trust` as its trust policy, answering its ARN.
async fn role(router: &MultiServiceRouter, name: &str, trust: &str) -> String {
    let created = iam_ok(
        router,
        &format!("CreateRole({name})"),
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup CreateRole({name}): no <Arn> in {created}"))
}

// ---------------------------------------------------------------------------
// Writing DICOM Part 10 files
// ---------------------------------------------------------------------------

/// The transfer syntax UID of Explicit VR Little Endian, the encoding most DICOM files on
/// disk carry (DICOM PS3.6, registry of transfer syntax UIDs).
const EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";

/// The transfer syntax UID of Implicit VR Little Endian — the DICOM default encoding, in
/// which an element carries no value representation of its own.
const IMPLICIT_VR_LE: &str = "1.2.840.10008.1.2";

/// The SOP class every file below declares: Secondary Capture Image Storage.
const SOP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.7";

/// `value`, padded to an even length with `pad` — every DICOM data element's value has an
/// even length.
fn padded(value: &[u8], pad: u8) -> Vec<u8> {
    let mut bytes = value.to_vec();
    if bytes.len() % 2 == 1 {
        bytes.push(pad);
    }
    bytes
}

/// One data element in EXPLICIT VR little endian: the tag, the value representation, then
/// the value's length — 32-bit for the value representations DICOM gives the long form,
/// 16-bit for the rest (DICOM PS3.5 section 7.1.2).
fn explicit(group: u16, element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&group.to_le_bytes());
    out.extend_from_slice(&element.to_le_bytes());
    out.extend_from_slice(vr);
    if matches!(vr, b"OB" | b"OW" | b"OF" | b"SQ" | b"UT" | b"UN") {
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&u32::try_from(value.len()).expect("a short value").to_le_bytes());
    } else {
        out.extend_from_slice(&u16::try_from(value.len()).expect("a short value").to_le_bytes());
    }
    out.extend_from_slice(value);
    out
}

/// One data element in IMPLICIT VR little endian: the tag and a 32-bit length, with the
/// value representation left to the data dictionary.
fn implicit(group: u16, element: u16, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&group.to_le_bytes());
    out.extend_from_slice(&element.to_le_bytes());
    out.extend_from_slice(&u32::try_from(value.len()).expect("a short value").to_le_bytes());
    out.extend_from_slice(value);
    out
}

/// One DICOM attribute a test file carries: its tag, its value representation (for the
/// explicit encoding) and its value, padded as DICOM pads that value representation —
/// text with a space, a UID with a NUL.
struct Attribute {
    group: u16,
    element: u16,
    vr: &'static [u8; 2],
    value: Vec<u8>,
}

/// A text attribute (padded with a space).
fn text(group: u16, element: u16, vr: &'static [u8; 2], value: &str) -> Attribute {
    Attribute {
        group,
        element,
        vr,
        value: padded(value.as_bytes(), b' '),
    }
}

/// A UID attribute (padded with a NUL, as DICOM pads a UI value).
fn uid(group: u16, element: u16, value: &str) -> Attribute {
    Attribute {
        group,
        element,
        vr: b"UI",
        value: padded(value.as_bytes(), 0),
    }
}

/// What one DICOM file in an import prefix carries.
struct Instance {
    /// The object key the file is staged under, relative to the import prefix.
    key: &'static str,
    transfer_syntax: &'static str,
    study_uid: &'static str,
    series_uid: &'static str,
    sop_uid: &'static str,
    patient_id: &'static str,
    patient_name: &'static str,
    modality: &'static str,
    /// The attributes DICOM leaves optional that this file happens to carry.
    study_description: Option<&'static str>,
    accession_number: Option<&'static str>,
    /// The instance's pixel data, or `None` for an instance that carries none.
    pixel_data: Option<&'static [u8]>,
}

impl Instance {
    /// The DICOM Part 10 file this instance is: the 128-byte preamble, the `DICM` magic
    /// code, the file meta group (always explicit VR little endian, as DICOM PS3.10
    /// requires) and then the dataset in the transfer syntax the meta group declares.
    fn part10(&self) -> Vec<u8> {
        let meta = {
            let mut meta = Vec::new();
            meta.extend(explicit(0x0002, 0x0001, b"OB", &[0x00, 0x01]));
            meta.extend(explicit(0x0002, 0x0002, b"UI", &padded(SOP_CLASS.as_bytes(), 0)));
            meta.extend(explicit(
                0x0002,
                0x0003,
                b"UI",
                &padded(self.sop_uid.as_bytes(), 0),
            ));
            meta.extend(explicit(
                0x0002,
                0x0010,
                b"UI",
                &padded(self.transfer_syntax.as_bytes(), 0),
            ));
            meta
        };
        let mut attributes = vec![
            uid(0x0008, 0x0018, self.sop_uid),
            text(0x0008, 0x0020, b"DA", "20240115"),
            text(0x0008, 0x0030, b"TM", "101500"),
        ];
        if let Some(accession) = self.accession_number {
            attributes.push(text(0x0008, 0x0050, b"SH", accession));
        }
        attributes.push(text(0x0008, 0x0060, b"CS", self.modality));
        if let Some(description) = self.study_description {
            attributes.push(text(0x0008, 0x1030, b"LO", description));
        }
        attributes.extend([
            text(0x0010, 0x0010, b"PN", self.patient_name),
            text(0x0010, 0x0020, b"LO", self.patient_id),
            text(0x0010, 0x0030, b"DA", "19800101"),
            text(0x0010, 0x0040, b"CS", "F"),
            text(0x0018, 0x0015, b"CS", "CHEST"),
            uid(0x0020, 0x000D, self.study_uid),
            uid(0x0020, 0x000E, self.series_uid),
            text(0x0020, 0x0010, b"SH", "STUDY-1"),
            text(0x0020, 0x0011, b"IS", "3"),
        ]);
        if let Some(pixels) = self.pixel_data {
            attributes.push(Attribute {
                group: 0x7FE0,
                element: 0x0010,
                vr: b"OB",
                value: padded(pixels, 0),
            });
        }
        let explicit_dataset = self.transfer_syntax == EXPLICIT_VR_LE;
        let mut dataset = Vec::new();
        for attribute in &attributes {
            if explicit_dataset {
                dataset.extend(explicit(
                    attribute.group,
                    attribute.element,
                    attribute.vr,
                    &attribute.value,
                ));
            } else {
                dataset.extend(implicit(attribute.group, attribute.element, &attribute.value));
            }
        }

        let mut file = vec![0_u8; 128];
        file.extend_from_slice(b"DICM");
        // (0002,0000) File Meta Information Group Length: the byte count of the meta
        // group's remaining elements, which is how a reader knows where the dataset
        // begins.
        let group_length = u32::try_from(meta.len()).expect("a short meta group");
        file.extend(explicit(
            0x0002,
            0x0000,
            b"UL",
            &group_length.to_le_bytes(),
        ));
        file.extend(meta);
        file.extend(dataset);
        file
    }
}

// ---------------------------------------------------------------------------
// The import table
// ---------------------------------------------------------------------------

/// One row of the import table: the prefix's contents, and what the import must leave.
struct ImportCase {
    case: &'static str,
    /// The prefix under the bucket the objects are staged under, and the one the job is
    /// pointed at.
    prefix: &'static str,
    instances: Vec<Instance>,
    /// Objects of the prefix that are NOT readable DICOM files, by key and bytes: a file
    /// of another format, and the malformed shapes a hostile or truncated object takes.
    /// Each must be passed over rather than turned into an image set.
    other_objects: Vec<(&'static str, Vec<u8>)>,
    /// How many image sets the completed import must leave in the data store.
    expected_image_sets: usize,
}

/// Pixel data of a secondary capture: eight bytes, which is what `GetImageFrame` must
/// serve back byte for byte.
const PIXELS_A: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0xfd, 0xfe, 0xff, 0x00];

/// A second frame's pixel data, so a case holding two instances can tell them apart.
const PIXELS_B: &[u8] = &[0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];

/// The study UID prefix every case's studies are built under, so one case's study cannot
/// be found by another's search.
const STUDY_ROOT: &str = "1.2.826.0.1.3680043.9.7133";

/// A well-formed Part 10 file CUT OFF part-way through its dataset.
fn truncated_part10() -> Vec<u8> {
    let whole = malformed_source().part10();
    whole[..whole.len() - 12].to_vec()
}

/// A well-formed Part 10 file whose last element — its pixel data — declares a value
/// length a thousand times larger than [`MAX_IMPORTED_OBJECT_BYTES`], the most this
/// emulator reads of one whole object.
///
/// The length field of a long-form explicit-VR element is the four bytes before the value,
/// so only those four are rewritten: everything else about the file stays well formed, and
/// what the reader is asked for is a value the file does not carry.
fn overlong_value_part10() -> Vec<u8> {
    let mut bytes = malformed_source().part10();
    let declared = u32::try_from(MAX_IMPORTED_OBJECT_BYTES)
        .expect("the read ceiling fits a DICOM length field")
        .saturating_mul(1_000);
    let length_at = bytes.len() - PIXELS_A.len() - 4;
    bytes[length_at..length_at + 4].copy_from_slice(&declared.to_le_bytes());
    bytes
}

/// The file the two malformed objects above are built from: well formed, and describing a
/// study no row of the table imports, so a malformed object that WERE read would show up as
/// an image set nothing expected.
fn malformed_source() -> Instance {
    Instance {
        key: "malformed.dcm",
        transfer_syntax: EXPLICIT_VR_LE,
        study_uid: "1.2.826.0.1.3680043.9.7133.9.1",
        series_uid: "1.2.826.0.1.3680043.9.7133.9.1.1",
        sop_uid: "1.2.826.0.1.3680043.9.7133.9.1.1.1",
        patient_id: "PID-MALFORMED",
        patient_name: "MAL^FORMED",
        modality: "CT",
        study_description: Some("MALFORMED"),
        accession_number: Some("ACC-MALFORMED"),
        pixel_data: Some(PIXELS_A),
    }
}

/// The rows of the import table.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "one instance of one series",
            prefix: "single/",
            instances: vec![Instance {
                key: "ct-0001.dcm",
                transfer_syntax: EXPLICIT_VR_LE,
                study_uid: "1.2.826.0.1.3680043.9.7133.1.1",
                series_uid: "1.2.826.0.1.3680043.9.7133.1.1.1",
                sop_uid: "1.2.826.0.1.3680043.9.7133.1.1.1.1",
                patient_id: "PID-SINGLE",
                patient_name: "DOE^JANE",
                modality: "CT",
                study_description: Some("CHEST CT"),
                accession_number: Some("ACC-1"),
                pixel_data: Some(PIXELS_A),
            }],
            other_objects: vec![],
            expected_image_sets: 1,
        },
        ImportCase {
            case: "two instances of one series, beside objects that are not readable DICOM",
            prefix: "series/in/",
            instances: vec![
                Instance {
                    key: "slice-1.dcm",
                    transfer_syntax: EXPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.2.1",
                    series_uid: "1.2.826.0.1.3680043.9.7133.2.1.1",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.2.1.1.1",
                    patient_id: "PID-SERIES",
                    patient_name: "ROE^RICHARD",
                    modality: "MR",
                    study_description: Some("BRAIN MR"),
                    accession_number: None,
                    pixel_data: Some(PIXELS_A),
                },
                Instance {
                    key: "nested/slice-2.dcm",
                    transfer_syntax: EXPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.2.1",
                    series_uid: "1.2.826.0.1.3680043.9.7133.2.1.1",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.2.1.1.2",
                    patient_id: "PID-SERIES",
                    patient_name: "ROE^RICHARD",
                    modality: "MR",
                    study_description: None,
                    accession_number: Some("ACC-2"),
                    pixel_data: Some(PIXELS_B),
                },
            ],
            other_objects: vec![
                (
                    "manifest.json",
                    br#"{"note":"not a DICOM file"}"#.to_vec(),
                ),
                ("checksums.txt", b"deadbeef  slice-1.dcm".to_vec()),
                // A file CUT OFF part-way through its dataset: malformed input, which
                // describes no image set rather than a partial one.
                ("truncated.dcm", truncated_part10()),
                // The hostile shape: a well-formed file whose last element declares a
                // value a thousand times larger than this emulator will read a whole
                // object of. A DICOM value length is a 32-bit field INSIDE the bytes the
                // caller staged, so this is what an object built to make the reader
                // allocate from it looks like.
                ("overlong-value.dcm", overlong_value_part10()),
            ],
            expected_image_sets: 1,
        },
        ImportCase {
            case: "two series of one study, one of them carrying no pixel data",
            prefix: "two-series/",
            instances: vec![
                Instance {
                    key: "a.dcm",
                    transfer_syntax: EXPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.3.1",
                    series_uid: "1.2.826.0.1.3680043.9.7133.3.1.1",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.3.1.1.1",
                    patient_id: "PID-TWO",
                    patient_name: "SMITH^SAM",
                    modality: "CT",
                    study_description: Some("TWO SERIES"),
                    accession_number: Some("ACC-3"),
                    pixel_data: Some(PIXELS_A),
                },
                Instance {
                    key: "b.dcm",
                    // The DICOM default encoding, in which an element carries no value
                    // representation of its own.
                    transfer_syntax: IMPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.3.1",
                    series_uid: "1.2.826.0.1.3680043.9.7133.3.1.2",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.3.1.2.1",
                    patient_id: "PID-TWO",
                    patient_name: "SMITH^SAM",
                    modality: "SR",
                    study_description: Some("TWO SERIES"),
                    accession_number: Some("ACC-3"),
                    pixel_data: None,
                },
            ],
            other_objects: vec![],
            expected_image_sets: 2,
        },
        ImportCase {
            case: "two studies in one prefix",
            prefix: "two-studies/",
            instances: vec![
                Instance {
                    key: "first.dcm",
                    transfer_syntax: EXPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.4.1",
                    series_uid: "1.2.826.0.1.3680043.9.7133.4.1.1",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.4.1.1.1",
                    patient_id: "PID-FIRST",
                    patient_name: "ONE^PATIENT",
                    modality: "CT",
                    study_description: Some("FIRST STUDY"),
                    accession_number: Some("ACC-4"),
                    pixel_data: Some(PIXELS_A),
                },
                Instance {
                    key: "second.dcm",
                    transfer_syntax: EXPLICIT_VR_LE,
                    study_uid: "1.2.826.0.1.3680043.9.7133.4.2",
                    series_uid: "1.2.826.0.1.3680043.9.7133.4.2.1",
                    sop_uid: "1.2.826.0.1.3680043.9.7133.4.2.1.1",
                    patient_id: "PID-SECOND",
                    patient_name: "TWO^PATIENT",
                    modality: "MR",
                    study_description: Some("SECOND STUDY"),
                    accession_number: Some("ACC-5"),
                    pixel_data: Some(PIXELS_B),
                },
            ],
            other_objects: vec![],
            expected_image_sets: 2,
        },
    ]
}

/// Create a data store and poll `GetDatastore` until it reports `ACTIVE`.
async fn active_datastore(router: &MultiServiceRouter, case: &str, token: &str) -> String {
    let created = imaging_ok(
        router,
        case,
        "POST",
        "/datastore",
        Some(json!({ "datastoreName": "import.store", "clientToken": token })),
    )
    .await;
    let datastore_id = created.string(&["datastoreId"]);
    let deadline = tokio::time::Instant::now() + POLL_BUDGET;
    loop {
        let read = imaging_ok(
            router,
            case,
            "GET",
            &format!("/datastore/{datastore_id}"),
            None,
        )
        .await;
        let status = read.string(&["datastoreProperties", "datastoreStatus"]);
        if status == "ACTIVE" {
            return datastore_id;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the data store never reached ACTIVE, last {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetDICOMImportJob` until the job reports a terminal `jobStatus`, and answer it.
async fn settled_job(
    router: &MultiServiceRouter,
    case: &str,
    datastore_id: &str,
    job_id: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + POLL_BUDGET;
    loop {
        let read = imaging_ok(
            router,
            case,
            "GET",
            &format!("/getDICOMImportJob/datastore/{datastore_id}/job/{job_id}"),
            None,
        )
        .await;
        let status = read.string(&["jobProperties", "jobStatus"]);
        if status == "COMPLETED" || status == "FAILED" {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the import job never reached a terminal status, last {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Search the data store for the image sets of one study, polling until the search answers
/// `expected` of them.
async fn image_sets_of_study(
    router: &MultiServiceRouter,
    case: &str,
    datastore_id: &str,
    study_uid: &str,
    expected: usize,
) -> Vec<Value> {
    let criteria = json!({
        "filters": [{ "operator": "EQUAL", "values": [{ "DICOMStudyInstanceUID": study_uid }] }],
    });
    let deadline = tokio::time::Instant::now() + POLL_BUDGET;
    loop {
        let found = imaging_ok(
            router,
            case,
            "POST",
            &format!("/datastore/{datastore_id}/searchImageSets"),
            Some(criteria.clone()),
        )
        .await;
        let summaries = found.json()["imageSetsMetadataSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if summaries.len() == expected {
            return summaries;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected SearchImageSets to answer {expected} image sets for study \
             {study_uid}, got {summaries:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE (above): a completed DICOM import leaves one `ACTIVE` / `IMPORTED` image set per
/// series the prefix's DICOM files describe, searchable by the study they belong to,
/// carrying those files' attributes and serving their pixel data.
#[tokio::test]
async fn a_completed_dicom_import_materialises_the_image_sets_it_read() {
    let router = emulator();
    let created = s3(&router, "PUT", &format!("/{BUCKET}"), &[], Bytes::new()).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateBucket must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let role_arn = role(&router, "healthimaging-import", HEALTHIMAGING_TRUST).await;

    for (index, row) in import_cases().into_iter().enumerate() {
        let case = row.case;
        // Everything the prefix holds, written through Amazon S3's own API, so what the
        // import reads is what that service holds.
        for instance in &row.instances {
            let written = s3(
                &router,
                "PUT",
                &format!("/{BUCKET}/{}{}", row.prefix, instance.key),
                &[("content-type", "application/dicom")],
                Bytes::from(instance.part10()),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: PutObject({}) must succeed, got {} with {:?}",
                instance.key,
                written.status,
                written.text()
            );
        }
        for (key, body) in &row.other_objects {
            let written = s3(
                &router,
                "PUT",
                &format!("/{BUCKET}/{}{key}", row.prefix),
                &[("content-type", "application/octet-stream")],
                Bytes::from(body.clone()),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: PutObject({key}) must succeed, got {} with {:?}",
                written.status,
                written.text()
            );
        }

        let datastore_id = active_datastore(&router, case, &format!("import-store-{index}")).await;
        let submitted = imaging_ok(
            &router,
            case,
            "POST",
            &format!("/startDICOMImportJob/datastore/{datastore_id}"),
            Some(json!({
                "clientToken": format!("import-job-{index}"),
                "dataAccessRoleArn": role_arn,
                "inputS3Uri": format!("s3://{BUCKET}/{}", row.prefix),
                "outputS3Uri": format!("s3://{BUCKET}/out/"),
            })),
        )
        .await;
        assert_eq!(
            submitted.string(&["jobStatus"]),
            "SUBMITTED",
            "case {case:?}: expected SUBMITTED from the submit, got {:?}",
            submitted.text()
        );
        let job_id = submitted.string(&["jobId"]);
        assert_eq!(
            settled_job(&router, case, &datastore_id, &job_id).await,
            "COMPLETED",
            "case {case:?}: expected the import to reach COMPLETED"
        );

        // What the prefix described, grouped the way an image set groups it: one per
        // series of a study.
        let mut series_of_study: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut instances_of_study: BTreeMap<&str, usize> = BTreeMap::new();
        for instance in &row.instances {
            series_of_study
                .entry(instance.study_uid)
                .or_default()
                .insert(instance.series_uid);
            *instances_of_study.entry(instance.study_uid).or_default() += 1;
        }

        let mut found_image_sets = 0_usize;
        for (study_uid, series) in &series_of_study {
            let summaries =
                image_sets_of_study(&router, case, &datastore_id, study_uid, series.len()).await;
            found_image_sets += summaries.len();
            // Every series of the study is reported, each by exactly one image set.
            let reported_series: BTreeSet<String> = summaries
                .iter()
                .map(|summary| {
                    summary["DICOMTags"]["DICOMSeriesInstanceUID"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let expected_series: BTreeSet<String> =
                series.iter().map(|uid| (*uid).to_string()).collect();
            assert_eq!(
                reported_series, expected_series,
                "case {case:?}: expected one image set per series of study {study_uid}"
            );

            for summary in &summaries {
                let image_set_id = summary["imageSetId"].as_str().unwrap_or_default().to_string();
                let series_uid = summary["DICOMTags"]["DICOMSeriesInstanceUID"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let source: Vec<&Instance> = row
                    .instances
                    .iter()
                    .filter(|instance| instance.series_uid == series_uid)
                    .collect();
                let first = source.first().copied().unwrap_or_else(|| {
                    panic!("case {case:?}: no imported instance for series {series_uid}")
                });

                // The attributes the files carried, read back off the search summary.
                let tags = &summary["DICOMTags"];
                for (member, expected) in [
                    ("DICOMStudyInstanceUID", (*study_uid).to_string()),
                    ("DICOMPatientId", first.patient_id.to_string()),
                    ("DICOMPatientName", first.patient_name.to_string()),
                    ("DICOMSeriesModality", first.modality.to_string()),
                    ("DICOMStudyDate", "20240115".to_string()),
                    ("DICOMStudyTime", "101500".to_string()),
                ] {
                    assert_eq!(
                        tags[member].as_str().unwrap_or_default(),
                        expected,
                        "case {case:?}: expected {member} {expected:?} on image set \
                         {image_set_id}, got {tags:?}"
                    );
                }
                assert_eq!(
                    tags["DICOMNumberOfStudyRelatedSeries"]
                        .as_u64()
                        .unwrap_or_default() as usize,
                    series.len(),
                    "case {case:?}: expected the study's series count on {image_set_id}, got \
                     {tags:?}"
                );
                assert_eq!(
                    tags["DICOMNumberOfStudyRelatedInstances"]
                        .as_u64()
                        .unwrap_or_default() as usize,
                    instances_of_study[*study_uid],
                    "case {case:?}: expected the study's instance count on {image_set_id}, got \
                     {tags:?}"
                );

                // The image set the import left is ACTIVE and IMPORTED, read through
                // GetImageSet rather than off the search.
                let properties = imaging_ok(
                    &router,
                    case,
                    "POST",
                    &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSet"),
                    None,
                )
                .await;
                assert_eq!(
                    properties.string(&["imageSetState"]),
                    "ACTIVE",
                    "case {case:?}: expected the imported image set ACTIVE, got {:?}",
                    properties.text()
                );
                assert_eq!(
                    properties.string(&["imageSetWorkflowStatus"]),
                    "IMPORTED",
                    "case {case:?}: expected the imported image set IMPORTED, got {:?}",
                    properties.text()
                );
                assert_eq!(
                    properties.string(&["versionId"]),
                    "1",
                    "case {case:?}: expected the imported image set at version 1, got {:?}",
                    properties.text()
                );

                // The aggregated metadata document places every imported instance in the
                // hierarchy, and names the frames of each.
                let metadata = imaging_ok(
                    &router,
                    case,
                    "POST",
                    &format!(
                        "/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSetMetadata"
                    ),
                    None,
                )
                .await;
                let document = metadata.decoded_json(case);
                assert_eq!(
                    document["Study"]["DICOM"]["StudyInstanceUID"]
                        .as_str()
                        .unwrap_or_default(),
                    *study_uid,
                    "case {case:?}: expected the metadata document to name the study, got \
                     {document}"
                );
                let series_level = &document["Study"]["Series"][series_uid.as_str()];
                for instance in &source {
                    let placed = &series_level["Instances"][instance.sop_uid];
                    if instance.pixel_data.is_none() {
                        assert!(
                            placed.is_null(),
                            "case {case:?}: an instance carrying no pixel data must place no \
                             frames, got {placed}"
                        );
                        continue;
                    }
                    let frames = placed["ImageFrames"].as_array().cloned().unwrap_or_default();
                    assert_eq!(
                        frames.len(),
                        1,
                        "case {case:?}: expected one image frame for instance {}, got {placed}",
                        instance.sop_uid
                    );
                    let image_frame_id =
                        frames[0]["ID"].as_str().unwrap_or_default().to_string();

                    // The pixel data the file carried, served back byte for byte.
                    let frame = imaging_ok(
                        &router,
                        case,
                        "POST",
                        &format!(
                            "/datastore/{datastore_id}/imageSet/{image_set_id}/getImageFrame"
                        ),
                        Some(json!({ "imageFrameId": image_frame_id })),
                    )
                    .await;
                    assert_eq!(
                        frame.body.as_ref(),
                        instance.pixel_data.unwrap_or_default(),
                        "case {case:?}: expected the imported pixel data of instance {}, got {:?}",
                        instance.sop_uid,
                        frame.body
                    );
                    // An imported frame has no stored transfer syntax, and the response
                    // member documents that case: "if the stored transfer syntax is
                    // missing, the returned contentType is image/jph".
                    assert_eq!(
                        frame.header("content-type").unwrap_or_default(),
                        "image/jph",
                        "case {case:?}: expected the documented contentType for a frame with no \
                         stored transfer syntax, for instance {}",
                        instance.sop_uid
                    );
                }
            }
        }
        assert_eq!(
            found_image_sets, row.expected_image_sets,
            "case {case:?}: expected the import to leave {} image sets",
            row.expected_image_sets
        );
        // And the data store holds NO OTHER image set: an object of the prefix that is not
        // a readable DICOM file describes none, so nothing was minted for the malformed
        // ones either (each of those files describes a study no row imports, which an
        // unfiltered search would report).
        let every = imaging_ok(
            &router,
            case,
            "POST",
            &format!("/datastore/{datastore_id}/searchImageSets?maxResults=50"),
            Some(json!({})),
        )
        .await;
        let held = every.json()["imageSetsMetadataSummaries"]
            .as_array()
            .map_or(0, Vec::len);
        assert_eq!(
            held, row.expected_image_sets,
            "case {case:?}: expected the store to hold exactly {} image sets, got {:?}",
            row.expected_image_sets,
            every.text()
        );
    }
}

/// RULE (above), the data plane the import opens: the image set a completed import left is
/// an ordinary `ACTIVE` image set, so `CopyImageSet` copies it — the copy reaches
/// `ACTIVE` / `COPIED` and holds the same DICOM attributes and the same image frames as the
/// image set it was taken from.
#[tokio::test]
async fn an_imported_image_set_copies_with_matching_metadata() {
    let router = emulator();
    let created = s3(&router, "PUT", &format!("/{BUCKET}"), &[], Bytes::new()).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateBucket must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let role_arn = role(&router, "healthimaging-copy", HEALTHIMAGING_TRUST).await;

    let case = "a copy of an imported image set";
    let instance = Instance {
        key: "copy/study.dcm",
        transfer_syntax: EXPLICIT_VR_LE,
        study_uid: "1.2.826.0.1.3680043.9.7133.5.1",
        series_uid: "1.2.826.0.1.3680043.9.7133.5.1.1",
        sop_uid: "1.2.826.0.1.3680043.9.7133.5.1.1.1",
        patient_id: "PID-COPY",
        patient_name: "COPY^CANDIDATE",
        modality: "CT",
        study_description: Some("COPY STUDY"),
        accession_number: Some("ACC-COPY"),
        pixel_data: Some(PIXELS_A),
    };
    let written = s3(
        &router,
        "PUT",
        &format!("/{BUCKET}/{}", instance.key),
        &[("content-type", "application/dicom")],
        Bytes::from(instance.part10()),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "setup: PutObject must succeed, got {} with {:?}",
        written.status,
        written.text()
    );

    let datastore_id = active_datastore(&router, case, "copy-store").await;
    let submitted = imaging_ok(
        &router,
        case,
        "POST",
        &format!("/startDICOMImportJob/datastore/{datastore_id}"),
        Some(json!({
            "clientToken": "copy-import",
            "dataAccessRoleArn": role_arn,
            "inputS3Uri": format!("s3://{BUCKET}/copy/"),
            "outputS3Uri": format!("s3://{BUCKET}/out/"),
        })),
    )
    .await;
    let job_id = submitted.string(&["jobId"]);
    assert_eq!(
        settled_job(&router, case, &datastore_id, &job_id).await,
        "COMPLETED",
        "case {case:?}: expected the import to reach COMPLETED"
    );
    let summaries =
        image_sets_of_study(&router, case, &datastore_id, instance.study_uid, 1).await;
    let source_id = summaries[0]["imageSetId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The source as the copy request has to name it: its own latest version.
    let source = imaging_ok(
        &router,
        case,
        "POST",
        &format!("/datastore/{datastore_id}/imageSet/{source_id}/getImageSet"),
        None,
    )
    .await;
    let latest_version_id = source.string(&["versionId"]);
    let source_document = imaging_ok(
        &router,
        case,
        "POST",
        &format!("/datastore/{datastore_id}/imageSet/{source_id}/getImageSetMetadata"),
        None,
    )
    .await
    .decoded_json(case);

    let copied = imaging_ok(
        &router,
        case,
        "POST",
        &format!("/datastore/{datastore_id}/imageSet/{source_id}/copyImageSet"),
        Some(json!({ "sourceImageSet": { "latestVersionId": latest_version_id } })),
    )
    .await;
    let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);
    assert!(
        !destination_id.is_empty(),
        "case {case:?}: expected the copy to name its destination, got {:?}",
        copied.text()
    );

    // The copy reaches its terminal state and status, polled the way a client polls.
    let deadline = tokio::time::Instant::now() + POLL_BUDGET;
    loop {
        let read = imaging_ok(
            &router,
            case,
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{destination_id}/getImageSet"),
            None,
        )
        .await;
        let state = read.string(&["imageSetState"]);
        let status = read.string(&["imageSetWorkflowStatus"]);
        if state == "ACTIVE" && status == "COPIED" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the copy never reached ACTIVE/COPIED, last {state:?}/{status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // The copy describes the same study, patient and series as its source, and holds the
    // same frames: the two documents differ only in which image set they are.
    let mut destination_document = imaging_ok(
        &router,
        case,
        "POST",
        &format!("/datastore/{datastore_id}/imageSet/{destination_id}/getImageSetMetadata"),
        None,
    )
    .await
    .decoded_json(case);
    let mut expected = source_document.clone();
    for document in [&mut expected, &mut destination_document] {
        document
            .as_object_mut()
            .expect("an aggregated metadata document")
            .remove("ImageSetID");
    }
    assert_eq!(
        destination_document, expected,
        "case {case:?}: expected the copy's metadata to match its source's"
    );
}

/// RULE (above), the role half: `dataAccessRoleArn` is resolved against the account's IAM
/// records before anything is written, so a role the account does not hold and a role
/// whose trust policy does not admit this service are both refused 403
/// `AccessDeniedException` — and the refused call leaves the data store holding exactly
/// the jobs it held before.
///
/// The rows differ along WHY the role cannot be assumed (no such role; a role held but
/// trusting another service's principal) and along the role's name, and are checked
/// against a control row naming a role this service can assume.
#[tokio::test]
async fn an_unassumable_data_access_role_refuses_the_import_and_creates_no_job() {
    let router = emulator();
    let created = s3(&router, "PUT", &format!("/{BUCKET}"), &[], Bytes::new()).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateBucket must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let assumable = role(&router, "healthimaging-assumable", HEALTHIMAGING_TRUST).await;
    let untrusted = role(&router, "healthimaging-untrusted", LAMBDA_TRUST).await;
    let absent = format!("arn:aws:iam::000000000000:role/{STUDY_ROOT}-never-created");

    let datastore_id = active_datastore(&router, "role rows", "import-role-store").await;

    // The control: a role this service can assume is served, and the job it created is
    // what the refusals below must not add to.
    let admitted = imaging_ok(
        &router,
        "an assumable role",
        "POST",
        &format!("/startDICOMImportJob/datastore/{datastore_id}"),
        Some(json!({
            "clientToken": "role-control",
            "dataAccessRoleArn": assumable,
            "inputS3Uri": format!("s3://{BUCKET}/empty/"),
            "outputS3Uri": format!("s3://{BUCKET}/out/"),
        })),
    )
    .await;
    assert_eq!(
        admitted.string(&["jobStatus"]),
        "SUBMITTED",
        "case \"an assumable role\": expected SUBMITTED, got {:?}",
        admitted.text()
    );

    let listed_before = imaging_ok(
        &router,
        "before the refusals",
        "GET",
        &format!("/listDICOMImportJobs/datastore/{datastore_id}?maxResults=50"),
        None,
    )
    .await;
    let held_before = listed_before.json()["jobSummaries"]
        .as_array()
        .map_or(0, Vec::len);

    for (case, role_arn) in [
        ("a role the account does not hold", absent.as_str()),
        ("a role trusting another service", untrusted.as_str()),
    ] {
        let refused = imaging(
            &router,
            "POST",
            &format!("/startDICOMImportJob/datastore/{datastore_id}"),
            Some(json!({
                "clientToken": format!("role-{}", case.len()),
                "dataAccessRoleArn": role_arn,
                "inputS3Uri": format!("s3://{BUCKET}/empty/"),
                "outputS3Uri": format!("s3://{BUCKET}/out/"),
            })),
        )
        .await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with {:?}",
            refused.status,
            refused.text()
        );
        let code = refused
            .header("x-amzn-errortype")
            .unwrap_or_else(|| refused.string(&["__type"]));
        assert!(
            code.contains("AccessDeniedException"),
            "case {case:?}: expected AccessDeniedException, got {code:?} with {:?}",
            refused.text()
        );
        let expected = format!(
            "Amazon Medical Imaging is not authorized to assume role: {role_arn}. \
             Please update the role's trust policy."
        );
        assert_eq!(
            refused.string(&["message"]),
            expected,
            "case {case:?}: expected the refusal text {expected:?}, got {:?}",
            refused.text()
        );

        // Nothing was written: the store holds exactly the jobs it held before.
        let listed = imaging_ok(
            &router,
            case,
            "GET",
            &format!("/listDICOMImportJobs/datastore/{datastore_id}?maxResults=50"),
            None,
        )
        .await;
        let held = listed.json()["jobSummaries"].as_array().map_or(0, Vec::len);
        assert_eq!(
            held, held_before,
            "case {case:?}: expected the refused submit to create no job, got {held} jobs \
             where {held_before} were held"
        );
    }
}
