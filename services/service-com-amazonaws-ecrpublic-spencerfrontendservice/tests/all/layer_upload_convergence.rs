// SPDX-License-Identifier: CC0-1.0

//! Racing `CompleteLayerUpload` calls, driven end to end over the wire.
//!
//! The rule under test: completing an upload is a CONSUMING commit, so however many
//! callers complete concurrently, the registry converges — at least one call succeeds, and
//! the repository ends up holding the layer those bytes digest to exactly ONCE, with its
//! own byte count, whoever won. The upload id is consumed by the call that wins, so every
//! other caller of that id addresses nothing and is refused with a modelled client
//! refusal; and when two distinct uploads carry the same bytes, the loser is refused
//! because the repository already holds that layer. The committed layer set is read back
//! twice, through `BatchCheckLayerAvailability` and from the store, because a report must
//! not change what the next caller sees.
//!
//! All the callers of one row are in flight together on several worker threads, so the
//! store decides the winner rather than the order the test issued the calls in.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"). The digests the cases expect are computed in the
//! test with its own `sha2`, never read out of a response. The wire plumbing, the
//! upload-store reader and the seeding helpers are the session suite's shared harness
//! (`tests/all/layer_upload_sessions.rs`, `tests/all/layer_upload_completion.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state;

use crate::layer_upload_completion::{
    availability, digest_of, given_parts, given_upload, layer_bytes, reported_layers,
};
use crate::layer_upload_sessions::{
    call, error_code, given_repository, ok, service_with_engine, stored_uploads, REGISTRY_ALIAS,
    REGISTRY_ID,
};

/// The modelled refusals a completion that lost a race may answer with, both HTTP 400 for
/// this operation: `UploadNotFoundException` ("the upload can't be found, or the specified
/// upload ID isn't valid for this repository") once the winner consumed the id, and
/// `LayerAlreadyExistsException` ("the image layer already exists in the associated
/// repository") once the winner committed the layer. Source: the operation's modelled
/// errors, `smithy_query(query="operation", operation="CompleteLayerUpload")`.
const RACE_LOSER_REFUSALS: [&str; 2] = ["UploadNotFoundException", "LayerAlreadyExistsException"];

/// The layers one repository of this registry holds, by digest, read from the store the
/// router was built over: `BatchCheckLayerAvailability` answers the digests a request
/// names, so the whole committed set is read here instead.
fn committed_layers(
    engine: &Arc<engine::InMemoryEngine>,
    repository: &str,
) -> BTreeMap<String, i64> {
    state::service_state(&**engine)
        .expect("the ecr-public store is reachable from the engine the router was built over")
        .lock()
        .repositories
        .get(repository)
        .map(|stored| stored.layers.clone())
        .unwrap_or_default()
}

/// The same digest value spelled with upper-case hex: `LayerDigest` is
/// `^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$` (`smithy_query(query="operation",
/// operation="CompleteLayerUpload")`), so both spellings name one layer and racers may mix
/// them.
fn upper_case_hex(digest: &str) -> String {
    match digest.split_once(':') {
        Some((algorithm, hex)) => format!("{algorithm}:{}", hex.to_ascii_uppercase()),
        None => digest.to_ascii_uppercase(),
    }
}

/// One more upload opened against a repository this registry already holds: the second
/// test needs SEVERAL uploads of one repository, which `given_upload` (one repository per
/// upload) does not open.
async fn open_upload(
    router: &smithy_server_runtime::MultiServiceRouter,
    repository: &str,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "InitiateLayerUpload",
        json!({"repositoryName": repository}),
        case,
    )
    .await;
    answer["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload answered {answer}"))
        .to_string()
}

/// The digest, availability and size each racer's successful answer reported.
fn winner_facts(body: &Value) -> (String, String, String, String) {
    (
        body["layerDigest"].as_str().unwrap_or_default().to_string(),
        body["registryId"].as_str().unwrap_or_default().to_string(),
        body["repositoryName"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        body["uploadId"].as_str().unwrap_or_default().to_string(),
    )
}

/// Racing completions of ONE upload id converge: exactly one caller commits the layer the
/// upload's parts carried, every other caller of that id is refused with a modelled client
/// refusal, and the repository holds that layer once with its own byte count.
///
/// The rows differ along what the rule generalises over — how many callers race, how many
/// bytes the layer carries, how many parts they arrived in, and how the request names the
/// registry (not at all, by account id, by alias) — and the racers of one row differ in
/// how they spell the digest they claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_completions_of_one_upload_commit_the_layer_exactly_once() {
    // (case, racers, layer length, how many parts, the `registryId` named)
    let rows: Vec<(&str, usize, usize, usize, Option<&str>)> = vec![
        ("two racers, one byte in one part, no registryId", 2, 1, 1, None),
        (
            "three racers, an odd size in one part, registryId by account id",
            3,
            777,
            1,
            Some(REGISTRY_ID),
        ),
        (
            "five racers, a kibibyte in three parts, registryId by alias",
            5,
            1024,
            3,
            Some(REGISTRY_ALIAS),
        ),
        (
            "eight racers, an uneven size in four parts",
            8,
            4097,
            4,
            None,
        ),
    ];

    let (router, engine) = service_with_engine();
    let router = Arc::new(router);

    for (index, (case, racers, length, part_count, registry)) in rows.into_iter().enumerate() {
        let repository = format!("race/one-upload-{racers}");
        let layer = layer_bytes(length, index + 1);
        let expected_digest = digest_of(&layer);
        let upload_id = given_upload(&router, case, &repository).await;
        given_parts(&router, &repository, &upload_id, &layer, part_count, case).await;

        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let repository = repository.clone();
            let upload_id = upload_id.clone();
            // Half the racers claim the digest in upper-case hex: one value, two
            // spellings, so the winner cannot depend on which one arrived first.
            let claimed = if racer % 2 == 0 {
                expected_digest.clone()
            } else {
                upper_case_hex(&expected_digest)
            };
            let registry = registry.map(str::to_string);
            tasks.push(tokio::spawn(async move {
                let mut request = json!({
                    "repositoryName": repository,
                    "uploadId": upload_id,
                    "layerDigests": [claimed],
                });
                if let Some(registry) = registry {
                    request["registryId"] = json!(registry);
                }
                let (status, body) = call(&router, "CompleteLayerUpload", request).await;
                (racer, status, body)
            }));
        }

        let mut answers = Vec::new();
        for task in tasks {
            answers.push(task.await.expect("the racing completion task completed"));
        }

        let winners: Vec<&(usize, u16, Value)> =
            answers.iter().filter(|answer| answer.1 == 200).collect();
        assert!(
            !winners.is_empty(),
            "case {case:?}: at least one of {racers} racing completions must commit the \
             layer, got {answers:?}"
        );
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: the upload is consumed by the completion that wins, so exactly \
             one of {racers} racers may succeed, got {answers:?}"
        );
        assert_eq!(
            winner_facts(&winners[0].2),
            (
                expected_digest.clone(),
                REGISTRY_ID.to_string(),
                repository.clone(),
                upload_id.clone()
            ),
            "case {case:?}: the winner reports the digest of the uploaded bytes beside the \
             registry, repository and upload named, got {:?}",
            winners[0].2
        );

        for (racer, status, body) in answers.iter().filter(|answer| answer.1 != 200) {
            assert_eq!(
                *status, 400,
                "case {case:?}: racer {racer} lost, so it must be refused with a client \
                 refusal, got {status} with {body}"
            );
            let code = error_code(body);
            assert!(
                RACE_LOSER_REFUSALS
                    .iter()
                    .any(|refusal| code.contains(refusal)),
                "case {case:?}: racer {racer} must be refused with one of \
                 {RACE_LOSER_REFUSALS:?}, got {code:?} with {body}"
            );
        }

        // The winner consumed the upload, so a caller arriving after the race addresses
        // nothing and gets the same refusal the losers got.
        assert!(
            !stored_uploads(&engine).contains_key(&upload_id),
            "case {case:?}: the completed upload {upload_id:?} must be gone from the store, \
             got {:?}",
            stored_uploads(&engine).keys().collect::<Vec<_>>()
        );
        let (late_status, late_body) = call(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [expected_digest.clone()],
            }),
        )
        .await;
        assert_eq!(
            late_status, 400,
            "case {case:?}: completing the consumed id again is refused, got {late_status} \
             with {late_body}"
        );
        assert!(
            error_code(&late_body).contains("UploadNotFoundException"),
            "case {case:?}: expected UploadNotFoundException after the race, got {late_body}"
        );

        // Whoever won, the repository holds that one layer with its own byte count — and
        // reading it back does not change it.
        let expected_size = i64::try_from(length).unwrap_or(i64::MAX);
        let expected_set = BTreeMap::from([(expected_digest.clone(), expected_size)]);
        for read in 1..=2 {
            assert_eq!(
                committed_layers(&engine, &repository),
                expected_set,
                "case {case:?}: read {read} of the committed layer set must hold the raced \
                 layer exactly once"
            );
            let report =
                availability(&router, &repository, std::slice::from_ref(&expected_digest), case)
                    .await;
            assert_eq!(
                reported_layers(&report, case),
                vec![(
                    expected_digest.clone(),
                    "AVAILABLE".to_string(),
                    Some(expected_size)
                )],
                "case {case:?}: read {read} must report the raced layer as available, got \
                 {report}"
            );
        }
    }
}

/// Racing completions of DISTINCT uploads that carry the same bytes converge on the same
/// one layer: the layer's identity is its digest, so the first commit wins and every other
/// upload of those bytes is refused because the repository already holds that layer.
///
/// The rows differ in how many uploads race and in the size and part count of the bytes
/// they all carry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_completions_of_distinct_uploads_of_one_layer_commit_it_once() {
    // (case, how many uploads race, layer length, how many parts each upload sends)
    let rows: Vec<(&str, usize, usize, usize)> = vec![
        ("two uploads of one byte", 2, 1, 1),
        ("three uploads of 2048 bytes in two parts", 3, 2048, 2),
        ("four uploads of 5001 bytes in three parts", 4, 5001, 3),
    ];

    let (router, engine) = service_with_engine();
    let router = Arc::new(router);

    for (index, (case, uploads, length, part_count)) in rows.into_iter().enumerate() {
        let repository = format!("race/same-bytes-{uploads}");
        let layer = layer_bytes(length, index + 11);
        let expected_digest = digest_of(&layer);

        given_repository(&router, case, &repository).await;
        let mut upload_ids = Vec::new();
        for _ in 0..uploads {
            let upload_id = open_upload(&router, &repository, case).await;
            given_parts(&router, &repository, &upload_id, &layer, part_count, case).await;
            upload_ids.push(upload_id);
        }

        let mut tasks = Vec::new();
        for upload_id in upload_ids.clone() {
            let router = Arc::clone(&router);
            let repository = repository.clone();
            let claimed = expected_digest.clone();
            tasks.push(tokio::spawn(async move {
                let (status, body) = call(
                    &router,
                    "CompleteLayerUpload",
                    json!({
                        "repositoryName": repository,
                        "uploadId": upload_id,
                        "layerDigests": [claimed],
                    }),
                )
                .await;
                (upload_id, status, body)
            }));
        }

        let mut answers = Vec::new();
        for task in tasks {
            answers.push(task.await.expect("the racing completion task completed"));
        }

        let winners: Vec<&(String, u16, Value)> =
            answers.iter().filter(|answer| answer.1 == 200).collect();
        assert!(
            !winners.is_empty(),
            "case {case:?}: at least one of {uploads} uploads of the same bytes must commit \
             the layer, got {answers:?}"
        );
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: the layer is one layer, so exactly one of {uploads} uploads may \
             commit it, got {answers:?}"
        );
        let winning_upload = winners[0].0.clone();
        assert_eq!(
            winner_facts(&winners[0].2),
            (
                expected_digest.clone(),
                REGISTRY_ID.to_string(),
                repository.clone(),
                winning_upload.clone()
            ),
            "case {case:?}: the winner reports the digest of the uploaded bytes beside the \
             registry, repository and upload named, got {:?}",
            winners[0].2
        );
        assert!(
            upload_ids.contains(&winning_upload),
            "case {case:?}: the winning upload {winning_upload:?} was never opened, opened \
             {upload_ids:?}"
        );

        for (upload_id, status, body) in answers.iter().filter(|answer| answer.1 != 200) {
            assert_eq!(
                *status, 400,
                "case {case:?}: the completion of {upload_id:?} lost, so it must be refused \
                 with a client refusal, got {status} with {body}"
            );
            assert!(
                error_code(body).contains("LayerAlreadyExistsException"),
                "case {case:?}: the completion of {upload_id:?} must be refused because the \
                 repository already holds that layer, got {body}"
            );
        }

        // The winner's id is consumed, and the repository holds the raced layer once with
        // its own byte count however many uploads carried it.
        assert!(
            !stored_uploads(&engine).contains_key(&winning_upload),
            "case {case:?}: the winning upload {winning_upload:?} must be gone from the \
             store, got {:?}",
            stored_uploads(&engine).keys().collect::<Vec<_>>()
        );
        let expected_size = i64::try_from(length).unwrap_or(i64::MAX);
        assert_eq!(
            committed_layers(&engine, &repository),
            BTreeMap::from([(expected_digest.clone(), expected_size)]),
            "case {case:?}: the committed layer set must hold the raced layer exactly once"
        );
        let report = availability(
            &router,
            &repository,
            std::slice::from_ref(&expected_digest),
            case,
        )
        .await;
        assert_eq!(
            reported_layers(&report, case),
            vec![(
                expected_digest.clone(),
                "AVAILABLE".to_string(),
                Some(expected_size)
            )],
            "case {case:?}: the raced layer must be reported available with its byte count, \
             got {report}"
        );
    }
}
