// SPDX-License-Identifier: CC0-1.0

//! The object-storage location an agent runtime's `codeConfiguration` artifact names is
//! RESOLVED while the create or the update is served, and a request naming a location
//! object storage holds no object at is refused `ValidationException` (HTTP 400) with
//! nothing persisted.
//!
//! Two operations store such an artifact — `CreateAgentRuntime$agentRuntimeArtifact` and
//! `UpdateAgentRuntime$agentRuntimeArtifact` — and the runtime RUNS that code, so a
//! location whose object is not there is a refused request rather than a runtime a
//! client believes is invocable.
//!
//! Every suite here mounts the object-storage service beside this one on ONE engine
//! ([`Emulator::with_object_storage`]) — what the assembled emulator does — and stores
//! the objects it expects a request to find through object storage's own front door, so
//! the object a create resolves is one object storage really holds. The rows span what
//! the rule generalises over: the artifact union variant, whether the bucket exists,
//! whether the key exists, the form of the location (a flat key, a nested key and a
//! prefix an object is stored under), the depth of the prefix, whether the prefix ends
//! at a path separator, and whether the location names a version.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The Region every request in this suite is signed for. Not the emulator's default,
/// so the object-storage endpoint the handler resolves the location at is the one the
/// REQUEST named rather than a constant.
const REGION: &str = "us-west-2";

/// A `runtime` value from `AgentManagedRuntimeType` and an `entryPoint` list within
/// `EntryPoints`' 1..2 bound (`smithy_query shape=CodeConfiguration`).
const RUNTIME: &str = "PYTHON_3_12";

/// This emulator, with object storage mounted beside the control plane on the one
/// shared engine.
struct Emulator {
    router: MultiServiceRouter,
    object_storage: bool,
}

impl Emulator {
    /// An emulator with object storage answering, which is what the assembled
    /// emulator is.
    fn with_object_storage() -> Self {
        Self::over_engine(true)
    }

    /// An emulator with ONLY the control plane mounted: nothing can be asked about a
    /// caller's object.
    fn without_object_storage() -> Self {
        Self::over_engine(false)
    }

    fn over_engine(object_storage: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and
        // the handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        if object_storage {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self {
            router,
            object_storage,
        }
    }

    /// One `restJson1` request to the control plane, signed for [`REGION`] — which is
    /// what the router routes on and what the handlers read the Region from.
    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{REGION}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    async fn create_runtime(&self, body: &Value) -> Answer {
        self.call("PUT", "/runtimes/", Some(body)).await
    }

    async fn update_runtime(&self, id: &str, body: &Value) -> Answer {
        self.call("PUT", &format!("/runtimes/{id}/"), Some(body)).await
    }

    async fn get_runtime(&self, id: &str) -> Answer {
        self.call("GET", &format!("/runtimes/{id}/"), None).await
    }

    async fn list_runtimes(&self) -> Answer {
        self.call("POST", "/runtimes/", None).await
    }

    /// One path-style request to object storage's front door. Object storage is a REST
    /// service: the bucket and the key live in the URI and the body is the object's own
    /// bytes, so this is a raw request. The credential scope names `s3`, which is what
    /// the router claims on.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        assert!(
            self.object_storage,
            "an emulator built without object storage cannot be seeded with objects"
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{REGION}/s3/aws4_request, \
             SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Store `body` at `key` in `bucket`, creating the bucket first, so a request that
    /// resolves that location finds the object there. Both writes go over object
    /// storage's own front door, so the object a create reads is one object storage
    /// really holds.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let created = self
            .s3(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert!(
            created == 200 || created == 409,
            "seeding s3://{bucket}: expected 200 or 409 from CreateBucket, got {created}"
        );
        let stored = self.s3("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            stored, 200,
            "seeding s3://{bucket}/{key}: expected 200 from PutObject, got {stored}"
        );
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` puts in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// How much of the code object storage holds before the request runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// The bucket exists and holds an object the location resolves to.
    Object,
    /// The bucket exists but holds no object at or under the location.
    BucketOnly,
    /// The bucket exists and holds a key that merely shares the location's value as
    /// a STRING (`<prefix>.bak`), which is a different object and not the artifact.
    NeighbourOnly,
    /// Object storage holds no such bucket.
    Nothing,
}

/// One code location a request names: where the request says the code is, what object
/// storage holds, and whether that object resolves.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    bucket: &'static str,
    /// `S3Location$prefix` — the value the request carries.
    prefix: &'static str,
    /// `S3Location$versionId` — the value the request carries, when it names one.
    /// Object storage holds no version of this id, since nothing in this suite
    /// enables versioning.
    version_id: Option<&'static str>,
    /// The key object storage really holds, when it holds one.
    key: &'static str,
    stored: Stored,
}

impl Row {
    /// Whether the request naming this location is expected to be served: object
    /// storage holds a key at or under the prefix, and the location names no version
    /// it holds nothing for — "If not specified, the latest version of the object is
    /// used" (`S3Location$versionId`), so a named version is one stored object rather
    /// than whatever the prefix covers.
    fn resolves(&self) -> bool {
        self.stored == Stored::Object && self.version_id.is_none()
    }

    /// Store as much of the code as the row says object storage holds.
    async fn seed(&self, emulator: &Emulator) {
        match self.stored {
            Stored::Object => emulator.put_object(self.bucket, self.key, ZIP).await,
            // The bucket is seeded by storing an object the location does not
            // resolve to.
            Stored::BucketOnly => {
                emulator
                    .put_object(self.bucket, "other/package.zip", ZIP)
                    .await;
            }
            // A key sharing the location's value as a string: the location names
            // neither this key nor anything under itself.
            Stored::NeighbourOnly => {
                emulator
                    .put_object(self.bucket, &format!("{}.bak", self.prefix), ZIP)
                    .await;
            }
            Stored::Nothing => {}
        }
    }

    /// The artifact the request carries for this row.
    fn artifact(&self) -> Value {
        let mut artifact = code_artifact(self.bucket, self.prefix);
        if let Some(version_id) = self.version_id {
            artifact["codeConfiguration"]["code"]["s3"]["versionId"] = json!(version_id);
        }
        artifact
    }
}

/// The first bytes of a zip archive — a deployment package is one, and nothing in the
/// emulator interprets its contents.
const ZIP: &[u8] = b"PK\x03\x04";

/// A `codeConfiguration` artifact at `bucket`/`prefix`.
fn code_artifact(bucket: &str, prefix: &str) -> Value {
    json!({"codeConfiguration": {
        "code": {"s3": {"bucket": bucket, "prefix": prefix}},
        "runtime": RUNTIME,
        "entryPoint": ["app.handler"],
    }})
}

/// A `containerConfiguration` artifact, whose URI satisfies `RuntimeContainerUri`'s
/// ECR pattern. It names no object-storage location, so there is nothing to resolve.
fn container_artifact() -> Value {
    json!({"containerConfiguration": {
        "containerUri": "public.ecr.aws/agentcore/demo:v1"
    }})
}

/// The locations the rule is asserted over. One row per thing it generalises over: an
/// object at the exact key, the same under a nested key, a prefix the bucket holds an
/// object under (with and without a trailing separator), a key the bucket does not hold,
/// a nested key it does not hold, a bucket that is not there at all, a prefix nothing is
/// stored under, a location naming a version object storage holds nothing for, and a
/// bucket holding only a longer key that shares the named key's value.
const ROWS: &[Row] = &[
    Row {
        case: "an object at the flat key the location names",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        version_id: None,
        key: "deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "an object at a nested key the location names",
        bucket: "agentcore-artifacts",
        prefix: "agents/2026/deployment_package.zip",
        version_id: None,
        key: "agents/2026/deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "a prefix the bucket holds an object under",
        bucket: "agentcore-artifacts",
        prefix: "agents/py/",
        version_id: None,
        key: "agents/py/deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "a key the bucket does not hold",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        version_id: None,
        key: "",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a nested key the bucket does not hold",
        bucket: "agentcore-artifacts",
        prefix: "agents/2026/deployment_package.zip",
        version_id: None,
        key: "",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a prefix nothing is stored under",
        bucket: "agentcore-artifacts",
        prefix: "agents/node/",
        version_id: None,
        key: "",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a bucket object storage does not hold",
        bucket: "agentcore-missing-bucket",
        prefix: "deployment_package.zip",
        version_id: None,
        key: "",
        stored: Stored::Nothing,
    },
    Row {
        case: "a stored object named with a version object storage holds nothing for",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        version_id: Some(VERSION_ID),
        key: "deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "a prefix with an object under it, named with a version",
        bucket: "agentcore-artifacts",
        prefix: "agents/py/",
        version_id: Some(VERSION_ID),
        key: "agents/py/deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "a prefix with no trailing separator the bucket holds an object under",
        bucket: "agentcore-artifacts",
        prefix: "agents/go",
        version_id: None,
        key: "agents/go/deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "only a longer key sharing the named key's value",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        version_id: None,
        key: "",
        stored: Stored::NeighbourOnly,
    },
];

/// A `versionId` object storage holds no version for. Shaped like a version id object
/// storage issues — 32 characters of its published alphabet — so it names an ABSENT
/// version rather than a malformed argument, and within `S3Location$versionId`'s
/// modelled 3..1024 length.
const VERSION_ID: &str = "3HL4kqtJlcpXroDTDmJ_RGrmHJ1PyFCY";

/// The execution role every request in this suite names. No IAM is mounted, so nothing
/// is known about it and the role gate refuses nothing: what each case turns on is the
/// code object alone.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRuntimeRole";

/// The create request for `name` carrying `artifact`.
fn create_request(name: &str, artifact: Value) -> Value {
    json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": artifact,
        "roleArn": ROLE_ARN,
    })
}

/// Assert that `answer` is this service's refusal of a code object that is not there:
/// HTTP 400 `ValidationException`, the message the recorded refusal carries, and the
/// member path in the `fieldList` the shape declares for it.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a code object that is not there, got {} and \
         body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got body {}",
        answer.body
    );
    // The sentence the recorded refusal carries, and the path through the artifact
    // union to the member that failed — both spelled out here rather than read off the
    // implementation, so a drift in either is a failing test.
    assert_eq!(
        answer.body["message"],
        json!("S3 object not found in bucket"),
        "case {case:?}: unexpected refusal message, got body {}",
        answer.body
    );
    assert_eq!(
        answer.body["fieldList"][0]["name"],
        json!("agentRuntimeArtifact.codeConfiguration.code.s3"),
        "case {case:?}: the refusal must name the member that failed, got body {}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"],
        json!("FieldValidationFailed"),
        "case {case:?}: a member of the request failed validation, got body {}",
        answer.body
    );
}

/// `CreateAgentRuntime` resolves the code location its artifact names: a location
/// object storage holds an object at yields a runtime that reads back with that
/// artifact, and one it holds nothing at is refused 400 `ValidationException` with no
/// runtime recorded.
#[tokio::test]
async fn create_agent_runtime_resolves_its_code_object() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        row.seed(&emulator).await;

        let request = create_request("code_agent", row.artifact());
        let created = emulator.create_runtime(&request).await;

        if row.resolves() {
            assert_eq!(
                created.status, 202,
                "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
                created.status, created.body
            );
            // The round trip: the artifact the create resolved is the one the stored
            // runtime reports.
            let id = created.body["agentRuntimeId"].as_str().unwrap_or_default();
            let read = emulator.get_runtime(id).await;
            assert_eq!(
                read.body["agentRuntimeArtifact"],
                row.artifact(),
                "case {case:?}: GetAgentRuntime must report the artifact the create stored, \
                 got {} and body {}",
                read.status,
                read.body
            );
            continue;
        }

        assert_refused(case, &created);
        // Refused BEFORE anything is recorded: no runtime of that name is listed, so a
        // client cannot see one it would believe is invocable.
        let listed = emulator.list_runtimes().await;
        assert_eq!(
            listed.body["agentRuntimes"],
            json!([]),
            "case {case:?}: no agent runtime may have been recorded, got {} and body {}",
            listed.status,
            listed.body
        );
        // And the name is still free: a second create naming it, with an object storage
        // really holds, is served rather than refused as a conflict.
        emulator
            .put_object("agentcore-artifacts", "held/package.zip", ZIP)
            .await;
        let again = emulator
            .create_runtime(&create_request(
                "code_agent",
                code_artifact("agentcore-artifacts", "held/package.zip"),
            ))
            .await;
        assert_eq!(
            again.status, 202,
            "case {case:?}: the refused name must still be free, got {} and body {}",
            again.status, again.body
        );
    }
}

/// `UpdateAgentRuntime` resolves the code location of the version it would publish:
/// the same gate, on the same member, of the other operation that stores an artifact.
/// A refused update publishes no version, so the runtime still reports the artifact it
/// was created with.
#[tokio::test]
async fn update_agent_runtime_resolves_its_code_object() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        row.seed(&emulator).await;

        // Created with a container artifact, so the runtime under test exists whatever
        // the row says object storage holds.
        let created = emulator
            .create_runtime(&create_request("updated_agent", container_artifact()))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: seeding the runtime: expected HTTP 202, got {} and body {}",
            created.status, created.body
        );
        let id = created.body["agentRuntimeId"]
            .as_str()
            .expect("the create reports an agentRuntimeId")
            .to_string();

        let update = json!({
            "agentRuntimeArtifact": row.artifact(),
            "roleArn": ROLE_ARN,
        });
        let updated = emulator.update_runtime(&id, &update).await;

        if row.resolves() {
            assert_eq!(
                updated.status, 202,
                "case {case:?}: expected HTTP 202 from UpdateAgentRuntime, got {} and body {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.body["agentRuntimeVersion"],
                json!("2"),
                "case {case:?}: a served update publishes the next version, got body {}",
                updated.body
            );
            let read = emulator.get_runtime(&id).await;
            assert_eq!(
                read.body["agentRuntimeArtifact"],
                row.artifact(),
                "case {case:?}: GetAgentRuntime must report the artifact the update stored, \
                 got {} and body {}",
                read.status,
                read.body
            );
            continue;
        }

        assert_refused(case, &updated);
        let read = emulator.get_runtime(&id).await;
        assert_eq!(
            read.body["agentRuntimeVersion"],
            json!("1"),
            "case {case:?}: a refused update publishes no version, got {} and body {}",
            read.status,
            read.body
        );
        assert_eq!(
            read.body["agentRuntimeArtifact"],
            container_artifact(),
            "case {case:?}: the runtime must still report the artifact it was created with, \
             got body {}",
            read.body
        );
    }
}

/// A `containerConfiguration` artifact names no object-storage location, so nothing is
/// resolved for it: the create is served with object storage mounted and answering, and
/// the runtime reports the container it was created with.
#[tokio::test]
async fn a_container_artifact_resolves_no_object() {
    let case = "a container artifact with object storage answering";
    let emulator = Emulator::with_object_storage();
    // An object storage that holds nothing at all: a location-based gate applied to
    // this artifact would refuse it.
    let created = emulator
        .create_runtime(&create_request("container_agent", container_artifact()))
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        created.status, created.body
    );
    let id = created.body["agentRuntimeId"].as_str().unwrap_or_default();
    let read = emulator.get_runtime(id).await;
    assert_eq!(
        read.body["agentRuntimeArtifact"],
        container_artifact(),
        "case {case:?}: GetAgentRuntime must report the container artifact, got {} and body {}",
        read.status,
        read.body
    );
}

/// An emulator assembled WITHOUT object storage cannot say the code object is absent,
/// so it refuses no request for it.
///
/// The request such an emulator makes is answered by the router's own `UnknownService`
/// — a statement about the emulator, not about the caller's object — and reporting it as
/// a missing object would tell a client its code is not there when nothing looked for
/// it. The create is served and the location the request named is stored as it stands.
#[tokio::test]
async fn an_emulator_without_object_storage_refuses_no_code_object() {
    let case = "no object storage mounted";
    let emulator = Emulator::without_object_storage();
    let artifact = code_artifact("agentcore-artifacts", "deployment_package.zip");

    let created = emulator
        .create_runtime(&create_request("unresolved_agent", artifact.clone()))
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        created.status, created.body
    );
    let id = created.body["agentRuntimeId"].as_str().unwrap_or_default();
    let read = emulator.get_runtime(id).await;
    assert_eq!(
        read.body["agentRuntimeArtifact"],
        artifact,
        "case {case:?}: the location the request named must be stored as it stands, got {} and \
         body {}",
        read.status,
        read.body
    );
}
