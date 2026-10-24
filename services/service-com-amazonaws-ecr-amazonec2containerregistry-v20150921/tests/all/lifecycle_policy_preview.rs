// SPDX-License-Identifier: CC0-1.0

//! The ECR lifecycle policy preview, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes. The
//! rule under test: `StartLifecyclePolicyPreview` evaluates a lifecycle policy document
//! against the images the named repository holds at that instant and stores the
//! evaluation as the repository's one preview, which answers `IN_PROGRESS` until the
//! clock makes its completion due and is then reported, unchanged, by
//! `GetLifecyclePolicyPreview`.
//!
//! The harness (`emulator`/`call`/`ok`) is the three functions each suite in this crate
//! owns: `tests/all.rs` compiles every suite as a private module of one test crate, so
//! they are not shared between suites.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. It is what routes a request to ECR.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator vends, spelled out here rather than imported
/// so a wrong registry id in the implementation fails this test.
const REGISTRY_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// `LifecyclePolicyText`'s modelled minimum length (`@length(min: 100, max: 30720)`), so
/// every policy this suite sends has to reach it.
const MIN_POLICY_LENGTH: usize = 100;

/// Long enough past [`state::LIFECYCLE_PREVIEW_DELAY`] that the completion the clock owes
/// is certainly due. Derived from the span itself, never a bare number.
const PAST_THE_DELAY: Duration = state::LIFECYCLE_PREVIEW_DELAY.saturating_add(
    Duration::from_millis(150),
);

/// The repository every preview case in [`a_preview_reports_the_images_the_policy_acts_on`]
/// is taken of.
const PREVIEWED: &str = "lifecycle-preview";

/// The images that repository holds, oldest push first: two tag families and two
/// untagged images, so a rule can select on tag status, on a prefix and on a pattern.
const IMAGES: [(&str, &str, Option<&str>); 5] = [
    ("prod-1", r#"{"schemaVersion":2,"image":"prod-1"}"#, Some("prod-1")),
    ("prod-2", r#"{"schemaVersion":2,"image":"prod-2"}"#, Some("prod-2")),
    ("beta-9", r#"{"schemaVersion":2,"image":"beta-9"}"#, Some("beta-9")),
    ("bare-a", r#"{"schemaVersion":2,"image":"bare-a"}"#, None),
    ("bare-b", r#"{"schemaVersion":2,"image":"bare-b"}"#, None),
];

/// One row of the preview table.
struct PreviewCase<'a> {
    /// What the row is about.
    case: &'a str,
    /// The policy's `rules`.
    rules: Value,
    /// The `(image label, appliedRulePriority)` pairs the preview must report.
    acted: Vec<(&'a str, i64)>,
    /// The `action.type` every reported image must carry.
    action_type: &'a str,
    /// The `action.targetStorageClass` every reported image must carry.
    target_storage_class: Option<&'a str>,
    /// `summary.expiringImageTotalCount`.
    expiring: i64,
    /// `summary.transitioningImageTotalCounts`, or `Value::Null` when no image moves.
    transitioning: Value,
}

/// One reported preview result: the digest, the rule that claimed the image, the action
/// and its target storage class.
type PreviewRow = (String, i64, String, Option<String>);

/// The digest a registry addresses a manifest by: `sha256:<hex>` over the manifest
/// bytes. Computed here independently of the crate's own helper.
fn manifest_digest(manifest: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(manifest.as_bytes())))
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A policy document as the wire carries it: padded with trailing spaces, which JSON
/// ignores, up to [`MIN_POLICY_LENGTH`].
fn as_policy_text(document: &str) -> String {
    let mut text = document.to_string();
    while text.len() < MIN_POLICY_LENGTH {
        text.push(' ');
    }
    text
}

/// The policy document carrying `rules`.
fn policy_text(rules: Value) -> String {
    as_policy_text(&json!({"rules": rules}).to_string())
}

/// One rule: `{"rulePriority": .., "selection": {..}, "action": {..}}`.
fn rule(priority: i64, selection: Value, action: Value) -> Value {
    json!({"rulePriority": priority, "selection": selection, "action": action})
}

/// The `expire` action every rule carries unless the row is about transitioning.
fn expire() -> Value {
    json!({"type": "expire"})
}

/// The reported `(digest, appliedRulePriority, action type, targetStorageClass)` rows.
fn preview_rows(body: &Value) -> BTreeSet<PreviewRow> {
    body["previewResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            (
                result["imageDigest"].as_str().unwrap_or_default().to_string(),
                result["appliedRulePriority"].as_i64().unwrap_or_default(),
                at(result, "/action/type").as_str().unwrap_or_default().to_string(),
                at(result, "/action/targetStorageClass")
                    .as_str()
                    .map(ToString::to_string),
            )
        })
        .collect()
}

/// Create a repository and push [`IMAGES`] into it, oldest first.
///
/// The pushes are spaced so that push order is recency order: an `imageCountMoreThan`
/// rule keeps the newest matching images, and two images pushed within the same clock
/// tick would make which of them that is arbitrary.
async fn given_repository_with_images(router: &MultiServiceRouter, name: &str) {
    ok(router, name, "CreateRepository", &json!({"repositoryName": name})).await;
    for (label, manifest, tag) in IMAGES {
        let mut request = json!({"repositoryName": name, "imageManifest": manifest});
        if let Some(tag) = tag {
            request["imageTag"] = json!(tag);
        }
        ok(router, label, "PutImage", &request).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The digest of each [`IMAGES`] label.
fn digests() -> BTreeMap<&'static str, String> {
    IMAGES
        .iter()
        .map(|(label, manifest, _)| (*label, manifest_digest(manifest)))
        .collect()
}

/// Start a preview, wait out the completion the clock owes, and return what
/// `GetLifecyclePolicyPreview` reports.
async fn completed_preview(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    text: &str,
) -> Value {
    let started = ok(
        router,
        case,
        "StartLifecyclePolicyPreview",
        &json!({"repositoryName": repository, "lifecyclePolicyText": text}),
    )
    .await;
    assert_eq!(
        at(&started, "/status"),
        json!(state::PREVIEW_STATUS_IN_PROGRESS),
        "case {case:?}: a preview that has just started is still running, got {:?}",
        at(&started, "/status")
    );
    assert_eq!(
        at(&started, "/lifecyclePolicyText"),
        json!(text),
        "case {case:?}: the start reports the policy it is previewing, got {:?}",
        at(&started, "/lifecyclePolicyText")
    );
    assert_eq!(
        at(&started, "/registryId"),
        json!(REGISTRY_ID),
        "case {case:?}: expected registryId {REGISTRY_ID}, got {:?}",
        at(&started, "/registryId")
    );
    assert_eq!(
        at(&started, "/repositoryName"),
        json!(repository),
        "case {case:?}: the start names the repository previewed, got {:?}",
        at(&started, "/repositoryName")
    );

    tokio::time::sleep(PAST_THE_DELAY).await;

    let preview = ok(
        router,
        case,
        "GetLifecyclePolicyPreview",
        &json!({"repositoryName": repository}),
    )
    .await;
    assert_eq!(
        at(&preview, "/status"),
        json!(state::PREVIEW_STATUS_COMPLETE),
        "case {case:?}: past the owed completion the preview is COMPLETE, got {:?}",
        at(&preview, "/status")
    );
    preview
}

/// A preview reports exactly the images its policy acts on, each with the rule that
/// claimed it, and summarises them.
///
/// The rows differ along what the rule generalises over: which tag status a rule selects,
/// whether it selects by prefix or by wildcard pattern, which count type and count bounds
/// it applies, how many rules the document carries and in which order they are listed,
/// and which action they take.
#[tokio::test]
async fn a_preview_reports_the_images_the_policy_acts_on() {
    let untagged_beyond_the_newest = json!({
        "tagStatus": "untagged",
        "countType": "imageCountMoreThan",
        "countNumber": 1,
    });
    let any_beyond_the_newest = json!({
        "tagStatus": "any",
        "countType": "imageCountMoreThan",
        "countNumber": 1,
    });
    let cases = vec![
        PreviewCase {
            case: "untagged images beyond the newest one",
            rules: json!([rule(1, untagged_beyond_the_newest.clone(), expire())]),
            acted: vec![("bare-a", 1)],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 1,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "tagged images selected by prefix",
            rules: json!([rule(
                7,
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )]),
            acted: vec![("prod-1", 7)],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 1,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "tagged images selected by wildcard patterns",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "tagPatternList": ["prod*", "*ta-9"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 2,
                }),
                expire(),
            )]),
            acted: vec![("prod-1", 1)],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 1,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "a pattern no tag matches",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "tagPatternList": ["release-*"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )]),
            acted: vec![],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 0,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "two prefixes, keeping the newest match",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod", "beta"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )]),
            acted: vec![("prod-1", 1), ("prod-2", 1)],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 2,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "any image beyond the newest two",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 2,
                }),
                expire(),
            )]),
            acted: vec![("prod-1", 1), ("prod-2", 1), ("beta-9", 1)],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 3,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "a count as large as the repository holds",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": IMAGES.len(),
                }),
                expire(),
            )]),
            acted: vec![],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 0,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "nothing was pushed more than a day ago",
            rules: json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": 1,
                }),
                expire(),
            )]),
            acted: vec![],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 0,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "two rules: the first that matches claims the image",
            rules: json!([
                rule(1, untagged_beyond_the_newest.clone(), expire()),
                rule(2, any_beyond_the_newest.clone(), expire()),
            ]),
            acted: vec![
                ("bare-a", 1),
                ("prod-1", 2),
                ("prod-2", 2),
                ("beta-9", 2),
            ],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 4,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "rules listed out of order are applied in rule priority order",
            rules: json!([
                rule(20, any_beyond_the_newest.clone(), expire()),
                rule(10, untagged_beyond_the_newest.clone(), expire()),
            ]),
            acted: vec![
                ("bare-a", 10),
                ("prod-1", 20),
                ("prod-2", 20),
                ("beta-9", 20),
            ],
            action_type: "EXPIRE",
            target_storage_class: None,
            expiring: 4,
            transitioning: Value::Null,
        },
        PreviewCase {
            case: "a transition to archive storage",
            rules: json!([rule(
                1,
                untagged_beyond_the_newest.clone(),
                json!({"type": "transition", "targetStorageClass": "ARCHIVE"}),
            )]),
            acted: vec![("bare-a", 1)],
            action_type: "TRANSITION",
            target_storage_class: Some("ARCHIVE"),
            expiring: 0,
            transitioning: json!([{"targetStorageClass": "ARCHIVE", "imageTotalCount": 1}]),
        },
    ];

    let router = emulator();
    given_repository_with_images(&router, PREVIEWED).await;
    let digests = digests();

    for PreviewCase {
        case,
        rules,
        acted,
        action_type,
        target_storage_class,
        expiring,
        transitioning,
    } in cases
    {
        let text = policy_text(rules);
        let preview = completed_preview(&router, case, PREVIEWED, &text).await;

        let expected: BTreeSet<PreviewRow> = acted
            .iter()
            .map(|(label, priority)| {
                (
                    digests[label].clone(),
                    *priority,
                    action_type.to_string(),
                    target_storage_class.map(ToString::to_string),
                )
            })
            .collect();
        assert_eq!(
            preview_rows(&preview),
            expected,
            "case {case:?}: the preview must report the images the policy acts on, got {:?}",
            at(&preview, "/previewResults")
        );
        assert_eq!(
            at(&preview, "/lifecyclePolicyText"),
            json!(text),
            "case {case:?}: the preview reports the policy it was started with, got {:?}",
            at(&preview, "/lifecyclePolicyText")
        );
        assert_eq!(
            at(&preview, "/summary/expiringImageTotalCount"),
            json!(expiring),
            "case {case:?}: expected {expiring} expiring image(s), got {:?}",
            at(&preview, "/summary/expiringImageTotalCount")
        );
        assert_eq!(
            at(&preview, "/summary/transitioningImageTotalCounts"),
            transitioning,
            "case {case:?}: expected the transitioning counts {transitioning:?}, got {:?}",
            at(&preview, "/summary/transitioningImageTotalCounts")
        );

        // Every reported image is reported from the stored image record: its tags, the
        // instant it was pushed and the storage class it is in.
        for result in preview["previewResults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let digest = result["imageDigest"].as_str().unwrap_or_default().to_string();
            let (label, _, tag) = IMAGES
                .iter()
                .find(|(_, manifest, _)| manifest_digest(manifest) == digest)
                .copied()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the preview reported an unknown digest {digest:?}")
                });
            assert_eq!(
                at(&result, "/imageTags"),
                tag.map_or(Value::Null, |tag| json!([tag])),
                "case {case:?}, image {label:?}: expected the stored tags {tag:?}, got {:?}",
                at(&result, "/imageTags")
            );
            assert!(
                at(&result, "/imagePushedAt").as_f64().is_some_and(|at| at > 0.0),
                "case {case:?}, image {label:?}: imagePushedAt is the push instant, got {:?}",
                at(&result, "/imagePushedAt")
            );
            assert_eq!(
                at(&result, "/storageClass"),
                json!("STANDARD"),
                "case {case:?}, image {label:?}: every stored image is in standard \
                 storage, got {:?}",
                at(&result, "/storageClass")
            );
        }
    }
}

/// A preview runs for a span the clock owes: while it is running a second start is
/// refused, the read reports `IN_PROGRESS` with no results, and once the completion falls
/// due the results are there and a new preview may be started.
#[tokio::test]
async fn a_second_preview_is_refused_until_the_clock_completes_the_first() {
    let router = emulator();
    given_repository_with_images(&router, "concurrent").await;
    let text = policy_text(json!([rule(
        1,
        json!({
            "tagStatus": "untagged",
            "countType": "imageCountMoreThan",
            "countNumber": 1,
        }),
        expire(),
    )]));
    let request = json!({"repositoryName": "concurrent", "lifecyclePolicyText": text});

    let started = ok(
        &router,
        "the first start",
        "StartLifecyclePolicyPreview",
        &request,
    )
    .await;
    assert_eq!(
        at(&started, "/status"),
        json!("IN_PROGRESS"),
        "case {:?}: the preview starts running, got {:?}",
        "the first start",
        at(&started, "/status")
    );

    // The span is never zero: a caller acting straight after the start still meets the
    // transient state and the error it answers with.
    let running = ok(
        &router,
        "reading a running preview",
        "GetLifecyclePolicyPreview",
        &json!({"repositoryName": "concurrent"}),
    )
    .await;
    assert_eq!(
        at(&running, "/status"),
        json!("IN_PROGRESS"),
        "case {:?}: a preview that has not completed is still IN_PROGRESS, got {:?}",
        "reading a running preview",
        at(&running, "/status")
    );
    assert_eq!(
        at(&running, "/previewResults"),
        Value::Null,
        "case {:?}: a running preview has no results yet, got {:?}",
        "reading a running preview",
        at(&running, "/previewResults")
    );
    assert_eq!(
        at(&running, "/summary"),
        Value::Null,
        "case {:?}: a running preview has nothing to summarise yet, got {:?}",
        "reading a running preview",
        at(&running, "/summary")
    );

    let refused = call(&router, "StartLifecyclePolicyPreview", &request).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400 while a preview is running, got {} with {:?}",
        "a second start while one runs", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "LifecyclePolicyPreviewInProgressException",
        "case {:?}: expected LifecyclePolicyPreviewInProgressException, got {:?}",
        "a second start while one runs",
        refused.body
    );

    tokio::time::sleep(PAST_THE_DELAY).await;

    let completed = ok(
        &router,
        "reading the completed preview",
        "GetLifecyclePolicyPreview",
        &json!({"repositoryName": "concurrent"}),
    )
    .await;
    assert_eq!(
        at(&completed, "/status"),
        json!("COMPLETE"),
        "case {:?}: the completion is owed by the clock, got {:?}",
        "reading the completed preview",
        at(&completed, "/status")
    );
    assert_eq!(
        preview_rows(&completed).len(),
        1,
        "case {:?}: the untagged image beyond the newest one is acted on, got {:?}",
        "reading the completed preview",
        at(&completed, "/previewResults")
    );

    // And with nothing running, a start is accepted again and replaces the preview.
    let restarted = ok(
        &router,
        "a start once the first completed",
        "StartLifecyclePolicyPreview",
        &request,
    )
    .await;
    assert_eq!(
        at(&restarted, "/status"),
        json!("IN_PROGRESS"),
        "case {:?}: the new preview is running, got {:?}",
        "a start once the first completed",
        at(&restarted, "/status")
    );
    let replaced = ok(
        &router,
        "the replaced preview",
        "GetLifecyclePolicyPreview",
        &json!({"repositoryName": "concurrent"}),
    )
    .await;
    assert_eq!(
        at(&replaced, "/status"),
        json!("IN_PROGRESS"),
        "case {:?}: the repository carries the newly started preview, got {:?}",
        "the replaced preview",
        at(&replaced, "/status")
    );
}

/// A preview read pages, filters and selects over the stored evaluation, and its summary
/// is of the whole preview rather than of the page.
#[tokio::test]
async fn preview_results_are_paged_filtered_and_selected() {
    let router = emulator();
    given_repository_with_images(&router, "paged").await;
    let digests = digests();
    let text = policy_text(json!([rule(
        3,
        json!({
            "tagStatus": "any",
            "countType": "imageCountMoreThan",
            "countNumber": 1,
        }),
        expire(),
    )]));
    completed_preview(
        &router,
        "every image but the newest expires",
        "paged",
        &text,
    )
    .await;

    // The newest push is the one image the rule keeps; the other four are the preview.
    let kept = "bare-b";
    let all: BTreeSet<String> = digests
        .iter()
        .filter(|(label, _)| **label != kept)
        .map(|(_, digest)| digest.clone())
        .collect();
    let total = i64::try_from(all.len()).expect("four results fit an i64");

    // Paging: every page of `maxResults` carries at most that many results, and the
    // pages together carry the whole preview exactly once.
    for page_size in [1_i64, 2, 3, 4, 100] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"repositoryName": "paged", "maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, &case, "GetLifecyclePolicyPreview", &request).await;
            let rows = preview_rows(&page);
            assert!(
                rows.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page carries at most maxResults results, got {}",
                rows.len()
            );
            assert_eq!(
                at(&page, "/summary/expiringImageTotalCount"),
                json!(total),
                "case {case:?}: the summary is of the preview, not of the page, got {:?}",
                at(&page, "/summary/expiringImageTotalCount")
            );
            seen.extend(rows.into_iter().map(|(digest, ..)| digest));
            token = at(&page, "/nextToken").as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            all.len(),
            "case {case:?}: paging must report each result once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all,
            "case {case:?}: paging must report the whole preview, got {seen:?}"
        );
    }

    // Filtering by tag status, and selecting by image identifier.
    let tagged: BTreeSet<String> = ["prod-1", "prod-2", "beta-9"]
        .iter()
        .map(|label| digests[label].clone())
        .collect();
    let untagged: BTreeSet<String> = BTreeSet::from([digests["bare-a"].clone()]);
    let selections: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("no filter at all", json!({}), all.clone()),
        (
            "the tagged images",
            json!({"filter": {"tagStatus": "TAGGED"}}),
            tagged,
        ),
        (
            "the untagged images",
            json!({"filter": {"tagStatus": "UNTAGGED"}}),
            untagged,
        ),
        (
            "either tag status",
            json!({"filter": {"tagStatus": "ANY"}}),
            all.clone(),
        ),
        (
            "one image by digest",
            json!({"imageIds": [{"imageDigest": digests["bare-a"]}]}),
            BTreeSet::from([digests["bare-a"].clone()]),
        ),
        (
            "one image by tag",
            json!({"imageIds": [{"imageTag": "beta-9"}]}),
            BTreeSet::from([digests["beta-9"].clone()]),
        ),
        (
            "two images, one by digest and one by digest and tag",
            json!({"imageIds": [
                {"imageDigest": digests["bare-a"]},
                {"imageDigest": digests["prod-1"], "imageTag": "prod-1"},
            ]}),
            BTreeSet::from([digests["bare-a"].clone(), digests["prod-1"].clone()]),
        ),
        (
            "an image the preview does not act on",
            json!({"imageIds": [{"imageDigest": digests[kept]}]}),
            BTreeSet::new(),
        ),
        (
            "a digest paired with a tag that is on another image",
            json!({"imageIds": [{"imageDigest": digests["prod-1"], "imageTag": "prod-2"}]}),
            BTreeSet::new(),
        ),
        (
            "a digest the registry does not hold",
            json!({"imageIds": [{"imageDigest": manifest_digest("{\"schemaVersion\":2}")}]}),
            BTreeSet::new(),
        ),
        (
            "a tagged filter and one untagged image selected",
            json!({
                "filter": {"tagStatus": "TAGGED"},
                "imageIds": [{"imageDigest": digests["bare-a"]}],
            }),
            BTreeSet::new(),
        ),
    ];

    for (case, mut request, expected) in selections {
        request["repositoryName"] = json!("paged");
        let answer = ok(&router, case, "GetLifecyclePolicyPreview", &request).await;
        let reported: BTreeSet<String> = preview_rows(&answer)
            .into_iter()
            .map(|(digest, ..)| digest)
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the digests {expected:?}, got {:?}",
            at(&answer, "/previewResults")
        );
        assert_eq!(
            at(&answer, "/summary/expiringImageTotalCount"),
            json!(total),
            "case {case:?}: the summary is of the preview, not of the selection, got {:?}",
            at(&answer, "/summary/expiringImageTotalCount")
        );
    }
}

/// What the registry does not hold, and what the policy format does not allow, is refused
/// with the error the model declares for it — and nothing is stored for a refused call.
#[tokio::test]
async fn a_preview_of_what_the_registry_cannot_evaluate_is_refused() {
    let router = emulator();
    ok(
        &router,
        "the repository the refusals are taken against",
        "CreateRepository",
        &json!({"repositoryName": "refusals"}),
    )
    .await;
    let valid = policy_text(json!([rule(
        1,
        json!({
            "tagStatus": "any",
            "countType": "imageCountMoreThan",
            "countNumber": 1,
        }),
        expire(),
    )]));

    // Each row: the case, the operation, the request, and the error code it must answer.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a repository the registry does not hold",
            "StartLifecyclePolicyPreview",
            json!({"repositoryName": "no-such-repo", "lifecyclePolicyText": valid}),
            "RepositoryNotFoundException",
        ),
        (
            "a registry that is not this one",
            "StartLifecyclePolicyPreview",
            json!({
                "repositoryName": "refusals",
                "registryId": "210987654321",
                "lifecyclePolicyText": valid,
            }),
            "InvalidParameterException",
        ),
        (
            "no policy named and none set on the repository",
            "StartLifecyclePolicyPreview",
            json!({"repositoryName": "refusals"}),
            "LifecyclePolicyNotFoundException",
        ),
        (
            "reading a preview of a repository the registry does not hold",
            "GetLifecyclePolicyPreview",
            json!({"repositoryName": "no-such-repo"}),
            "RepositoryNotFoundException",
        ),
        (
            "reading a preview nobody started",
            "GetLifecyclePolicyPreview",
            json!({"repositoryName": "refusals"}),
            "LifecyclePolicyPreviewNotFoundException",
        ),
        (
            "paging a read that names the images to include",
            "GetLifecyclePolicyPreview",
            json!({
                "repositoryName": "refusals",
                "imageIds": [{"imageTag": "v1"}],
                "maxResults": 1,
            }),
            "InvalidParameterException",
        ),
        (
            "a token with a read that names the images to include",
            "GetLifecyclePolicyPreview",
            json!({
                "repositoryName": "refusals",
                "imageIds": [{"imageTag": "v1"}],
                "nextToken": "6162",
            }),
            "InvalidParameterException",
        ),
        (
            "an image identifier naming neither a digest nor a tag",
            "GetLifecyclePolicyPreview",
            json!({"repositoryName": "refusals", "imageIds": [{}]}),
            "InvalidParameterException",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );
    }
}

/// A policy document that breaks the format is refused as an invalid parameter, and
/// leaves the repository with no preview.
///
/// The rows differ along which part of the document is wrong: the document itself, the
/// rule list, the rule priority, the selection, its tag lists, its count, and the action.
#[tokio::test]
async fn a_policy_document_that_breaks_the_format_is_refused() {
    let router = emulator();
    ok(
        &router,
        "the repository the documents are sent against",
        "CreateRepository",
        &json!({"repositoryName": "documents"}),
    )
    .await;

    let selection = json!({
        "tagStatus": "any",
        "countType": "imageCountMoreThan",
        "countNumber": 1,
    });
    let cases: Vec<(&str, String)> = vec![
        (
            "not JSON at all",
            as_policy_text("expire everything older than a fortnight, please"),
        ),
        ("JSON that is not an object", as_policy_text("[1, 2, 3]")),
        ("no rules member", as_policy_text(r#"{"Rules": []}"#)),
        ("a rules member that is not an array", {
            as_policy_text(r#"{"rules": {"rulePriority": 1}}"#)
        }),
        ("an empty rule list", policy_text(json!([]))),
        (
            "a rule priority below the modelled minimum of 1",
            policy_text(json!([rule(0, selection.clone(), expire())])),
        ),
        (
            "a rule priority that is not an integer",
            policy_text(json!([{
                "rulePriority": "first",
                "selection": selection.clone(),
                "action": expire(),
            }])),
        ),
        (
            "two rules with the same priority",
            policy_text(json!([
                rule(1, selection.clone(), expire()),
                rule(1, selection.clone(), expire()),
            ])),
        ),
        (
            "no selection",
            policy_text(json!([{"rulePriority": 1, "action": expire()}])),
        ),
        (
            "a tag status outside TagStatus",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "retagged",
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "tagged images selected with no tag list",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "a tag prefix list on a rule selecting untagged images",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "untagged",
                    "tagPrefixList": ["prod"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "a tag pattern list that is not a list of strings",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "tagPatternList": [7],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "both tag lists on one rule",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod"],
                    "tagPatternList": ["prod*"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "a count type the format does not define",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePulled",
                    "countUnit": "days",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "no count number",
            policy_text(json!([rule(
                1,
                json!({"tagStatus": "any", "countType": "imageCountMoreThan"}),
                expire(),
            )])),
        ),
        (
            "a count number below the format's minimum of 1",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 0,
                }),
                expire(),
            )])),
        ),
        (
            "a negative count number",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": -1,
                }),
                expire(),
            )])),
        ),
        (
            "an age with no count unit",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countNumber": 14,
                }),
                expire(),
            )])),
        ),
        (
            "an age counted in something other than days",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "weeks",
                    "countNumber": 2,
                }),
                expire(),
            )])),
        ),
        (
            "an image count carrying a count unit",
            policy_text(json!([rule(
                1,
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countUnit": "days",
                    "countNumber": 1,
                }),
                expire(),
            )])),
        ),
        (
            "no action",
            policy_text(json!([{"rulePriority": 1, "selection": selection.clone()}])),
        ),
        (
            "an action type outside ImageActionType",
            policy_text(json!([rule(
                1,
                selection.clone(),
                json!({"type": "delete"}),
            )])),
        ),
        (
            "a transition with no target storage class",
            policy_text(json!([rule(
                1,
                selection.clone(),
                json!({"type": "transition"}),
            )])),
        ),
        (
            "a transition to a storage class the model does not carry",
            policy_text(json!([rule(
                1,
                selection.clone(),
                json!({"type": "transition", "targetStorageClass": "GLACIER"}),
            )])),
        ),
        (
            "a target storage class on an expiry",
            policy_text(json!([rule(
                1,
                selection.clone(),
                json!({"type": "expire", "targetStorageClass": "ARCHIVE"}),
            )])),
        ),
    ];

    for (case, text) in cases {
        let answer = call(
            &router,
            "StartLifecyclePolicyPreview",
            &json!({"repositoryName": "documents", "lifecyclePolicyText": text}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
    }

    // A refused start stored nothing: there is still no preview to read.
    let read = call(
        &router,
        "GetLifecyclePolicyPreview",
        &json!({"repositoryName": "documents"}),
    )
    .await;
    assert_eq!(
        read.error_code(),
        "LifecyclePolicyPreviewNotFoundException",
        "a refused start must leave no preview behind, got {:?}",
        read.body
    );
}

/// `sinceImagePushed` acts on an image exactly when it was pushed MORE than the
/// rule's count of days ago.
///
/// The ages a push can have reach back further than a test can wait, so this row set
/// evaluates the policy against image records of a chosen age directly, through the same
/// `state` helpers the handler calls. The rows walk each count's own bound: a day short
/// of it, exactly on it, and a second past it.
#[tokio::test]
async fn an_image_is_expired_only_once_it_is_older_than_the_count_in_days() {
    let now = SystemTime::now();
    let day = Duration::from_secs(60 * 60 * 24);

    // 1 is the count's own minimum, so the rows start there.
    for days in [1_u64, 14, 365] {
        let count = day * u32::try_from(days).expect("the row's count fits a u32");
        // (case, how long ago the image was pushed, whether the rule acts on it)
        let ages: Vec<(String, Duration, bool)> = vec![
            (
                format!("{days} days: pushed just now"),
                Duration::ZERO,
                false,
            ),
            (
                format!("{days} days: a day short of the count"),
                count.saturating_sub(day),
                false,
            ),
            (format!("{days} days: exactly on the count"), count, false),
            (
                format!("{days} days: a second past the count"),
                count + Duration::from_secs(1),
                true,
            ),
            (
                format!("{days} days: a year past the count"),
                count + day * 365,
                true,
            ),
        ];

        let rules = state::parse_lifecycle_policy(&policy_text(json!([rule(
            1,
            json!({
                "tagStatus": "any",
                "countType": "sinceImagePushed",
                "countUnit": "days",
                "countNumber": days,
            }),
            expire(),
        )])))
        .expect("the row's policy document is well formed");

        for (case, age, expired) in ages {
            let manifest = format!("{{\"schemaVersion\":2,\"case\":\"{case}\"}}");
            let digest = manifest_digest(&manifest);
            let image = state::ImageRecord::pushed(digest.clone(), manifest, None, now - age);
            let images = BTreeMap::from([(digest.clone(), image)]);
            let acted = state::evaluate_lifecycle_policy(&rules, &images, now);
            assert_eq!(
                acted.contains_key(&digest),
                expired,
                "case {case:?}: expected acted-on == {expired}, got {:?}",
                acted.keys().collect::<Vec<_>>()
            );
            if expired {
                let result = &acted[&digest];
                assert_eq!(
                    result.applied_rule_priority, 1,
                    "case {case:?}: the only rule claimed the image, got priority {}",
                    result.applied_rule_priority
                );
                assert_eq!(
                    result.action_type, "EXPIRE",
                    "case {case:?}: an expire rule expires the image, got {:?}",
                    result.action_type
                );
            }
        }
    }
}
