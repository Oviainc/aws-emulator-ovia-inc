// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ONE rule about the catalog METADATA of a lens Amazon Web
//! Services publishes: its description, the `LensVersion` its content is published at and
//! the instants it was created and last revised are that lens's OWN data, so every lens
//! carries its own and every surface that reports the lens reports that lens's.
//!
//! The rules pinned here:
//!
//! 1. `ListLenses` reports each AWS-official lens from its own catalog row: the row's
//!    `LensVersion`, `UpdatedAt` at midnight of the day that version names, `CreatedAt` at
//!    midnight of the day the lens was first published (never later than `UpdatedAt`), and
//!    the description AWS's lens catalog publishes for it — the framework lens's own
//!    published sentence, and for every other lens the catalog's short form
//!    (`state::CATALOG_DESCRIPTION_PREFIX` … `state::CATALOG_DESCRIPTION_SUFFIX`) over the
//!    subject its `LensName` names. The versions are therefore NOT one value shared by the
//!    catalog: the lenses report more than one.
//! 2. Every surface that echoes a lens's identity echoes THAT lens's row: `GetLens`, a
//!    workload's `ListLensReviews` summary and `GetLensReview`, and a review template's
//!    `GetReviewTemplateLensReview` all report the same name and version for the same lens,
//!    beside the partition-global AWS-official `LensArn`.
//! 3. A lens AWS has revised reports an `UpdatedAt` strictly later than its `CreatedAt`; a
//!    lens carried at its first published version reports the two instants equal.
//!
//! The rows differ along what the rule generalises over: rule 1 runs over the WHOLE
//! AWS-official catalog (`state::OFFICIAL_LENS_CATALOG`), so every lens is a row whatever
//! its name, version source and question set; rule 2's rows are four lenses chosen to differ
//! in where their version comes from (the framework lens's own published version, two lenses
//! published at versions of AWS's own, one carried at this emulator's content revision) and
//! in whether the lens has ever been revised; rule 3's two rows are the revised lens and an
//! unrevised one.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The client and the resource builders are the shared `super::workload_harness`.

use serde_json::Value;
use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, official_lens_arn, review_template, Answer,
    Client, SIGNED_REGION,
};

/// The lenses rule 2 drives end to end, with the version each must report. Each row's
/// version is the one its catalog row carries, read from the catalog so the wire reads are
/// checked against the one table every lens reader goes through.
///
/// (case, LensAlias)
const REVIEWED_LENSES: [(&str, &str); 4] = [
    // The framework lens: the lens a workload reviews by default, published at a version of
    // its own and revised since it was created.
    ("framework-lens", "wellarchitected"),
    // Two specialised lenses published at versions AWS states for them, neither of which is
    // the framework lens's.
    ("lens-at-its-own-published-version", "serverless"),
    ("another-lens-at-its-own-published-version", "connectedmobility"),
    // A specialised lens whose row carries this emulator's content revision.
    ("lens-at-the-catalog-content-revision", "sap"),
];

/// `ListLenses` over the AWS-official half of the catalog, at a page that holds all of it.
async fn official_summaries(client: &Client, case: &str) -> Vec<Value> {
    let answer = client
        .send("GET", "/lenses?LensType=AWS_OFFICIAL&MaxResults=50", None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLenses 200, got {answer:?}"
    );
    let summaries = answer
        .body
        .get("LensSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case:?}: one page must hold the whole official catalog, got {answer:?}"
    );
    assert_eq!(
        summaries.len(),
        state::OFFICIAL_LENS_CATALOG.len(),
        "case {case:?}: expected one summary per catalog entry, got {summaries:?}"
    );
    summaries
}

/// `GetLens`: `GET /lenses/{LensAlias}`.
async fn get_lens(client: &Client, alias: &str) -> Answer {
    client
        .send(
            "GET",
            &format!("/lenses/{}", encode_path_segment(alias)),
            None,
        )
        .await
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`.
async fn get_lens_review(client: &Client, workload_id: &str, alias: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(alias)
            ),
            None,
        )
        .await
}

/// `ListLensReviews`: `GET /workloads/{WorkloadId}/lensReviews`.
async fn lens_review_summaries(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One member of a JSON object as a string, empty when the object carries none.
fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Midnight UTC of the day `day` names, rendered the way a restJson1 `Timestamp` travels.
fn midnight_epoch_seconds(day: &str) -> f64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{day}T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("a catalog day must be a calendar day: {day:?} ({error})"));
    parsed.as_secs_f64()
}

/// The description AWS's lens catalog publishes for the lens `name` names: the framework
/// lens's own published sentence, and the catalog's short form for every other lens.
fn catalog_description(name: &str) -> String {
    match name.strip_suffix(state::LENS_NAME_SUFFIX) {
        Some(subject) => format!(
            "{}{subject}{}",
            state::CATALOG_DESCRIPTION_PREFIX,
            state::CATALOG_DESCRIPTION_SUFFIX
        ),
        None => state::FRAMEWORK_LENS_DESCRIPTION.to_string(),
    }
}

/// Rule 1, over the whole AWS-official catalog.
#[tokio::test]
async fn every_official_lens_is_listed_with_its_own_version_instants_and_description() {
    let client = Client::new();
    let summaries = official_summaries(&client, "catalog").await;

    let mut versions: Vec<String> = Vec::new();
    for official in state::OFFICIAL_LENS_CATALOG {
        let case = official.alias;
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "LensAlias") == official.alias)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the lens must be listed, got {summaries:?}")
            });

        assert_eq!(
            text(&summary, "LensVersion"),
            official.version,
            "case {case:?}: expected this lens's own version, got {summary:?}"
        );
        assert_eq!(
            text(&summary, "LensArn"),
            official_lens_arn(SIGNED_REGION, official.alias),
            "case {case:?}: expected the partition-global official ARN, got {summary:?}"
        );
        assert_eq!(
            text(&summary, "Description"),
            catalog_description(official.name),
            "case {case:?}: expected the catalog's description of this lens, got {summary:?}"
        );

        let created = summary.get("CreatedAt").and_then(Value::as_f64);
        let updated = summary.get("UpdatedAt").and_then(Value::as_f64);
        assert_eq!(
            updated,
            Some(midnight_epoch_seconds(official.version)),
            "case {case:?}: UpdatedAt is midnight of the day the version names, got {summary:?}"
        );
        assert_eq!(
            created,
            Some(midnight_epoch_seconds(
                official.first_published.unwrap_or(official.version)
            )),
            "case {case:?}: CreatedAt is midnight of the day the lens was first published, \
             got {summary:?}"
        );
        assert!(
            created.zip(updated).is_some_and(|(at, then)| at <= then),
            "case {case:?}: a lens is created no later than it is revised, got {summary:?}"
        );

        versions.push(text(&summary, "LensVersion"));
    }

    versions.sort();
    versions.dedup();
    assert!(
        versions.len() > 1,
        "case \"catalog\": a lens's version is its own, so the catalog reports more than one, \
         got {versions:?}"
    );
}

/// Rule 2, over the four lenses of [`REVIEWED_LENSES`].
#[tokio::test]
async fn every_surface_that_reports_a_lens_reports_that_lens_own_name_and_version() {
    for (case, alias) in REVIEWED_LENSES {
        let official = state::official_lens(alias)
            .unwrap_or_else(|| panic!("case {case:?}: {alias:?} is carried by the catalog"));
        let client = Client::new();

        // A workload reviewing exactly this lens, and a review template applying it.
        let mut request = minimal_create(&format!("lens-metadata-{alias}"), &format!("tok-{alias}"));
        request["Lenses"] = Value::Array(vec![Value::String(alias.to_string())]);
        let workload_id = created_id(&client.create(request).await, case);
        let template_arn =
            review_template(&client, &format!("lens-metadata-{alias}"), &[alias]).await;

        // (what the surface is called, the lens as that surface reports it)
        let mut reported: Vec<(String, Value)> = Vec::new();

        let read = get_lens(&client, alias).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetLens 200, got {read:?}"
        );
        let lens = read.body.get("Lens").cloned().unwrap_or(Value::Null);
        assert_eq!(
            text(&lens, "LensVersion"),
            official.version,
            "case {case:?}: GetLens must report this lens's own version, got {lens:?}"
        );
        assert_eq!(
            text(&lens, "Name"),
            official.name,
            "case {case:?}: GetLens must report this lens's own name, got {lens:?}"
        );
        assert_eq!(
            text(&lens, "Description"),
            catalog_description(official.name),
            "case {case:?}: GetLens must report the catalog's description, got {lens:?}"
        );
        reported.push(("GetLens".to_string(), lens));

        let summaries = lens_review_summaries(&client, &workload_id, case).await;
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "LensAlias") == alias)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the review of {alias:?} must be listed, got {summaries:?}")
            });
        reported.push(("ListLensReviews".to_string(), summary));

        let review = get_lens_review(&client, &workload_id, alias).await;
        assert_eq!(
            review.status, 200,
            "case {case:?}: expected GetLensReview 200, got {review:?}"
        );
        reported.push((
            "GetLensReview".to_string(),
            review.body.get("LensReview").cloned().unwrap_or(Value::Null),
        ));

        let template_review = client.get_review_template_lens_review(&template_arn, alias).await;
        assert_eq!(
            template_review.status, 200,
            "case {case:?}: expected GetReviewTemplateLensReview 200, got {template_review:?}"
        );
        reported.push((
            "GetReviewTemplateLensReview".to_string(),
            template_review
                .body
                .get("LensReview")
                .cloned()
                .unwrap_or(Value::Null),
        ));

        for (surface, lens) in &reported {
            let surface_case = format!("{case}/{surface}");
            assert_eq!(
                text(lens, "LensVersion"),
                official.version,
                "case {surface_case:?}: expected this lens's own version, got {lens:?}"
            );
            assert_eq!(
                text(lens, "LensArn"),
                official_lens_arn(SIGNED_REGION, alias),
                "case {surface_case:?}: expected the partition-global official ARN, got {lens:?}"
            );
            // `GetLens` names the lens in `Lens$Name`, a lens review in
            // `LensReview$LensName`: both report the lens's own full name.
            let name = match surface.as_str() {
                "GetLens" => text(lens, "Name"),
                _ => text(lens, "LensName"),
            };
            assert_eq!(
                name, official.name,
                "case {surface_case:?}: expected this lens's own name, got {lens:?}"
            );
        }
    }
}

/// Rule 3: the revised lens and an unrevised one, as `ListLenses` reports them.
#[tokio::test]
async fn a_revised_lens_reports_a_later_update_than_its_creation() {
    let client = Client::new();
    let summaries = official_summaries(&client, "revisions").await;

    // (case, LensAlias, whether the catalog carries a first-publication day of its own)
    let rows: [(&str, &str, bool); 2] = [
        ("revised", "wellarchitected", true),
        ("never-revised", "sap", false),
    ];
    for (case, alias, revised) in rows {
        let official = state::official_lens(alias)
            .unwrap_or_else(|| panic!("case {case:?}: {alias:?} is carried by the catalog"));
        assert_eq!(
            official.first_published.is_some(),
            revised,
            "case {case:?}: the row must state whether the lens was revised, got {:?}",
            official.first_published
        );

        let summary = summaries
            .iter()
            .find(|summary| text(summary, "LensAlias") == alias)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the lens must be listed, got {summaries:?}"));
        let created = summary.get("CreatedAt").and_then(Value::as_f64);
        let updated = summary.get("UpdatedAt").and_then(Value::as_f64);
        if revised {
            assert!(
                created.zip(updated).is_some_and(|(at, then)| at < then),
                "case {case:?}: a revised lens was updated after it was created, got {summary:?}"
            );
        } else {
            assert_eq!(
                created, updated,
                "case {case:?}: a lens carried at its first version reports one instant, \
                 got {summary:?}"
            );
        }
    }
}
