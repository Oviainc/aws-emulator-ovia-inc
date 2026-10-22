// SPDX-License-Identifier: CC0-1.0

//! Opening a layer upload, end to end over the wire.
//!
//! The rule under test: `InitiateLayerUpload` opens ONE new upload against a repository
//! this registry holds, hands back an upload id no other upload shares plus the part
//! size the client is to send future parts in, and records that upload in the service's
//! state for the later `UploadLayerPart`/`CompleteLayerUpload` calls to address — while
//! a request naming a repository the registry does not hold, or a registry that is not
//! this one, opens nothing.
//!
//! The wire harness is `crate::wire`'s. An upload is not reportable by any implemented
//! operation yet (`UploadLayerPart`, `CompleteLayerUpload` and `GetDownloadUrlForLayer`
//! are 501 stubs with issues of their own), so these cases read the persisted record
//! back from the store, through the engine handle `wire::emulator_with_engine` returns.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, ok, REGISTRY_ID};

/// The ceiling on one layer part, from the model: `UploadLayerPart`'s `layerPartBlob` is
/// `@length(min: 0, max: 20971520)`
/// (`smithy_query(query="operation",operation="UploadLayerPart")`). A `partSize` the
/// client is told to send parts in therefore cannot exceed it.
const MAX_LAYER_PART_BYTES: i64 = 20_971_520;

/// The uploads the store holds, by upload id.
fn stored_uploads(engine: &Arc<engine::InMemoryEngine>) -> BTreeMap<String, state::LayerUpload> {
    state::service_state(&**engine)
        .expect("the ecr store is reachable from the engine the router was built over")
        .lock()
        .uploads
        .clone()
}

/// Whether `candidate` has the layout the model gives `UploadId`:
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`
/// (`smithy_query(query="operation",operation="InitiateLayerUpload")`, output
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
async fn given_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
}

/// Opening an upload mints a fresh, well-formed upload id bound to the named
/// repository, reports a usable part size, and persists exactly one upload per call.
///
/// The rows differ along the repository name's form and length — the only thing the
/// request varies — so the rule is asserted over the class of names the model admits,
/// not over one name.
#[tokio::test]
async fn initiating_an_upload_mints_one_persisted_upload_per_call() {
    // `repositoryName` is `@length(2..256)` with pattern
    // `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`:
    // the rows walk its lower bound, its separators, its nesting and its upper bound.
    let longest = "a".repeat(256);
    let cases: Vec<(&str, &str, usize)> = vec![
        ("shortest admissible name", "ab", 1),
        ("plain name, several uploads", "layers", 4),
        ("dotted, underscored and hyphenated", "team.app_2__beta-1", 2),
        ("nested path", "org/team/app", 3),
        ("longest admissible name", longest.as_str(), 1),
    ];

    let (router, engine) = emulator_with_engine();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut expected_total = 0_usize;

    for (case, name, uploads) in cases {
        given_repository(&router, case, name).await;
        // The repository as it stands before any upload is opened: opening one adds an
        // upload, and must not touch the repository itself.
        let before = ok(
            &router,
            case,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;

        for attempt in 0..uploads {
            let answer = ok(
                &router,
                case,
                "InitiateLayerUpload",
                &json!({"repositoryName": name}),
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
                "case {case:?} attempt {attempt}: one call opens exactly one upload, \
                 got {} stored",
                uploads_now.len()
            );
            let stored = uploads_now.get(&upload_id).unwrap_or_else(|| {
                panic!("case {case:?} attempt {attempt}: upload {upload_id:?} was not persisted")
            });
            assert_eq!(
                stored.repository_name, name,
                "case {case:?} attempt {attempt}: the upload belongs to the repository \
                 the request named, got {:?}",
                stored.repository_name
            );
            assert_eq!(
                Some(stored.part_size),
                part_size,
                "case {case:?} attempt {attempt}: the stored part size is the one \
                 reported, got {:?}",
                stored.part_size
            );
        }

        let after = ok(
            &router,
            case,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;
        assert_eq!(
            after, before,
            "case {case:?}: opening an upload changes nothing about the repository, \
             got {after:?}"
        );
    }
}

/// An upload is opened in the registry the request names, which can only be this one:
/// omitting `registryId` and naming this registry both work, and naming another
/// registry is refused with the modelled `InvalidParameterException` and opens nothing.
#[tokio::test]
async fn an_upload_is_opened_only_in_this_registry() {
    let (router, engine) = emulator_with_engine();
    given_repository(&router, "registry scoping", "scoped").await;

    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "no registryId: the default registry is assumed",
            json!({"repositoryName": "scoped"}),
            None,
        ),
        (
            "this registry, named explicitly",
            json!({"repositoryName": "scoped", "registryId": REGISTRY_ID}),
            None,
        ),
        (
            "another account's registry",
            json!({"repositoryName": "scoped", "registryId": "123456789012"}),
            Some("InvalidParameterException"),
        ),
        (
            "another account's registry, repository unknown there too",
            json!({"repositoryName": "elsewhere", "registryId": "123456789012"}),
            Some("InvalidParameterException"),
        ),
    ];

    let mut expected_uploads = 0_usize;
    for (case, request, expected_error) in cases {
        let answer = call(&router, "InitiateLayerUpload", &request).await;
        match expected_error {
            None => {
                expected_uploads += 1;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
            }
            Some(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case:?}: expected {code}, got {:?}",
                    answer.body
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

/// There is no upload without a repository: a name the registry does not hold — never
/// created, or created and then deleted — is refused with the modelled
/// `RepositoryNotFoundException`, and nothing is recorded.
#[tokio::test]
async fn an_upload_against_an_unknown_repository_is_refused() {
    let (router, engine) = emulator_with_engine();

    given_repository(&router, "deleted repository", "gone").await;
    ok(
        &router,
        "deleted repository",
        "DeleteRepository",
        &json!({"repositoryName": "gone"}),
    )
    .await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "never created",
            json!({"repositoryName": "missing-repo"}),
        ),
        (
            "created and then deleted",
            json!({"repositoryName": "gone"}),
        ),
        (
            "nested path that was never created",
            json!({"repositoryName": "org/team/absent"}),
        ),
    ];

    for (case, request) in cases {
        let answer = call(&router, "InitiateLayerUpload", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "RepositoryNotFoundException",
            "case {case:?}: expected RepositoryNotFoundException, got {:?}",
            answer.body
        );
        let stored = stored_uploads(&engine);
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused call records no upload, got {stored:?}"
        );
    }
}

/// The store is bounded: it holds [`state::MAX_LAYER_UPLOADS`] in-flight uploads, and
/// the call past that cap opens no upload — it is refused with the `ServerException`
/// this operation models, the registry's own capacity rather than a fault in the
/// request.
///
/// The cap is read from the constant, so the boundary tested is wherever the
/// implementation puts it. All but the last two uploads are seeded straight into the
/// store (the suite holds the engine): what is under test is the boundary, not a
/// thousand round trips.
#[tokio::test]
async fn the_upload_past_the_cap_is_refused() {
    let (router, engine) = emulator_with_engine();
    given_repository(&router, "the upload cap", "capped").await;

    // One short of the cap, so the next wire call is the one that fills it.
    let seeded = state::MAX_LAYER_UPLOADS - 1;
    {
        let store = state::service_state(&*engine).expect("the ecr store");
        let mut store = store.lock();
        for index in 0..seeded {
            store.uploads.insert(
                format!("seeded-upload-{index}"),
                state::LayerUpload {
                    repository_name: "capped".to_string(),
                    part_size: state::REPORTED_LAYER_PART_SIZE,
                    started_at: std::time::SystemTime::now(),
                },
            );
        }
    }

    let last = call(
        &router,
        "InitiateLayerUpload",
        &json!({"repositoryName": "capped"}),
    )
    .await;
    assert_eq!(
        last.status,
        200,
        "the upload that fills the cap is still opened, got {} with {:?}",
        last.status,
        last.body
    );
    assert_eq!(
        stored_uploads(&engine).len(),
        state::MAX_LAYER_UPLOADS,
        "the store holds every upload opened up to the cap"
    );

    let refused = call(
        &router,
        "InitiateLayerUpload",
        &json!({"repositoryName": "capped"}),
    )
    .await;
    assert_eq!(
        refused.status, 500,
        "the upload past the cap is refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServerException",
        "expected ServerException past {} uploads, got {:?}",
        state::MAX_LAYER_UPLOADS,
        refused.body
    );
    assert_eq!(
        stored_uploads(&engine).len(),
        state::MAX_LAYER_UPLOADS,
        "a refused call opens no upload, so the store stays at the cap"
    );
}
