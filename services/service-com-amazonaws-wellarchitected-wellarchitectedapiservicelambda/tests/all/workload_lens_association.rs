// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AssociateLenses`.
//!
//! The rule these pin, over a TABLE of requests rather than one recorded call: "Associate a
//! lens to a workload" (`smithy_query(query="operation", operation="AssociateLenses")`)
//! records, on the workload the request names, one lens review per DISTINCT lens its
//! `LensAliases` name — a lens is one lens however it is spelled, so an alias and that
//! lens's ARN name the same review — leaving the lenses the workload already reviews as
//! they stand, and reports the change through `Workload$UpdatedAt`. Both identifiers are
//! resolved before anything is written, so a workload that is not stored is a modelled 404
//! `ResourceNotFoundException` and an alias that names no lens is the modelled 400
//! `ValidationException` naming the lens it could not get, each recording nothing,
//! and an alias naming a custom lens with no published version is the modelled 400
//! `ValidationException` naming `LensAliases` — the same resolution a create that applies
//! lenses runs. Two ceilings bound it: "Up to 10 lenses can be specified" (`LensAliases`
//! doc) bounds ONE request, checked before any per-entry work since the member publishes no
//! modelled maximum, and "A maximum of 20 lenses can be associated with a workload" (this
//! operation's doc) bounds the workload — each answered with the modelled 400, because the
//! operation models no `ServiceQuotaExceededException`.
//!
//! Every workload and custom lens is minted over HTTP and everything asserted crosses the
//! HTTP boundary: requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::time::Duration;

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::lens_resolution::field_names;
use super::workload_harness::{
    age_workload, assert_field_validation, assert_not_found, assert_unresolvable_lens, created_id,
    member, minimal_create, official_lens_arn, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The modelled `ResourceType` of a workload (`state::WORKLOAD_RESOURCE_TYPE`), as the wire
/// carries it on a failure about one.
const WORKLOAD_TYPE: &str = "WORKLOAD";

/// How far back a workload's `UpdatedAt` is moved before a request under test, so a write
/// the handler makes to it is observable whatever the clock's granularity between two
/// in-process requests.
const AGE: Duration = Duration::from_secs(5);

/// `AssociateLenses`: `PATCH /workloads/{WorkloadId}/associateLenses`.
async fn associate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

/// A workload created over HTTP reviewing `lenses`, and nothing else.
async fn created_workload(client: &Client, name: &str, lenses: &[String]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, name)
}

/// The `Workload$Lenses` a stored workload reports through `GetWorkload`, and the
/// `Workload$UpdatedAt` it reports beside them, as the epoch seconds restJson1 carries it.
async fn stored_lenses(client: &Client, workload_id: &str, case: &str) -> (Vec<String>, f64) {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    let workload = answer.body.get("Workload").cloned().unwrap_or(Value::Null);
    let lenses = workload
        .get("Lenses")
        .and_then(Value::as_array)
        .map(|lenses| {
            lenses
                .iter()
                .map(|lens| lens.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let updated_at = workload
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the workload must report UpdatedAt"));
    (lenses, updated_at)
}

/// The `LensAlias` of every lens review `ListLensReviews` reports for a workload, in the
/// order it pages over them.
async fn reviewed_lenses(client: &Client, workload_id: &str, case: &str) -> Vec<String> {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    assert!(
        answer.body.get("NextToken").is_none(),
        "case {case:?}: every review of this workload must fit one page, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary
                        .get("LensAlias")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`. The alias is
/// percent-encoded because a custom lens is named by its ARN, which carries `:` and `/`
/// inside ONE path segment.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                super::workload_harness::encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// One row of the association table: the lenses the workload is created reviewing, the
/// aliases the request names, the lens list the workload must then report (in order, as
/// sent) and how many DISTINCT reviews it must then have.
struct AssociationCase {
    case: &'static str,
    name: &'static str,
    created: Vec<String>,
    associated: Vec<String>,
    stored: Vec<String>,
    reviews: usize,
}

/// `AssociateLenses` records one review per distinct lens the request names, keeps the
/// lenses the workload already reviews, and reports the change through `UpdatedAt` — and
/// every lens it recorded is readable back through `GetWorkload`, `ListLensReviews` and
/// `GetLensReview`.
#[tokio::test]
async fn associate_lenses_records_one_review_per_distinct_lens_named() {
    // The rows differ along what the rule generalises over: how many aliases one request
    // names, which SPELLING names the lens (a bare alias or that lens's official ARN,
    // whose Region is deliberately not the request's), whether the list repeats a lens,
    // whether it names one the workload already reviews, and the order the lenses arrive
    // in.
    let rows = vec![
        AssociationCase {
            case: "one alias",
            name: "lens-associate-one",
            created: vec!["wellarchitected".to_string()],
            associated: vec!["serverless".to_string()],
            stored: vec!["wellarchitected".to_string(), "serverless".to_string()],
            reviews: 2,
        },
        AssociationCase {
            case: "five aliases in one request",
            name: "lens-associate-five",
            created: vec!["wellarchitected".to_string()],
            associated: vec![
                "serverless".to_string(),
                "iot".to_string(),
                "sap".to_string(),
                "devops".to_string(),
                "genai".to_string(),
            ],
            stored: vec![
                "wellarchitected".to_string(),
                "serverless".to_string(),
                "iot".to_string(),
                "sap".to_string(),
                "devops".to_string(),
                "genai".to_string(),
            ],
            reviews: 6,
        },
        AssociationCase {
            case: "an official lens named by its ARN",
            name: "lens-associate-arn",
            created: vec!["wellarchitected".to_string()],
            associated: vec![official_lens_arn("us-east-1", "serverless")],
            stored: vec![
                "wellarchitected".to_string(),
                official_lens_arn("us-east-1", "serverless"),
            ],
            reviews: 2,
        },
        AssociationCase {
            case: "the same lens named twice is one review",
            name: "lens-associate-duplicate",
            created: vec!["wellarchitected".to_string()],
            associated: vec![
                "serverless".to_string(),
                official_lens_arn("us-east-1", "serverless"),
            ],
            stored: vec!["wellarchitected".to_string(), "serverless".to_string()],
            reviews: 2,
        },
        AssociationCase {
            case: "a lens the workload already reviews, beside a new one",
            name: "lens-associate-already-held",
            created: vec!["wellarchitected".to_string()],
            associated: vec!["wellarchitected".to_string(), "machinelearning".to_string()],
            stored: vec![
                "wellarchitected".to_string(),
                "machinelearning".to_string(),
            ],
            reviews: 2,
        },
        AssociationCase {
            case: "a workload whose framework lens arrives second",
            name: "lens-associate-order",
            created: vec!["serverless".to_string()],
            associated: vec!["machinelearning".to_string(), "wellarchitected".to_string()],
            stored: vec![
                "serverless".to_string(),
                "machinelearning".to_string(),
                "wellarchitected".to_string(),
            ],
            reviews: 3,
        },
    ];

    for row in rows {
        let client = Client::new();
        let workload_id = created_workload(&client, row.name, &row.created).await;
        age_workload(&client, &workload_id, AGE);
        let (_, before) = stored_lenses(&client, &workload_id, row.case).await;

        let answer = associate_lenses(&client, &workload_id, &row.associated).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected AssociateLenses 200, got {answer:?}",
            row.case
        );

        let (lenses, after) = stored_lenses(&client, &workload_id, row.case).await;
        assert_eq!(
            lenses, row.stored,
            "case {:?}: the workload must report the lenses as sent, got {lenses:?}",
            row.case
        );
        assert!(
            after > before,
            "case {:?}: associating a lens must move UpdatedAt, got {after} after {before}",
            row.case
        );

        let reviews = reviewed_lenses(&client, &workload_id, row.case).await;
        assert_eq!(
            reviews.len(),
            row.reviews,
            "case {:?}: expected {} lens reviews, got {reviews:?}",
            row.case,
            row.reviews
        );

        // Every lens the request named — including one named twice and one the workload
        // already reviewed — has a review to read, under the spelling the request used.
        for lens in &row.associated {
            let answer = get_lens_review(&client, &workload_id, lens).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: the review of {lens:?} must be readable, got {answer:?}",
                row.case
            );
            let alias = answer
                .body
                .get("LensReview")
                .and_then(|review| review.get("LensAlias"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                alias, lens,
                "case {:?}: the review must echo the lens as asked for, got {answer:?}",
                row.case
            );
        }
    }
}

/// A second association of lenses the workload already reviews changes NOTHING: no lens is
/// recorded twice and `UpdatedAt` does not move, however the request spells the lens.
#[tokio::test]
async fn re_associating_a_lens_the_workload_already_reviews_changes_nothing() {
    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-associate-idempotent",
        &["wellarchitected".to_string()],
    )
    .await;

    let first = associate_lenses(&client, &workload_id, &["serverless".to_string()]).await;
    assert_eq!(
        first.status, 200,
        "the first association must be recorded, got {first:?}"
    );
    let (expected, before) = stored_lenses(&client, &workload_id, "after the first").await;
    assert_eq!(
        expected,
        vec!["wellarchitected".to_string(), "serverless".to_string()],
        "the workload must review both lenses, got {expected:?}"
    );

    // The rows are the spellings that name lenses the workload ALREADY reviews: the alias
    // it was associated under, that lens's ARN, the framework lens the create applied, and
    // every one of them at once.
    let rows = vec![
        ("the same alias", vec!["serverless".to_string()]),
        (
            "that lens's ARN",
            vec![official_lens_arn("us-east-1", "serverless")],
        ),
        (
            "the lens the create applied",
            vec!["wellarchitected".to_string()],
        ),
        (
            "every lens it already reviews",
            vec![
                "wellarchitected".to_string(),
                official_lens_arn("us-west-2", "serverless"),
                "serverless".to_string(),
            ],
        ),
    ];

    for (case, lenses) in rows {
        let answer = associate_lenses(&client, &workload_id, &lenses).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: re-associating a reviewed lens must succeed, got {answer:?}"
        );
        let (lenses, after) = stored_lenses(&client, &workload_id, case).await;
        assert_eq!(
            lenses, expected,
            "case {case:?}: the lens list must not change, got {lenses:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a request that asks for no change must not move UpdatedAt, got \
             {after} after {before}"
        );
        let reviews = reviewed_lenses(&client, &workload_id, case).await;
        assert_eq!(
            reviews.len(),
            2,
            "case {case:?}: the workload must still have exactly two reviews, got {reviews:?}"
        );
    }
}

/// A `LensAliases` entry that names no lens is the modelled 400 naming the lens it could
/// not get and records nothing; a workload that is not stored is a 404 over the `WORKLOAD`
/// type, resolved first.
#[tokio::test]
async fn an_alias_that_names_no_lens_is_refused_and_records_nothing() {
    // The rows differ along WHAT the name is: a bare word no catalog alias carries, a near
    // miss of one, a catalog alias in another case (catalog membership is exact — AWS's
    // aliases are published lowercase), an official-form ARN whose alias is outside the
    // catalog, a custom-form ARN naming no stored lens, and a custom-form ARN of another
    // account.
    let unknown: Vec<(&str, String)> = vec![
        ("bare-word", "not-a-lens".to_string()),
        ("near-miss-of-an-alias", "serverles".to_string()),
        ("alias-in-another-case", "SERVERLESS".to_string()),
        (
            "official-arn-with-unknown-alias",
            official_lens_arn("us-east-1", "not-a-lens"),
        ),
        ("custom-arn-of-no-stored-lens", ABSENT_LENS_ARN.to_string()),
        (
            "custom-arn-of-another-account",
            "arn:aws:wellarchitected:us-west-2:999999999999:lens/0123456789abcdef0123456789abcdef"
                .to_string(),
        ),
    ];

    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-associate-unknown",
        &["wellarchitected".to_string()],
    )
    .await;

    for (name, entry) in &unknown {
        // Each entry is sent alone, after an alias that does resolve, and before ones that
        // do — so the refusal depends neither on the entry's POSITION in the list nor on
        // the list's length.
        for (position, lenses) in [
            ("alone", vec![entry.clone()]),
            (
                "after-a-resolvable-lens",
                vec!["serverless".to_string(), entry.clone()],
            ),
            (
                "before-resolvable-lenses",
                vec![
                    entry.clone(),
                    "serverless".to_string(),
                    official_lens_arn("us-east-1", "iot"),
                ],
            ),
        ] {
            let case = format!("{name}-{position}");
            age_workload(&client, &workload_id, AGE);
            let (before, updated_before) = stored_lenses(&client, &workload_id, &case).await;

            let answer = associate_lenses(&client, &workload_id, &lenses).await;
            assert_unresolvable_lens(&case, &answer, entry);

            let (after, updated_after) = stored_lenses(&client, &workload_id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused association must record no lens, got {after:?}"
            );
            assert_eq!(
                updated_after, updated_before,
                "case {case:?}: a refused association must not move UpdatedAt, got \
                 {updated_after} after {updated_before}"
            );
        }
    }

    // A workload that is not stored is resolved FIRST, so a caller whose workload is gone
    // is not told its lens is — even when the request also names a lens that resolves to
    // nothing.
    let absent = "0123456789abcdef0123456789abcdef";
    for (case, lenses) in [
        ("resolvable-lens", vec!["serverless".to_string()]),
        ("unknown-lens-too", vec!["not-a-lens".to_string()]),
    ] {
        let answer = associate_lenses(&client, absent, &lenses).await;
        assert_not_found(
            case,
            &answer,
            WORKLOAD_TYPE,
            absent,
            &format!("workload with ID {absent}"),
        );
    }
}

/// A custom lens can be associated only once `CreateLensVersion` has published a version of
/// it: "The lens cannot be applied to workloads … until it's published with
/// CreateLensVersion" (`ImportLens` doc). Until then the association is the modelled 400
/// naming `LensAliases` and records nothing.
#[tokio::test]
async fn a_custom_lens_is_associable_only_once_a_version_is_published() {
    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-associate-custom",
        &["wellarchitected".to_string()],
    )
    .await;

    let published_document =
        lens_document("Associable lens", None, &[("governance", 2, 2)]).to_string();
    let draft_document = lens_document(
        "Draft lens",
        Some("a lens no version was ever published from"),
        &[("security", 1, 3)],
    )
    .to_string();
    let published_arn = imported_arn(
        &import(
            &client,
            &published_document,
            None,
            "tok-import-associable",
            None,
        )
        .await,
        "published",
    );
    let draft_arn = imported_arn(
        &import(&client, &draft_document, None, "tok-import-draft", None).await,
        "draft",
    );

    // An import still in flight: the lens EXISTS — `ImportLens` already answered its ARN —
    // and no version has been published from it, so it cannot be applied yet.
    refuse_unpublished(&client, &workload_id, "import-in-flight", &published_arn).await;
    wait_for_the_import_to_land();
    refuse_unpublished(&client, &workload_id, "landed-draft", &published_arn).await;

    let published = create_version(&client, &published_arn, "1.0", Some(true), "tok-publish").await;
    assert_eq!(
        published.status, 200,
        "CreateLensVersion must publish the draft, got {published:?}"
    );

    // Published: associable, by its ARN and by that ARN with the version pinned after it,
    // which names the same lens — so the second spelling adds no second review.
    for (case, lens, reviews) in [
        ("published-arn", published_arn.clone(), 2usize),
        ("version-pinned-arn", format!("{published_arn}/1.0"), 2),
    ] {
        let answer = associate_lenses(&client, &workload_id, std::slice::from_ref(&lens)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published custom lens must be associable, got {answer:?}"
        );
        let (lenses, _) = stored_lenses(&client, &workload_id, case).await;
        assert_eq!(
            lenses,
            vec!["wellarchitected".to_string(), published_arn.clone()],
            "case {case:?}: the workload must report the custom lens as first associated, got \
             {lenses:?}"
        );
        let found = reviewed_lenses(&client, &workload_id, case).await;
        assert_eq!(
            found.len(),
            reviews,
            "case {case:?}: expected {reviews} lens reviews, got {found:?}"
        );
        let review = get_lens_review(&client, &workload_id, &lens).await;
        assert_eq!(
            review.status, 200,
            "case {case:?}: the review of the custom lens must be readable, got {review:?}"
        );
    }

    // The lens that was never published is still refused after the other one was.
    refuse_unpublished(&client, &workload_id, "draft-after-the-publish", &draft_arn).await;
}

/// One request names at most `state::MAX_LENS_ALIASES_PER_REQUEST` lenses ("Up to 10 lenses
/// can be specified", the `LensAliases` doc), which the member itself does not state.
/// Checked at the ceiling, which is served, and one past it, which is refused and records
/// nothing. Each row runs against its OWN workload, so what the row varies is the length of
/// the request and nothing else.
#[tokio::test]
async fn the_lens_aliases_quota_bounds_one_request() {
    const MAX_ALIASES: usize = state::MAX_LENS_ALIASES_PER_REQUEST;

    // The over-the-ceiling row needs MAX_ALIASES + 1 aliases that all resolve, so the quota
    // is what it meets rather than lens resolution.
    assert!(
        state::AWS_OFFICIAL_LENS_ALIASES.len() > MAX_ALIASES,
        "the AWS-official lens catalog must carry more than {MAX_ALIASES} aliases to test the \
         per-request quota, got {}",
        state::AWS_OFFICIAL_LENS_ALIASES.len()
    );

    for (case, count) in [
        ("one-alias", 1usize),
        ("at-the-ceiling", MAX_ALIASES),
        ("one-past-the-ceiling", MAX_ALIASES + 1),
    ] {
        let client = Client::new();
        let workload_id = created_workload(
            &client,
            &format!("lens-associate-quota-{case}"),
            &["wellarchitected".to_string()],
        )
        .await;
        // Distinct aliases of lenses that exist, none of them the one the create applied,
        // so the row varies the COUNT and nothing else.
        let lenses: Vec<String> = state::AWS_OFFICIAL_LENS_ALIASES
            .iter()
            .filter(|alias| **alias != state::FRAMEWORK_LENS_ALIAS)
            .take(count)
            .map(|alias| (*alias).to_string())
            .collect();
        assert_eq!(
            lenses.len(),
            count,
            "case {case:?}: the catalog must supply {count} aliases, got {lenses:?}"
        );
        age_workload(&client, &workload_id, AGE);
        let (before, updated_before) = stored_lenses(&client, &workload_id, case).await;

        let answer = associate_lenses(&client, &workload_id, &lenses).await;

        if count <= MAX_ALIASES {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {count} aliases are within the quota, got {answer:?}"
            );
            let (after, _) = stored_lenses(&client, &workload_id, case).await;
            assert_eq!(
                after.len(),
                before.len() + count,
                "case {case:?}: every named lens must be associated, got {after:?}"
            );
        } else {
            assert_refused(case, &answer);
            let (after, updated_after) = stored_lenses(&client, &workload_id, case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused association must record no lens, got {after:?}"
            );
            assert_eq!(
                updated_after, updated_before,
                "case {case:?}: a refused association must not move UpdatedAt, got \
                 {updated_after} after {updated_before}"
            );
        }
    }
}

/// A workload is reviewed against at most `state::MAX_LENSES_ASSOCIATED_PER_WORKLOAD`
/// lenses ("A maximum of 20 lenses can be associated with a workload", the `AssociateLenses`
/// doc), however many requests attached them. Filled to the ceiling, which is served, and
/// one past it, which is refused and records nothing.
///
/// The lenses are the whole AWS-official catalog plus as many published custom lenses as it
/// takes to reach the ceiling, so the ceiling is what the last request meets rather than the
/// size of any one catalog.
#[tokio::test]
async fn the_per_workload_ceiling_bounds_the_lenses_a_workload_reviews() {
    const MAX_LENSES: usize = state::MAX_LENSES_ASSOCIATED_PER_WORKLOAD;

    let official: Vec<String> = state::AWS_OFFICIAL_LENS_ALIASES
        .iter()
        .map(|alias| (*alias).to_string())
        .collect();
    assert!(
        official.len() <= MAX_LENSES,
        "this test fills a workload from the official catalog upwards, so the catalog must not \
         already exceed {MAX_LENSES} lenses, got {}",
        official.len()
    );

    let client = Client::new();
    // One create attaches at most its own quota, so the workload starts with that many of
    // the official lenses and the rest are associated below.
    let at_create = official.len().min(state::MAX_LENSES_PER_WORKLOAD);
    let workload_id = created_workload(
        &client,
        "lens-associate-ceiling",
        &official[..at_create],
    )
    .await;

    // Every custom lens the ceiling needs, plus the ONE that must be refused past it.
    let custom_needed = MAX_LENSES - official.len() + 1;
    let mut custom: Vec<String> = Vec::with_capacity(custom_needed);
    for index in 0..custom_needed {
        let document = lens_document(
            &format!("Ceiling lens {index}"),
            None,
            &[("governance", 1, 2)],
        )
        .to_string();
        custom.push(imported_arn(
            &import(
                &client,
                &document,
                None,
                &format!("tok-import-ceiling-{index}"),
                None,
            )
            .await,
            &format!("ceiling-{index}"),
        ));
    }
    wait_for_the_import_to_land();
    for (index, arn) in custom.iter().enumerate() {
        let published = create_version(
            &client,
            arn,
            "1.0",
            Some(true),
            &format!("tok-publish-ceiling-{index}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "the custom lens {arn:?} must publish a version, got {published:?}"
        );
    }

    // Fill the workload to the ceiling: the rest of the official catalog, then the custom
    // lenses, in requests no longer than the per-request quota.
    let mut to_fill: Vec<String> = official[at_create..].to_vec();
    to_fill.extend_from_slice(&custom[..MAX_LENSES - official.len()]);
    for chunk in to_fill.chunks(state::MAX_LENS_ALIASES_PER_REQUEST) {
        let answer = associate_lenses(&client, &workload_id, chunk).await;
        assert_eq!(
            answer.status, 200,
            "filling the workload to the ceiling must be served, got {answer:?} for {chunk:?}"
        );
    }
    let (at_ceiling, updated_before) = stored_lenses(&client, &workload_id, "at-the-ceiling").await;
    assert_eq!(
        at_ceiling.len(),
        MAX_LENSES,
        "a workload at the ceiling must review {MAX_LENSES} lenses, got {at_ceiling:?}"
    );
    let reviews = reviewed_lenses(&client, &workload_id, "at-the-ceiling").await;
    assert_eq!(
        reviews.len(),
        MAX_LENSES,
        "a workload at the ceiling must report {MAX_LENSES} reviews, got {reviews:?}"
    );

    // One past it: the lens resolves and the request is within the per-request quota, so the
    // per-workload ceiling is what refuses it — and the workload is left exactly as it was.
    let extra = custom
        .last()
        .expect("the ceiling needs at least one custom lens past it")
        .clone();
    let answer = associate_lenses(&client, &workload_id, std::slice::from_ref(&extra)).await;
    assert_refused("one-past-the-ceiling", &answer);
    let (after, updated_after) = stored_lenses(&client, &workload_id, "one-past-the-ceiling").await;
    assert_eq!(
        after, at_ceiling,
        "a refused association must record no lens, got {after:?}"
    );
    assert_eq!(
        updated_after, updated_before,
        "a refused association must not move UpdatedAt, got {updated_after} after \
         {updated_before}"
    );
    let review = get_lens_review(&client, &workload_id, &extra).await;
    assert_field_validation(
        "one-past-the-ceiling",
        &review,
        "LensAlias",
        &format!("No lens review for lens alias {extra} was found in workload {workload_id}."),
    );
}

/// The refusal a request that asks for more lenses than a ceiling allows gets: the modelled
/// 400 `ValidationException` naming the member that carried the aliases. `AssociateLenses`
/// models no `ServiceQuotaExceededException`, so a ceiling is reported as a bad request.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        field_names(answer),
        vec![state::LENS_ALIASES_MEMBER.to_string()],
        "case {case:?}: the refusal must name the offending member, got {answer:?}"
    );
}

/// An association naming a custom lens that exists and has no published version is refused
/// with the 400 an association naming a lens the caller cannot reach AT ALL gets — the
/// service does not disclose which of the two it is — and records nothing.
async fn refuse_unpublished(client: &Client, workload_id: &str, case: &str, lens: &str) {
    age_workload(client, workload_id, AGE);
    let (before, updated_before) = stored_lenses(client, workload_id, case).await;

    let answer = associate_lenses(client, workload_id, &[lens.to_string()]).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an unpublished lens must be refused, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "Message"),
        format!(
            "[Validation] Failed to get lenses {lens}. User might be not authorized to \
             access lens."
        ),
        "case {case:?}: the refusal must be worded as a lens it could not get, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "Reason"),
        "",
        "case {case:?}: the refusal carries Reason unclassified, got {answer:?}"
    );
    assert_eq!(
        field_names(&answer),
        Vec::<String>::new(),
        "case {case:?}: the refusal reports no per-member finding, got {answer:?}"
    );

    let (after, updated_after) = stored_lenses(client, workload_id, case).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused association must record no lens, got {after:?}"
    );
    assert_eq!(
        updated_after, updated_before,
        "case {case:?}: a refused association must not move UpdatedAt, got {updated_after} after \
         {updated_before}"
    );
}
