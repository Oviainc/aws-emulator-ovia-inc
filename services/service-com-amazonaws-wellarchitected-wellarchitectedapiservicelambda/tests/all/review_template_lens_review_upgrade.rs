// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpgradeReviewTemplateLensReview`.
//!
//! The rule pinned here, over TABLES of inputs rather than one request: a lens review of a
//! review template is a review against ONE VERSION of the lens — the version the lens's
//! content was published as when the lens was applied — so it reports that version, the
//! lens's name as of it, `CURRENT` while it is the lens's latest version and `NOT_CURRENT`
//! once a later one has been published (with the template itself reporting
//! `LENS_NOT_CURRENT` while any of its lens reviews is), and the upgrade is what moves the
//! review to the lens's latest version.
//!
//! The suites that pin the rest of that review are `super::review_template_lens_review`
//! (what it reports) and `super::review_template_lens_review_reads` (the read itself).
//! What this one adds:
//!
//! 1. The upgrade MOVES the review to the lens's latest published version, so the version,
//!    the name and the status the review reports follow it, and the template's update
//!    status goes back to `CURRENT`. Rows differ along how many versions were published
//!    after the lens was applied, the form of their version names, and whether the request
//!    carries a `ClientRequestToken`.
//! 2. An upgrade of a review already against the lens's latest version moves nothing and
//!    still answers its empty success — including for the AWS-official lenses, each of
//!    which carries exactly one version here, and for a lens this emulator holds no
//!    content for, which has no version to be at.
//! 3. A template ARN or a `LensAlias` that names no lens review of a stored template is
//!    the modelled refusal, and the refused request writes nothing.
//! 4. The upgrade is idempotent per `ClientRequestToken`: the same token with the same
//!    parameters answers the original request's result WITHOUT moving the review again,
//!    and with different parameters is the modelled 409.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the review-template seeding are the shared
//! `super::workload_harness`; the lens import and publish helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    encode_path_segment, official_lens_arn, review_template, Answer, Client,
};

/// The alias of the one AWS-official lens this emulator ships a question set for.
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

/// The members `ReviewTemplateUpdateStatus` publishes (`smithy_query(query="shape",
/// shape="ReviewTemplateUpdateStatus")`).
const TEMPLATE_CURRENT: &str = "CURRENT";
const TEMPLATE_LENS_NOT_CURRENT: &str = "LENS_NOT_CURRENT";

/// `PUT /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/upgrade`, the wire form of
/// `UpgradeReviewTemplateLensReview`. Both labels are non-greedy, so each travels
/// percent-encoded in one path segment, exactly as an SDK sends it; `ClientRequestToken` is
/// bound to no HTTP member, so it travels in the body, and a request that names none sends
/// no body at all.
async fn upgrade(client: &Client, template_arn: &str, lens: &str, token: Option<&str>) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/upgrade",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            token.map(|token| json!({ "ClientRequestToken": token })),
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

/// The `(LensVersion, LensStatus, LensName)` a lens review of `template_arn` reports — the
/// three members that say WHICH version of the lens the review is against.
async fn reviewed_lens(
    client: &Client,
    template_arn: &str,
    lens: &str,
    case: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let answer = client
        .get_review_template_lens_review(template_arn, lens)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
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

/// The `UpdateStatus` the stored review template reports.
async fn template_update_status(client: &Client, template_arn: &str, case: &str) -> Option<String> {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("ReviewTemplate")
        .and_then(|template| text(template, "UpdateStatus"))
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

/// Rule 1. A review stays against the version its lens was applied at until it is
/// upgraded, and the upgrade moves it to the lens's LATEST published version.
///
/// Rows differ along how many versions are published after the lens was applied, the form
/// of their version names, and whether the upgrade request carries a `ClientRequestToken`
/// — the last row publishing none, so the review is already against the latest version and
/// the upgrade has nothing to move.
#[tokio::test]
async fn upgrading_moves_the_review_to_the_lenss_latest_version() {
    // (case, the versions published AFTER the lens was applied, the upgrade's token)
    let rows: [(&str, &[&str], Option<&str>); 4] = [
        ("one-later-version", &["2.0"], Some("tok-one")),
        ("two-later-versions", &["2.0", "3.0"], Some("tok-two")),
        (
            "dated-later-versions",
            &["2024-01-01", "2025-01-15"],
            None,
        ),
        ("no-later-version", &[], Some("tok-none")),
    ];

    for (case, later, token) in rows {
        let client = Client::new();
        let applied_version = "1.0";
        let applied_name = format!("Lens applied for {case}");
        let document = lens_document(&applied_name, None, &[("governance", 1, 2)]).to_string();
        let arn = custom_lens_at(&client, case, &document, applied_version).await;

        // The template applies the lens as it stands, so its review is against the version
        // the lens's content is published as right now.
        let template_arn = review_template(&client, case, &[arn.as_str()]).await;
        assert_eq!(
            reviewed_lens(&client, &template_arn, &arn, case).await,
            (
                Some(applied_version.to_string()),
                Some(CURRENT.to_string()),
                Some(applied_name.clone())
            ),
            "case {case:?}: a fresh review is against the version the lens was applied at"
        );
        assert_eq!(
            template_update_status(&client, &template_arn, case).await,
            Some(TEMPLATE_CURRENT.to_string()),
            "case {case:?}: a template whose lenses are current reports CURRENT"
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
            publish_later_version(
                &client,
                &arn,
                case,
                &index.to_string(),
                &document,
                version,
            )
            .await;
            latest_version = (*version).to_string();
            latest_name = name;

            // The review does not follow the publish: it is still against the version it
            // was applied at, and now reports it as NOT_CURRENT.
            assert_eq!(
                reviewed_lens(&client, &template_arn, &arn, case).await,
                (
                    Some(applied_version.to_string()),
                    Some(NOT_CURRENT.to_string()),
                    Some(applied_name.clone())
                ),
                "case {case:?}: publishing {version} must leave the review where it was"
            );
            assert_eq!(
                template_update_status(&client, &template_arn, case).await,
                Some(TEMPLATE_LENS_NOT_CURRENT.to_string()),
                "case {case:?}: a template with a superseded lens review reports \
                 LENS_NOT_CURRENT"
            );
        }

        let upgraded = upgrade(&client, &template_arn, &arn, token).await;
        assert_eq!(
            (upgraded.status, upgraded.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected UpgradeReviewTemplateLensReview 200, got {upgraded:?}"
        );

        // The round trip: the read reports the upgraded review, against the lens's latest
        // version and named by THAT version's content.
        assert_eq!(
            reviewed_lens(&client, &template_arn, &arn, case).await,
            (
                Some(latest_version.clone()),
                Some(CURRENT.to_string()),
                Some(latest_name.clone())
            ),
            "case {case:?}: the upgrade must move the review to the lens's latest version"
        );
        assert_eq!(
            template_update_status(&client, &template_arn, case).await,
            Some(TEMPLATE_CURRENT.to_string()),
            "case {case:?}: an upgraded template reports CURRENT again"
        );

        // Upgrading again, under a token of its own, has nothing left to move.
        let settled = client
            .get_review_template_lens_review(&template_arn, &arn)
            .await;
        let again = upgrade(&client, &template_arn, &arn, Some(&format!("tok-again-{case}"))).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: an upgrade of an already current review must succeed, got {again:?}"
        );
        let after = client
            .get_review_template_lens_review(&template_arn, &arn)
            .await;
        assert_eq!(
            (after.status, &after.body),
            (settled.status, &settled.body),
            "case {case:?}: an upgrade with nothing to move must change nothing, got {after:?}"
        );
    }
}

/// Rule 2. An AWS-official lens carries exactly one version here, so a review of one is
/// already against the lens's latest version and the upgrade moves nothing; a lens this
/// emulator holds no content for has no version to be at, and the upgrade still answers its
/// empty success.
///
/// Rows differ along the lens applied, whether this emulator ships its content, and the
/// spelling the upgrade names it by (bare alias or official lens ARN).
#[tokio::test]
async fn upgrading_an_official_lens_review_answers_without_moving_it() {
    let framework_arn = official_lens_arn(REGION, FRAMEWORK_LENS);

    // (case, the lens the template applies, the spelling the upgrade names it by, whether
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

    for (case, applied, named_by, shipped) in rows {
        let client = Client::new();
        let template_arn = review_template(&client, case, &[applied]).await;
        let before = client
            .get_review_template_lens_review(&template_arn, named_by)
            .await;
        let (version, status, _) = reviewed_lens(&client, &template_arn, named_by, case).await;
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

        let upgraded = upgrade(&client, &template_arn, named_by, Some(&format!("tok-{case}"))).await;
        assert_eq!(
            (upgraded.status, upgraded.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected UpgradeReviewTemplateLensReview 200, got {upgraded:?}"
        );

        let after = client
            .get_review_template_lens_review(&template_arn, named_by)
            .await;
        assert_eq!(
            (after.status, &after.body),
            (before.status, &before.body),
            "case {case:?}: an upgrade with nothing to move must leave the review exactly \
             as it was, got {after:?}"
        );
        assert_eq!(
            template_update_status(&client, &template_arn, case).await,
            Some(TEMPLATE_CURRENT.to_string()),
            "case {case:?}: the template stays CURRENT"
        );
    }
}

/// Rule 3. The upgrade resolves a lens review of a stored template: a template ARN nothing
/// is stored under is the modelled 404, a lens the stored template does not apply is the
/// modelled 400 naming `LensAlias`, and a `LensAlias` outside its published `@length(1,
/// 128)` is refused as a label before any lookup. Either way nothing is written.
///
/// Rows differ along which label is at fault and why.
#[tokio::test]
async fn an_upgrade_that_resolves_no_lens_review_is_refused_and_writes_nothing() {
    let client = Client::new();
    let template_arn = review_template(&client, "upgrade-refusals", &[FRAMEWORK_LENS]).await;
    let absent_template = format!(
        "arn:aws:wellarchitected:{REGION}:{}:review-template/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    );
    let too_long_alias = "a".repeat(129);

    let before_template = client.get_review_template(&template_arn).await;
    let before_review = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;

    // (case, TemplateArn, LensAlias, expected status, expected error code)
    let rows: [(&str, &str, &str, u16, &str); 5] = [
        (
            "absent-template",
            absent_template.as_str(),
            FRAMEWORK_LENS,
            404,
            "ResourceNotFoundException",
        ),
        (
            "official-lens-not-applied",
            template_arn.as_str(),
            UNSHIPPED_LENS,
            400,
            "ValidationException",
        ),
        (
            "custom-lens-that-does-not-exist",
            template_arn.as_str(),
            ABSENT_LENS_ARN,
            400,
            "ValidationException",
        ),
        (
            "alias-one-past-the-published-maximum",
            template_arn.as_str(),
            too_long_alias.as_str(),
            400,
            "ValidationException",
        ),
        (
            "absent-template-and-a-lens-it-could-not-apply",
            absent_template.as_str(),
            ABSENT_LENS_ARN,
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, template, lens, status, error_code) in rows {
        let answer = upgrade(&client, template, lens, Some(&format!("tok-{case}"))).await;
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

    let after_template = client.get_review_template(&template_arn).await;
    let after_review = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    assert_eq!(
        (after_template.status, &after_template.body),
        (before_template.status, &before_template.body),
        "case \"after-the-refusals\": a refused upgrade must not touch the template, got \
         {after_template:?}"
    );
    assert_eq!(
        (after_review.status, &after_review.body),
        (before_review.status, &before_review.body),
        "case \"after-the-refusals\": a refused upgrade must not touch the lens review, got \
         {after_review:?}"
    );
}

/// Rule 4. The upgrade is idempotent per `ClientRequestToken`: a retry with the same token
/// and the same parameters answers the original request's result — so it does NOT move the
/// review onto a version published since — while the same token with different parameters
/// is the modelled 409 that upgrades nothing.
///
/// The rows of the conflict table differ along WHICH parameter the second request changes.
#[tokio::test]
async fn an_upgrade_is_idempotent_per_client_request_token() {
    let client = Client::new();
    let applied_name = "Token lens as applied";
    let document = lens_document(applied_name, None, &[("governance", 1, 2)]).to_string();
    let arn = custom_lens_at(&client, "token", &document, "1.0").await;
    // A second template applying the SAME lens, so a token spent on one names different
    // parameters when it is sent for the other.
    let template_arn = review_template(&client, "token-first", &[arn.as_str(), FRAMEWORK_LENS]).await;
    let other_template_arn = review_template(&client, "token-second", &[arn.as_str()]).await;

    let second_name = "Token lens revision one";
    let second = lens_document(second_name, None, &[("governance", 2, 2)]).to_string();
    publish_later_version(&client, &arn, "token", "second", &second, "2.0").await;

    let first = upgrade(&client, &template_arn, &arn, Some("tok-upgrade")).await;
    assert_eq!(
        first.status, 200,
        "case \"first\": expected UpgradeReviewTemplateLensReview 200, got {first:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &template_arn, &arn, "first").await,
        (
            Some("2.0".to_string()),
            Some(CURRENT.to_string()),
            Some(second_name.to_string())
        ),
        "case \"first\": the upgrade must move the review to 2.0"
    );

    // A third version is published, and the ORIGINAL request is retried: the result of the
    // original request is what comes back, so the review stays where that request left it.
    let third_name = "Token lens revision two";
    let third = lens_document(third_name, None, &[("governance", 3, 2)]).to_string();
    publish_later_version(&client, &arn, "token", "third", &third, "3.0").await;

    let replay = upgrade(&client, &template_arn, &arn, Some("tok-upgrade")).await;
    assert_eq!(
        (replay.status, &replay.body),
        (first.status, &first.body),
        "case \"replay\": a retry under the same token must answer the original result, got \
         {replay:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &template_arn, &arn, "replay").await,
        (
            Some("2.0".to_string()),
            Some(NOT_CURRENT.to_string()),
            Some(second_name.to_string())
        ),
        "case \"replay\": a replayed upgrade must not move the review again"
    );

    // (case, the TemplateArn the second request names, the LensAlias it names)
    let conflicts: [(&str, &str, &str); 2] = [
        (
            "another-template",
            other_template_arn.as_str(),
            arn.as_str(),
        ),
        ("another-lens", template_arn.as_str(), FRAMEWORK_LENS),
    ];
    for (case, template, lens) in conflicts {
        let answer = upgrade(&client, template, lens, Some("tok-upgrade")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a token reused with other parameters must be refused, got \
             {answer:?}"
        );
        assert_eq!(
            text(&answer.body, "ResourceType").as_deref(),
            Some("REVIEW_TEMPLATE"),
            "case {case:?}: the conflict names the resource the token was spent on, got \
             {answer:?}"
        );
    }
    assert_eq!(
        reviewed_lens(&client, &other_template_arn, &arn, "another-template").await,
        (
            Some("1.0".to_string()),
            Some(NOT_CURRENT.to_string()),
            Some(applied_name.to_string())
        ),
        "case \"another-template\": a refused upgrade must leave that template's review \
         where it was"
    );

    // A lens review is ONE review however the request spells its lens, so a retry that
    // names the same review by the lens's ARN rather than by its alias is the same request
    // and replays rather than colliding.
    let by_alias = upgrade(&client, &template_arn, FRAMEWORK_LENS, Some("tok-framework")).await;
    assert_eq!(
        by_alias.status, 200,
        "case \"by-alias\": expected UpgradeReviewTemplateLensReview 200, got {by_alias:?}"
    );
    let by_arn = upgrade(
        &client,
        &template_arn,
        &official_lens_arn(REGION, FRAMEWORK_LENS),
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
    let fresh = upgrade(&client, &template_arn, &arn, Some("tok-upgrade-again")).await;
    assert_eq!(
        fresh.status, 200,
        "case \"fresh-token\": expected UpgradeReviewTemplateLensReview 200, got {fresh:?}"
    );
    assert_eq!(
        reviewed_lens(&client, &template_arn, &arn, "fresh-token").await,
        (
            Some("3.0".to_string()),
            Some(CURRENT.to_string()),
            Some(third_name.to_string())
        ),
        "case \"fresh-token\": a fresh token upgrades the review to the latest version"
    );
}
