// SPDX-License-Identifier: CC0-1.0

//! `DescribeImageSigningStatus`, end to end over the wire.
//!
//! The rule under test: a push stores one signing job per signing PROFILE the registry's
//! stored signing configuration points at the repository pushed to — "When an image is
//! pushed to a matching repository, a signing job is created using the specified profile"
//! (`SigningRule`), and "if the image matched signing rules that reference different
//! signing profiles, a status is returned for each profile"
//! (`DescribeImageSigningStatus`) — and the read reports one `ImageSigningStatus` per job:
//! `IN_PROGRESS` until the clock owes that job its terminal value
//! ([`signing::IMAGE_SIGNING_DELAY`]), then `COMPLETE` when the AWS Signer profile the rule
//! named can sign and `FAILED`, with a `failureCode` and a `failureReason`, when it cannot.
//! The jobs are decided AT the push, so a rule stored afterwards signs nothing the registry
//! already holds and deleting the configuration withdraws no status; an image pushed while
//! no rule matched its repository reports the empty list. Reading changes nothing.
//!
//! AWS Signer is mounted beside this registry on one engine, exactly as the assembled
//! router mounts them, so the signing profile a rule names is a profile this emulator
//! really holds — created through Signer's own front door — rather than one assumed to
//! exist.
//!
//! The ECR calls go through `crate::wire`, so every case exercises routing,
//! deserialization, the handlers, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, given_layer, ok, REGION, REGISTRY_ID};

use ecr::state::signing;

/// Long enough past [`signing::IMAGE_SIGNING_DELAY`] that the terminal status the clock
/// owes every job is certainly due. Derived from the span itself, never a bare number.
const PAST_THE_DELAY: Duration =
    signing::IMAGE_SIGNING_DELAY.saturating_add(Duration::from_millis(200));

/// The only value `SigningRepositoryFilterType` admits
/// (`smithy_query(query="shape",shape="SigningRepositoryFilterType")` → `["WILDCARD_MATCH"]`).
const WILDCARD_MATCH: &str = "WILDCARD_MATCH";

/// The `SigningStatus` values a status can be in
/// (`smithy_query(query="shape",shape="SigningStatus")` → `["IN_PROGRESS","COMPLETE","FAILED"]`).
const IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`IN_PROGRESS`].
const COMPLETE: &str = "COMPLETE";
/// See [`IN_PROGRESS`].
const FAILED: &str = "FAILED";

/// The container-image signing platform a signing profile for an OCI image is created on
/// (`signer:ListSigningPlatforms` publishes it; it takes no `signingMaterial`).
const CONTAINER_PLATFORM: &str = "Notation-OCI-SHA384-ECDSA";

/// The profile that exists and can sign, so a job under it completes.
const ACTIVE_PROFILE: &str = "release_profile";

/// The profile that exists but has been canceled, which "cannot perform new signing jobs"
/// (`signer:CancelSigningProfile`).
const CANCELED_PROFILE: &str = "retired_profile";

/// A profile name no `PutSigningProfile` ever created in this emulator.
const MISSING_PROFILE: &str = "absent_profile";

/// Another account's registry: a well-formed `RegistryId` (`^[0-9]{12}$`) that is not this
/// one, so an ARN carrying it names another account's signing profile.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A Region other than the one this suite's calls are made against, so an ARN carrying it
/// names another Region's signing profile.
const FOREIGN_REGION: &str = "us-east-2";

/// A signing-profile ARN, built to `SigningProfileArn`'s modelled pattern
/// `^arn:aws(-[a-z]+)*:signer:[a-z0-9-]+:[0-9]{12}:\/signing-profiles\/[a-zA-Z0-9_]{2,}$`
/// — the form Signer's own create answers with, which [`given_signing_profile`] checks.
fn profile_arn(account: &str, region: &str, name: &str) -> String {
    format!("arn:aws:signer:{region}:{account}:/signing-profiles/{name}")
}

/// The ARN of a signing profile of THIS account in the Region this suite calls.
fn local_profile_arn(name: &str) -> String {
    profile_arn(REGISTRY_ID, REGION, name)
}

/// One emulator: a router carrying this registry and AWS Signer over ONE shared engine, so
/// a signing rule naming a Signer profile names a profile this emulator holds.
fn emulator_with_signer() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ecr::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_signer_wallabyservice::EmulatedService::new(engine),
    );
    router
}

/// One call on AWS Signer's restJson1 wire, whose credential scope's Region is the Region
/// the profile is created in.
async fn signer_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "signer {method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, body)
}

/// Create the signing profile `name` through Signer's own `PutSigningProfile`, canceling it
/// afterwards when `canceled`, and answer the ARN Signer minted for it.
async fn given_signing_profile(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    canceled: bool,
) -> String {
    let (status, body) = signer_call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({"platformId": CONTAINER_PLATFORM})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutSigningProfile for {name:?} must succeed, got {status} with {body:?}"
    );
    let arn = body["arn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        arn,
        local_profile_arn(name),
        "case {case:?}: Signer's own profile ARN must be the form this suite builds, got {arn:?}"
    );
    if canceled {
        let (status, body) =
            signer_call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: CancelSigningProfile for {name:?} must succeed, got {status} \
             with {body:?}"
        );
    }
    arn
}

/// A rule naming `profile_arn`, with one `WILDCARD_MATCH` filter per entry of `filters` (and
/// no `repositoryFilters` member at all when there are none).
fn rule(profile_arn: &str, filters: &[&str]) -> Value {
    let mut rule = json!({"signingProfileArn": profile_arn});
    if !filters.is_empty() {
        rule["repositoryFilters"] = Value::Array(
            filters
                .iter()
                .map(|filter| json!({"filter": filter, "filterType": WILDCARD_MATCH}))
                .collect(),
        );
    }
    rule
}

/// Store `rules` as the registry's whole signing configuration.
async fn given_configuration(router: &MultiServiceRouter, case: &str, rules: Value) {
    ok(
        router,
        case,
        "PutSigningConfiguration",
        &json!({"signingConfiguration": {"rules": rules}}),
    )
    .await;
}

/// Create `repository` and push one single-layer image into it, answering the digest the
/// registry addresses it by.
async fn given_pushed_image(router: &MultiServiceRouter, case: &str, repository: &str) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let content = format!("{repository}-layer").into_bytes();
    let layer = given_layer(router, case, repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest, "imageTag": "sign-target"}),
    )
    .await;
    ecr::state::manifest_digest(&manifest)
}

/// The `signingStatuses` reported for one image, in signing-profile order so a comparison
/// does not depend on the order the rules happened to be stored in. The read is made by
/// image TAG, the way a client that just pushed one addresses it.
async fn statuses(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Vec<Value> {
    let answer = ok(
        router,
        case,
        "DescribeImageSigningStatus",
        &json!({"repositoryName": repository, "imageId": {"imageTag": "sign-target"}}),
    )
    .await;
    assert_eq!(
        at(&answer, "/imageId/imageDigest"),
        json!(digest),
        "case {case:?}: the read reports the image it was asked about, got {answer:?}"
    );
    assert_eq!(
        at(&answer, "/repositoryName"),
        json!(repository),
        "case {case:?}: the read names the repository it was asked about, got {answer:?}"
    );
    assert_eq!(
        at(&answer, "/registryId"),
        json!(REGISTRY_ID),
        "case {case:?}: the read names the registry holding the image, got {answer:?}"
    );
    let mut reported: Vec<Value> = answer["signingStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    reported.sort_by_key(|entry| {
        at(entry, "/signingProfileArn")
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    reported
}

/// The profile ARNs a report names, in the order [`statuses`] sorted them.
fn named_profiles(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|entry| {
            at(entry, "/signingProfileArn")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// What the clock owes a job naming one profile: the terminal status it settles on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The profile the rule names can sign, so the signature is generated.
    Complete,
    /// The profile the rule names cannot sign, so the job fails saying why.
    Failed,
}

impl Outcome {
    /// The `SigningStatus` this outcome settles on.
    fn status(self) -> &'static str {
        match self {
            Self::Complete => COMPLETE,
            Self::Failed => FAILED,
        }
    }
}

/// One row: what it is about, the repository pushed to, the rules in force at that push,
/// and the `(signing profile ARN, outcome)` pairs expected — in signing-profile order.
type Row = (
    &'static str,
    &'static str,
    Vec<Value>,
    Vec<(String, Outcome)>,
);

/// Assert that `entry` reports `status`, and that `failureCode`/`failureReason` are present
/// exactly when it is `FAILED` — "only present if `status` is `FAILED`"
/// (`ImageSigningStatus$failureCode`, `$failureReason`).
fn assert_entry(case: &str, entry: &Value, profile: &str, status: &str) {
    assert_eq!(
        at(entry, "/status"),
        json!(status),
        "case {case:?}: the job under {profile:?} must report {status}, got {entry:?}"
    );
    let code = at(entry, "/failureCode");
    let reason = at(entry, "/failureReason");
    if status == FAILED {
        let code = code.as_str().unwrap_or_default().to_string();
        let reason = reason.as_str().unwrap_or_default().to_string();
        assert!(
            !code.is_empty(),
            "case {case:?}: a failed job must carry a failureCode, got {entry:?}"
        );
        assert!(
            reason.contains(profile),
            "case {case:?}: a failed job's failureReason must name the profile {profile:?}, \
             got {reason:?}"
        );
    } else {
        assert_eq!(
            (code.clone(), reason.clone()),
            (Value::Null, Value::Null),
            "case {case:?}: a {status} job carries neither failureCode nor failureReason, \
             got {entry:?}"
        );
    }
}

/// The rows the table drives, built once the profiles' ARNs are known. They differ along
/// what the rule generalises over: whether the rule carries a filter at all, where the
/// wildcard sits in the filter that covers (or does not cover) the repository, how many
/// rules and how many distinct profiles the configuration names, whether two rules name one
/// profile, and whether the profile a rule names can sign — one that exists and is active,
/// one that exists and was canceled, one that does not exist, one of another account and one
/// of another Region. The report's own case is the single `WILDCARD_MATCH` rule naming an
/// active profile.
fn rows() -> Vec<Row> {
    let active = local_profile_arn(ACTIVE_PROFILE);
    let canceled = local_profile_arn(CANCELED_PROFILE);
    let missing = local_profile_arn(MISSING_PROFILE);
    let foreign_account = profile_arn(FOREIGN_ACCOUNT, REGION, ACTIVE_PROFILE);
    let foreign_region = profile_arn(REGISTRY_ID, FOREIGN_REGION, ACTIVE_PROFILE);

    vec![
        (
            "a wildcard filter covering the repository",
            "signed-app",
            vec![rule(&active, &["signed-*"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a rule with no filters at all, which signs every repository",
            "unfiltered",
            vec![rule(&active, &[])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a filter that is the bare wildcard",
            "anything",
            vec![rule(&active, &["*"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a wildcard filter matching the end of the repository name",
            "team/production",
            vec![rule(&active, &["*/production"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a wildcard filter matching the middle of the repository name",
            "eu-prod-app",
            vec![rule(&active, &["*prod*"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "an exact filter naming the repository",
            "exact-match",
            vec![rule(&active, &["exact-match"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a filter that does not cover the repository",
            "unmatched-app",
            vec![rule(&active, &["signed-*"])],
            vec![],
        ),
        (
            "a second filter of the same rule covering the repository",
            "second-filter",
            vec![rule(&active, &["other-*", "second-*"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "two rules naming different profiles, both covering the repository",
            "two-profiles",
            vec![rule(&active, &["two-*"]), rule(&canceled, &["*profiles"])],
            vec![
                (active.clone(), Outcome::Complete),
                (canceled.clone(), Outcome::Failed),
            ],
        ),
        (
            "two rules naming ONE profile, which is one signing of the image",
            "one-profile",
            vec![rule(&active, &["one-*"]), rule(&active, &["*profile"])],
            vec![(active.clone(), Outcome::Complete)],
        ),
        (
            "a rule naming a profile AWS Signer does not hold",
            "missing-profile",
            vec![rule(&missing, &[])],
            vec![(missing, Outcome::Failed)],
        ),
        (
            "a rule naming a canceled profile, which cannot sign",
            "canceled-profile",
            vec![rule(&canceled, &[])],
            vec![(canceled, Outcome::Failed)],
        ),
        (
            "a rule naming another account's profile of the same name",
            "foreign-account",
            vec![rule(&foreign_account, &[])],
            vec![(foreign_account, Outcome::Failed)],
        ),
        (
            "a rule naming another Region's profile of the same name",
            "foreign-region",
            vec![rule(&foreign_region, &[])],
            vec![(foreign_region, Outcome::Failed)],
        ),
        (
            "a configuration with no rules at all",
            "no-rules",
            vec![],
            vec![],
        ),
    ]
}

/// A push under the registry's signing configuration records one job per matched profile,
/// each reported `IN_PROGRESS` straight after the push and settling on the clock to the
/// outcome the profile it names gives it.
///
/// Every row runs on its own emulator. The pushes are all made first and the owed settling
/// is waited for ONCE, so the suite waits one span rather than one per row.
#[tokio::test]
async fn a_push_under_a_signing_rule_reports_one_status_per_profile() {
    let mut pushed: Vec<(Row, MultiServiceRouter, String)> = Vec::new();

    for row in rows() {
        let (case, repository, rules, expected) = row.clone();
        let router = emulator_with_signer();
        given_signing_profile(&router, case, ACTIVE_PROFILE, false).await;
        given_signing_profile(&router, case, CANCELED_PROFILE, true).await;
        given_configuration(&router, case, Value::Array(rules)).await;
        let digest = given_pushed_image(&router, case, repository).await;

        // Straight after the push every job the clock owes is still running, whatever
        // outcome it is going to settle on.
        let running = statuses(&router, case, repository, &digest).await;
        let profiles: Vec<String> = expected
            .iter()
            .map(|(profile, _)| profile.clone())
            .collect();
        assert_eq!(
            named_profiles(&running),
            profiles,
            "case {case:?}: expected one job per matched profile {profiles:?} straight \
             after the push, got {running:?}"
        );
        for (entry, (profile, _)) in running.iter().zip(expected.iter()) {
            assert_entry(case, entry, profile, IN_PROGRESS);
        }

        pushed.push((row, router, digest));
    }

    tokio::time::sleep(PAST_THE_DELAY).await;

    for ((case, repository, _, expected), router, digest) in &pushed {
        let settled = statuses(router, case, repository, digest).await;
        let profiles: Vec<String> = expected
            .iter()
            .map(|(profile, _)| profile.clone())
            .collect();
        assert_eq!(
            named_profiles(&settled),
            profiles,
            "case {case:?}: expected one job per matched profile {profiles:?} past the \
             owed settling, got {settled:?}"
        );
        for (entry, (profile, outcome)) in settled.iter().zip(expected.iter()) {
            assert_entry(case, entry, profile, outcome.status());
        }

        // Reading reports the same thing twice: the statuses are derived from the stored
        // jobs and their due instant, so reading one changes nothing.
        let again = statuses(router, case, repository, digest).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a second read must report the same statuses, got {again:?}"
        );
    }
}

/// The two reasons a job cannot sign are told apart: a profile AWS Signer does not hold and
/// one it holds but that can no longer sign carry DIFFERENT failure codes, and both name the
/// profile in their reason.
#[tokio::test]
async fn the_two_reasons_a_job_cannot_sign_report_different_codes() {
    let case = "one rule per failing profile";
    let router = emulator_with_signer();
    let canceled = given_signing_profile(&router, case, CANCELED_PROFILE, true).await;
    let missing = local_profile_arn(MISSING_PROFILE);

    given_configuration(
        &router,
        case,
        json!([rule(&canceled, &[]), rule(&missing, &[])]),
    )
    .await;
    let repository = "both-failures";
    let digest = given_pushed_image(&router, case, repository).await;
    tokio::time::sleep(PAST_THE_DELAY).await;

    let settled = statuses(&router, case, repository, &digest).await;
    let codes: Vec<String> = settled
        .iter()
        .map(|entry| {
            assert_entry(
                case,
                entry,
                at(entry, "/signingProfileArn").as_str().unwrap_or_default(),
                FAILED,
            );
            at(entry, "/failureCode")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        codes.len(),
        2,
        "case {case:?}: both rules must report a failed job, got {settled:?}"
    );
    assert_ne!(
        codes[0], codes[1],
        "case {case:?}: a profile that does not exist and one that cannot sign must report \
         different failure codes, got {codes:?}"
    );
}

/// Signing is decided at the push: a rule stored after an image was pushed signs nothing of
/// it, an image pushed after the rule is signed, and deleting the configuration does not
/// withdraw a status already recorded.
#[tokio::test]
async fn a_rule_stored_after_a_push_signs_nothing_already_held() {
    let router = emulator_with_signer();
    let case = "pushed before the rule";
    let repository = "retroactive";
    let active = given_signing_profile(&router, case, ACTIVE_PROFILE, false).await;

    let before = given_pushed_image(&router, case, repository).await;
    given_configuration(&router, case, json!([rule(&active, &["retro*"])])).await;
    tokio::time::sleep(PAST_THE_DELAY).await;
    assert_eq!(
        statuses(&router, case, repository, &before).await,
        Vec::<Value>::new(),
        "case {case:?}: an image the registry already held is not signed by a later rule"
    );

    // An image pushed after the rule is signed, and the job settles.
    let case = "pushed after the rule";
    let content = b"pushed-after-the-rule".to_vec();
    let layer = given_layer(&router, case, repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    ok(
        &router,
        case,
        "PutImage",
        &json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "sign-target",
        }),
    )
    .await;
    let after = ecr::state::manifest_digest(&manifest);
    tokio::time::sleep(PAST_THE_DELAY).await;
    let settled = statuses(&router, case, repository, &after).await;
    assert_eq!(
        named_profiles(&settled),
        vec![active.clone()],
        "case {case:?}: the job the rule asked for is reported, got {settled:?}"
    );
    assert_entry(case, &settled[0], &active, COMPLETE);

    // Removing the configuration is what a put with no rules reads as, and it does not
    // unmake a signature already made.
    let case = "configuration removed";
    ok(&router, case, "DeleteSigningConfiguration", &json!({})).await;
    let kept = statuses(&router, case, repository, &after).await;
    assert_eq!(
        kept, settled,
        "case {case:?}: a signature already made keeps its status, got {kept:?}"
    );
}

/// A repository matched by no rule of a configuration that matches ANOTHER repository gets
/// no job at all: the empty list, not a status naming the profile.
#[tokio::test]
async fn a_repository_outside_the_filter_gets_no_status() {
    let case = "one repository matched, one not";
    let router = emulator_with_signer();
    let active = given_signing_profile(&router, case, ACTIVE_PROFILE, false).await;
    given_configuration(&router, case, json!([rule(&active, &["signed-app"])])).await;

    let matched = given_pushed_image(&router, case, "signed-app").await;
    let outside = given_pushed_image(&router, case, "other-app").await;
    tokio::time::sleep(PAST_THE_DELAY).await;

    let signed = statuses(&router, case, "signed-app", &matched).await;
    assert_eq!(
        named_profiles(&signed),
        vec![active.clone()],
        "case {case:?}: the matched repository's image is signed, got {signed:?}"
    );
    assert_entry(case, &signed[0], &active, COMPLETE);
    assert_eq!(
        statuses(&router, case, "other-app", &outside).await,
        Vec::<Value>::new(),
        "case {case:?}: an image in a repository the filter does not name is not signed"
    );
}
