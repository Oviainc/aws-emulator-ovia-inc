// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the JOB DOCUMENT an OTA update deploys: `CreateOTAUpdate`
//! models no `document` member, so the document the update's AWS IoT job hands its
//! targets is one the service GENERATES from the request.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `POST /otaUpdates/{otaUpdateId}` and then `GET /jobs/{jobId}/job-document`
//! for the `awsIotJobId` the create reported, signed for the `iot` signing name — and
//! also reads the SHARED job-execution store the IoT Jobs data plane serves, which is
//! where the copy a device is handed lives.
//!
//! The rules under test:
//!
//! * the deployed job's document is a single `afr_ota` object stating, per requested
//!   file and in request order, that file's name as the path the device writes, its id
//!   (the id it has in its stream, or its position in `files`), its `fileType` and the
//!   download location its `fileLocation.s3Location` names, beside the transfer
//!   `protocols` the request asked for and the `streamname` its files are served from;
//! * what the model documents as not sent to a device as part of the job document — a
//!   file's `attributes`, the update's `additionalParameters` — is not in it;
//! * the code-signing material a file carries is stated under the keys a device reads
//!   it by;
//! * both views `GetJobDocument` selects between report that document (an OTA update
//!   names no substitution parameters), and every targeted device's execution carries
//!   the same copy;
//! * a request whose files would generate a document past what `JobDocument` may hold
//!   is refused and records nothing.
//!
//! The tables span what the rules generalise over — how many files one update carries
//! and in which location form, which protocols it names, whether a pre-signed lifetime
//! is asked for, which code-signing material a file supplies, and how large the
//! generated document grows — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    MAX_JOB_DOCUMENT_BYTES, OTA_JOB_DOCUMENT_ROOT, OTA_UPDATE_JOB_ID_PREFIX,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::JobExecutionStore;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for; the download locations the document states are
/// scoped to it.
const REGION: &str = "us-east-1";
/// An IAM role ARN of the shape `CreateOTAUpdate$roleArn` is modelled with (len
/// 20..2048); the operation neither assumes nor resolves it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ota-update-role";
/// The bucket the firmware images of these cases live in.
const BUCKET: &str = "firmware-images";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service, and how the service picks
/// the region its locations are scoped to.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can read the SHARED job-execution store the control
/// plane writes the device-facing copy of the document into.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    async fn create_ota_update(&self, ota_update_id: &str, request: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/otaUpdates/{ota_update_id}"),
            Some(request),
        )
        .await
    }

    async fn get_ota_update(&self, ota_update_id: &str) -> Reply {
        self.call("GET", &format!("/otaUpdates/{ota_update_id}"), None)
            .await
    }

    /// `GetJobDocument` with `beforeSubstitution` left out, which is the modelled
    /// default (`false`).
    async fn job_document(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/jobs/{job_id}/job-document"), None)
            .await
    }

    /// `GetJobDocument` with `beforeSubstitution` spelled out.
    async fn job_document_before(&self, job_id: &str, before_substitution: bool) -> Reply {
        self.call(
            "GET",
            &format!("/jobs/{job_id}/job-document?beforeSubstitution={before_substitution}"),
            None,
        )
        .await
    }

    /// The document the SHARED store holds for `thing_name`'s execution of a job: what
    /// the device is handed.
    fn execution_document(&self, thing_name: &str, job_id: &str) -> Option<String> {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .executions_for(thing_name)
            .iter()
            .find(|record| record.job_id == job_id)
            .and_then(|record| record.job_document.clone())
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// The id of the AWS IoT job an OTA update deploys through.
fn job_id_of(ota_update_id: &str) -> String {
    format!("{OTA_UPDATE_JOB_ID_PREFIX}{ota_update_id}")
}

/// A file located in S3, the form `OTAUpdateFile$fileLocation.s3Location` names.
fn s3_file(file_name: &str, key: &str) -> Value {
    json!({
        "fileName": file_name,
        "fileType": 0,
        "fileVersion": "1.0.0",
        "fileLocation": {"s3Location": {"bucket": BUCKET, "key": key}}
    })
}

/// A file located in an IoT stream, the other form `fileLocation` names.
fn stream_file(file_name: &str, stream_id: &str, file_id: i64) -> Value {
    json!({
        "fileName": file_name,
        "fileLocation": {"stream": {"streamId": stream_id, "fileId": file_id}}
    })
}

/// The `afr_ota` object of a document, or a panic naming what was reported instead.
fn afr_ota(case: &str, document: &str) -> Value {
    let parsed: Value = serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("case {case:?}: expected JSON, got {document:?}: {error}"));
    let object = parsed.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected a JSON object, got {parsed:?}");
    });
    assert_eq!(
        object.keys().map(String::as_str).collect::<Vec<&str>>(),
        vec![OTA_JOB_DOCUMENT_ROOT],
        "case {case:?}: expected the document to hang off one {OTA_JOB_DOCUMENT_ROOT} \
         member, got {parsed:?}"
    );
    object[OTA_JOB_DOCUMENT_ROOT].clone()
}

/// One row of the document table.
struct DocumentCase {
    case: &'static str,
    ota_update_id: &'static str,
    /// The things the update targets, each as a `thing/…` ARN.
    targets: &'static [&'static str],
    /// The `files` member, always at least one (modelled `len 1..`).
    files: Vec<Value>,
    /// The `protocols` member, when the row names any. The catalog they are drawn from
    /// is the modelled `Protocol` enum, `MQTT | HTTP`
    /// (`smithy_query(query="shape",shape="Protocols,Protocol")`), with `Protocols`
    /// modelled `len 1..2`.
    protocols: Option<Vec<&'static str>>,
    /// `awsJobPresignedUrlConfig.expiresInSec`, when the row asks for one. The values
    /// come from the member's own documentation: "Valid values are 60 - 3600, the
    /// default value is 1800 seconds".
    expires_in_sec: Option<i64>,
    /// The `additionalParameters` the row carries, "which are name-value pairs. They
    /// won't be sent to devices as a part of the Job document".
    additional_parameters: Option<Value>,
    /// Text the generated document must NOT carry: what the model documents as never
    /// sent to a device as part of the job document.
    withheld: Vec<&'static str>,
}

impl DocumentCase {
    /// The request body this row sends.
    fn request(&self) -> Value {
        let mut request = json!({
            "targets": self.targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
            "files": self.files.clone(),
            "roleArn": ROLE_ARN,
        });
        if let Some(protocols) = &self.protocols {
            request["protocols"] = json!(protocols);
        }
        if let Some(seconds) = self.expires_in_sec {
            request["awsJobPresignedUrlConfig"] = json!({"expiresInSec": seconds});
        }
        if let Some(parameters) = &self.additional_parameters {
            request["additionalParameters"] = parameters.clone();
        }
        request
    }
}

/// RULE: the job an OTA update deploys carries a document the service generates from
/// the request — one `afr_ota` object naming the protocols and stream the update
/// transfers over and, per requested file, that file's path, id, type and download
/// location — and that document is what the job's reader and the targeted devices' own
/// executions report.
#[tokio::test]
async fn an_ota_update_deploys_a_job_document_generated_from_the_request() {
    let emulator = Emulator::new();

    let cases: Vec<DocumentCase> = vec![
        DocumentCase {
            case: "one S3 file, no protocols and no pre-signed lifetime",
            ota_update_id: "ota-doc-minimal",
            targets: &["doc-sensor-a"],
            files: vec![s3_file("firmware.bin", "firmware.bin")],
            protocols: None,
            expires_in_sec: None,
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "an S3 object under a prefix, over HTTP, at the longest pre-signed lifetime",
            ota_update_id: "ota-doc-http",
            targets: &["doc-sensor-b", "doc-sensor-c"],
            files: vec![s3_file("image.bin", "firmware/2.0.0/image.bin")],
            protocols: Some(vec!["HTTP"]),
            expires_in_sec: Some(3600),
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "a versioned S3 object at the shortest pre-signed lifetime",
            ota_update_id: "ota-doc-versioned",
            targets: &["doc-sensor-d"],
            files: vec![json!({
                "fileName": "firmware.bin",
                "fileLocation": {
                    "s3Location": {"bucket": BUCKET, "key": "firmware.bin", "version": "objver-7"}
                }
            })],
            protocols: Some(vec!["HTTP"]),
            expires_in_sec: Some(60),
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "one streamed file over MQTT",
            ota_update_id: "ota-doc-mqtt",
            targets: &["doc-sensor-e"],
            files: vec![stream_file("firmware.bin", "stream-one", 0)],
            protocols: Some(vec!["MQTT"]),
            expires_in_sec: None,
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "three files, both protocols, a stream beside two S3 objects",
            ota_update_id: "ota-doc-mixed",
            targets: &["doc-sensor-f", "doc-sensor-g"],
            files: vec![
                s3_file("firmware.bin", "firmware.bin"),
                stream_file("bootloader.bin", "stream-two", 7),
                s3_file("config.json", "configs/config.json"),
            ],
            protocols: Some(vec!["HTTP", "MQTT"]),
            expires_in_sec: Some(1800),
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "what is documented as never sent to a device stays out",
            ota_update_id: "ota-doc-withheld",
            targets: &["doc-sensor-h"],
            files: vec![json!({
                "fileName": "firmware.bin",
                "fileType": 255,
                "fileLocation": {"s3Location": {"bucket": BUCKET, "key": "firmware.bin"}},
                "attributes": {"board": "rev-c"}
            })],
            protocols: Some(vec!["HTTP"]),
            expires_in_sec: None,
            additional_parameters: Some(json!({"channel": "beta"})),
            withheld: vec!["board", "rev-c", "channel", "beta"],
        },
        DocumentCase {
            case: "an S3 location naming no key states no download location",
            ota_update_id: "ota-doc-partial-location",
            targets: &["doc-sensor-i"],
            files: vec![json!({
                "fileName": "firmware.bin",
                "fileLocation": {"s3Location": {"bucket": BUCKET}}
            })],
            protocols: Some(vec!["HTTP"]),
            expires_in_sec: None,
            additional_parameters: None,
            withheld: Vec::new(),
        },
        DocumentCase {
            case: "a file the request named no name for",
            ota_update_id: "ota-doc-unnamed-file",
            targets: &["doc-sensor-j"],
            files: vec![json!({
                "fileLocation": {"s3Location": {"bucket": BUCKET, "key": "unnamed.bin"}}
            })],
            protocols: None,
            expires_in_sec: None,
            additional_parameters: None,
            withheld: Vec::new(),
        },
    ];

    for case in &cases {
        for thing_name in case.targets {
            assert_eq!(
                emulator.create_thing(thing_name).await.status,
                200,
                "case {:?}: the target device must be registrable",
                case.case
            );
        }

        let request = case.request();
        let created = emulator.create_ota_update(case.ota_update_id, &request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, created.status, created.body
        );
        let job_id = created.text("awsIotJobId").unwrap_or_default();
        assert_eq!(
            job_id,
            job_id_of(case.ota_update_id),
            "case {:?}: expected the job id derived from the update id, got {:?}",
            case.case,
            created.body
        );

        // The deployed job HAS a document, and it is the generated one.
        let reply = emulator.job_document(&job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the job's document reader, got {} {:?}",
            case.case, reply.status, reply.body
        );
        let document = reply.text("document").unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the deployed job to report a document, got {:?}",
                case.case, reply.body
            )
        });
        assert!(
            !document.is_empty() && document.len() <= MAX_JOB_DOCUMENT_BYTES,
            "case {:?}: expected a non-empty document of at most {MAX_JOB_DOCUMENT_BYTES} \
             bytes, got {} bytes",
            case.case,
            document.len()
        );

        let update = afr_ota(case.case, &document);
        assert_eq!(
            update.get("protocols"),
            request.get("protocols"),
            "case {:?}: expected the protocols the request named, got {update:?}",
            case.case
        );

        // The stream the files are served from is the one the first streamed file
        // names; an update whose files are all in S3 names none.
        let expected_stream = case.files.iter().find_map(|file| {
            file.pointer("/fileLocation/stream/streamId")
                .and_then(Value::as_str)
        });
        assert_eq!(
            update.get("streamname").and_then(Value::as_str),
            expected_stream,
            "case {:?}: expected the stream the files are served from, got {update:?}",
            case.case
        );

        let entries = update
            .get("files")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: expected a files array, got {update:?}", case.case));
        assert_eq!(
            entries.len(),
            case.files.len(),
            "case {:?}: expected one entry per requested file, got {entries:?}",
            case.case
        );

        for (index, (requested, entry)) in case.files.iter().zip(entries).enumerate() {
            let where_ = format!("{} / files[{index}]", case.case);

            assert_eq!(
                entry.get("filepath"),
                requested.get("fileName"),
                "case {where_:?}: expected the requested file name as the device's path, \
                 got {entry:?}"
            );
            assert_eq!(
                entry.get("fileType"),
                requested.get("fileType"),
                "case {where_:?}: expected the requested fileType, got {entry:?}"
            );

            // A streamed file is addressed by its id IN the stream; an S3 object has no
            // such id, so the entry is told apart by the file's position in `files`.
            let expected_id = requested
                .pointer("/fileLocation/stream/fileId")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| i64::try_from(index).expect("a file index fits an i64"));
            assert_eq!(
                entry.get("fileid").and_then(Value::as_i64),
                Some(expected_id),
                "case {where_:?}: expected fileid {expected_id}, got {entry:?}"
            );

            let bucket = requested
                .pointer("/fileLocation/s3Location/bucket")
                .and_then(Value::as_str);
            let key = requested
                .pointer("/fileLocation/s3Location/key")
                .and_then(Value::as_str);
            let url = entry.get("update_data_url").and_then(Value::as_str);
            match (bucket, key) {
                (Some(bucket), Some(key)) => {
                    let url = url.unwrap_or_else(|| {
                        panic!(
                            "case {where_:?}: expected the download location of \
                             {bucket}/{key}, got {entry:?}"
                        )
                    });
                    assert!(
                        url.starts_with(&format!(
                            "https://{bucket}.s3.{REGION}.amazonaws.com/{key}"
                        )),
                        "case {where_:?}: expected the S3 object's own URL, got {url:?}"
                    );
                    let version = requested
                        .pointer("/fileLocation/s3Location/version")
                        .and_then(Value::as_str);
                    assert_eq!(
                        url.contains(&format!("versionId={}", version.unwrap_or_default())),
                        version.is_some(),
                        "case {where_:?}: the object version is stated only when the \
                         request pinned one, got {url:?}"
                    );
                    assert_eq!(
                        url.contains(&format!(
                            "X-Amz-Expires={}",
                            case.expires_in_sec.unwrap_or_default()
                        )),
                        case.expires_in_sec.is_some(),
                        "case {where_:?}: the lifetime is stated only when the request \
                         asked for one, got {url:?}"
                    );
                }
                _ => assert!(
                    url.is_none(),
                    "case {where_:?}: a location naming no bucket and key is no download \
                     location, got {url:?}"
                ),
            }
        }

        for withheld in &case.withheld {
            assert!(
                !document.contains(withheld),
                "case {:?}: expected {withheld:?} to stay out of the document, got \
                 {document:?}",
                case.case
            );
        }

        // An OTA update names no substitution parameters, so both views of its job's
        // document are the document as generated.
        for before_substitution in [false, true] {
            let view = emulator
                .job_document_before(&job_id, before_substitution)
                .await;
            assert_eq!(
                view.text("document").as_deref(),
                Some(document.as_str()),
                "case {:?} (beforeSubstitution={before_substitution}): expected the \
                 generated document, got {:?}",
                case.case,
                view.body
            );
        }

        // ROUND TRIP through the data plane: every targeted device's execution carries
        // the same copy, which is what a device agent downloads from.
        for thing_name in case.targets {
            assert_eq!(
                emulator.execution_document(thing_name, &job_id).as_deref(),
                Some(document.as_str()),
                "case {:?}: expected {thing_name}'s execution to carry the job's \
                 document, got {:?}",
                case.case,
                emulator.execution_document(thing_name, &job_id)
            );
        }
    }
}

/// One row of the code-signing table.
struct SigningCase {
    case: &'static str,
    ota_update_id: &'static str,
    /// The `codeSigning` member of the requested file.
    code_signing: Value,
    /// The `certfile` the entry must state, if any.
    certfile: Option<&'static str>,
    /// The key the signature must be stated under, and its value, if any.
    signature: Option<(&'static str, String)>,
}

/// RULE: the code-signing material a requested file carries is stated in that file's
/// document entry under the keys a device verifies the image by — the signing
/// certificate's path on the device, and a supplied signature under the key naming the
/// algorithms it was made with.
#[tokio::test]
async fn the_code_signing_material_a_file_carries_is_in_its_document_entry() {
    let emulator = Emulator::new();

    // The signature bytes travel as a base64 blob (`CodeSigningSignature$inlineDocument`
    // is "A base64 encoded binary representation of the code signing signature").
    let signature_bytes = b"a signature over the firmware image".to_vec();
    let signature = aws_smithy_types::base64::encode(signature_bytes.as_slice());

    let cases: Vec<SigningCase> = vec![
        SigningCase {
            case: "a custom signature with its algorithms and a certificate chain",
            ota_update_id: "ota-sign-custom",
            code_signing: json!({
                "customCodeSigning": {
                    "signature": {"inlineDocument": signature.clone()},
                    "certificateChain": {"certificateName": "signer-cert.pem"},
                    // FreeRTOS's own example pair, named in the member's documentation.
                    "hashAlgorithm": "SHA256",
                    "signatureAlgorithm": "ECDSA"
                }
            }),
            certfile: Some("signer-cert.pem"),
            signature: Some(("sig-sha256-ecdsa", signature.clone())),
        },
        SigningCase {
            case: "another algorithm pair names another signature key",
            ota_update_id: "ota-sign-rsa",
            code_signing: json!({
                "customCodeSigning": {
                    "signature": {"inlineDocument": signature.clone()},
                    "hashAlgorithm": "SHA1",
                    "signatureAlgorithm": "RSA"
                }
            }),
            certfile: None,
            signature: Some(("sig-sha1-rsa", signature.clone())),
        },
        SigningCase {
            case: "a signing profile naming the certificate's path on the device",
            ota_update_id: "ota-sign-profile",
            code_signing: json!({
                "startSigningJobParameter": {
                    "signingProfileName": "firmware-profile",
                    "signingProfileParameter": {
                        "platform": "AmazonFreeRTOS-Default",
                        "certificatePathOnDevice": "/device/certs/signer.crt"
                    }
                }
            }),
            certfile: Some("/device/certs/signer.crt"),
            signature: None,
        },
        SigningCase {
            case: "a signature whose algorithms the request did not name is not stated",
            ota_update_id: "ota-sign-unlabelled",
            code_signing: json!({
                "customCodeSigning": {
                    "signature": {"inlineDocument": signature.clone()},
                    "certificateChain": {"certificateName": "signer-cert.pem"}
                }
            }),
            certfile: Some("signer-cert.pem"),
            signature: None,
        },
        SigningCase {
            case: "a signing job done elsewhere puts no signature in the document",
            ota_update_id: "ota-sign-signer-job",
            code_signing: json!({"awsSignerJobId": "signer-job-1"}),
            certfile: None,
            signature: None,
        },
    ];

    for case in &cases {
        let mut file = s3_file("firmware.bin", "firmware.bin");
        file["codeSigning"] = case.code_signing.clone();
        let request = json!({
            "targets": [thing_arn("doc-signing-sensor")],
            "files": [file],
            "protocols": ["HTTP"],
            "roleArn": ROLE_ARN,
        });

        let created = emulator.create_ota_update(case.ota_update_id, &request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, created.status, created.body
        );

        let job_id = job_id_of(case.ota_update_id);
        let reply = emulator.job_document(&job_id).await;
        let document = reply.text("document").unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the deployed job to report a document, got {:?}",
                case.case, reply.body
            )
        });
        let update = afr_ota(case.case, &document);
        let entry = update
            .pointer("/files/0")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: expected one file entry, got {update:?}", case.case));

        assert_eq!(
            entry.get("certfile").and_then(Value::as_str),
            case.certfile,
            "case {:?}: expected the signing certificate the request named, got {entry:?}",
            case.case
        );
        match &case.signature {
            Some((key, expected)) => assert_eq!(
                entry.get(*key).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {:?}: expected the supplied signature under {key:?}, got {entry:?}",
                case.case
            ),
            None => {
                let signature_keys: Vec<&String> = entry
                    .as_object()
                    .map(|entry| entry.keys().filter(|key| key.starts_with("sig-")).collect())
                    .unwrap_or_default();
                assert!(
                    signature_keys.is_empty(),
                    "case {:?}: expected no signature to be stated, got {signature_keys:?}",
                    case.case
                );
            }
        }
    }
}

/// RULE: `JobDocument` is bounded (`len 0..32768`), so a request whose files would
/// generate a document past that bound is refused — no update, no job and no document
/// a reader could not report — while one that fits is deployed.
#[tokio::test]
async fn a_generated_document_past_what_a_job_document_may_hold_is_refused() {
    let emulator = Emulator::new();

    // `S3Key` is modelled with a minimum of 1 and NO maximum, so the key alone decides
    // how large the generated document grows.
    let fitting_key = format!("firmware/{}.bin", "k".repeat(30_000));
    let fitting = emulator
        .create_ota_update(
            "ota-doc-large",
            &json!({
                "targets": [thing_arn("doc-sensor-large")],
                "files": [s3_file("firmware.bin", &fitting_key)],
                "protocols": ["HTTP"],
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        fitting.status, 200,
        "case \"a document that still fits\": expected 200, got {} {:?}",
        fitting.status, fitting.body
    );
    let document = emulator
        .job_document(&job_id_of("ota-doc-large"))
        .await
        .text("document")
        .unwrap_or_default();
    assert!(
        document.contains(&fitting_key) && document.len() <= MAX_JOB_DOCUMENT_BYTES,
        "case \"a document that still fits\": expected the requested key inside a \
         document of at most {MAX_JOB_DOCUMENT_BYTES} bytes, got {} bytes",
        document.len()
    );

    let oversized_key = format!("firmware/{}.bin", "k".repeat(MAX_JOB_DOCUMENT_BYTES));
    let refused = emulator
        .create_ota_update(
            "ota-doc-oversized",
            &json!({
                "targets": [thing_arn("doc-sensor-oversized")],
                "files": [s3_file("firmware.bin", &oversized_key)],
                "protocols": ["HTTP"],
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"a document past the bound\": expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("InvalidRequestException"),
        "case \"a document past the bound\": expected InvalidRequestException, got {:?}",
        refused.error_code
    );
    assert!(
        refused
            .message()
            .contains(&format!("at most {MAX_JOB_DOCUMENT_BYTES} bytes")),
        "case \"a document past the bound\": expected the refusal to state the bound, \
         got {:?}",
        refused.message()
    );

    // The refused create recorded nothing: neither the update nor the job it would
    // have deployed exists.
    let update = emulator.get_ota_update("ota-doc-oversized").await;
    assert_eq!(
        update.status, 404,
        "case \"a document past the bound\": the refused create must record no update, \
         got {} {:?}",
        update.status, update.body
    );
    let job = emulator.job_document(&job_id_of("ota-doc-oversized")).await;
    assert_eq!(
        job.status, 404,
        "case \"a document past the bound\": the refused create must deploy no job, got \
         {} {:?}",
        job.status, job.body
    );
    assert_eq!(
        emulator
            .execution_document("doc-sensor-oversized", &job_id_of("ota-doc-oversized")),
        None,
        "case \"a document past the bound\": the refused create must queue no execution"
    );
}
