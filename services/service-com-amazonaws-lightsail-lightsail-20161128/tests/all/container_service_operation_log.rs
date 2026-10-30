// SPDX-License-Identifier: CC0-1.0

//! What the MUTATING container-service calls leave behind in the account's
//! append-only operation log, read back by resource and account-wide, driven over
//! the wire.
//!
//! Every control-plane case here goes through the same front door a client uses:
//! `POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handlers and the awsJson error envelope are all
//! exercised rather than bypassed. The one case that needs an image in the
//! account's container image registry pushes it the way a registry client does,
//! because `RegisterContainerImage` registers an image the registry already
//! holds.
//!
//! The rule under test: a mutating call appends exactly ONE entry to the
//! account's operation log, naming the modelled `ResourceType` of the resource it
//! acted on and that resource's own name, and `GetOperations` and
//! `GetOperationsForResource` report it. For the container-service calls that is
//! the modelled `ContainerService` resource type and the service's name — for the
//! image-scoped calls too, because `Operation$resourceType` targets an enum with
//! no member for a container image. The entry carries the modelled
//! `OperationType` member named for the call that wrote it, and it is `Started`
//! while the clock still owes the call's work its terminal status and `Succeeded`
//! at once when the work is finished as the call returns. The log is append-only,
//! so entries are reported oldest first, an entry outlives the resource it names,
//! and a REFUSED call appends nothing.
//!
//! The sibling `operations_log` and `operations_for_resource` suites pin the two
//! readers themselves; this one pins what the container-service writers put in
//! front of them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    OPERATIONS_FOR_RESOURCE_PAGE_SIZE, OPERATIONS_PAGE_SIZE, STATE_TRANSITION_MILLIS,
};

// The wire error code and the minted-id check this test target already exposes.
use crate::instance_lifecycle::{error_code, is_uuid_shaped};

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint the cases reach the emulator on: the loopback address and port
/// the emulator's own listener binds. The registry address a log in announces is
/// built from it, so a push is addressed at the same endpoint.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The modelled `ResourceType` member every entry these calls write reports
/// (`crate::state::RESOURCE_TYPE_CONTAINER_SERVICE`).
const CONTAINER_SERVICE: &str = "ContainerService";

/// The two modelled `OperationStatus` members an entry of this log can report:
/// `Started` while the clock still owes the work its terminal status, `Succeeded`
/// once it has settled (`crate::state::OPERATION_STATUS_STARTED` /
/// `OPERATION_STATUS_SUCCEEDED`).
const STARTED: &str = "Started";
/// See [`STARTED`].
const SUCCEEDED: &str = "Succeeded";

/// The media type an OCI image manifest is pushed as.
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
/// The media type an image configuration blob is referenced as.
const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
/// The media type an image layer is referenced as.
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
/// The media type an opaque blob is pushed as.
const BLOB_MEDIA_TYPE: &str = "application/octet-stream";

/// How long a case waits for the clock to settle what a call left outstanding.
const SETTLE_DEADLINE: Duration = Duration::from_secs(5);

/// How far a reported instant may sit from the one the rule computes and still be
/// it: the two are the same millisecond rendered as epoch seconds, so the slack
/// covers only the rounding of that rendering.
const INSTANT_SLACK_SECONDS: f64 = 1e-3;

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// One set of registry log in credentials and the address they log in at.
struct Login {
    username: String,
    password: String,
    /// The request target of the announced address: its path, with no host.
    base: String,
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
        // request authenticates with HTTP Basic credentials.
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

    /// Call `operation` in `region` and require `want`, answering the body.
    async fn ok(&self, case: &str, region: &str, operation: &str, body: Value, want: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want,
            "case {case}: expected {operation} in {region} to answer HTTP {want}, got {status} \
             {parsed:?}"
        );
        parsed
    }

    /// Create one set of registry log in credentials in `region`.
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
    /// a registry client does, answering the status and the response headers that
    /// matter to a push.
    async fn registry(
        &self,
        method: &str,
        target: &str,
        login: &Login,
        content_type: &str,
        body: Vec<u8>,
    ) -> (u16, Option<String>) {
        let credentials = ::aws_smithy_types::base64::encode(format!(
            "{}:{}",
            login.username, login.password
        ));
        let request = ::http::Request::builder()
            .method(method)
            .uri(target)
            .header("host", ENDPOINT_HOST)
            .header("authorization", format!("Basic {credentials}"))
            .header("content-type", content_type)
            .body(bytes::Bytes::from(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let digest = response
            .headers()
            .get("docker-content-digest")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
        (response.status().as_u16(), digest)
    }

    /// Push one whole image — a configuration blob, one layer, then the manifest
    /// that references them — to `repository`, and answer the digest the registry
    /// reported for the manifest, which IS the digest of the image.
    ///
    /// `marker` makes each pushed image's bytes its own, so two pushes of one
    /// suite are two distinct images.
    async fn push_image(&self, case: &str, login: &Login, repository: &str, marker: &str) -> String {
        let base = &login.base;
        let mut descriptors = Vec::new();
        for (media_type, content) in [
            (
                CONFIG_MEDIA_TYPE,
                format!("{{\"architecture\":\"amd64\",\"os\":\"linux\",\"id\":{marker:?}}}")
                    .into_bytes(),
            ),
            (LAYER_MEDIA_TYPE, format!("layer of {marker}").into_bytes()),
        ] {
            let digest = sha256_digest(&content);
            let (status, reported) = self
                .registry(
                    "POST",
                    &format!("{base}/v2/{repository}/blobs/uploads/?digest={digest}"),
                    login,
                    BLOB_MEDIA_TYPE,
                    content.clone(),
                )
                .await;
            assert_eq!(
                (status, reported.as_deref()),
                (201, Some(digest.as_str())),
                "case {case}: expected the registry to store the blob under the digest of its \
                 own bytes, got HTTP {status} and {reported:?}"
            );
            descriptors.push(json!({
                "mediaType": media_type,
                "digest": digest,
                "size": content.len(),
            }));
        }
        let layers = descriptors.split_off(1);
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": MANIFEST_MEDIA_TYPE,
            "config": descriptors[0],
            "layers": layers,
        })
        .to_string()
        .into_bytes();
        let digest = sha256_digest(&manifest);
        let (status, reported) = self
            .registry(
                "PUT",
                &format!("{base}/v2/{repository}/manifests/{marker}"),
                login,
                MANIFEST_MEDIA_TYPE,
                manifest,
            )
            .await;
        assert_eq!(
            (status, reported.as_deref()),
            (201, Some(digest.as_str())),
            "case {case}: expected the registry to store the manifest under the digest of its \
             own bytes, got HTTP {status} and {reported:?}"
        );
        digest
    }

    /// Every entry `GetOperationsForResource` reports for `name`, oldest first,
    /// walking every page.
    async fn log_for_resource(&self, case: &str, name: &str) -> Vec<Value> {
        self.walk(
            case,
            "GetOperationsForResource",
            json!({"resourceName": name}),
            OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
        )
        .await
    }

    /// Every entry `GetOperations` reports for the whole account, oldest first,
    /// walking every page.
    async fn account_log(&self, case: &str) -> Vec<Value> {
        self.walk(case, "GetOperations", json!({}), OPERATIONS_PAGE_SIZE)
            .await
    }

    /// Walk every page of a paged read of the log, asserting the published page
    /// size is respected and the walk terminates.
    async fn walk(
        &self,
        case: &str,
        operation: &str,
        base: Value,
        page_size: usize,
    ) -> Vec<Value> {
        let mut entries: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = base.clone();
            if let Some(token) = &token {
                body["pageToken"] = json!(token);
            }
            // The log is not regional: an entry is reported by id and by name,
            // so the region a read is made in selects nothing.
            let answered = self.ok(case, "us-east-1", operation, body, 200).await;
            let page = answered["operations"].as_array().cloned().unwrap_or_else(|| {
                panic!("case {case}: expected an operations array from {operation}, got {answered:?}")
            });
            assert!(
                page.len() <= page_size,
                "case {case}: expected at most {page_size} entries a page from {operation}, got {}",
                page.len()
            );
            entries.extend(page);
            match answered["nextPageToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                entries.len() < 1_000,
                "case {case}: the walk over {operation} did not terminate after {} entries",
                entries.len()
            );
        }
        entries
    }
}

/// The digest of `content` as the registry protocol spells one, computed here
/// with the `sha2` crate so the test's expectation does not come from the code
/// under test.
fn sha256_digest(content: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// The `id` of every entry of `entries`, in the order the log reports them.
fn ids(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("every operation-log entry carries an id, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The `operationType` of every entry of `entries`, in log order.
fn operation_types(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["operationType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("every operation-log entry carries an operationType, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One mutating call a row makes.
///
/// A step says WHAT to call; whether the call leaves the clock any work is
/// derived from the state the row has driven the service into, so no row can
/// claim a change it did not ask for.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `CreateContainerService`, with or without a deployment named at create.
    Create { with_deployment: bool },
    /// `UpdateContainerService` asking for `scale` nodes — a change only when the
    /// service is not already at that scale.
    Rescale(i64),
    /// `UpdateContainerService` asking for the service to be disabled or enabled
    /// — a change only when it is not already so.
    SetDisabled(bool),
    /// `CreateContainerServiceDeployment`.
    Deploy,
    /// Push an image to the account's registry, then `RegisterContainerImage` it
    /// under `label`.
    RegisterImage(&'static str),
    /// `DeleteContainerImage` of the image this row registered last.
    DeleteImage,
    /// `DeleteContainerService`.
    Delete,
}

impl Step {
    /// The modelled `OperationType` member the entry this call writes carries
    /// (`smithy_query(query="shape",shape="OperationType")`, which declares one
    /// for each of these six calls).
    fn operation_type(self) -> &'static str {
        match self {
            Step::Create { .. } => "CreateContainerService",
            Step::Rescale(_) | Step::SetDisabled(_) => "UpdateContainerService",
            Step::Deploy => "CreateContainerServiceDeployment",
            Step::RegisterImage(_) => "RegisterContainerImage",
            Step::DeleteImage => "DeleteContainerImage",
            Step::Delete => "DeleteContainerService",
        }
    }
}

/// One case: a container service and the mutating calls made against it.
///
/// The rows differ along everything the rule generalises over: WHICH of the six
/// mutating calls are made and in what order, how many entries the service's
/// slice of the log ends up holding, whether each call leaves the clock work or
/// finishes as it returns, the Region the calls are made in, and the form of the
/// name the modelled `ContainerServiceName` pattern
/// `^[a-z0-9]{1,2}|[a-z0-9][a-z0-9-]+[a-z0-9]$` admits — one character, two, a
/// hyphenated name and an all-digit one.
struct Row {
    case: &'static str,
    region: &'static str,
    service: &'static str,
    steps: &'static [Step],
}

/// The table. Every named Region is one `GetRegions` lists
/// (`crate::state::REGION_SPECS`, reported by the `region_catalog` suite).
const ROWS: &[Row] = &[
    Row {
        case: "the whole lifecycle of a service created without a deployment",
        region: "us-east-1",
        service: "ops-log-svc",
        steps: &[
            Step::Create {
                with_deployment: false,
            },
            Step::Rescale(2),
            Step::Deploy,
            Step::RegisterImage("app"),
            Step::DeleteImage,
            Step::Delete,
        ],
    },
    Row {
        case: "a two-character name created WITH a deployment, disabled and enabled again",
        region: "us-west-2",
        service: "ab",
        steps: &[
            Step::Create {
                with_deployment: true,
            },
            Step::SetDisabled(true),
            // Asking again for the state the service is already in changes
            // nothing, so this call's work is finished as it returns.
            Step::SetDisabled(true),
            Step::SetDisabled(false),
            Step::Delete,
        ],
    },
    Row {
        case: "a hyphenated name, an update that changes nothing and two images under one label",
        region: "eu-west-1",
        service: "ops-log-service-2026",
        steps: &[
            Step::Create {
                with_deployment: false,
            },
            // A service is created with one node, so this asks for the scale it
            // already carries.
            Step::Rescale(1),
            Step::RegisterImage("web"),
            Step::RegisterImage("web"),
            Step::DeleteImage,
            Step::Delete,
        ],
    },
    Row {
        case: "a two-character name starting with a digit, touched once",
        region: "ap-northeast-1",
        service: "9z",
        steps: &[Step::Create {
            with_deployment: false,
        }],
    },
    Row {
        case: "an all-digit name scaled to the top of the modelled range",
        region: "ca-central-1",
        service: "2026",
        steps: &[
            Step::Create {
                with_deployment: false,
            },
            Step::Deploy,
            Step::Rescale(20),
            Step::Delete,
        ],
    },
];

/// The deployment one `Step::Deploy` or a create's own `deployment` describes:
/// one container serving HTTP on port 80.
fn deployment() -> Value {
    json!({"containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}}})
}

/// What a row knows about the service it is driving, so the expectation for each
/// step is derived rather than restated.
struct Driving {
    /// The nodes the service is configured with; a create makes it one.
    scale: i64,
    /// Whether the service is disabled.
    disabled: bool,
    /// The name of the image the row registered last.
    last_image: Option<String>,
    /// How many images the row has pushed, so each push is its own image.
    pushes: usize,
}

/// What one step is expected to have appended.
struct Expected {
    /// The modelled `OperationType` the entry carries.
    operation_type: &'static str,
    /// How many state-transition spans the clock owed the call's work when the
    /// call returned — `0` for work that was finished as it returned, and one per
    /// phase the service still had to walk through otherwise.
    spans: u32,
}

/// Make `step` against `row`'s service and answer what it must have appended.
async fn drive(emulator: &Emulator, row: &Row, state: &mut Driving, step: Step) -> Expected {
    let case = row.case;
    let region = row.region;
    let service = row.service;
    let spans = match step {
        Step::Create { with_deployment } => {
            let mut body = json!({"serviceName": service, "power": "nano", "scale": 1});
            if with_deployment {
                body["deployment"] = deployment();
            }
            emulator
                .ok(case, region, "CreateContainerService", body, 201)
                .await;
            state.scale = 1;
            state.disabled = false;
            // A service is still being created as the call returns, so the
            // create's work is owed the whole walk the new service has left: it
            // settles once the service reaches the status it is heading for —
            // ready with nothing to launch, and running once the deployment a
            // create named is active, which is one phase further on.
            if with_deployment {
                2
            } else {
                1
            }
        }
        Step::Rescale(scale) => {
            emulator
                .ok(
                    case,
                    region,
                    "UpdateContainerService",
                    json!({"serviceName": service, "scale": scale}),
                    200,
                )
                .await;
            let changed = scale != state.scale;
            state.scale = scale;
            u32::from(changed)
        }
        Step::SetDisabled(disabled) => {
            emulator
                .ok(
                    case,
                    region,
                    "UpdateContainerService",
                    json!({"serviceName": service, "isDisabled": disabled}),
                    200,
                )
                .await;
            let changed = disabled != state.disabled;
            state.disabled = disabled;
            u32::from(changed)
        }
        Step::Deploy => {
            let mut body = deployment();
            body["serviceName"] = json!(service);
            emulator
                .ok(
                    case,
                    region,
                    "CreateContainerServiceDeployment",
                    body,
                    201,
                )
                .await;
            1
        }
        Step::RegisterImage(label) => {
            state.pushes += 1;
            let marker = format!("{service}-{}", state.pushes);
            let login = emulator.login(case, region).await;
            let digest = emulator.push_image(case, &login, service, &marker).await;
            let answered = emulator
                .ok(
                    case,
                    region,
                    "RegisterContainerImage",
                    json!({"serviceName": service, "label": label, "digest": digest}),
                    200,
                )
                .await;
            let image = answered["containerImage"]["image"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected the registered image's name, got {answered:?}")
                })
                .to_string();
            state.last_image = Some(image);
            // The image is registered as the call returns.
            0
        }
        Step::DeleteImage => {
            let image = state
                .last_image
                .clone()
                .unwrap_or_else(|| panic!("case {case}: a row deletes an image it registered"));
            emulator
                .ok(
                    case,
                    region,
                    "DeleteContainerImage",
                    json!({"serviceName": service, "image": image}),
                    204,
                )
                .await;
            state.last_image = None;
            0
        }
        Step::Delete => {
            emulator
                .ok(
                    case,
                    region,
                    "DeleteContainerService",
                    json!({"serviceName": service}),
                    204,
                )
                .await;
            0
        }
    };
    Expected {
        operation_type: step.operation_type(),
        spans,
    }
}

/// The `createdAt` of `entry`, in the epoch seconds the wire carries it in.
fn created_at(case: &str, entry: &Value) -> f64 {
    entry["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a createdAt on {entry:?}"))
}

/// The `statusChangedAt` of `entry`, in epoch seconds.
fn status_changed_at(case: &str, entry: &Value) -> f64 {
    entry["statusChangedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a statusChangedAt on {entry:?}"))
}

/// Poll the log of `name` until every entry it holds is terminal.
///
/// The transition is owed by the CLOCK, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until_settled(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let entries = emulator.log_for_resource(case, name).await;
        let outstanding: Vec<&Value> = entries
            .iter()
            .filter(|entry| entry["isTerminal"].as_bool() != Some(true))
            .collect();
        if outstanding.is_empty() {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected every entry of {name}'s log to settle within \
             {SETTLE_DEADLINE:?}, still outstanding: {outstanding:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every mutating container-service call appends exactly ONE entry to the
/// account's operation log, naming the modelled `ContainerService` resource type,
/// the service's own name and the modelled `OperationType` member of the call
/// that wrote it — so `GetOperationsForResource` reports them oldest first, an
/// entry outlives the service it names, and the entry is `Succeeded` at once when
/// the call's work is finished as it returns and settles from `Started` when the
/// clock still owes it.
#[tokio::test]
async fn every_mutating_container_service_call_appends_its_log_entry() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();

        assert!(
            emulator.log_for_resource(case, row.service).await.is_empty(),
            "case {case}: a container service nothing has created has no log entries yet"
        );

        let mut state = Driving {
            scale: 0,
            disabled: false,
            last_image: None,
            pushes: 0,
        };
        let mut expected: Vec<Expected> = Vec::new();
        for step in row.steps {
            expected.push(drive(&emulator, row, &mut state, *step).await);
            // Exactly one entry per call, and a call whose work finished as it
            // returned is already terminal when the next caller reads the log.
            let entries = emulator.log_for_resource(case, row.service).await;
            assert_eq!(
                entries.len(),
                expected.len(),
                "case {case}: expected {step:?} to append exactly one entry, got {entries:?}"
            );
            let last = entries.last().expect("the log holds the entry just written");
            let appended = &expected[expected.len() - 1];
            if appended.spans == 0 {
                assert_eq!(
                    (last["status"].as_str(), last["isTerminal"].as_bool()),
                    (Some(SUCCEEDED), Some(true)),
                    "case {case}: {step:?} finishes its work as it returns, so its entry is \
                     terminal at once, got {last:?}"
                );
            }
            assert!(
                [STARTED, SUCCEEDED].contains(&last["status"].as_str().unwrap_or_default()),
                "case {case}: expected a modelled OperationStatus on {step:?}'s entry, got \
                 {last:?}"
            );
        }

        // The log read back names this service, in the order the calls were made
        // — including the entries of the calls that acted on an IMAGE of it, and
        // the delete's own entry, which outlives the record it names.
        let settled = wait_until_settled(&emulator, case, row.service).await;
        assert_eq!(
            operation_types(&settled),
            expected
                .iter()
                .map(|want| want.operation_type.to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected one entry per mutating call, in call order, got {settled:?}"
        );

        let entry_ids = ids(&settled);
        assert_eq!(
            entry_ids.iter().collect::<BTreeSet<&String>>().len(),
            entry_ids.len(),
            "case {case}: every entry is its own operation, got {entry_ids:?}"
        );
        let mut previous = 0.0_f64;
        for (entry, want) in settled.iter().zip(&expected) {
            assert_eq!(
                entry["resourceType"].as_str(),
                Some(CONTAINER_SERVICE),
                "case {case}: expected the {CONTAINER_SERVICE} resource type, got {entry:?}"
            );
            assert_eq!(
                entry["resourceName"].as_str(),
                Some(row.service),
                "case {case}: expected the entry to name the container service, got {entry:?}"
            );
            assert_eq!(
                entry["location"]["regionName"].as_str(),
                Some(row.region),
                "case {case}: expected the entry located where the service is, got {entry:?}"
            );
            assert!(
                entry["id"].as_str().is_some_and(is_uuid_shaped),
                "case {case}: expected a minted operation id, got {entry:?}"
            );
            assert_eq!(
                (entry["status"].as_str(), entry["isTerminal"].as_bool()),
                (Some(SUCCEEDED), Some(true)),
                "case {case}: expected every entry to have settled, got {entry:?}"
            );

            // The log is append-only, so it is reported oldest first.
            let created = created_at(case, entry);
            assert!(
                created > 0.0 && created >= previous,
                "case {case}: expected the log oldest first, got {created} after {previous} on \
                 {entry:?}"
            );
            previous = created;

            // A call that left the clock work was `Started` when it returned and
            // was settled one transition span later; one that finished as it
            // returned never changed status at all.
            let want_delay =
                f64::from(want.spans) * STATE_TRANSITION_MILLIS as f64 / 1000.0;
            let delay = status_changed_at(case, entry) - created;
            assert!(
                (delay - want_delay).abs() < INSTANT_SLACK_SECONDS,
                "case {case}: expected {} to settle {want_delay}s after it was recorded, got \
                 {delay}s on {entry:?}",
                want.operation_type
            );
        }
    }
}

/// RULE: the account-wide read of the log is the union of its per-resource reads
/// — so every entry the container-service calls appended is reported by
/// `GetOperations` as well, in the one append order, beside the entries of
/// another account resource's calls, and each is reported by its own id through
/// `GetOperation`.
#[tokio::test]
async fn the_account_log_reports_every_container_service_entry() {
    let case = "every row's calls on one account";
    let emulator = Emulator::new();

    // One entry of a resource that is NOT a container service, so the
    // account-wide read is shown to hold both kinds.
    emulator
        .ok(
            case,
            "us-east-1",
            "AllocateStaticIp",
            json!({"staticIpName": "ops-log-static-ip"}),
            200,
        )
        .await;
    let mut want: Vec<String> = ids(&emulator.log_for_resource(case, "ops-log-static-ip").await);
    assert_eq!(
        want.len(),
        1,
        "case {case}: expected the static IP's own entry, got {want:?}"
    );

    for row in ROWS {
        let mut state = Driving {
            scale: 0,
            disabled: false,
            last_image: None,
            pushes: 0,
        };
        for step in row.steps {
            drive(&emulator, row, &mut state, *step).await;
        }
        want.extend(ids(
            &emulator.log_for_resource(row.case, row.service).await,
        ));
    }

    let account = emulator.account_log(case).await;
    assert_eq!(
        ids(&account),
        want,
        "case {case}: expected the account log to be the union of the per-resource logs, in \
         append order, got {account:?}"
    );

    for entry in &account {
        let id = entry["id"].as_str().unwrap_or_default();
        let by_id = emulator
            .ok(case, "us-east-1", "GetOperation", json!({"operationId": id}), 200)
            .await["operation"]
            .clone();
        for member in [
            "id",
            "resourceName",
            "resourceType",
            "operationType",
            "createdAt",
            "status",
        ] {
            assert_eq!(
                by_id[member], entry[member],
                "case {case}: GetOperation and GetOperations must agree on {member}, got \
                 {by_id:?} and {entry:?}"
            );
        }
    }
}

/// RULE: a REFUSED mutating call appends nothing — the log it would have been
/// appended to is left exactly as it was found, whichever of the six calls was
/// refused and whatever it was refused for.
#[tokio::test]
async fn a_refused_container_service_call_appends_nothing() {
    let case = "a refused container-service call";
    let emulator = Emulator::new();

    // One service to refuse a second create of, and one image in the registry so
    // a refused register is refused for the digest and not for the service.
    let existing = "ops-log-refused";
    emulator
        .ok(
            case,
            "us-east-1",
            "CreateContainerService",
            json!({"serviceName": existing, "power": "nano", "scale": 1}),
            201,
        )
        .await;
    let before = ids(&emulator.account_log(case).await);
    assert_eq!(
        before.len(),
        1,
        "case {case}: expected the create's own entry, got {before:?}"
    );

    // A name nothing created, used by every refusal that is about a service that
    // is not there.
    let missing = "ops-log-never-created";
    let refusals: &[(&str, &str, Value, u16, &str)] = &[
        (
            "a create of a name the account already holds",
            "CreateContainerService",
            json!({"serviceName": existing, "power": "nano", "scale": 1}),
            400,
            "InvalidInputException",
        ),
        (
            "a create whose public domain names name no certificate of the account",
            "CreateContainerService",
            json!({
                "serviceName": "ops-log-no-certificate",
                "power": "nano",
                "scale": 1,
                "publicDomainNames": {"ops-log-no-such-certificate": ["example.com"]},
            }),
            400,
            "NotFoundException",
        ),
        (
            "an update of a service that is not there",
            "UpdateContainerService",
            json!({"serviceName": missing, "scale": 2}),
            400,
            "NotFoundException",
        ),
        (
            "a deployment to a service that is not there",
            "CreateContainerServiceDeployment",
            json!({
                "serviceName": missing,
                "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
            }),
            400,
            "NotFoundException",
        ),
        (
            "a register of a digest the registry does not hold",
            "RegisterContainerImage",
            json!({
                "serviceName": existing,
                "label": "app",
                "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            }),
            400,
            "NotFoundException",
        ),
        (
            "an image delete on a service that is not there",
            "DeleteContainerImage",
            json!({"serviceName": missing, "image": ":x.app.1"}),
            400,
            "InvalidInputException",
        ),
        (
            "a delete of a service that is not there",
            "DeleteContainerService",
            json!({"serviceName": missing}),
            400,
            "NotFoundException",
        ),
    ];

    for (refusal, operation, body, want_status, want_code) in refusals {
        let (status, answered) = emulator.call("us-east-1", operation, body.clone()).await;
        assert_eq!(
            status, *want_status,
            "case {refusal}: expected HTTP {want_status} from {operation}, got {status} \
             {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            *want_code,
            "case {refusal}: expected {want_code} from {operation}, got {answered:?}"
        );
        assert_eq!(
            ids(&emulator.account_log(case).await),
            before,
            "case {refusal}: a refused call must append nothing to the log"
        );
        assert!(
            emulator.log_for_resource(case, missing).await.is_empty(),
            "case {refusal}: a refused call must append nothing under a name it did not create"
        );
    }
}
