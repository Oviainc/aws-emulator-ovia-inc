// SPDX-License-Identifier: CC0-1.0

//! The documented container image workflow of a Lightsail container service,
//! driven end to end: `CreateContainerServiceRegistryLogin` → push to the
//! announced registry address → `RegisterContainerImage` → `GetContainerImages`
//! → `DeleteContainerImage`.
//!
//! The rules under test:
//!
//! * the announced registry address serves the container image registry API, and
//!   it is CONTENT-ADDRESSED: a blob and a manifest are stored under the digest
//!   of their own bytes, a digest a client claims is checked against the digest
//!   of what arrived, a manifest is accepted only once every blob it references
//!   is in the registry, and a read answers exactly the bytes that were pushed;
//! * `RegisterContainerImage` registers an image the account's registry in the
//!   request's region HOLDS: it answers 200 with the service-assigned name
//!   `:<service>.<label>.<version>`, where the version is 1 for the first image
//!   registered under that label and one more for each after it (the modelled
//!   `RegisterContainerImageRequest$label` documentation), carrying the digest
//!   the request named and the instant it was registered;
//! * a digest the registry does not hold — nothing pushed it, or it was pushed
//!   to another region's registry — is refused with the service's
//!   `DoesNotExist` envelope and registers nothing;
//! * a registered image round-trips: `GetContainerImages` reports it in
//!   registration order and `DeleteContainerImage` removes exactly it.
//!
//! Every control-plane case goes through the same front door a client uses
//! (`POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>` and an
//! `awsJson1_1` body) dispatched by the multi-service router, and every registry
//! case is an unsigned plain HTTP request carrying the HTTP Basic credentials a
//! registry client presents.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint the cases reach the emulator on: the loopback address and port
/// the emulator's own listener binds.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The media type an OCI image manifest is pushed as.
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// The media type an image configuration blob is referenced as.
const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// The media type an image layer is referenced as.
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// The media type an opaque blob is pushed and read as.
const BLOB_MEDIA_TYPE: &str = "application/octet-stream";

/// How far a reported `createdAt` may sit from the call's own instant and still
/// be that instant: the slack covers only the wall-clock time the calls take.
const CLOCK_SLACK_SECONDS: f64 = 120.0;

/// One answer from the registry address.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Answer {
    /// The value of the header `name`, if the answer carries one.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The `code` of the registry API error envelope this answer carries, if it
    /// carries one.
    fn error_code(&self) -> Option<String> {
        let parsed: Value = serde_json::from_slice(&self.body).ok()?;
        parsed["errors"][0]["code"].as_str().map(str::to_string)
    }

    /// The body as text, for a message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One set of registry log in credentials and the address they log in at.
#[derive(Debug, Clone)]
struct Login {
    username: String,
    password: String,
    /// The request target of the announced address: its path, with no host.
    base: String,
}

/// How a blob is sent to the registry — the three ways the registry API admits,
/// which a client picks between by size and by whether it can resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushStyle {
    /// One `POST` carrying both the digest and the bytes.
    Monolithic,
    /// `POST` to start a session, then one `PUT` carrying the bytes.
    Session,
    /// `POST` to start a session, a `PATCH` per chunk, then an empty `PUT`.
    Chunked,
}

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: a control-plane call routes by `X-Amz-Target`
        // and names its region in the credential SCOPE alone, and a registry
        // request is unsigned by construction — it authenticates with HTTP Basic
        // credentials.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(&self, case: &str, region: &str, operation: &str, body: Value, want: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want,
            "case {case}: expected {operation} in {region} to answer HTTP {want}, got {status} \
             {parsed:?}"
        );
        parsed
    }

    /// Create the container service `name` in `region` (HTTP 201, its modelled
    /// success status).
    async fn create_service(&self, case: &str, region: &str, name: &str) {
        self.ok(
            case,
            region,
            "CreateContainerService",
            json!({"serviceName": name, "power": "nano", "scale": 1}),
            201,
        )
        .await;
    }

    /// Create one set of registry log in credentials in `region`, the way the
    /// documented workflow starts.
    async fn login(&self, case: &str, region: &str) -> Login {
        let body = self
            .ok(
                case,
                region,
                "CreateContainerServiceRegistryLogin",
                json!({}),
                200,
            )
            .await;
        let login = &body["registryLogin"];
        let field = |name: &str| {
            login[name]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a string {name} in the log in, got {login:?}")
                })
                .to_string()
        };
        let registry = field("registry");
        let base = registry
            .strip_prefix(&format!("https://{ENDPOINT_HOST}"))
            .unwrap_or_else(|| {
                panic!("case {case}: expected the registry address on this endpoint, got {registry}")
            })
            .to_string();
        Login {
            username: field("username"),
            password: field("password"),
            base,
        }
    }

    /// Make one request to the registry, presenting `login`'s credentials the way
    /// a registry client does.
    async fn registry(
        &self,
        method: &str,
        target: &str,
        login: &Login,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> Answer {
        self.registry_request(method, target, login, content_type, None, body)
            .await
    }

    /// Make one request to the registry carrying `range` as its
    /// `Content-Range`, the way a client sending a blob in chunks says where
    /// each chunk goes.
    async fn registry_with_range(
        &self,
        method: &str,
        target: &str,
        login: &Login,
        range: Option<&str>,
        body: Vec<u8>,
    ) -> Answer {
        self.registry_request(method, target, login, Some(BLOB_MEDIA_TYPE), range, body)
            .await
    }

    /// One request to the registry: the method, the target, the credentials and
    /// the headers a registry client sends with what it is pushing.
    async fn registry_request(
        &self,
        method: &str,
        target: &str,
        login: &Login,
        content_type: Option<&str>,
        range: Option<&str>,
        body: Vec<u8>,
    ) -> Answer {
        let credentials = ::aws_smithy_types::base64::encode(format!(
            "{}:{}",
            login.username, login.password
        ));
        let mut builder = ::http::Request::builder()
            .method(method)
            .uri(target)
            .header("host", ENDPOINT_HOST)
            .header("authorization", format!("Basic {credentials}"));
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        if let Some(range) = range {
            builder = builder.header("content-range", range);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        Answer {
            status: response.status().as_u16(),
            headers,
            body: response.body().to_vec(),
        }
    }

    /// Push one blob to `repository` in the way `style` says, asserting the
    /// registry stored it under the digest of its own bytes, and answering that
    /// digest.
    async fn push_blob(
        &self,
        case: &str,
        login: &Login,
        repository: &str,
        style: PushStyle,
        content: &[u8],
    ) -> String {
        let digest = sha256_digest(content);
        let base = &login.base;
        let answer = match style {
            PushStyle::Monolithic => {
                self.registry(
                    "POST",
                    &format!("{base}/v2/{repository}/blobs/uploads/?digest={digest}"),
                    login,
                    Some(BLOB_MEDIA_TYPE),
                    content.to_vec(),
                )
                .await
            }
            PushStyle::Session | PushStyle::Chunked => {
                let started = self
                    .registry(
                        "POST",
                        &format!("{base}/v2/{repository}/blobs/uploads/"),
                        login,
                        None,
                        Vec::new(),
                    )
                    .await;
                assert_eq!(
                    started.status, 202,
                    "case {case}: expected HTTP 202 for a started upload, got {started:?}"
                );
                let session = started
                    .header("location")
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a session address, got {started:?}")
                    })
                    .to_string();
                assert!(
                    started.header("docker-upload-uuid").is_some(),
                    "case {case}: expected the session's id, got {started:?}"
                );
                if style == PushStyle::Session {
                    self.registry(
                        "PUT",
                        &format!("{session}?digest={digest}"),
                        login,
                        Some(BLOB_MEDIA_TYPE),
                        content.to_vec(),
                    )
                    .await
                } else {
                    // Chunked: a `PATCH` per chunk, then a `PUT` carrying
                    // nothing but the digest of the whole blob. One-byte chunks
                    // make the row drive several of them whatever its size.
                    for chunk in content.chunks(1).map(<[u8]>::to_vec) {
                        let appended = self
                            .registry("PATCH", &session, login, Some(BLOB_MEDIA_TYPE), chunk)
                            .await;
                        assert_eq!(
                            appended.status, 202,
                            "case {case}: expected HTTP 202 for an appended chunk, got \
                             {appended:?}"
                        );
                    }
                    self.registry(
                        "PUT",
                        &format!("{session}?digest={digest}"),
                        login,
                        None,
                        Vec::new(),
                    )
                    .await
                }
            }
        };
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 for a stored blob, got {answer:?}"
        );
        assert_eq!(
            answer.header("docker-content-digest"),
            Some(digest.as_str()),
            "case {case}: expected the registry to report the digest of the pushed bytes, got \
             {answer:?}"
        );
        assert_eq!(
            answer.header("location"),
            Some(format!("{base}/v2/{repository}/blobs/{digest}").as_str()),
            "case {case}: expected the stored blob's own address, got {answer:?}"
        );
        digest
    }

    /// Push one whole image — a configuration blob, the layers, then the
    /// manifest — to `repository` under `tag`, and answer the manifest bytes and
    /// the digest the registry reported for them.
    async fn push_image(
        &self,
        case: &str,
        login: &Login,
        repository: &str,
        tag: &str,
        style: PushStyle,
        layers: &[&[u8]],
    ) -> (Vec<u8>, String) {
        // The version check a registry client makes first.
        let version = self
            .registry("GET", &format!("{}/v2/", login.base), login, None, Vec::new())
            .await;
        assert_eq!(
            version.status, 200,
            "case {case}: expected the version check to answer HTTP 200, got {version:?}"
        );
        assert_eq!(
            version.header("docker-distribution-api-version"),
            Some("registry/2.0"),
            "case {case}: expected the registry API version, got {version:?}"
        );

        let config = format!("{{\"architecture\":\"amd64\",\"os\":\"linux\",\"case\":{case:?}}}")
            .into_bytes();
        let config_digest = self
            .push_blob(case, login, repository, style, &config)
            .await;
        let mut layer_descriptors = Vec::new();
        for layer in layers {
            let digest = self.push_blob(case, login, repository, style, layer).await;
            layer_descriptors.push(json!({
                "mediaType": LAYER_MEDIA_TYPE,
                "digest": digest,
                "size": layer.len(),
            }));
        }
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": MANIFEST_MEDIA_TYPE,
            "config": {
                "mediaType": CONFIG_MEDIA_TYPE,
                "digest": config_digest,
                "size": config.len(),
            },
            "layers": layer_descriptors,
        })
        .to_string()
        .into_bytes();
        let answer = self
            .registry(
                "PUT",
                &format!("{}/v2/{repository}/manifests/{tag}", login.base),
                login,
                Some(MANIFEST_MEDIA_TYPE),
                manifest.clone(),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 for a stored manifest, got {answer:?}"
        );
        let digest = sha256_digest(&manifest);
        assert_eq!(
            answer.header("docker-content-digest"),
            Some(digest.as_str()),
            "case {case}: expected the digest of the manifest's own bytes, got {answer:?}"
        );
        (manifest, digest)
    }

    /// The `containerImages` `GetContainerImages` reports for `service`.
    async fn images(&self, case: &str, region: &str, service: &str) -> Vec<Value> {
        let body = self
            .ok(
                case,
                region,
                "GetContainerImages",
                json!({"serviceName": service}),
                200,
            )
            .await;
        body["containerImages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a containerImages list, got {body:?}"))
            .clone()
    }

    /// The `image` of every image that registry reports, in the reported order.
    async fn image_names(&self, case: &str, region: &str, service: &str) -> Vec<String> {
        self.images(case, region, service)
            .await
            .iter()
            .map(|image| image["image"].as_str().unwrap_or("<no image>").to_string())
            .collect()
    }

    /// Register `digest` to `service` under `label`, answering the status and
    /// body.
    async fn register(
        &self,
        region: &str,
        service: &str,
        label: &str,
        digest: &str,
    ) -> (u16, Value) {
        self.call(
            region,
            "RegisterContainerImage",
            json!({"serviceName": service, "label": label, "digest": digest}),
        )
        .await
    }
}

/// The digest of `content` as the registry protocol spells one, computed here
/// with the `sha2` crate so the test's expectation does not come from the code
/// under test.
fn sha256_digest(content: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// Seconds since the Unix epoch, the unit `createdAt` is carried in.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One image pushed and registered. The rows differ along everything the rule
/// generalises over: the region the workflow is driven in, the container service
/// name, the repository name (one component, several, and the separators the
/// registry API's name grammar admits), the tag, how the blobs are sent, the
/// number of layers and their sizes — including an empty layer, whose digest is
/// the digest of no bytes at all.
#[derive(Debug, Clone)]
struct PushRow {
    case: &'static str,
    region: &'static str,
    service: &'static str,
    repository: &'static str,
    tag: &'static str,
    style: PushStyle,
    layers: Vec<&'static [u8]>,
}

/// The table of pushes.
fn push_rows() -> Vec<PushRow> {
    vec![
        PushRow {
            case: "one layer pushed whole, in the default region",
            region: "us-east-1",
            service: "web-service",
            repository: "web-service",
            tag: "latest",
            style: PushStyle::Monolithic,
            layers: vec![b"a single layer of image content"],
        },
        PushRow {
            case: "three layers through upload sessions, in another region",
            region: "us-west-2",
            service: "ab",
            repository: "ab/site",
            tag: "v1.2.3",
            style: PushStyle::Session,
            layers: vec![b"first layer", b"second layer", b"third layer"],
        },
        PushRow {
            case: "one layer sent in chunks, a nested repository name",
            region: "eu-west-1",
            service: "registry-2026",
            repository: "team.one/sub_project/app-2026",
            tag: "release_2026.10",
            style: PushStyle::Chunked,
            layers: vec![b"chunked"],
        },
        PushRow {
            case: "an empty layer and a larger one, in a fourth region",
            region: "ap-northeast-1",
            service: "r",
            repository: "r",
            tag: "x",
            style: PushStyle::Session,
            layers: vec![b"", &[b'z'; 4096]],
        },
    ]
}

/// The labels one image is registered under, in order, exercising the version
/// rule: the same label twice over, then another label, then a label at the
/// modelled `ContainerLabel` maximum length (53).
const LABEL_ROWS: &[&str] = &[
    "web",
    "web",
    "mystaticwebsite",
    "maximum-length-label-abcdefghijabcdefghijabcdefghijab",
    "web",
];

/// Digests nothing pushed, in the forms a caller can present: a well-formed
/// sha256 digest of content that was never sent, a truncated one, another
/// algorithm, and no algorithm at all.
const UNPUSHED_DIGESTS: &[(&str, &str)] = &[
    (
        "a well-formed digest of content nothing pushed",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
    ("a truncated digest", "sha256:0f1e2d"),
    ("a digest naming another algorithm", "sha512:9c"),
    ("a digest with no algorithm prefix at all", "deadbeef"),
];

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the announced registry address serves the registry API and is
/// content-addressed — a blob and a manifest are stored under the digest of
/// their own bytes and read back as exactly those bytes.
#[tokio::test]
async fn the_announced_registry_address_stores_what_was_pushed_to_it() {
    for row in push_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let login = emulator.login(case, row.region).await;
        let (manifest, digest) = emulator
            .push_image(
                case,
                &login,
                row.repository,
                row.tag,
                row.style,
                &row.layers,
            )
            .await;

        // Every layer reads back byte for byte, under the digest of its own
        // content.
        for layer in &row.layers {
            let layer_digest = sha256_digest(layer);
            let read = emulator
                .registry(
                    "GET",
                    &format!("{}/v2/{}/blobs/{layer_digest}", login.base, row.repository),
                    &login,
                    None,
                    Vec::new(),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the pushed layer to be readable, got {read:?}"
            );
            assert_eq!(
                read.body.as_slice(),
                *layer,
                "case {case}: expected the layer to read back byte for byte, got {} bytes",
                read.body.len()
            );
            assert_eq!(
                read.header("docker-content-digest"),
                Some(layer_digest.as_str()),
                "case {case}: expected the blob's digest on the read, got {read:?}"
            );

            // A `HEAD` carries the metadata of that read and no body.
            let head = emulator
                .registry(
                    "HEAD",
                    &format!("{}/v2/{}/blobs/{layer_digest}", login.base, row.repository),
                    &login,
                    None,
                    Vec::new(),
                )
                .await;
            assert_eq!(
                (head.status, head.body.len(), head.header("content-length")),
                (200, 0, Some(layer.len().to_string().as_str())),
                "case {case}: expected a bodied-metadata HEAD of the layer, got {head:?}"
            );
        }

        // The manifest reads back under its tag and under its digest alike, with
        // the media type it was pushed as.
        for (which, reference) in [("the tag", row.tag.to_string()), ("the digest", digest.clone())]
        {
            let read = emulator
                .registry(
                    "GET",
                    &format!(
                        "{}/v2/{}/manifests/{reference}",
                        login.base, row.repository
                    ),
                    &login,
                    None,
                    Vec::new(),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the manifest to be readable by {which}, got {read:?}"
            );
            assert_eq!(
                read.body, manifest,
                "case {case}: expected the manifest read by {which} to be the pushed bytes, got \
                 {}",
                read.text()
            );
            assert_eq!(
                read.header("content-type"),
                Some(MANIFEST_MEDIA_TYPE),
                "case {case}: expected the pushed media type on the read by {which}, got {read:?}"
            );
        }

        // And the tag is one of the repository's tags.
        let tags = emulator
            .registry(
                "GET",
                &format!("{}/v2/{}/tags/list", login.base, row.repository),
                &login,
                None,
                Vec::new(),
            )
            .await;
        let listed: Value = serde_json::from_slice(&tags.body).unwrap_or(Value::Null);
        assert_eq!(
            listed,
            json!({"name": row.repository, "tags": [row.tag]}),
            "case {case}: expected the pushed tag to be listed, got {listed:?}"
        );
    }
}

/// RULE: `RegisterContainerImage` registers an image the account's registry
/// holds — 200 with the service-assigned `:<service>.<label>.<version>` name,
/// the digest the request named and the instant of the call — the version being
/// 1 for the first image of a label and one more for each after it; and the
/// registered image round-trips through `GetContainerImages` in registration
/// order.
#[tokio::test]
async fn a_pushed_image_registers_and_reads_back() {
    for row in push_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_service(case, row.region, row.service).await;
        let login = emulator.login(case, row.region).await;
        let (_, digest) = emulator
            .push_image(
                case,
                &login,
                row.repository,
                row.tag,
                row.style,
                &row.layers,
            )
            .await;

        // The version each label is at, counted here so the expectation comes
        // from the documented rule rather than from the answer.
        let mut versions: BTreeMap<&str, i64> = BTreeMap::new();
        let mut registered: Vec<String> = Vec::new();
        for label in LABEL_ROWS {
            let case = format!("{case}, registering {label:?}");
            let before = unix_now();
            let (status, body) = emulator
                .register(row.region, row.service, label, &digest)
                .await;
            let after = unix_now();
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );

            let version = versions.entry(label).or_insert(0);
            *version += 1;
            let want_name = format!(":{}.{label}.{}", row.service, version);
            let image = &body["containerImage"];
            assert_eq!(
                image["image"].as_str(),
                Some(want_name.as_str()),
                "case {case}: expected the service-assigned name, got {image:?}"
            );
            assert_eq!(
                image["digest"].as_str(),
                Some(digest.as_str()),
                "case {case}: expected the digest the request named, got {image:?}"
            );
            let created_at = image["createdAt"].as_f64().unwrap_or_else(|| {
                panic!("case {case}: expected a numeric createdAt, got {image:?}")
            });
            assert!(
                (before - CLOCK_SLACK_SECONDS..=after + CLOCK_SLACK_SECONDS).contains(&created_at),
                "case {case}: expected createdAt at the call's instant ({before}..={after}), got \
                 {created_at}"
            );
            registered.push(want_name);

            // The registry reports exactly what has been registered so far, in
            // registration order.
            let reported = emulator.image_names(&case, row.region, row.service).await;
            assert_eq!(
                reported, registered,
                "case {case}: expected the registry to report {registered:?}, got {reported:?}"
            );
        }

        // Deleting one registered name removes exactly that one.
        let target = registered
            .first()
            .cloned()
            .expect("the table registers at least one image");
        let (status, body) = emulator
            .call(
                row.region,
                "DeleteContainerImage",
                json!({"serviceName": row.service, "image": target}),
            )
            .await;
        assert_eq!(
            status, 204,
            "case {case}: expected HTTP 204 for a held image, got {status} {body:?}"
        );
        let want: Vec<String> = registered
            .iter()
            .filter(|name| **name != target)
            .cloned()
            .collect();
        let got = emulator.image_names(case, row.region, row.service).await;
        assert_eq!(
            got, want,
            "case {case}: expected {want:?} after the delete, got {got:?}"
        );
    }
}

/// RULE: a digest the account's registry in the request's region does not hold
/// names nothing to register — whatever form it takes, and including a digest
/// pushed to ANOTHER region's registry — so the call is refused with the
/// service's `DoesNotExist` envelope and registers nothing.
#[tokio::test]
async fn registering_a_digest_the_registry_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let elsewhere = "eu-central-1";
    let service = "refusals";
    emulator.create_service("the service", region, service).await;
    emulator
        .create_service("the service elsewhere", elsewhere, service)
        .await;

    // An image pushed to ANOTHER region's registry: its digest is held by that
    // registry and by no other.
    let other_login = emulator.login("a log in elsewhere", elsewhere).await;
    let (_, elsewhere_digest) = emulator
        .push_image(
            "an image pushed elsewhere",
            &other_login,
            "elsewhere",
            "latest",
            PushStyle::Session,
            &[b"content pushed in another region"],
        )
        .await;

    let mut rows: Vec<(String, String)> = UNPUSHED_DIGESTS
        .iter()
        .map(|(case, digest)| ((*case).to_string(), (*digest).to_string()))
        .collect();
    rows.push((
        "a digest pushed to another region's registry".to_string(),
        elsewhere_digest.clone(),
    ));

    for (case, digest) in &rows {
        let (status, refused) = emulator.register(region, service, "web", digest).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case}: expected the NotFoundException wire code, got {refused:?}"
        );
        assert_eq!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(format!("Image with digest \"{digest}\" not found.").as_str()),
            "case {case}: expected the message to name the digest the request carried, got \
             {refused:?}"
        );
        assert!(
            emulator.images(case, region, service).await.is_empty(),
            "case {case}: a refused registration must register nothing, got {:?}",
            emulator.images(case, region, service).await
        );
    }

    // The same digest registers in the region whose registry holds it, which is
    // what makes the refusals above about the registry and not about the digest.
    let (status, body) = emulator
        .register(elsewhere, service, "web", &elsewhere_digest)
        .await;
    assert_eq!(
        status, 200,
        "case the digest in its own region: expected HTTP 200, got {status} {body:?}"
    );
}

/// RULE: the registry stores content under the digest of its own bytes, so a
/// digest a client claims that is not the digest of what it sent is refused and
/// nothing is stored.
#[tokio::test]
async fn a_claimed_digest_that_is_not_the_content_is_refused() {
    let emulator = Emulator::new();
    let login = emulator.login("the log in", "us-east-1").await;
    let content = b"the bytes actually sent".to_vec();
    let truthful = sha256_digest(&content);
    let claimed = sha256_digest(b"other bytes entirely");

    for (case, style) in [
        ("a whole-blob push", PushStyle::Monolithic),
        ("a push through a session", PushStyle::Session),
    ] {
        let target = match style {
            PushStyle::Monolithic => format!("{}/v2/app/blobs/uploads/?digest={claimed}", login.base),
            _ => {
                let started = emulator
                    .registry(
                        "POST",
                        &format!("{}/v2/app/blobs/uploads/", login.base),
                        &login,
                        None,
                        Vec::new(),
                    )
                    .await;
                let session = started
                    .header("location")
                    .unwrap_or_else(|| panic!("case {case}: expected a session, got {started:?}"))
                    .to_string();
                format!("{session}?digest={claimed}")
            }
        };
        let answer = emulator
            .registry(
                if style == PushStyle::Monolithic {
                    "POST"
                } else {
                    "PUT"
                },
                &target,
                &login,
                Some(BLOB_MEDIA_TYPE),
                content.clone(),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a digest that is not the content's, got {answer:?}"
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some("DIGEST_INVALID"),
            "case {case}: expected the registry's DIGEST_INVALID, got {}",
            answer.text()
        );

        // Neither digest names anything in the registry afterwards.
        for (which, digest) in [("the claimed digest", &claimed), ("the true digest", &truthful)] {
            let read = emulator
                .registry(
                    "GET",
                    &format!("{}/v2/app/blobs/{digest}", login.base),
                    &login,
                    None,
                    Vec::new(),
                )
                .await;
            assert_eq!(
                read.status, 404,
                "case {case}: expected {which} to name nothing after the refusal, got {read:?}"
            );
            assert_eq!(
                read.error_code().as_deref(),
                Some("BLOB_UNKNOWN"),
                "case {case}: expected BLOB_UNKNOWN for {which}, got {}",
                read.text()
            );
        }
    }
}

/// RULE: a manifest is accepted only once every blob it references is in the
/// registry, and only when it is a manifest at all — so an image whose layer was
/// never pushed, a body that is not a manifest, and a manifest put under a
/// digest that is not its own are each refused, and nothing is then registrable.
#[tokio::test]
async fn a_manifest_the_registry_cannot_stand_behind_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let service = "manifests";
    emulator.create_service("the service", region, service).await;
    let login = emulator.login("the log in", region).await;
    let base = &login.base;

    // One blob IS pushed, so a refusal below is about what the manifest adds and
    // not about an empty registry.
    let present = emulator
        .push_blob(
            "the pushed configuration",
            &login,
            "app",
            PushStyle::Session,
            b"a configuration that was pushed",
        )
        .await;
    let absent = sha256_digest(b"a layer nothing ever pushed");

    let referencing_absent = json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "config": {"mediaType": CONFIG_MEDIA_TYPE, "digest": present, "size": 31},
        "layers": [{"mediaType": LAYER_MEDIA_TYPE, "digest": absent, "size": 27}],
    })
    .to_string()
    .into_bytes();
    let well_formed = json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "config": {"mediaType": CONFIG_MEDIA_TYPE, "digest": present, "size": 31},
        "layers": [],
    })
    .to_string()
    .into_bytes();

    let rows: Vec<(&str, String, Vec<u8>, u16, &str)> = vec![
        (
            "a manifest referencing a layer nothing pushed",
            format!("{base}/v2/app/manifests/latest"),
            referencing_absent.clone(),
            404,
            "MANIFEST_BLOB_UNKNOWN",
        ),
        (
            "a body that is not JSON at all",
            format!("{base}/v2/app/manifests/latest"),
            b"not a manifest".to_vec(),
            400,
            "MANIFEST_INVALID",
        ),
        (
            "a JSON document referencing nothing",
            format!("{base}/v2/app/manifests/latest"),
            json!({"schemaVersion": 2, "mediaType": MANIFEST_MEDIA_TYPE})
                .to_string()
                .into_bytes(),
            400,
            "MANIFEST_INVALID",
        ),
        (
            "a manifest put under a digest that is not its own",
            format!("{base}/v2/app/manifests/{absent}"),
            well_formed.clone(),
            400,
            "DIGEST_INVALID",
        ),
        (
            "a reference that is neither a digest nor a tag",
            format!("{base}/v2/app/manifests/{}", "-leading-hyphen"),
            well_formed.clone(),
            400,
            "MANIFEST_INVALID",
        ),
    ];

    for (case, target, body, want_status, want_code) in rows {
        let answer = emulator
            .registry("PUT", &target, &login, Some(MANIFEST_MEDIA_TYPE), body.clone())
            .await;
        assert_eq!(
            answer.status, want_status,
            "case {case}: expected HTTP {want_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some(want_code),
            "case {case}: expected the registry's {want_code}, got {}",
            answer.text()
        );
        // Nothing was stored, so the digest of those bytes registers nothing.
        let digest = sha256_digest(&body);
        let (status, refused) = emulator.register(region, service, "web", &digest).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refused manifest to register nothing, got {status} \
             {refused:?}"
        );
    }

    // The same manifest put under its own digest IS stored and registers: the
    // refusals above are about what each row got wrong.
    let answer = emulator
        .registry(
            "PUT",
            &format!("{base}/v2/app/manifests/{}", sha256_digest(&well_formed)),
            &login,
            Some(MANIFEST_MEDIA_TYPE),
            well_formed.clone(),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case a manifest put under its own digest: expected HTTP 201, got {answer:?}"
    );
    let (status, body) = emulator
        .register(region, service, "web", &sha256_digest(&well_formed))
        .await;
    assert_eq!(
        status, 200,
        "case a manifest put under its own digest: expected it to register, got {status} {body:?}"
    );
}

/// RULE: an upload session is the registry's to hand out and to forget — a
/// session it never minted is unknown, and a cancelled one is gone, so neither
/// can be finished.
#[tokio::test]
async fn an_upload_session_the_registry_does_not_hold_is_unknown() {
    let emulator = Emulator::new();
    let login = emulator.login("the log in", "us-east-1").await;
    let base = &login.base;
    let content = b"content for a session".to_vec();
    let digest = sha256_digest(&content);

    // A session id nothing minted.
    for (case, method, target, body) in [
        (
            "a PATCH to a session nothing minted",
            "PATCH",
            format!("{base}/v2/app/blobs/uploads/0000-not-a-session"),
            content.clone(),
        ),
        (
            "a PUT finishing a session nothing minted",
            "PUT",
            format!("{base}/v2/app/blobs/uploads/0000-not-a-session?digest={digest}"),
            content.clone(),
        ),
        (
            "a GET of a session nothing minted",
            "GET",
            format!("{base}/v2/app/blobs/uploads/0000-not-a-session"),
            Vec::new(),
        ),
    ] {
        let answer = emulator
            .registry(method, &target, &login, Some(BLOB_MEDIA_TYPE), body)
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some("BLOB_UPLOAD_UNKNOWN"),
            "case {case}: expected BLOB_UPLOAD_UNKNOWN, got {}",
            answer.text()
        );
    }

    // A session the registry DID mint reports how far it has got, and is gone
    // once it is cancelled.
    let started = emulator
        .registry(
            "POST",
            &format!("{base}/v2/app/blobs/uploads/"),
            &login,
            None,
            Vec::new(),
        )
        .await;
    let session = started
        .header("location")
        .expect("a minted session has an address")
        .to_string();
    let appended = emulator
        .registry("PATCH", &session, &login, Some(BLOB_MEDIA_TYPE), content.clone())
        .await;
    assert_eq!(
        (appended.status, appended.header("range")),
        (202, Some(format!("0-{}", content.len() - 1).as_str())),
        "case a session that took bytes: expected the range it holds, got {appended:?}"
    );
    let status = emulator
        .registry("GET", &session, &login, None, Vec::new())
        .await;
    assert_eq!(
        (status.status, status.header("range")),
        (202, Some(format!("0-{}", content.len() - 1).as_str())),
        "case reading a session's progress: expected the same range, got {status:?}"
    );
    let cancelled = emulator
        .registry("DELETE", &session, &login, None, Vec::new())
        .await;
    assert_eq!(
        cancelled.status, 204,
        "case cancelling a session: expected HTTP 204, got {cancelled:?}"
    );
    let finished = emulator
        .registry(
            "PUT",
            &format!("{session}?digest={digest}"),
            &login,
            Some(BLOB_MEDIA_TYPE),
            Vec::new(),
        )
        .await;
    assert_eq!(
        finished.error_code().as_deref(),
        Some("BLOB_UPLOAD_UNKNOWN"),
        "case finishing a cancelled session: expected BLOB_UPLOAD_UNKNOWN, got {}",
        finished.text()
    );
}

/// RULE: the registry serves its own API and nothing else under the announced
/// address — a path outside `/v2`, a method that endpoint does not serve, and a
/// repository name the API's grammar does not admit are each refused in the
/// registry's own envelope.
#[tokio::test]
async fn a_request_outside_the_registry_api_is_refused() {
    let emulator = Emulator::new();
    let login = emulator.login("the log in", "us-east-1").await;
    let base = &login.base;

    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "the announced address itself, which is not an API path",
            "GET",
            base.clone(),
            "UNSUPPORTED",
        ),
        (
            "a path that only starts like the API's",
            "GET",
            format!("{base}/v2extra/"),
            "UNSUPPORTED",
        ),
        (
            "a method the version check does not serve",
            "DELETE",
            format!("{base}/v2/"),
            "UNSUPPORTED",
        ),
        (
            "a method a manifest does not serve",
            "POST",
            format!("{base}/v2/app/manifests/latest"),
            "UNSUPPORTED",
        ),
        (
            "a repository name the API's grammar does not admit",
            "GET",
            format!("{base}/v2/App/manifests/latest"),
            "NAME_INVALID",
        ),
        (
            "a tag list of a repository nothing was pushed to",
            "GET",
            format!("{base}/v2/app/tags/list"),
            "NAME_UNKNOWN",
        ),
        (
            "a manifest reference nothing pushed",
            "GET",
            format!("{base}/v2/app/manifests/latest"),
            "MANIFEST_UNKNOWN",
        ),
    ];

    for (case, method, target, want_code) in rows {
        let answer = emulator
            .registry(method, &target, &login, None, Vec::new())
            .await;
        assert_eq!(
            answer.error_code().as_deref(),
            Some(want_code),
            "case {case}: expected the registry's {want_code}, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// RULE: a manifest is held in a media type the registry stores — so a push
/// declaring one of them is accepted (through the `Content-Type` header or the
/// manifest's own `mediaType` member, in the canonical spelling whatever case
/// or parameters the push used) and a push declaring anything else is refused,
/// which is also what keeps a value a client sent from ever being answered as
/// a header.
#[tokio::test]
async fn a_manifest_is_held_only_in_a_media_type_the_registry_stores() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let login = emulator.login("the log in", region).await;
    let base = &login.base;
    let config = emulator
        .push_blob(
            "the configuration",
            &login,
            "app",
            PushStyle::Monolithic,
            b"a configuration blob",
        )
        .await;
    // One manifest per row, with the row's own case in an annotation so its
    // bytes — and therefore its digest — are its own: the media type travels in
    // the row's declaration.
    let manifest = |case: &str, declared: &str| {
        json!({
            "schemaVersion": 2,
            "mediaType": declared,
            "config": {"mediaType": CONFIG_MEDIA_TYPE, "digest": config, "size": 20},
            "layers": [],
            "annotations": {"case": case},
        })
        .to_string()
        .into_bytes()
    };

    // (case, the Content-Type header, the body's mediaType, the media type the
    // read must answer with — None when the push must be refused)
    let rows: Vec<(&str, Option<&str>, &str, Option<&str>)> = vec![
        (
            "the OCI image manifest media type in the header",
            Some(MANIFEST_MEDIA_TYPE),
            MANIFEST_MEDIA_TYPE,
            Some(MANIFEST_MEDIA_TYPE),
        ),
        (
            "an index media type declared by the manifest alone",
            None,
            "application/vnd.oci.image.index.v1+json",
            Some("application/vnd.oci.image.index.v1+json"),
        ),
        (
            "the Docker schema 2 media type, with a parameter and odd case",
            Some("Application/vnd.docker.distribution.manifest.v2+JSON; charset=utf-8"),
            "application/vnd.docker.distribution.manifest.v2+json",
            Some("application/vnd.docker.distribution.manifest.v2+json"),
        ),
        (
            "the header's media type, where the manifest declares another",
            Some("application/vnd.docker.distribution.manifest.list.v2+json"),
            MANIFEST_MEDIA_TYPE,
            Some("application/vnd.docker.distribution.manifest.list.v2+json"),
        ),
        (
            "a media type no registry manifest has",
            Some("text/plain"),
            "text/plain",
            None,
        ),
        (
            // A JSON string may carry a control character, which is not a value
            // any HTTP header can take.
            "a manifest declaring a media type carrying a control character",
            None,
            "application/vnd.oci.image.manifest.v1+json\r\nx-injected: yes",
            None,
        ),
        (
            "a manifest declaring an empty media type",
            None,
            "",
            None,
        ),
    ];

    for (case, header, declared, want) in rows {
        let body = manifest(case, declared);
        let digest = sha256_digest(&body);
        let answer = emulator
            .registry(
                "PUT",
                &format!("{base}/v2/app/manifests/{digest}"),
                &login,
                header,
                body.clone(),
            )
            .await;
        match want {
            Some(want_media_type) => {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected HTTP 201, got {answer:?}"
                );
                let read = emulator
                    .registry(
                        "GET",
                        &format!("{base}/v2/app/manifests/{digest}"),
                        &login,
                        None,
                        Vec::new(),
                    )
                    .await;
                assert_eq!(
                    (read.status, read.header("content-type")),
                    (200, Some(want_media_type)),
                    "case {case}: expected the stored media type on the read, got {read:?}"
                );
                assert_eq!(
                    read.body, body,
                    "case {case}: expected the pushed bytes on the read, got {}",
                    read.text()
                );
            }
            None => {
                assert_eq!(
                    (answer.status, answer.error_code().as_deref()),
                    (400, Some("MANIFEST_INVALID")),
                    "case {case}: expected the registry's MANIFEST_INVALID, got {answer:?}"
                );
                // Nothing was stored, so a read of that reference is the clean
                // "unknown" answer rather than anything the push left behind.
                let read = emulator
                    .registry(
                        "GET",
                        &format!("{base}/v2/app/manifests/{digest}"),
                        &login,
                        None,
                        Vec::new(),
                    )
                    .await;
                assert_eq!(
                    (read.status, read.error_code().as_deref()),
                    (404, Some("MANIFEST_UNKNOWN")),
                    "case {case}: expected the refused manifest to be absent, got {read:?}"
                );
            }
        }
    }
}

/// RULE: an upload session belongs to the repository it was started against and
/// takes its chunks in order — a session addressed under another repository is
/// not that session, a chunk that does not carry on from where the session got
/// to is refused, and a claimed digest that is not the content's leaves the
/// session exactly as it was, so the client can still finish it correctly.
#[tokio::test]
async fn an_upload_session_is_scoped_ordered_and_survives_a_wrong_digest() {
    let emulator = Emulator::new();
    let login = emulator.login("the log in", "us-east-1").await;
    let base = &login.base;
    let first = b"first half ".to_vec();
    let second = b"second half".to_vec();
    let whole: Vec<u8> = first.iter().chain(second.iter()).copied().collect();

    let started = emulator
        .registry(
            "POST",
            &format!("{base}/v2/app/blobs/uploads/"),
            &login,
            None,
            Vec::new(),
        )
        .await;
    let session = started
        .header("location")
        .expect("a started upload has an address")
        .to_string();

    // A chunk that does not start where the session stands.
    let skipped = emulator
        .registry_with_range(
            "PATCH",
            &session,
            &login,
            Some("5-15"),
            first.clone(),
        )
        .await;
    assert_eq!(
        (skipped.status, skipped.error_code().as_deref()),
        (416, Some("BLOB_UPLOAD_INVALID")),
        "case a chunk starting past the session: expected it to be refused, got {skipped:?}"
    );

    // The chunk that does carry on from it.
    let appended = emulator
        .registry_with_range(
            "PATCH",
            &session,
            &login,
            Some(&format!("0-{}", first.len() - 1)),
            first.clone(),
        )
        .await;
    assert_eq!(
        (appended.status, appended.header("range")),
        (202, Some(format!("0-{}", first.len() - 1).as_str())),
        "case the chunk at the session's own offset: expected it to be taken, got {appended:?}"
    );

    // The same session id under another repository's path names no session.
    let elsewhere = session.replace("/v2/app/", "/v2/other/");
    for (case, method, body) in [
        ("a GET under another repository", "GET", Vec::new()),
        ("a PATCH under another repository", "PATCH", second.clone()),
        ("a DELETE under another repository", "DELETE", Vec::new()),
    ] {
        let answer = emulator
            .registry(method, &elsewhere, &login, Some(BLOB_MEDIA_TYPE), body)
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (404, Some("BLOB_UPLOAD_UNKNOWN")),
            "case {case}: expected it to name no session, got {answer:?}"
        );
    }

    // A finishing request claiming the wrong digest stores nothing and leaves
    // the session where it was.
    let wrong = sha256_digest(b"some other content entirely");
    let refused = emulator
        .registry(
            "PUT",
            &format!("{session}?digest={wrong}"),
            &login,
            Some(BLOB_MEDIA_TYPE),
            second.clone(),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("DIGEST_INVALID")),
        "case finishing with the wrong digest: expected DIGEST_INVALID, got {refused:?}"
    );
    let status = emulator
        .registry("GET", &session, &login, None, Vec::new())
        .await;
    assert_eq!(
        (status.status, status.header("range")),
        (202, Some(format!("0-{}", first.len() - 1).as_str())),
        "case the session after a wrong digest: expected it to hold what it held, got {status:?}"
    );

    // And the same session finishes with the right one.
    let digest = sha256_digest(&whole);
    let stored = emulator
        .registry(
            "PUT",
            &format!("{session}?digest={digest}"),
            &login,
            Some(BLOB_MEDIA_TYPE),
            second.clone(),
        )
        .await;
    assert_eq!(
        (stored.status, stored.header("docker-content-digest")),
        (201, Some(digest.as_str())),
        "case finishing with the content's own digest: expected the blob to be stored, got \
         {stored:?}"
    );
    let read = emulator
        .registry(
            "GET",
            &format!("{base}/v2/app/blobs/{digest}"),
            &login,
            None,
            Vec::new(),
        )
        .await;
    assert_eq!(
        read.body, whole,
        "case reading the finished blob: expected every byte of both chunks, got {}",
        read.text()
    );
}

/// RULE (the table's own source): every label the version rows register is
/// inside the modelled `ContainerLabel` length of 1..53, and the longest is
/// exactly that maximum.
#[tokio::test]
async fn the_table_labels_match_the_modelled_bounds() {
    /// The modelled `ContainerLabel` maximum length.
    const MAX_LABEL_LEN: usize = 53;

    let longest = LABEL_ROWS
        .iter()
        .map(|label| label.len())
        .max()
        .expect("the table has labels");
    assert_eq!(
        longest, MAX_LABEL_LEN,
        "case the longest label: expected the modelled maximum of {MAX_LABEL_LEN}, got {longest}"
    );
    for label in LABEL_ROWS {
        assert!(
            (1..=MAX_LABEL_LEN).contains(&label.len()),
            "case {label:?}: expected a label inside the modelled 1..{MAX_LABEL_LEN}, got {}",
            label.len()
        );
    }
}
