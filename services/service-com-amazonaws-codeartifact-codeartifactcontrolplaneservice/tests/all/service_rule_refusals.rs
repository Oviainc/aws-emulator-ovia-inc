// SPDX-License-Identifier: CC0-1.0

//! The refusals and outcomes CodeArtifact decides by its OWN rules rather than by a
//! modelled constraint, over the wire.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted is the status, error code, message
//! and body a CodeArtifact client sees.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. `durationSeconds`' accepted set is DISCONTINUOUS (`0`, then 900..=43200), so its two
//!    ends are refused by two different layers: a non-zero value below the minimum fails
//!    the SERVICE's rule and is stated in the service's own sentence, while a value past
//!    the modelled `@range` ceiling fails the MODEL and keeps the shared
//!    constraint-violation sentence.
//! 2. `durationSeconds=0` ties the token to the expiration of the caller's own temporary
//!    credentials, so a caller with no session behind its credential — there being no
//!    expiry to tie it to — gets the full documented authorization period, the LONGEST a
//!    token is issued for and not the shortest.
//! 3. A conflict on a name that is unique within another resource names that scope and
//!    nothing else, and changes nothing about the resource it collided with.
//! 4. A publish into a package whose origin configuration blocks publishing is a
//!    VALIDATION failure (400) with a fixed sentence, not an authorization one: no
//!    credential change can make the request valid.
//! 5. A `targetStatus` the model admits but `UpdatePackageVersionsStatus` does not own is
//!    refused by the service's own rule, not by a value set narrower than the model's.
//! 6. A per-version batch is ALL-OR-NOTHING: one version that cannot make the transition
//!    leaves every sibling `SKIPPED`, unmoved and still re-appliable on its own.
//! 7. A version's `revision` identifies its CONTENT, so an operation that only relabels it
//!    — a status move, a disposal — leaves the revision the publish produced.
//! 8. Disposal relabels a version and retains its asset records: that operation's asset
//!    deletion is out-of-band work, not part of the call that disposed the version.
//! 9. A package group the domain does not hold is refused in the service's own sentence.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The status catalogs come from the model's generated constants and from the service's own
// constant, so a change to either moves these tables rather than leaving a hand-copied
// list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::PACKAGE_VERSION_STATUS_VALUES;
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::UPDATABLE_PACKAGE_VERSION_STATUSES;

use crate::domain_repository_lifecycle::{create_repository, describe_repository};
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT};
use crate::package_group_hierarchy::{create_package_group, describe_package_group};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};
use crate::update_operations_persist::{
    dispose_versions, put_origin_configuration, update_versions_status,
};

/// The published minimum and maximum of `durationSeconds`' accepted interval: "Valid values
/// are 0 and any number between 900 (15 minutes) and 43200 (12 hours)"
/// (`GetAuthorizationTokenRequest$durationSeconds`), the maximum also being the member's
/// modelled `@range` ceiling.
const MIN_DURATION_SECONDS: i64 = 900;
const MAX_DURATION_SECONDS: i64 = 43_200;

/// The full authorization period a token is issued for when nothing narrows it:
/// "CodeArtifact authorization tokens are valid for a period of 12 hours".
const MAX_PERIOD_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// How far an asserted `expiration` may sit from `now + period`: the wall-clock cost of
/// the calls themselves, which are handler invocations rather than sleeps.
const CLOCK_SLACK_SECONDS: f64 = 60.0;

/// `GetAuthorizationToken`, with `durationSeconds` sent only when named.
async fn get_token(
    router: &MultiServiceRouter,
    domain: &str,
    duration_seconds: Option<i64>,
) -> Answer {
    let mut uri = format!("/v1/authorization-token?domain={domain}");
    if let Some(duration) = duration_seconds {
        uri.push_str(&format!("&duration={duration}"));
    }
    call(router, "POST", &uri, None).await
}

/// `DeletePackageGroup`, for the rows whose group was created and then removed.
async fn delete_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/package-group?domain={domain}&package-group={pattern}"),
        None,
    )
    .await
}

/// `ListPackageVersionAssets` for the version a [`Publish`] addresses.
async fn list_assets(router: &MultiServiceRouter, request: &Publish) -> Answer {
    let mut uri = format!(
        "/v1/package/version/assets?domain={}&repository={}&format={}&package={}&version={}",
        request.domain, request.repository, request.format, request.package, request.version
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    call(router, "POST", &uri, Some(json!({}))).await
}

/// Now, as epoch seconds.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The `status` and `revision` `DescribePackageVersion` reports for a version.
fn described(answer: &Answer) -> (Option<String>, Option<String>) {
    (
        answer.body["packageVersion"]["status"]
            .as_str()
            .map(str::to_string),
        answer.body["packageVersion"]["revision"]
            .as_str()
            .map(str::to_string),
    )
}

// ---------------------------------------------------------------------------
// RULE 1: the two ends of the accepted duration set are two different layers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_two_ends_of_the_accepted_duration_set_are_refused_by_different_layers() {
    let router = emulator();
    let domain = "duration-rules";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed before a token is requested"
    );

    // Rows inside the gap: both of its ends, a value in the middle, and the smallest
    // non-zero value there is. Each one satisfies the modelled `@range`, so what refuses it
    // is the service's own rule, in the service's own sentence.
    for duration in [1_i64, 100, 500, MIN_DURATION_SECONDS - 1] {
        let answer = get_token(&router, domain, Some(duration)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case in-gap {duration}: expected a 400 ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!(
                "Invalid durationSeconds value of {duration}: value must be 0 or between \
                 {MIN_DURATION_SECONDS} and {MAX_DURATION_SECONDS} (inclusive)."
            ),
            "case in-gap {duration}: expected the service's own sentence, got {answer:?}"
        );
        assert!(
            answer.body.get("authorizationToken").is_none(),
            "case in-gap {duration}: a refusal must carry no token, got {answer:?}"
        );
    }

    // Rows past the modelled ceiling: one past it and one far past it. These fail the
    // MODEL, so the sentence is the shared constraint-violation envelope — a different
    // sentence, which is how a client tells the two refusals apart.
    for duration in [MAX_DURATION_SECONDS + 1, 86_400, 999_999] {
        let answer = get_token(&router, domain, Some(duration)).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case above-ceiling {duration}: expected a 400 ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!(
                "1 validation error detected: Value '{duration}' at 'durationSeconds' failed \
                 to satisfy constraint: Member must have value less than or equal to \
                 {MAX_DURATION_SECONDS}"
            ),
            "case above-ceiling {duration}: expected the modelled-range sentence, got {answer:?}"
        );
    }

    // The controls: every member of the accepted set is served.
    for duration in [None, Some(0), Some(MIN_DURATION_SECONDS), Some(7_200), Some(MAX_DURATION_SECONDS)] {
        let answer = get_token(&router, domain, duration).await;
        assert_eq!(
            answer.status, 200,
            "case accepted {duration:?}: expected a 200, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: `durationSeconds=0` is the longest period, not the shortest.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_zero_duration_token_gets_the_full_authorization_period() {
    let router = emulator();
    let domain = "zero-duration-rules";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);

    // (case, durationSeconds, the period the answer must carry). The rows span the accepted
    // set: the caller-credential value, the published minimum, an hour, and no member at
    // all — so "0 is the maximum" is asserted against values on both sides of an hour.
    let cases: [(&str, Option<i64>, f64); 4] = [
        ("the caller-credential value", Some(0), MAX_PERIOD_SECONDS),
        (
            "the published minimum",
            Some(MIN_DURATION_SECONDS),
            MIN_DURATION_SECONDS as f64,
        ),
        ("an hour", Some(3_600), 3_600.0),
        ("no durationSeconds", None, MAX_PERIOD_SECONDS),
    ];

    let mut zero_expiration = 0.0_f64;
    let mut hour_expiration = 0.0_f64;
    for (case, duration, period) in cases {
        let before = now_seconds();
        let answer = get_token(&router, domain, duration).await;
        let after = now_seconds();
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected a 200, got {answer:?}"
        );
        let Some(expiration) = answer.body.get("expiration").and_then(Value::as_f64) else {
            panic!("case {case:?}: expected an epoch-seconds expiration, got {answer:?}");
        };
        assert!(
            expiration >= before + period - CLOCK_SLACK_SECONDS
                && expiration <= after + period + CLOCK_SLACK_SECONDS,
            "case {case:?}: expected an expiration about {period} seconds out, got \
             {expiration} against {before}"
        );
        if duration == Some(0) {
            zero_expiration = expiration;
        }
        if duration == Some(3_600) {
            hour_expiration = expiration;
        }
    }

    assert!(
        zero_expiration > hour_expiration + 3_000.0,
        "a zero-duration token must outlive an hour-long one by the rest of the \
         authorization period, got {zero_expiration} against {hour_expiration}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a duplicate repository name states the scope the name is unique within.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_duplicate_repository_names_the_repository_and_its_domain() {
    let router = emulator();

    // Rows differ along what the rule generalises over: the shape and length of both names
    // (the modelled extremes included) and whether the colliding repository carries
    // upstreams the second call must not clobber.
    let cases: Vec<(&str, String, String, bool)> = vec![
        ("short names", "ab".to_string(), "ab".to_string(), false),
        (
            "dotted repository name, with an upstream",
            "conflict-domain".to_string(),
            "My.Repo_1-2".to_string(),
            true,
        ),
        (
            "the longest modelled names",
            format!("d{}e", "x".repeat(48)),
            format!("R{}9", "n".repeat(98)),
            false,
        ),
    ];

    for (index, (label, domain, repository, with_upstream)) in cases.into_iter().enumerate() {
        assert_eq!(
            create_domain(&router, &domain, None).await.status,
            200,
            "case {label:?}: CreateDomain must succeed"
        );
        let upstream = format!("{}-up", &repository[..2]);
        let upstreams: Vec<&str> = if with_upstream {
            assert_eq!(
                create_repository(&router, &domain, &upstream, None, &[])
                    .await
                    .status,
                200,
                "case {label:?}: the upstream repository must be created first"
            );
            vec![upstream.as_str()]
        } else {
            Vec::new()
        };
        let created =
            create_repository(&router, &domain, &repository, Some("original"), &upstreams).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first CreateRepository must succeed, got {created:?}"
        );
        let held = describe_repository(&router, &domain, &repository).await;

        let duplicate =
            create_repository(&router, &domain, &repository, Some("CLOBBERED"), &[]).await;
        assert_eq!(
            (duplicate.status, duplicate.code()),
            (409, "ConflictException".to_string()),
            "case {label:?}: expected a 409 ConflictException, got {duplicate:?}"
        );
        assert_eq!(
            duplicate.message(),
            format!("Repository {repository} already exists in domain {domain}."),
            "case {label:?}: expected the service's conflict sentence, got {duplicate:?}"
        );

        // The refusal changed nothing: the description, the upstreams and the creation time
        // are the ones the first call stored.
        let after = describe_repository(&router, &domain, &repository).await;
        assert_eq!(
            after.body["repository"], held.body["repository"],
            "case {label:?}: a refused create must not clobber the record, got {after:?}"
        );

        // The same name is free in ANOTHER domain, which is what "unique within the
        // domain" means.
        let elsewhere = format!("elsewhere-{index}");
        assert_eq!(
            create_domain(&router, &elsewhere, None).await.status,
            200,
            "case {label:?}: the second domain must be created"
        );
        let reused = create_repository(&router, &elsewhere, &repository, None, &[]).await;
        assert_eq!(
            reused.status, 200,
            "case {label:?}: the same repository name must be free in another domain, got \
             {reused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: a publish the origin configuration blocks is a validation failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_publish_blocked_by_the_origin_configuration_is_a_validation_failure() {
    // The sentence is entirely fixed text: it names neither the package nor the
    // repository, so every row must answer exactly this.
    const BLOCKED: &str = "This package is configured to block new versions from being published.";

    // Rows differ along what the rule generalises over: the namespace of the package, the
    // shape of the version string, and the size of the asset the blocked publish carries.
    let cases: Vec<(&str, Publish, Publish)> = vec![
        (
            "the default namespace",
            Publish::new("blocked-a", "1.0.0", "first.bin", b"one".to_vec()),
            Publish::new("blocked-a", "2.0.0", "second.bin", b"two".to_vec()),
        ),
        (
            "another namespace, a pre-release version",
            Publish::new("blocked-b", "1.0.0", "first.bin", binary_content(64, 3))
                .namespace("team-b"),
            Publish::new("blocked-b", "2.0.0-rc.1", "second.bin", binary_content(2048, 7))
                .namespace("team-b"),
        ),
        (
            "a dotted package name, a date-like version",
            Publish::new("org.example.tool", "20240101", "tool.jar", b"three".to_vec())
                .namespace("com.example.tools"),
            Publish::new("org.example.tool", "20240202", "tool.jar", b"four".to_vec())
                .namespace("com.example.tools"),
        ),
    ];

    for (label, seed, attempt) in &cases {
        let router = emulator_with_repository().await;
        assert_eq!(
            publish(&router, seed).await.status,
            200,
            "case {label:?}: the seeding publish must succeed"
        );
        let configured = put_origin_configuration(&router, seed, "BLOCK", "ALLOW").await;
        assert_eq!(
            configured.status, 200,
            "case {label:?}: PutPackageOriginConfiguration must succeed, got {configured:?}"
        );

        let refused = publish(&router, attempt).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: a blocked publish is a validation failure, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            BLOCKED,
            "case {label:?}: expected the service's fixed sentence, got {refused:?}"
        );

        // It stored nothing, and the version that was already there is untouched.
        let absent = describe(&router, attempt).await;
        assert_eq!(
            (absent.status, absent.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a blocked publish must store nothing, got {absent:?}"
        );
        let (status, _) = described(&describe(&router, seed).await);
        assert_eq!(
            status.as_deref(),
            Some("Published"),
            "case {label:?}: the version already held must be untouched, got {status:?}"
        );

        // The control: lifting the restriction lets exactly the same request through, so
        // the refusal was the origin configuration and nothing else.
        let lifted = put_origin_configuration(&router, seed, "ALLOW", "ALLOW").await;
        assert_eq!(
            lifted.status, 200,
            "case {label:?}: lifting the restriction must succeed, got {lifted:?}"
        );
        let accepted = publish(&router, attempt).await;
        assert_eq!(
            accepted.status, 200,
            "case {label:?}: the same publish must succeed once publishing is allowed, got \
             {accepted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a target status this operation does not own fails the service's rule.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_target_status_this_operation_does_not_own_is_the_services_own_refusal() {
    let router = emulator_with_repository().await;
    let request = Publish::new("targets", "1.0.0", "payload.bin", b"target".to_vec());
    assert_eq!(
        publish(&router, &request).await.status,
        200,
        "the publish setup must succeed"
    );
    let (_, held_revision) = described(&describe(&router, &request).await);

    // The catalog is the model's whole `PackageVersionStatus` enum; the rows are every
    // value this operation does not own, which is where the service's own rule applies.
    let refused_targets: Vec<&&str> = PACKAGE_VERSION_STATUS_VALUES
        .iter()
        .filter(|status| !UPDATABLE_PACKAGE_VERSION_STATUSES.contains(status))
        .collect();
    assert_eq!(
        refused_targets.len(),
        3,
        "expected three statuses this operation does not own, got {refused_targets:?}"
    );

    for target in &refused_targets {
        let refused = update_versions_status(
            &router,
            &request,
            &[request.version.as_str()],
            target,
            None,
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case targetStatus {target}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            "Target status is not allowed.",
            "case targetStatus {target}: expected the service's own rule rather than a \
             value set narrower than the model's, got {refused:?}"
        );
        assert_eq!(
            described(&describe(&router, &request).await),
            (Some("Published".to_string()), held_revision.clone()),
            "case targetStatus {target}: a refused request must change nothing"
        );
    }

    // The control: the statuses this operation DOES own go through, and `Disposed` is
    // reachable through the operation that owns it.
    for target in UPDATABLE_PACKAGE_VERSION_STATUSES {
        let moved = update_versions_status(
            &router,
            &request,
            &[request.version.as_str()],
            target,
            None,
            None,
        )
        .await;
        assert_eq!(
            moved.body["successfulVersions"][request.version.as_str()]["status"].as_str(),
            Some(*target),
            "case targetStatus {target}: the owned statuses must be applied, got {moved:?}"
        );
    }
    let disposed = dispose_versions(&router, &request, &[request.version.as_str()], None, None).await;
    assert_eq!(
        disposed.body["successfulVersions"][request.version.as_str()]["status"].as_str(),
        Some("Disposed"),
        "DisposePackageVersions owns the Disposed transition, got {disposed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 6: one version that cannot move leaves its siblings SKIPPED.
// ---------------------------------------------------------------------------

/// How the one version that stops a batch fails, and the error code it must report.
#[derive(Debug, Clone, Copy)]
enum Stopper {
    /// A version the package does not hold.
    Absent,
    /// A version `DisposePackageVersions` already disposed.
    Disposed,
    /// A version whose `versionRevisions` entry pins a revision it is not at.
    PinnedRevision,
}

#[tokio::test]
async fn one_version_that_cannot_move_skips_every_sibling_in_the_same_call() {
    // Rows differ along what the rule generalises over: HOW the stopping version fails,
    // how many siblings the call carries, and whether the stopper is named first or last.
    let cases: [(&str, Stopper, &str, usize, bool); 5] = [
        ("a disposed version, one sibling, stopper first", Stopper::Disposed, "NOT_ALLOWED", 1, true),
        ("a disposed version, two siblings, stopper last", Stopper::Disposed, "NOT_ALLOWED", 2, false),
        ("an absent version, one sibling, stopper first", Stopper::Absent, "NOT_FOUND", 1, true),
        ("an absent version, three siblings, stopper last", Stopper::Absent, "NOT_FOUND", 3, false),
        (
            "a stale pinned revision, two siblings, stopper first",
            Stopper::PinnedRevision,
            "MISMATCHED_REVISION",
            2,
            true,
        ),
    ];

    for (label, stopper, expected_code, sibling_count, stopper_first) in cases {
        let router = emulator_with_repository().await;
        let package = "batched";

        // The siblings: one published version each, all of them movable on their own.
        let siblings: Vec<Publish> = (0..sibling_count)
            .map(|index| {
                Publish::new(
                    package,
                    &format!("{}.0.0", index + 2),
                    &format!("part-{index}.bin"),
                    binary_content(32 + index, index as u8),
                )
            })
            .collect();
        for sibling in &siblings {
            assert_eq!(
                publish(&router, sibling).await.status,
                200,
                "case {label:?}: publishing sibling {} must succeed",
                sibling.version
            );
        }

        // The version that stops the call, and the request that names it.
        let stopping = Publish::new(package, "1.0.0", "stopper.bin", b"stop".to_vec());
        let mut pinned: Vec<(&str, &str)> = Vec::new();
        match stopper {
            Stopper::Absent => {}
            Stopper::Disposed => {
                assert_eq!(
                    publish(&router, &stopping).await.status,
                    200,
                    "case {label:?}: the stopping version must be published first"
                );
                let disposed =
                    dispose_versions(&router, &stopping, &[stopping.version.as_str()], None, None)
                        .await;
                assert_eq!(
                    disposed.status, 200,
                    "case {label:?}: disposing the stopping version must succeed, got \
                     {disposed:?}"
                );
            }
            Stopper::PinnedRevision => {
                assert_eq!(
                    publish(&router, &stopping).await.status,
                    200,
                    "case {label:?}: the stopping version must be published first"
                );
                pinned.push((stopping.version.as_str(), "not-the-held-revision"));
            }
        }

        let mut named: Vec<&str> = siblings.iter().map(|s| s.version.as_str()).collect();
        if stopper_first {
            named.insert(0, stopping.version.as_str());
        } else {
            named.push(stopping.version.as_str());
        }

        let answer = update_versions_status(
            &router,
            &stopping,
            &named,
            "Archived",
            None,
            if pinned.is_empty() {
                None
            } else {
                Some(pinned.as_slice())
            },
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {label:?}: a per-version outcome is answered 200, got {answer:?}"
        );
        assert_eq!(
            answer.body["failedVersions"][stopping.version.as_str()]["errorCode"].as_str(),
            Some(expected_code),
            "case {label:?}: expected the stopper to fail {expected_code}, got {answer:?}"
        );
        assert!(
            answer.body["successfulVersions"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "case {label:?}: a stopped batch applies nothing, got {answer:?}"
        );

        for sibling in &siblings {
            assert_eq!(
                answer.body["failedVersions"][sibling.version.as_str()]["errorCode"].as_str(),
                Some("SKIPPED"),
                "case {label:?}: sibling {} must be SKIPPED, got {answer:?}",
                sibling.version
            );
            let (status, _) = described(&describe(&router, sibling).await);
            assert_eq!(
                status.as_deref(),
                Some("Published"),
                "case {label:?}: sibling {} must keep its status, got {status:?}",
                sibling.version
            );
        }

        // The control: the same transition, issued without the stopper, succeeds — so
        // nothing was wrong with the siblings themselves.
        let alone = update_versions_status(
            &router,
            &stopping,
            &siblings
                .iter()
                .map(|s| s.version.as_str())
                .collect::<Vec<&str>>(),
            "Archived",
            None,
            None,
        )
        .await;
        for sibling in &siblings {
            assert_eq!(
                alone.body["successfulVersions"][sibling.version.as_str()]["status"].as_str(),
                Some("Archived"),
                "case {label:?}: sibling {} must move on its own, got {alone:?}",
                sibling.version
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULES 7 and 8: relabelling a version moves neither its revision nor its assets.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn relabelling_a_version_leaves_its_content_revision_and_assets_alone() {
    // Rows differ along what the rule generalises over: the relabelling operation and its
    // target, the namespace of the package, and the size of the asset the version holds.
    let moves: Vec<(&str, Option<&str>, Publish)> = vec![
        (
            "a move to Unlisted",
            Some("Unlisted"),
            Publish::new("relabel-a", "1.0.0", "payload.bin", b"one".to_vec()),
        ),
        (
            "a move to Archived, namespaced",
            Some("Archived"),
            Publish::new("relabel-b", "2.0.0", "bundle.tar.gz", binary_content(4096, 11))
                .namespace("team-b"),
        ),
        (
            "a move to Published, which it already is",
            Some("Published"),
            Publish::new("relabel-c", "0.0.1", "p.bin", vec![0x7f]),
        ),
        (
            "a disposal",
            None,
            Publish::new("relabel-d", "3.1.4", "payload.bin", binary_content(512, 23)),
        ),
    ];

    for (label, target, request) in &moves {
        let router = emulator_with_repository().await;
        let published = publish(&router, request).await;
        assert_eq!(
            published.status, 200,
            "case {label:?}: the publish setup must succeed, got {published:?}"
        );
        let publish_revision = published.body["versionRevision"]
            .as_str()
            .expect("a publish reports the version's revision")
            .to_string();
        let assets_before = list_assets(&router, request).await;
        assert_eq!(
            assets_before.status, 200,
            "case {label:?}: the assets must list before the move, got {assets_before:?}"
        );

        let answer = match target {
            Some(target) => {
                update_versions_status(
                    &router,
                    request,
                    &[request.version.as_str()],
                    target,
                    None,
                    None,
                )
                .await
            }
            None => {
                dispose_versions(&router, request, &[request.version.as_str()], None, None).await
            }
        };
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the relabelling call must succeed, got {answer:?}"
        );

        // RULE 7: the revision the answer reports, and the one every later read reports, is
        // the one the publish produced.
        assert_eq!(
            answer.body["successfulVersions"][request.version.as_str()]["revision"].as_str(),
            Some(publish_revision.as_str()),
            "case {label:?}: expected the publish's own revision, got {answer:?}"
        );
        let (status, revision) = described(&describe(&router, request).await);
        assert_eq!(
            revision.as_deref(),
            Some(publish_revision.as_str()),
            "case {label:?}: a later read must report the same revision, got {revision:?}"
        );
        assert_eq!(
            status.as_deref(),
            Some(target.unwrap_or("Disposed")),
            "case {label:?}: the relabel itself must be committed, got {status:?}"
        );

        // RULE 8: the asset records are the ones the publish stored, whichever relabelling
        // this row made.
        let assets_after = list_assets(&router, request).await;
        assert_eq!(
            assets_after.body["assets"], assets_before.body["assets"],
            "case {label:?}: relabelling removes no asset record, got {assets_after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 9: an absent package group is refused in the service's own sentence.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_package_group_is_refused_in_the_services_own_sentence() {
    let router = emulator();
    let domain = "group-rules";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);

    // Rows differ along what the rule generalises over: the depth and format segment of the
    // pattern, and whether the group never existed or was created and then deleted.
    let cases: [(&str, &str, bool); 5] = [
        ("a generic namespace group, deleted", "/generic/team-a/*", true),
        ("an npm scope group, deleted", "/npm/acme/*", true),
        ("a format-wide group, never created", "/maven/*", false),
        ("a deep group, never created", "/npm/acme/widgets/*", false),
        ("a single-package group, deleted", "/pypi/tool", true),
    ];

    for (label, pattern, created_then_deleted) in cases {
        if created_then_deleted {
            let created = create_package_group(&router, domain, pattern, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: CreatePackageGroup must succeed, got {created:?}"
            );
            let deleted = delete_package_group(&router, domain, pattern).await;
            assert_eq!(
                deleted.status, 200,
                "case {label:?}: DeletePackageGroup must succeed, got {deleted:?}"
            );
        }

        let answer = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            format!(
                "Package Group not found not found. Package Group '{pattern}' in domain \
                 {domain} owned by account {ACCOUNT} does not exist"
            ),
            "case {label:?}: expected the service's own sentence, got {answer:?}"
        );
    }

    // The control: the group the domain DOES hold is described rather than refused.
    let root = describe_package_group(&router, domain, "/*").await;
    assert_eq!(
        root.status, 200,
        "the root group every domain is created with must be described, got {root:?}"
    );
}
