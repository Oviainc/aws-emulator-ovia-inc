// SPDX-License-Identifier: CC0-1.0

//! Opening an ECR Public layer upload, driven end to end over the wire.
//!
//! The rule under test: `InitiateLayerUpload` opens exactly ONE new upload against a
//! repository this registry holds, hands back an `uploadId` no other upload shares plus
//! the `partSize` the client is to send future layer parts in, and records that upload in
//! the service's state for the later `UploadLayerPart`/`CompleteLayerUpload` calls to
//! address — while a request naming a repository this registry does not hold, or a
//! registry that is not this one, opens nothing.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"). An in-flight upload is reported by no operation of
//! this model — `BatchCheckLayerAvailability` reports COMPLETED layers, not open uploads —
//! so the cases that need to see what was persisted read the record back from the store
//! through the engine the router was built over. The harness below is shared with the
//! layer-part and completion suites (`tests/all/layer_part_uploads.rs`,
//! `tests/all/layer_upload_completion.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// This registry's `registryId`: the canonical emulator account id
/// (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const REGISTRY_ID: &str = "000000000000";

/// This registry's alias, which `RegistryIdOrAlias` also admits in place of the account
/// id (`state::DEFAULT_REGISTRY_ALIAS`).
pub(crate) const REGISTRY_ALIAS: &str = "emulator";

/// The ceiling on one layer part, which the service reads from the model:
/// `UploadLayerPart$layerPartBlob` is `@length(min: 0, max: 20971520)`
/// (`smithy_query(query="operation", operation="UploadLayerPart")`), recorded once for
/// the crate as [`state::MAX_LAYER_PART_BYTES`]. A `partSize` the client is told to send
/// parts in therefore cannot exceed it.
const MAX_LAYER_PART_BYTES: i64 = state::MAX_LAYER_PART_BYTES;

/// A router with just this service mounted, plus the engine its state lives in.
pub(crate) fn service_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope the
    // shared `call` sends still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDECRPUBLIC/20260101/us-east-1\
             /ecr-public/aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// The modelled error code an `awsJson1_1` error body names.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The uploads the store holds, by upload id.
pub(crate) fn stored_uploads(
    engine: &Arc<engine::InMemoryEngine>,
) -> BTreeMap<String, state::LayerUpload> {
    state::service_state(&**engine)
        .expect("the ecr-public store is reachable from the engine the router was built over")
        .lock()
        .uploads
        .clone()
}

/// The layer bytes the store holds for ONE upload, or `None` when it holds no such
/// upload: read under the lock so no other upload's bytes are copied.
pub(crate) fn upload_bytes(
    engine: &Arc<engine::InMemoryEngine>,
    upload_id: &str,
) -> Option<Vec<u8>> {
    state::service_state(&**engine)
        .expect("the ecr-public store is reachable from the engine the router was built over")
        .lock()
        .uploads
        .get(upload_id)
        .map(|upload| upload.received.clone())
}

/// Whether `candidate` has the layout the model gives `UploadId`:
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`
/// (`smithy_query(query="operation", operation="InitiateLayerUpload")`, output
/// `uploadId`).
fn is_upload_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8_usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Create the repository `name`, failing the case if the create does not succeed.
pub(crate) async fn given_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": name}),
        case,
    )
    .await;
}

/// Opening an upload mints a fresh, well-formed upload id bound to the repository the
/// request named, reports a part size a client can actually send, and persists exactly
/// one upload per call — leaving the repository itself untouched.
///
/// The rows differ along the repository name's form, nesting and length (both published
/// bounds of `RepositoryName`, len 2..205) and along how many uploads one repository
/// carries at once, which is everything the request can vary.
#[tokio::test]
async fn initiating_an_upload_mints_one_persisted_upload_per_call() {
    // `repositoryName` is `@length(2..205)` with pattern
    // `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$`: the rows walk
    // its lower bound, its separators, its nesting and its upper bound. Source: the
    // model, via `smithy_query(query="operation", operation="InitiateLayerUpload")`.
    let longest = "z".repeat(205);
    let cases: Vec<(&str, &str, usize)> = vec![
        ("shortest admissible name", "ab", 1),
        ("plain name, several uploads", "nginx", 4),
        ("dotted, underscored and hyphenated", "team.app_2-beta", 2),
        ("nested path", "org/team/app", 3),
        ("longest admissible name", longest.as_str(), 1),
    ];

    let (router, engine) = service_with_engine();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut expected_total = 0_usize;

    for (case, name, uploads) in cases {
        given_repository(&router, case, name).await;
        // The repository as it stands before any upload is opened: opening one records an
        // upload and must not touch the repository.
        let before = ok(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            case,
        )
        .await;

        for attempt in 0..uploads {
            let answer = ok(
                &router,
                "InitiateLayerUpload",
                json!({"repositoryName": name}),
                case,
            )
            .await;
            expected_total += 1;

            let upload_id = answer["uploadId"].as_str().unwrap_or_default().to_string();
            assert!(
                is_upload_id(&upload_id),
                "case {case:?} attempt {attempt}: uploadId must have the modelled \
                 8-4-4-4-12 hex layout, got {upload_id:?}"
            );
            assert!(
                seen.insert(upload_id.clone()),
                "case {case:?} attempt {attempt}: every upload gets its own id, but \
                 {upload_id:?} was handed out twice"
            );

            let part_size = answer["partSize"].as_i64();
            assert!(
                part_size.is_some_and(|size| size > 0 && size <= MAX_LAYER_PART_BYTES),
                "case {case:?} attempt {attempt}: partSize must be a part a client can \
                 send (1..={MAX_LAYER_PART_BYTES} bytes), got {part_size:?}"
            );

            let uploads_now = stored_uploads(&engine);
            assert_eq!(
                uploads_now.len(),
                expected_total,
                "case {case:?} attempt {attempt}: one call opens exactly one upload, got \
                 {} stored",
                uploads_now.len()
            );
            let stored = uploads_now.get(&upload_id).unwrap_or_else(|| {
                panic!("case {case:?} attempt {attempt}: upload {upload_id:?} was not persisted")
            });
            assert_eq!(
                stored.repository_name, name,
                "case {case:?} attempt {attempt}: the upload belongs to the repository the \
                 request named, got {:?}",
                stored.repository_name
            );
            assert_eq!(
                Some(stored.part_size),
                part_size,
                "case {case:?} attempt {attempt}: the stored part size is the one reported, \
                 got {:?}",
                stored.part_size
            );
        }

        let after = ok(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            case,
        )
        .await;
        assert_eq!(
            after, before,
            "case {case:?}: opening an upload changes nothing about the repository, got \
             {after:?}"
        );
    }
}

/// An upload is opened in the registry the request names, which can only be this one:
/// omitting `registryId`, naming this registry's account id and naming its alias all
/// work, and naming another registry is refused with the modelled
/// `RegistryNotFoundException` and opens nothing.
#[tokio::test]
async fn an_upload_is_opened_only_in_this_registry() {
    let (router, engine) = service_with_engine();
    given_repository(&router, "registry scoping", "scoped").await;

    // `registryId` targets `RegistryIdOrAlias` (len 2..50), so a request may name this
    // registry by account id or by alias; every other value names a registry this
    // emulator does not serve.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "no registryId: the default public registry is assumed",
            json!({"repositoryName": "scoped"}),
            None,
        ),
        (
            "this registry, by account id",
            json!({"repositoryName": "scoped", "registryId": REGISTRY_ID}),
            None,
        ),
        (
            "this registry, by alias",
            json!({"repositoryName": "scoped", "registryId": REGISTRY_ALIAS}),
            None,
        ),
        (
            "another account's registry",
            json!({"repositoryName": "scoped", "registryId": "123456789012"}),
            Some("RegistryNotFoundException"),
        ),
        (
            "another registry's alias",
            json!({"repositoryName": "scoped", "registryId": "someoneelse"}),
            Some("RegistryNotFoundException"),
        ),
        (
            "another account's registry, repository unknown there too",
            json!({"repositoryName": "elsewhere", "registryId": "123456789012"}),
            Some("RegistryNotFoundException"),
        ),
    ];

    let mut expected_uploads = 0_usize;
    for (case, request, expected_error) in cases {
        let (status, parsed) = call(&router, "InitiateLayerUpload", request).await;
        match expected_error {
            None => {
                expected_uploads += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {parsed}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 {code}, got {status} with {parsed}"
                );
                let wire_type = error_code(&parsed);
                assert!(
                    wire_type.contains(code),
                    "case {case:?}: expected {code}, got {wire_type:?} in {parsed}"
                );
            }
        }
        let stored = stored_uploads(&engine);
        assert_eq!(
            stored.len(),
            expected_uploads,
            "case {case:?}: only an accepted call opens an upload, got {} stored",
            stored.len()
        );
    }
}

/// There is no upload without a repository: a name this registry does not hold — never
/// created, or created and then deleted — is refused with the modelled
/// `RepositoryNotFoundException`, and nothing is recorded.
#[tokio::test]
async fn an_upload_against_an_unknown_repository_is_refused() {
    let (router, engine) = service_with_engine();

    given_repository(&router, "deleted repository", "gone").await;
    ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "gone"}),
        "deleted repository",
    )
    .await;

    let cases: Vec<(&str, Value)> = vec![
        ("never created", json!({"repositoryName": "missing-repo"})),
        ("created and then deleted", json!({"repositoryName": "gone"})),
        (
            "nested path that was never created",
            json!({"repositoryName": "org/team/absent"}),
        ),
        (
            "shortest admissible name, never created",
            json!({"repositoryName": "ab"}),
        ),
    ];

    for (case, request) in cases {
        let (status, parsed) = call(&router, "InitiateLayerUpload", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        let wire_type = error_code(&parsed);
        assert!(
            wire_type.contains("RepositoryNotFoundException"),
            "case {case:?}: expected RepositoryNotFoundException, got {wire_type:?} in {parsed}"
        );
        let stored = stored_uploads(&engine);
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused call records no upload, got {stored:?}"
        );
    }
}

/// The store is bounded: it holds [`state::MAX_LAYER_UPLOADS`] in-flight uploads, and the
/// call past that cap opens none — it is refused with the `ServerException` this
/// operation models (HTTP 500), this registry's own capacity rather than a fault in the
/// request.
///
/// The cap is read from the constant, so the boundary tested is wherever the
/// implementation puts it. All but the last two uploads are seeded straight into the
/// store (this suite holds the engine): what is under test is the boundary, not a
/// thousand round trips.
#[tokio::test]
async fn the_upload_past_the_cap_is_refused() {
    let (router, engine) = service_with_engine();
    given_repository(&router, "the upload cap", "capped").await;

    // One short of the cap, so the next wire call is the one that fills it.
    let seeded = state::MAX_LAYER_UPLOADS - 1;
    {
        let store = state::service_state(&*engine).expect("the ecr-public store");
        let mut store = store.lock();
        for index in 0..seeded {
            store.uploads.insert(
                format!("seeded-upload-{index}"),
                state::LayerUpload {
                    repository_name: "capped".to_string(),
                    part_size: state::REPORTED_LAYER_PART_SIZE,
                    started_at: std::time::SystemTime::now(),
                    received: Vec::new(),
                },
            );
        }
    }

    let (status, parsed) = call(
        &router,
        "InitiateLayerUpload",
        json!({"repositoryName": "capped"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the upload that fills the cap is still opened, got {status} with {parsed}"
    );
    assert_eq!(
        stored_uploads(&engine).len(),
        state::MAX_LAYER_UPLOADS,
        "the store holds every upload opened up to the cap"
    );

    let (status, parsed) = call(
        &router,
        "InitiateLayerUpload",
        json!({"repositoryName": "capped"}),
    )
    .await;
    assert_eq!(
        status, 500,
        "the upload past the cap is refused, got {status} with {parsed}"
    );
    let wire_type = error_code(&parsed);
    assert!(
        wire_type.contains("ServerException"),
        "expected ServerException past {} uploads, got {wire_type:?} in {parsed}",
        state::MAX_LAYER_UPLOADS
    );
    assert_eq!(
        stored_uploads(&engine).len(),
        state::MAX_LAYER_UPLOADS,
        "a refused call opens no upload, so the store stays at the cap"
    );
}
