// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpgradeLensReview`.
//!
//! The rule pinned here, over TABLES of inputs rather than one request: a lens review of a
//! workload is a review against ONE VERSION of the lens — the version the lens's content
//! was published as when the lens was associated with the workload — so it reports that
//! version, the lens's name as of it, `CURRENT` while it is the lens's latest version and
//! `NOT_CURRENT` once a later one has been published; the upgrade MOVES the review to the
//! lens's latest version and keeps the review as it stood in the milestone the request
//! names, which `MilestoneName` is `@required` for.
//!
//! What each test adds:
//!
//! 1. The upgrade moves the review to the lens's latest published version, so the version,
//!    the name and the status a read of the review reports follow it, while the milestone
//!    it recorded still reports the review as it stood. Rows differ along how many versions
//!    were published after the lens was associated, the form of their version names, how
//!    the lens was associated (with the create or by `AssociateLenses`) and whether the
//!    request carries a `ClientRequestToken`.
//! 2. An AWS-official lens carries exactly one version here, so a review of one is already
//!    against the lens's latest version and the upgrade moves nothing while still recording
//!    its milestone — including for a lens this emulator holds no content for, which has no
//!    version to be at.
//! 3. A `WorkloadId` or a `LensAlias` that names no lens review, and a `MilestoneName` the
//!    workload already holds a milestone under, are each the modelled refusal, and the
//!    refused request writes nothing: neither the review nor the milestone list moves.
//! 4. The upgrade is idempotent per `ClientRequestToken`: the same token with the same
//!    parameters answers the original request's result WITHOUT moving the review again or
//!    recording a second milestone, and with different parameters is the modelled 409.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the workload seeding are the shared
//! `super::workload_harness`; the lens import and publish helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, official_lens_arn, Answer, Client,
};

/// The alias of the one AWS-official lens every workload this emulator creates reviews.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that this emulator's official-lens catalog carries no
/// content for, so it has no version a review of it could be at.
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// A lens alias AWS publishes whose content this emulator DOES ship, beside the framework
/// lens.
const SHIPPED_SPECIALISED_LENS: &str = "serverless";

/// The Region the harness signs every request for.
const REGION: &str = "us-west-2";

/// The two `LensStatus` members a lens review's currency is reported with
/// (`smithy_query(query="shape", shape="LensStatus")` publishes `CURRENT`, `NOT_CURRENT`,
/// `DEPRECATED`, `DELETED` and `UNSHARED`).
const CURRENT: &str = "CURRENT";
const NOT_CURRENT: &str = "NOT_CURRENT";

/// `PUT /workloads/{WorkloadId}/lensReviews/{LensAlias}/upgrade`, the wire form of
/// `UpgradeLensReview`. Both labels are non-greedy, so each travels percent-encoded in one
/// path segment, exactly as an SDK sends it; `MilestoneName` and `ClientRequestToken` are
/// bound to no HTTP member, so they travel in the body.
async fn upgrade(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone_name: &str,
    token: Option<&str>,
) -> Answer {
    let mut body = json!({ "MilestoneName": milestone_name });
    if let Some(token) = token {
        body["ClientRequestToken"] = json!(token);
    }
    client
        .send(
            "PUT",
            &format!(
                "/workloads/{}/lensReviews/{}/upgrade",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
}

/// One string member of a JSON object, absent when the object does not carry it.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The three members of a lens review that say WHICH version of the lens the review is
/// against: `(LensVersion, LensStatus, LensName)`.
type ReviewedLens = (Option<String>, Option<String>, Option<String>);

/// [`ReviewedLens`] as `GetLensReview` reports it — for the live review, or for the one a
/// milestone froze when `milestone` names one.
async fn reviewed_lens(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone: Option<i64>,
    case: &str,
) -> ReviewedLens {
    let query = milestone
        .map(|number| format!("?MilestoneNumber={number}"))
        .unwrap_or_default();
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}{query}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    let review = answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"));
    (
        text(&review, "LensVersion"),
        text(&review, "LensStatus"),
        text(&review, "LensName"),
    )
}

/// [`ReviewedLens`] as `ListLensReviews` reports it for the same review, so a listing and
/// a single-review read can be held to the same answer.
async fn listed_lens(client: &Client, workload_id: &str, lens: &str, case: &str) -> ReviewedLens {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    let summary = answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| text(summary, "LensAlias").as_deref() == Some(lens))
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: a summary for lens {lens:?} must be listed, got {answer:?}")
        });
    (
        text(&summary, "LensVersion"),
        text(&summary, "LensStatus"),
        text(&summary, "LensName"),
    )
}

/// The `(MilestoneNumber, MilestoneName)` of every milestone recorded against a workload,
/// in the order `ListMilestones` pages over them.
async fn milestones(client: &Client, workload_id: &str, case: &str) -> Vec<(i64, String)> {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestonesSummaries"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListMilestones 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    (
                        summary
                            .get("MilestoneNumber")
                            .and_then(Value::as_i64)
                            .unwrap_or_default(),
                        text(summary, "MilestoneName").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A workload created over HTTP reviewing `lenses`, and nothing else.
async fn created_workload(client: &Client, name: &str, lenses: &[&str]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, name)
}

/// `AssociateLenses`: `PATCH /workloads/{WorkloadId}/associateLenses`.
async fn associate_lenses(client: &Client, workload_id: &str, lenses: &[&str], case: &str) {
    let answer = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected AssociateLenses 200, got {answer:?}"
    );
}

/// Import a fresh custom lens carrying `document`, wait for the import to land and publish
/// it as `version`. Answers the lens ARN.
async fn custom_lens_at(client: &Client, case: &str, document: &str, version: &str) -> String {
    let imported = import(client, document, None, &format!("import-{case}"), None).await;
    let arn = imported_arn(&imported, case);
    wait_for_the_import_to_land();
    let published = create_version(client, &arn, version, Some(true), &format!("pub-{case}")).await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: expected CreateLensVersion 200, got {published:?}"
    );
    arn
}

/// Replace the lens's draft with `document` and publish it as `version`: the publish that
/// supersedes whatever version a review of this lens is against.
async fn publish_later_version(
    client: &Client,
    arn: &str,
    case: &str,
    label: &str,
    document: &str,
    version: &str,
) {
    let update = import(
        client,
        document,
        Some(arn),
        &format!("reimport-{case}-{label}"),
        None,
    )
    .await;
    assert_eq!(
        update.status, 200,
        "case {case:?}: the update import must be accepted, got {update:?}"
    );
    wait_for_the_import_to_land();
    let published = create_version(
        client,
        arn,
        version,
        Some(false),
        &format!("pub-{case}-{label}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: publishing {version} must succeed, got {published:?}"
    );
}

/// How the lens under test gets onto the workload — the two operations that associate one.
#[derive(Debug, Clone, Copy)]
enum Associated {
    /// Named in `CreateWorkloadInput$Lenses`.
    AtCreate,
    /// Added afterwards by `AssociateLenses`.
    ByAssociation,
}

/// Rule 1. A review stays against the version its lens was associated at until it is
/// upgraded; the upgrade moves it to the lens's LATEST published version and records the
/// milestone that keeps the review as it stood.
///
/// Rows differ along how many versions are published after the lens is associated, the form
/// of their version names, which operation associated the lens, and whether the upgrade
/// request carries a `ClientRequestToken` — the last row publishing none, so the review is
/// already against the latest version and the upgrade has nothing to move.
#[tokio::test]
async fn upgrading_moves_the_review_to_the_lenss_latest_version() {
    // (case, the versions published AFTER the lens was associated, how it was associated,
    // the upgrade's token)
    let rows: [(&str, &[&str], Associated, Option<&str>); 4] = [
        (
            "one-later-version",
            &["2.0"],
            Associated::AtCreate,
            Some("tok-one"),
        ),
        (
            "two-later-versions",
            &["2.0", "3.0"],
            Associated::AtCreate,
            None,
        ),
        (
            "dated-later-versions-associated-afterwards",
            &["2024-01-01", "2025-01-15"],
            Associated::ByAssociation,
            Some("tok-dated"),
        ),
        (
            "no-later-version",
            &[],
            Associated::ByAssociation,
            Some("tok-none"),
        ),
    ];

    for (case, later, associated, token) in rows {
        let client = Client::new();
        let applied_version = "1.0";
        let applied_name = format!("Lens applied for {case}");
        let document = lens_document(&applied_name, None, &[("governance", 1, 2)]).to_string();
        let arn = custom_lens_at(&client, case, &document, applied_version).await;

        // The workload reviews the lens as it stands, so its review is against the version
        // the lens's content is published as right now — whichever operation associated it.
        let workload_id = match associated {
            Associated::AtCreate => {
                created_workload(&client, case, &[FRAMEWORK_LENS, arn.as_str()]).await
            }
            Associated::ByAssociation => {
                let workload_id = created_workload(&client, case, &[FRAMEWORK_LENS]).await;
                associate_lenses(&client, &workload_id, &[arn.as_str()], case).await;
                workload_id
            }
        };
        let applied = (
            Some(applied_version.to_string()),
            Some(CURRENT.to_string()),
            Some(applied_name.clone()),
        );
        assert_eq!(
            reviewed_lens(&client, &workload_id, &arn, None, case).await,
            applied,
            "case {case:?}: a fresh review is against the version the lens was associated at"
        );
        assert_eq!(
            listed_lens(&client, &workload_id, &arn, case).await,
            applied,
            "case {case:?}: a listed review must agree with the single-review read"
        );

        // Every later publish supersedes the version the review is against, and names the
        // lens differently, so the name the review reports proves WHICH version it reads.
        let mut latest_version = applied_version.to_string();
        let mut latest_name = applied_name.clone();
        for (index, version) in later.iter().enumerate() {
            let name = format!("Lens {case} revision {index}");
            let document = lens_document(
                &name,
                Some("a later revision of the lens"),
                &[("governance", index + 2, 2)],
            )
            .to_string();
            publish_later_version(&client, &arn, case, &index.to_string(), &document, version).await;
            latest_version = (*version).to_string();
            latest_name = name;

            // The review does not follow the publish: it is still against the version it
            // was associated at, and now reports it as NOT_CURRENT.
            let superseded = (
                Some(applied_version.to_string()),
                Some(NOT_CURRENT.to_string()),
                Some(applied_name.clone()),
            );
            assert_eq!(
                reviewed_lens(&client, &workload_id, &arn, None, case).await,
                superseded,
                "case {case:?}: publishing {version} must leave the review where it was"
            );
            assert_eq!(
                listed_lens(&client, &workload_id, &arn, case).await,
                superseded,
                "case {case:?}: a listed review must agree about the superseded version"
            );
        }

        let milestone_name = format!("before upgrading {case}");
        let upgraded = upgrade(&client, &workload_id, &arn, &milestone_name, token).await;
        assert_eq!(
            (upgraded.status, upgraded.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected UpgradeLensReview 200, got {upgraded:?}"
        );

        // The round trip: the read reports the upgraded review, against the lens's latest
        // version and named by THAT version's content.
        let upgraded_to = (
            Some(latest_version.clone()),
            Some(CURRENT.to_string()),
            Some(latest_name.clone()),
        );
        assert_eq!(
            reviewed_lens(&client, &workload_id, &arn, None, case).await,
            upgraded_to,
            "case {case:?}: the upgrade must move the review to the lens's latest version"
        );
        assert_eq!(
            listed_lens(&client, &workload_id, &arn, case).await,
            upgraded_to,
            "case {case:?}: a listed review must agree with the upgraded read"
        );

        // The milestone the request named records the workload as it stood BEFORE the
        // move, so a read scoped to it still reports the version the review was against
        // then — reported against the lens as it stands NOW, so a version a later publish
        // superseded reads as NOT_CURRENT.
        let frozen = (
            Some(applied_version.to_string()),
            Some(
                if later.is_empty() { CURRENT } else { NOT_CURRENT }
                    .to_string(),
            ),
            Some(applied_name.clone()),
        );
        assert_eq!(
            milestones(&client, &workload_id, case).await,
            vec![(1, milestone_name.clone())],
            "case {case:?}: the upgrade must record the milestone it was asked for"
        );
        assert_eq!(
            reviewed_lens(&client, &workload_id, &arn, Some(1), case).await,
            frozen,
            "case {case:?}: the milestone must report the review as it stood before the \
             upgrade"
        );

        // Upgrading again, under a milestone and a token of its own, has nothing left to
        // move.
        let settled = reviewed_lens(&client, &workload_id, &arn, None, case).await;
        let again = upgrade(
            &client,
            &workload_id,
            &arn,
            &format!("after upgrading {case}"),
            Some(&format!("tok-again-{case}")),
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: an upgrade of an already current review must succeed, got {again:?}"
        );
        assert_eq!(
            reviewed_lens(&client, &workload_id, &arn, None, case).await,
            settled,
            "case {case:?}: an upgrade with nothing to move must leave the review where it is"
        );
        assert_eq!(
            milestones(&client, &workload_id, case)
                .await
                .into_iter()
                .map(|(number, _)| number)
                .collect::<Vec<i64>>(),
            vec![1, 2],
            "case {case:?}: every successful upgrade records the milestone it names"
        );
    }
}

/// Rule 2. An AWS-official lens carries exactly one version here, so a review of one is
/// already against the lens's latest version and the upgrade moves nothing; a lens this
/// emulator holds no content for has no version to be at, and the upgrade still answers its
/// empty success and records its milestone.
///
/// Rows differ along the lens reviewed, whether this emulator ships its content, and the
/// spelling the upgrade names it by (bare alias or official lens ARN).
#[tokio::test]
async fn upgrading_an_official_lens_review_answers_without_moving_it() {
    let framework_arn = official_lens_arn(REGION, FRAMEWORK_LENS);

    // (case, the lens the workload reviews, the spelling the upgrade names it by, whether
    // this emulator holds that lens's content)
    let rows: [(&str, &str, &str, bool); 4] = [
        ("framework-by-alias", FRAMEWORK_LENS, FRAMEWORK_LENS, true),
        (
            "framework-by-official-arn",
            FRAMEWORK_LENS,
            framework_arn.as_str(),
            true,
        ),
        (
            "specialised-lens",
            SHIPPED_SPECIALISED_LENS,
            SHIPPED_SPECIALISED_LENS,
            true,
        ),
        ("unshipped-lens", UNSHIPPED_LENS, UNSHIPPED_LENS, false),
    ];

    for (case, reviewed, named_by, shipped) in rows {
        let client = Client::new();
        let workload_id = created_workload(&client, case, &[reviewed]).await;
        let (version, status, _) = reviewed_lens(&client, &workload_id, named_by, None, case).await;
        if shipped {
            assert!(
                version.is_some() && status.as_deref() == Some(CURRENT),
                "case {case:?}: a review of a shipped lens is against its one version, got \
                 {version:?} and {status:?}"
            );
        } else {
            assert_eq!(
                (version.as_deref(), status.as_deref()),
                (None, None),
                "case {case:?}: a lens this emulator holds no content for has no version to \
                 be at"
            );
        }

        let before = reviewed_lens(&client, &workload_id, named_by, None, case).await;
        let upgraded = upgrade(
            &client,
            &workload_id,
            named_by,
            &format!("milestone for {case}"),
            Some(&format!("tok-{case}")),
        )
        .await;
        assert_eq!(
            (upgraded.status, upgraded.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected UpgradeLensReview 200, got {upgraded:?}"
        );
        assert_eq!(
            reviewed_lens(&client, &workload_id, named_by, None, case).await,
            before,
            "case {case:?}: an upgrade with nothing to move must leave the review exactly as \
             it was"
        );
        assert_eq!(
            milestones(&client, &workload_id, case).await,
            vec![(1, format!("milestone for {case}"))],
            "case {case:?}: the upgrade records the milestone it names even when the review \
             does not move"
        );
    }
}

/// Rule 3. The upgrade resolves a lens review of a stored workload and records a milestone
/// of it: a `WorkloadId` nothing is stored under, a lens the workload does not review, a
/// `MilestoneName` that workload already holds a milestone under and a label outside its
/// published length are each the modelled refusal. Either way nothing is written.
///
/// Rows differ along which member is at fault and why.
#[tokio::test]
async fn an_upgrade_that_resolves_no_lens_review_is_refused_and_writes_nothing() {
    let client = Client::new();
    let workload_id = created_workload(&client, "upgrade-refusals", &[FRAMEWORK_LENS]).await;
    let absent_workload = "0".repeat(32);
    let too_long_alias = "a".repeat(129);
    let taken_name = "the milestone already recorded";
    let recorded = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": taken_name, "ClientRequestToken": "tok-seed" })),
        )
        .await;
    assert_eq!(
        recorded.status, 200,
        "case \"seed\": expected CreateMilestone 200, got {recorded:?}"
    );

    let before_review = reviewed_lens(&client, &workload_id, FRAMEWORK_LENS, None, "before").await;
    let before_milestones = milestones(&client, &workload_id, "before").await;

    // (case, WorkloadId, LensAlias, MilestoneName, expected status, expected error code)
    let rows: [(&str, &str, &str, &str, u16, &str); 6] = [
        (
            "absent-workload",
            absent_workload.as_str(),
            FRAMEWORK_LENS,
            "a milestone of nothing",
            404,
            "ResourceNotFoundException",
        ),
        (
            "official-lens-not-reviewed",
            workload_id.as_str(),
            UNSHIPPED_LENS,
            "a milestone of an unreviewed lens",
            400,
            "ValidationException",
        ),
        (
            "custom-lens-that-does-not-exist",
            workload_id.as_str(),
            ABSENT_LENS_ARN,
            "a milestone of an absent lens",
            400,
            "ValidationException",
        ),
        (
            "milestone-name-already-taken",
            workload_id.as_str(),
            FRAMEWORK_LENS,
            taken_name,
            409,
            "ConflictException",
        ),
        (
            "milestone-name-taken-up-to-case-and-space",
            workload_id.as_str(),
            FRAMEWORK_LENS,
            "The Milestone  Already Recorded",
            409,
            "ConflictException",
        ),
        (
            "alias-one-past-the-published-maximum",
            workload_id.as_str(),
            too_long_alias.as_str(),
            "a milestone of an overlong alias",
            400,
            "ValidationException",
        ),
    ];

    for (case, workload, lens, milestone_name, status, error_code) in rows {
        let answer = upgrade(
            &client,
            workload,
            lens,
            milestone_name,
            Some(&format!("tok-{case}")),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, error_code,
            "case {case:?}: expected {error_code}, got {answer:?}"
        );
        assert!(
            answer
                .body
                .get("Message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: Message is @required on every modelled error of this operation, \
             got {answer:?}"
        );
    }

    assert_eq!(
        reviewed_lens(&client, &workload_id, FRAMEWORK_LENS, None, "after").await,
        before_review,
        "case \"after-the-refusals\": a refused upgrade must not touch the lens review"
    );
    assert_eq!(
        milestones(&client, &workload_id, "after").await,
        before_milestones,
        "case \"after-the-refusals\": a refused upgrade must record no milestone"
    );
}

/// Rule 4. The upgrade is idempotent per `ClientRequestToken`: a retry with the same token
/// and the same parameters answers the original request's result — so it neither moves the
/// review onto a version published since nor records a second milestone — while the same
/// token with different parameters is the modelled 409 that upgrades nothing.
///
/// The rows of the conflict table differ along WHICH parameter the second request changes.
#[tokio::test]
async fn an_upgrade_is_idempotent_per_client_request_token() {
    let client = Client::new();
    let applied_name = "Token lens as applied";
    let document = lens_document(applied_name, None, &[("governance", 1, 2)]).to_string();
    let arn = custom_lens_at(&client, "token", &document, "1.0").await;
    let workload_id = created_workload(&client, "token-first", &[FRAMEWORK_LENS, arn.as_str()]).await;
    // A second workload reviewing the SAME lens, so a token spent on one names different
    // parameters when it is sent for the other.
    let other_workload_id = created_workload(&client, "token-second", &[arn.as_str()]).await;

    let second_name = "Token lens revision one";
    let second = lens_document(second_name, None, &[("governance", 2, 2)]).to_string();
    publish_later_version(&client, &arn, "token", "second", &second, "2.0").await;

    let first = upgrade(&client, &workload_id, &arn, "before the first", Some("tok-upgrade")).await;
    assert_eq!(
        first.status, 200,
        "case \"first\": expected UpgradeLensReview 200, got {first:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &workload_id, &arn, None, "first").await,
        (
            Some("2.0".to_string()),
            Some(CURRENT.to_string()),
            Some(second_name.to_string())
        ),
        "case \"first\": the upgrade must move the review to 2.0"
    );

    // A third version is published, and the ORIGINAL request is retried: the result of the
    // original request is what comes back, so the review stays where that request left it
    // and the milestone it recorded stays the one milestone.
    let third_name = "Token lens revision two";
    let third = lens_document(third_name, None, &[("governance", 3, 2)]).to_string();
    publish_later_version(&client, &arn, "token", "third", &third, "3.0").await;

    let replay = upgrade(&client, &workload_id, &arn, "before the first", Some("tok-upgrade")).await;
    assert_eq!(
        (replay.status, &replay.body),
        (first.status, &first.body),
        "case \"replay\": a retry under the same token must answer the original result, got \
         {replay:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &workload_id, &arn, None, "replay").await,
        (
            Some("2.0".to_string()),
            Some(NOT_CURRENT.to_string()),
            Some(second_name.to_string())
        ),
        "case \"replay\": a replayed upgrade must not move the review again"
    );
    assert_eq!(
        milestones(&client, &workload_id, "replay").await,
        vec![(1, "before the first".to_string())],
        "case \"replay\": a replayed upgrade must not record a second milestone"
    );

    // (case, the WorkloadId the second request names, the LensAlias it names, the
    // MilestoneName it names)
    let conflicts: [(&str, &str, &str, &str); 3] = [
        (
            "another-workload",
            other_workload_id.as_str(),
            arn.as_str(),
            "before the first",
        ),
        (
            "another-lens",
            workload_id.as_str(),
            FRAMEWORK_LENS,
            "before the first",
        ),
        (
            "another-milestone-name",
            workload_id.as_str(),
            arn.as_str(),
            "a different milestone",
        ),
    ];
    for (case, workload, lens, milestone_name) in conflicts {
        let answer = upgrade(&client, workload, lens, milestone_name, Some("tok-upgrade")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a token reused with other parameters must be refused, got {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "ResourceType").as_deref(),
            Some("WORKLOAD"),
            "case {case:?}: the conflict names the resource the token was spent on, got \
             {answer:?}"
        );
    }
    assert_eq!(
        reviewed_lens(&client, &other_workload_id, &arn, None, "another-workload").await,
        (
            Some("1.0".to_string()),
            Some(NOT_CURRENT.to_string()),
            Some(applied_name.to_string())
        ),
        "case \"another-workload\": a refused upgrade must leave that workload's review \
         where it was"
    );
    assert!(
        milestones(&client, &other_workload_id, "another-workload")
            .await
            .is_empty(),
        "case \"another-workload\": a refused upgrade records no milestone"
    );

    // A lens review is ONE review however the request spells its lens, so a retry that
    // names the same review by the lens's ARN rather than by its alias is the same request
    // and replays rather than colliding.
    let by_alias = upgrade(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        "the framework milestone",
        Some("tok-framework"),
    )
    .await;
    assert_eq!(
        by_alias.status, 200,
        "case \"by-alias\": expected UpgradeLensReview 200, got {by_alias:?}"
    );
    let by_arn = upgrade(
        &client,
        &workload_id,
        &official_lens_arn(REGION, FRAMEWORK_LENS),
        "the framework milestone",
        Some("tok-framework"),
    )
    .await;
    assert_eq!(
        (by_arn.status, &by_arn.body),
        (by_alias.status, &by_alias.body),
        "case \"by-arn\": the same review named by the lens ARN is the same request, got \
         {by_arn:?}"
    );

    // A token of its own upgrades the review to the version published since.
    let fresh = upgrade(
        &client,
        &workload_id,
        &arn,
        "before the third",
        Some("tok-upgrade-again"),
    )
    .await;
    assert_eq!(
        fresh.status, 200,
        "case \"fresh-token\": expected UpgradeLensReview 200, got {fresh:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &workload_id, &arn, None, "fresh-token").await,
        (
            Some("3.0".to_string()),
            Some(CURRENT.to_string()),
            Some(third_name.to_string())
        ),
        "case \"fresh-token\": a fresh token upgrades the review to the latest version"
    );
}
