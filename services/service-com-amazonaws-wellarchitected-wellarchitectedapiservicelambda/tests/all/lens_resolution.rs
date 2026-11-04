// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the lenses a request APPLIES: how `CreateReviewTemplate` and
//! `CreateWorkload` resolve every entry of their `Lenses` list.
//!
//! The rule pinned here, over a TABLE of inputs rather than one request: a lens is applied
//! BY NAME — "Each lens is identified by its `LensSummary$LensAlias`" (`WorkloadLenses`
//! doc) — so every entry of a `Lenses` list has to name a lens the service holds: a lens
//! Amazon Web Services publishes (named by its alias or by its official ARN,
//! `state::AWS_OFFICIAL_LENS_ALIASES`) or a custom lens of this account a version has been
//! published from ("The lens cannot be applied to workloads … until it's published with
//! CreateLensVersion", `ImportLens` doc). A create whose list carries an entry that names
//! no such lens records NOTHING and is refused:
//!
//! * a `400 ValidationException` naming the lens the service could not get, carrying the
//!   entry AS SENT, when the name resolves to no lens at all — the name is a VALUE of the
//!   request, so it is refused as input and not reported as a missing resource;
//! * a `400 ValidationException` naming `Lenses`, when the entry names a custom lens that
//!   exists and has no published version — the lens resolved, so what the request got wrong
//!   is the member value.
//!
//! The lens list a `CreateWorkload` request attaches is bounded by the emulator's own
//! `state::MAX_LENSES_PER_WORKLOAD` quota, the counterpart of the review template's, which
//! is what keeps the resolution above bounded by something other than the request body
//! size; it is pinned here too.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import and
//! publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    assert_unresolvable_lens, minimal_create, official_lens_arn, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The members a `ValidationException` reported as having failed, in the order it reports
/// them.
pub(crate) fn field_names(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("Fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    field
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `CreateReviewTemplate` request applying `lenses`.
fn template_request(name: &str, token: &str, lenses: &[String]) -> Value {
    json!({
        "TemplateName": name,
        "Description": "a review template under lens resolution test",
        "Lenses": lenses,
        "ClientRequestToken": token,
    })
}

/// A `CreateWorkload` request applying `lenses`.
fn workload_request(name: &str, token: &str, lenses: &[String]) -> Value {
    let mut request = minimal_create(name, token);
    request["Lenses"] = json!(lenses);
    request
}

/// How many review templates the store holds, read off the shared state — the assertion
/// is about what a refused create RECORDED, not about a list projection of it; every
/// request the test asserts about still crosses the HTTP boundary. Delegated to the one
/// counter this crate's suites share (`super::workload_harness::stored_template_count`).
fn stored_templates(client: &Client) -> usize {
    super::workload_harness::stored_template_count(client)
}

/// How many workloads the store holds, the counterpart of [`stored_templates`] — the one
/// counter this crate's suites share (`super::workload_harness::stored_workload_count`).
fn stored_workloads(client: &Client) -> usize {
    super::workload_harness::stored_workload_count(client)
}

/// The `Lenses` a stored review template reports through `GetReviewTemplate`.
async fn template_lenses(client: &Client, template_arn: &str) -> Vec<String> {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "GetReviewTemplate of {template_arn:?} must answer 200, got {answer:?}"
    );
    lens_list(&answer, "ReviewTemplate")
}

/// The `Lenses` a stored workload reports through `GetWorkload`.
async fn workload_lenses(client: &Client, workload_id: &str) -> Vec<String> {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "GetWorkload of {workload_id:?} must answer 200, got {answer:?}"
    );
    lens_list(&answer, "Workload")
}

fn lens_list(answer: &Answer, resource: &str) -> Vec<String> {
    answer
        .body
        .get(resource)
        .and_then(|resource| resource.get("Lenses"))
        .and_then(Value::as_array)
        .map(|lenses| {
            lenses
                .iter()
                .map(|lens| lens.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every lens of the AWS-official catalog can be applied, named either way, and the stored
/// resource reports the name AS SENT.
///
/// The rows are the WHOLE catalog (`state::AWS_OFFICIAL_LENS_ALIASES`) — including the
/// lenses this emulator ships no question content for and the ones the issue never cited —
/// times the two spellings the `LensAlias` doc gives for an official lens: the bare alias,
/// and the ARN whose account segment is empty. The ARN's Region is deliberately NOT the
/// Region the request is signed for (`us-west-2`): an official lens is AWS's, not a
/// Region's resource.
#[tokio::test]
async fn every_lens_of_the_official_catalog_can_be_applied() {
    let client = Client::new();
    for alias in state::AWS_OFFICIAL_LENS_ALIASES {
        for (form, lens) in [
            ("alias", (*alias).to_string()),
            ("official-arn", official_lens_arn("us-east-1", alias)),
        ] {
            let case = format!("{alias}-{form}");
            let lenses = vec![lens.clone()];

            let created = client
                .create_review_template(template_request(
                    &format!("tmpl-{case}"),
                    &format!("tok-tmpl-{case}"),
                    &lenses,
                ))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: a lens of the official catalog must be applicable to a review \
                 template, got {created:?}"
            );
            let template_arn = member(&created, "TemplateArn").to_string();
            assert_eq!(
                template_lenses(&client, &template_arn).await,
                lenses,
                "case {case:?}: the stored template must report the lens as sent"
            );

            let created = client
                .create(workload_request(
                    &format!("wl-{case}"),
                    &format!("tok-wl-{case}"),
                    &lenses,
                ))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: a lens of the official catalog must be applicable to a workload, \
                 got {created:?}"
            );
            let workload_id = member(&created, "WorkloadId").to_string();
            assert_eq!(
                workload_lenses(&client, &workload_id).await,
                lenses,
                "case {case:?}: the stored workload must report the lens as sent"
            );
        }
    }
}

/// A `Lenses` entry that names no lens is the modelled 400 naming the lens the service
/// could not get, and records nothing — on both the operations that apply lenses.
///
/// The rows differ along WHAT the name is: a bare word no catalog alias carries, a near
/// miss of one, a catalog alias in another case (catalog membership is exact — AWS's
/// aliases are published lowercase), an official-form ARN whose alias is outside the
/// catalog, a custom-form ARN naming no stored lens, and a custom-form ARN of another
/// account. Each is sent alone, after a lens that does resolve, and before one — so the
/// refusal does not depend on the entry's POSITION in the list or on the list's length.
#[tokio::test]
async fn a_lenses_entry_that_names_no_lens_is_refused_and_stores_nothing() {
    let unknown: Vec<(&str, String)> = vec![
        // The instance the issue reported.
        ("bare-word", "not-a-lens".to_string()),
        ("near-miss-of-an-alias", "wellarchitecte".to_string()),
        ("alias-in-another-case", "WELLARCHITECTED".to_string()),
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
    for (name, entry) in &unknown {
        for (position, lenses) in [
            ("alone", vec![entry.clone()]),
            (
                "after-a-resolvable-lens",
                vec![state::FRAMEWORK_LENS_ALIAS.to_string(), entry.clone()],
            ),
            (
                "before-resolvable-lenses",
                vec![
                    entry.clone(),
                    state::FRAMEWORK_LENS_ALIAS.to_string(),
                    official_lens_arn("us-east-1", state::FRAMEWORK_LENS_ALIAS),
                ],
            ),
        ] {
            let case = format!("{name}-{position}");

            for (operation, answer) in [
                (
                    "CreateReviewTemplate",
                    client
                        .create_review_template(template_request(
                            &format!("tmpl-{case}"),
                            &format!("tok-tmpl-{case}"),
                            &lenses,
                        ))
                        .await,
                ),
                (
                    "CreateWorkload",
                    client
                        .create(workload_request(
                            &format!("wl-{case}"),
                            &format!("tok-wl-{case}"),
                            &lenses,
                        ))
                        .await,
                ),
            ] {
                assert_unresolvable_lens(&format!("{case}/{operation}"), &answer, entry);
            }

            assert_eq!(
                stored_templates(&client),
                0,
                "case {case:?}: a refused CreateReviewTemplate must store no template"
            );
            assert_eq!(
                stored_workloads(&client),
                0,
                "case {case:?}: a refused CreateWorkload must store no workload"
            );
        }
    }
}

/// A custom lens can be applied only once `CreateLensVersion` has published a version of
/// it: "The lens cannot be applied to workloads or shared with other Amazon Web Services
/// accounts until it's published with CreateLensVersion" (`ImportLens` doc).
///
/// The rows walk one lens through the three states an import leaves it in — an import still
/// in flight, a landed draft no version was published from, and a published version — and a
/// second lens stays a draft throughout, so the accepted and the refused state are both
/// exercised against the same store. The published lens is applied under both the spellings
/// that name it: its ARN, and that ARN with the version pinned after it.
#[tokio::test]
async fn a_custom_lens_is_applicable_only_once_a_version_is_published() {
    let client = Client::new();
    let published_document =
        lens_document("Published lens", None, &[("governance", 2, 2)]).to_string();
    let draft_document = lens_document(
        "Draft lens",
        Some("a lens no version was ever published from"),
        &[("security", 1, 3), ("cost", 2, 2)],
    )
    .to_string();

    let published_arn = imported_arn(
        &import(&client, &published_document, None, "tok-import-published", None).await,
        "published",
    );
    let draft_arn = imported_arn(
        &import(&client, &draft_document, None, "tok-import-draft", None).await,
        "draft",
    );

    // An import still in flight: the lens EXISTS — `ImportLens` already answered its ARN —
    // and no version has been published from it, so it cannot be applied yet.
    refuse_unpublished(&client, "import-in-flight", &published_arn).await;

    wait_for_the_import_to_land();

    // The import landed and left the content a DRAFT.
    refuse_unpublished(&client, "landed-draft", &published_arn).await;
    refuse_unpublished(&client, "second-lens-draft", &draft_arn).await;

    let published = create_version(&client, &published_arn, "1.0", Some(true), "tok-publish").await;
    assert_eq!(
        published.status, 200,
        "CreateLensVersion must publish the draft, got {published:?}"
    );

    // Published: applicable, by its ARN and by that ARN with the version pinned after it —
    // "A lens ARN may also pin a version after the alias, which names the same lens".
    for (case, lens) in [
        ("published-arn", published_arn.clone()),
        ("version-pinned-arn", format!("{published_arn}/1.0")),
    ] {
        let lenses = vec![lens.clone()];
        let created = client
            .create_review_template(template_request(
                &format!("tmpl-{case}"),
                &format!("tok-tmpl-{case}"),
                &lenses,
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a published custom lens must be applicable to a review template, got \
             {created:?}"
        );
        let template_arn = member(&created, "TemplateArn").to_string();
        assert_eq!(
            template_lenses(&client, &template_arn).await,
            lenses,
            "case {case:?}: the stored template must report the custom lens as sent"
        );

        let created = client
            .create(workload_request(
                &format!("wl-{case}"),
                &format!("tok-wl-{case}"),
                &lenses,
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a published custom lens must be applicable to a workload, got \
             {created:?}"
        );
        let workload_id = member(&created, "WorkloadId").to_string();
        assert_eq!(
            workload_lenses(&client, &workload_id).await,
            lenses,
            "case {case:?}: the stored workload must report the custom lens as sent"
        );
    }

    // The lens that was never published is still refused after the other one was.
    refuse_unpublished(&client, "draft-after-the-publish", &draft_arn).await;
}

/// The lens list one `CreateWorkload` request attaches is bounded by
/// `state::MAX_LENSES_PER_WORKLOAD` — a quota this emulator sets, because `WorkloadLenses`
/// publishes no `@length` of its own and the number is the one the service publishes for a
/// workload lens list elsewhere ("Up to 10 lenses can be specified", the `LensAliases`
/// doc). Being a quota and not a modelled constraint, it is answered with the modelled
/// `ServiceQuotaExceededException`. Checked at the ceiling, which is accepted, and one past
/// it, which is refused and stores nothing.
#[tokio::test]
async fn the_lens_ceiling_bounds_a_workloads_lens_list() {
    const MAX_LENSES: usize = state::MAX_LENSES_PER_WORKLOAD;

    let client = Client::new();
    // The over-the-ceiling row needs MAX_LENSES + 1 lenses that all resolve, so the quota
    // is what it meets rather than lens resolution.
    assert!(
        state::AWS_OFFICIAL_LENS_ALIASES.len() > MAX_LENSES,
        "the AWS-official lens catalog must carry more than {MAX_LENSES} aliases to test the \
         ceiling, got {}",
        state::AWS_OFFICIAL_LENS_ALIASES.len()
    );
    for (case, count) in [
        ("one-lens", 1usize),
        ("at-the-ceiling", MAX_LENSES),
        ("one-past-the-ceiling", MAX_LENSES + 1),
    ] {
        // Distinct aliases of lenses that exist, so the row varies the COUNT and nothing
        // else.
        let lenses: Vec<String> = state::AWS_OFFICIAL_LENS_ALIASES
            .iter()
            .take(count)
            .map(|alias| (*alias).to_string())
            .collect();
        let workloads_before = stored_workloads(&client);
        let answer = client
            .create(workload_request(
                &format!("wl-ceiling-{case}"),
                &format!("tok-ceiling-{case}"),
                &lenses,
            ))
            .await;

        if count <= MAX_LENSES {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {count} lenses are within the quota, got {answer:?}"
            );
            let workload_id = member(&answer, "WorkloadId").to_string();
            assert_eq!(
                workload_lenses(&client, &workload_id).await,
                lenses,
                "case {case:?}: a workload at or under the ceiling must attach all {count} lenses"
            );
        } else {
            assert_eq!(
                answer.status, 402,
                "case {case:?}: more than {MAX_LENSES} lenses must meet the quota, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "QuotaCode"),
                "LensesPerWorkload",
                "case {case:?}: the refusal must name the quota that was met, got {answer:?}"
            );
            assert_eq!(
                stored_workloads(&client),
                workloads_before,
                "case {case:?}: a refused CreateWorkload must store no workload"
            );
        }
    }
}

/// Both creates refuse a lens that exists and has no published version, with a 400 naming
/// `Lenses`, and record nothing. Asserted for a review template and a workload, since both
/// apply lenses by the same rule.
async fn refuse_unpublished(client: &Client, case: &str, lens: &str) {
    let templates_before = stored_templates(client);
    let workloads_before = stored_workloads(client);
    let lenses = vec![lens.to_string()];

    for (operation, answer) in [
        (
            "CreateReviewTemplate",
            client
                .create_review_template(template_request(
                    &format!("tmpl-{case}"),
                    &format!("tok-tmpl-{case}"),
                    &lenses,
                ))
                .await,
        ),
        (
            "CreateWorkload",
            client
                .create(workload_request(
                    &format!("wl-{case}"),
                    &format!("tok-wl-{case}"),
                    &lenses,
                ))
                .await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {operation} must refuse the unpublished lens {lens:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: {operation} expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case:?}: {operation} expected Reason FIELD_VALIDATION_FAILED, got {answer:?}"
        );
        assert_eq!(
            field_names(&answer),
            vec![state::LENSES_MEMBER.to_string()],
            "case {case:?}: {operation} must report the offending member, got {answer:?}"
        );
    }

    assert_eq!(
        stored_templates(client),
        templates_before,
        "case {case:?}: a refused CreateReviewTemplate must store no template"
    );
    assert_eq!(
        stored_workloads(client),
        workloads_before,
        "case {case:?}: a refused CreateWorkload must store no workload"
    );
}
