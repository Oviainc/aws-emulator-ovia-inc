// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetLensVersionDifference`: the difference between two
//! published versions of one lens.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. The difference from a BASE version to a TARGET version reports every pillar and
//!    question that is not identical in the two: one present only in the target is `NEW`,
//!    one present only in the base is `DELETED`, one present in both whose content
//!    changed is `UPDATED`, and a pillar that is identical is not reported at all. Rows
//!    differ along WHAT changed between the versions — a question added, a question
//!    dropped, a question's choices rewritten, a whole pillar added or dropped, and
//!    several of those at once.
//! 2. The difference is symmetric: swapping base and target maps `NEW` to `DELETED`,
//!    `DELETED` to `NEW` and leaves `UPDATED` alone, over that same table.
//! 3. A version the request omits is the lens's LATEST published version — the one
//!    published most recently, which the caller-chosen version NAMES do not order — and
//!    that version is what `LatestLensVersion` reports.
//! 4. The AWS-official lens whose content this emulator ships differences against its own
//!    shipped version, named either by its alias or by its ARN.
//! 5. A lens or version there is no content to compare is the modelled
//!    `ResourceNotFoundException`, and the modelled `LensVersion` bounds are refused at
//!    the boundary.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`; the import and
//! publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell, so the versions this read reports are the
//! ones `CreateLensVersion` really published.

use serde_json::Value;

use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ops::types::DIFFERENCE_STATUS_VALUES;
use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `GET /lenses/{LensAlias}/versionDifference`, the wire form of
/// `GetLensVersionDifference`. The alias is percent-encoded because a custom lens is
/// named by its ARN, which carries `:` and `/` inside ONE path segment; the versions
/// travel as `@httpQuery` parameters. `pub(crate)` because
/// `super::bare_lens_name_resolution` issues this operation through this one spelling of the
/// request.
pub(crate) async fn difference(
    client: &Client,
    alias: &str,
    base: Option<&str>,
    target: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(base) = base {
        query.push(format!("BaseLensVersion={base}"));
    }
    if let Some(target) = target {
        query.push(format!("TargetLensVersion={target}"));
    }
    let path = format!("/lenses/{}/versionDifference", encode_path_segment(alias));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    };
    client.send("GET", &uri, None).await
}

/// One reported pillar difference, as `(PillarId, DifferenceStatus, [(QuestionId,
/// DifferenceStatus)])` — the identity-and-status skeleton a row's expectation spells.
type ReportedPillar = (String, String, Vec<(String, String)>);

/// One expected pillar difference as a table row spells it: `(PillarId,
/// DifferenceStatus, [(QuestionId, DifferenceStatus)])`.
type ExpectedPillar<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

fn string_member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `PillarDifferences` of an answer, in the order they were reported, after checking
/// the invariants every reported difference has to satisfy: each names the pillar and the
/// question it is about, and each `DifferenceStatus` is a member of the modelled enum.
fn reported(answer: &Answer, case: &str) -> Vec<ReportedPillar> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensVersionDifference 200, got {answer:?}"
    );
    let pillars = answer
        .body
        .get("VersionDifferences")
        .and_then(|value| value.get("PillarDifferences"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: VersionDifferences.PillarDifferences must be reported, got {answer:?}")
        });
    pillars
        .iter()
        .map(|pillar| {
            let pillar_id = string_member(pillar, "PillarId");
            let pillar_name = string_member(pillar, "PillarName");
            let status = string_member(pillar, "DifferenceStatus");
            assert!(
                !pillar_id.is_empty() && !pillar_name.is_empty(),
                "case {case:?}: a pillar difference must name the pillar, got {pillar:?}"
            );
            assert!(
                DIFFERENCE_STATUS_VALUES.contains(&status.as_str()),
                "case {case:?}: {status:?} is not a modelled DifferenceStatus, got {pillar:?}"
            );
            let questions = pillar
                .get("QuestionDifferences")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a pillar difference must carry its question differences, got {pillar:?}")
                })
                .iter()
                .map(|question| {
                    let question_id = string_member(question, "QuestionId");
                    let question_status = string_member(question, "DifferenceStatus");
                    assert!(
                        !question_id.is_empty()
                            && !string_member(question, "QuestionTitle").is_empty(),
                        "case {case:?}: a question difference must name the question, got \
                         {question:?}"
                    );
                    assert!(
                        DIFFERENCE_STATUS_VALUES.contains(&question_status.as_str()),
                        "case {case:?}: {question_status:?} is not a modelled DifferenceStatus, \
                         got {question:?}"
                    );
                    (question_id, question_status)
                })
                .collect();
            (pillar_id, status, questions)
        })
        .collect()
}

/// A row's expectation, spelled as `(PillarId, DifferenceStatus, [(QuestionId,
/// DifferenceStatus)])`, lifted into what [`reported`] answers.
fn expected(rows: &[ExpectedPillar<'_>]) -> Vec<ReportedPillar> {
    rows.iter()
        .map(|(pillar_id, status, questions)| {
            (
                (*pillar_id).to_string(),
                (*status).to_string(),
                questions
                    .iter()
                    .map(|(question_id, question_status)| {
                        ((*question_id).to_string(), (*question_status).to_string())
                    })
                    .collect(),
            )
        })
        .collect()
}

/// One case of the difference table: the document each of the two versions carries, and
/// the differences the newer one has from the older.
struct Case {
    case: &'static str,
    /// `(pillar id, questions, choices per question)`, as `lens_document` takes them.
    base: &'static [(&'static str, usize, usize)],
    target: &'static [(&'static str, usize, usize)],
    differences: &'static [ExpectedPillar<'static>],
}

/// The table rules 1 and 2 are asserted over: every kind of change one version of a lens
/// can have from another.
///
/// `lens_document` names a pillar `Pillar <position>: <id>`, so a row only ever appends or
/// drops the LAST pillar — that keeps a pillar both versions carry at the same position,
/// and so leaves its NAME unchanged, which is what makes the rows about the question sets
/// they are meant to be about.
const CASES: &[Case] = &[
    Case {
        case: "identical",
        base: &[("governance", 2, 2)],
        target: &[("governance", 2, 2)],
        differences: &[],
    },
    Case {
        case: "question-added",
        base: &[("governance", 1, 2)],
        target: &[("governance", 2, 2)],
        differences: &[("governance", "UPDATED", &[("governance_q1", "NEW")])],
    },
    Case {
        case: "question-dropped",
        base: &[("governance", 3, 2)],
        target: &[("governance", 2, 2)],
        differences: &[("governance", "UPDATED", &[("governance_q2", "DELETED")])],
    },
    Case {
        case: "choices-rewritten",
        base: &[("governance", 1, 2)],
        target: &[("governance", 1, 3)],
        differences: &[("governance", "UPDATED", &[("governance_q0", "UPDATED")])],
    },
    Case {
        case: "pillar-added",
        base: &[("governance", 1, 2)],
        target: &[("governance", 1, 2), ("security", 2, 2)],
        differences: &[(
            "security",
            "NEW",
            &[("security_q0", "NEW"), ("security_q1", "NEW")],
        )],
    },
    Case {
        case: "pillar-dropped",
        base: &[("governance", 1, 2), ("security", 2, 2)],
        target: &[("governance", 1, 2)],
        differences: &[(
            "security",
            "DELETED",
            &[("security_q0", "DELETED"), ("security_q1", "DELETED")],
        )],
    },
    Case {
        case: "several-changes-at-once",
        base: &[("governance", 2, 2), ("security", 1, 2)],
        target: &[("governance", 2, 3), ("security", 1, 2), ("cost", 1, 2)],
        differences: &[
            (
                "governance",
                "UPDATED",
                &[
                    ("governance_q0", "UPDATED"),
                    ("governance_q1", "UPDATED"),
                ],
            ),
            ("cost", "NEW", &[("cost_q0", "NEW")]),
        ],
    },
];

/// The version names the table publishes the two documents under.
const BASE_VERSION: &str = "1.0";
const TARGET_VERSION: &str = "2.0";

/// Publish every row's two versions on one emulator and answer each row's lens ARN.
///
/// The imports of all the rows are issued before the one wait each import round needs, so
/// the table costs two waits rather than two per row.
async fn publish_the_table(client: &Client) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    for case in CASES {
        let document = lens_document(&format!("Lens for {}", case.case), None, case.base).to_string();
        arns.push(imported_arn(
            &import(client, &document, None, &format!("import-{}", case.case), None).await,
            case.case,
        ));
    }
    wait_for_the_import_to_land();

    for (case, arn) in CASES.iter().zip(&arns) {
        let published = create_version(
            client,
            arn,
            BASE_VERSION,
            Some(true),
            &format!("base-{}", case.case),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {:?}: the base version must publish, got {published:?}",
            case.case
        );
        let document = lens_document(
            &format!("Lens for {}", case.case),
            Some("the target version of this lens"),
            case.target,
        )
        .to_string();
        let update = import(
            client,
            &document,
            Some(arn),
            &format!("reimport-{}", case.case),
            None,
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {:?}: the target import must be accepted, got {update:?}",
            case.case
        );
    }
    wait_for_the_import_to_land();

    for (case, arn) in CASES.iter().zip(&arns) {
        let published = create_version(
            client,
            arn,
            TARGET_VERSION,
            Some(false),
            &format!("target-{}", case.case),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {:?}: the target version must publish, got {published:?}",
            case.case
        );
    }
    arns
}

/// Rule 1. Every kind of change between two published versions is reported, and nothing
/// that did not change is.
#[tokio::test]
async fn the_difference_reports_every_pillar_and_question_that_changed() {
    let client = Client::new();
    let arns = publish_the_table(&client).await;

    for (case, arn) in CASES.iter().zip(&arns) {
        let answer = difference(&client, arn, Some(BASE_VERSION), Some(TARGET_VERSION)).await;
        assert_eq!(
            reported(&answer, case.case),
            expected(case.differences),
            "case {:?}: the reported differences must be the ones the two versions have, got \
             {answer:?}",
            case.case
        );
        // The difference names the lens and the two versions it is over, beside the
        // lens's latest published version.
        assert_eq!(
            member(&answer, "LensAlias"),
            arn.as_str(),
            "case {:?}: the difference must name the lens as the request did, got {answer:?}",
            case.case
        );
        assert_eq!(
            member(&answer, "LensArn"),
            arn.as_str(),
            "case {:?}: the difference must report the lens's ARN, got {answer:?}",
            case.case
        );
        assert_eq!(
            member(&answer, "BaseLensVersion"),
            BASE_VERSION,
            "case {:?}: expected BaseLensVersion {BASE_VERSION}, got {answer:?}",
            case.case
        );
        assert_eq!(
            member(&answer, "TargetLensVersion"),
            TARGET_VERSION,
            "case {:?}: expected TargetLensVersion {TARGET_VERSION}, got {answer:?}",
            case.case
        );
        assert_eq!(
            member(&answer, "LatestLensVersion"),
            TARGET_VERSION,
            "case {:?}: the latest published version is the one published last, got {answer:?}",
            case.case
        );

        // A version compared with ITSELF differs in nothing, whatever that version holds.
        for version in [BASE_VERSION, TARGET_VERSION] {
            let same = difference(&client, arn, Some(version), Some(version)).await;
            assert!(
                reported(&same, case.case).is_empty(),
                "case {:?}: version {version} cannot differ from itself, got {same:?}",
                case.case
            );
        }
    }

    // The pillar NAME is reported as the version under comparison spells it: the table's
    // documents name a pillar after its position, which is what `lens_document` writes.
    let answer = difference(&client, &arns[4], Some(BASE_VERSION), Some(TARGET_VERSION)).await;
    let pillar = answer
        .body
        .pointer("/VersionDifferences/PillarDifferences/0")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        string_member(&pillar, "PillarName"),
        "Pillar 1: security",
        "a reported pillar must carry the name its lens version gives it, got {answer:?}"
    );
}

/// Rule 2. The difference is symmetric: swapping base and target inverts `NEW` and
/// `DELETED` and leaves `UPDATED` alone, over the same table.
#[tokio::test]
async fn swapping_base_and_target_inverts_every_difference() {
    let client = Client::new();
    let arns = publish_the_table(&client).await;

    for (case, arn) in CASES.iter().zip(&arns) {
        let forward = reported(
            &difference(&client, arn, Some(BASE_VERSION), Some(TARGET_VERSION)).await,
            case.case,
        );
        let backward = difference(&client, arn, Some(TARGET_VERSION), Some(BASE_VERSION)).await;
        let backward_reported = reported(&backward, case.case);

        let inverted: Vec<ReportedPillar> = forward
            .iter()
            .map(|(pillar_id, status, questions)| {
                (
                    pillar_id.clone(),
                    invert(status),
                    questions
                        .iter()
                        .map(|(question_id, question_status)| {
                            (question_id.clone(), invert(question_status))
                        })
                        .collect(),
                )
            })
            .collect();

        // The reported ORDER follows the target version, so the two directions are
        // compared as sets of pillars.
        assert_eq!(
            sorted(&backward_reported),
            sorted(&inverted),
            "case {:?}: the reverse difference must invert the forward one, got {backward:?}",
            case.case
        );
    }
}

/// `NEW` and `DELETED` swap when the two versions swap; `UPDATED` is its own inverse.
fn invert(status: &str) -> String {
    match status {
        "NEW" => "DELETED".to_string(),
        "DELETED" => "NEW".to_string(),
        other => other.to_string(),
    }
}

fn sorted(differences: &[ReportedPillar]) -> Vec<ReportedPillar> {
    let mut sorted: Vec<ReportedPillar> = differences
        .iter()
        .map(|(pillar_id, status, questions)| {
            let mut questions = questions.clone();
            questions.sort();
            (pillar_id.clone(), status.clone(), questions)
        })
        .collect();
    sorted.sort();
    sorted
}

/// Rule 3. An omitted version is the lens's latest published one — the version published
/// most recently, whatever its caller-chosen NAME sorts as — and that is what
/// `LatestLensVersion` reports. The rows differ along the order the version names are
/// published in.
#[tokio::test]
async fn an_omitted_version_is_the_lenss_latest_published_version() {
    // (case, the version names in publication order)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("ascending-names", vec!["1.0", "2.0"]),
        ("descending-names", vec!["9.9", "0.1"]),
        ("three-versions", vec!["b", "a", "c"]),
    ];

    for (case, versions) in cases {
        let client = Client::new();
        let first = lens_document("Latest lens", None, &[("governance", 1, 2)]).to_string();
        let arn = imported_arn(
            &import(&client, &first, None, &format!("import-{case}"), None).await,
            case,
        );
        wait_for_the_import_to_land();

        for (index, version) in versions.iter().enumerate() {
            // Each version carries one more question than the last, so a difference
            // between any two of them is non-empty and the two directions are distinct.
            let document = lens_document(
                "Latest lens",
                None,
                &[("governance", index + 1, 2)],
            )
            .to_string();
            let update = import(
                &client,
                &document,
                Some(&arn),
                &format!("reimport-{case}-{index}"),
                None,
            )
            .await;
            assert_eq!(
                update.status, 200,
                "case {case:?}: the import of version {version} must be accepted, got {update:?}"
            );
            wait_for_the_import_to_land();
            let published =
                create_version(&client, &arn, version, None, &format!("pub-{case}-{index}")).await;
            assert_eq!(
                published.status, 200,
                "case {case:?}: publishing {version} must succeed, got {published:?}"
            );
        }

        let latest = *versions
            .last()
            .expect("every row publishes at least one version");
        let oldest = versions[0];

        // Target omitted: the difference is against the latest version.
        let implied_target = difference(&client, &arn, Some(oldest), None).await;
        let explicit = difference(&client, &arn, Some(oldest), Some(latest)).await;
        assert_eq!(
            member(&implied_target, "TargetLensVersion"),
            latest,
            "case {case:?}: an omitted TargetLensVersion is the latest version, got \
             {implied_target:?}"
        );
        assert_eq!(
            reported(&implied_target, case),
            reported(&explicit, case),
            "case {case:?}: omitting the target must difference against the latest version, got \
             {implied_target:?}"
        );

        // Base omitted: the latest version is the base, which is the other direction.
        let implied_base = difference(&client, &arn, None, Some(oldest)).await;
        let reverse = difference(&client, &arn, Some(latest), Some(oldest)).await;
        assert_eq!(
            member(&implied_base, "BaseLensVersion"),
            latest,
            "case {case:?}: an omitted BaseLensVersion is the latest version, got {implied_base:?}"
        );
        assert_eq!(
            reported(&implied_base, case),
            reported(&reverse, case),
            "case {case:?}: omitting the base must difference from the latest version, got \
             {implied_base:?}"
        );

        // Both omitted: the latest version against itself, which differs in nothing.
        let neither = difference(&client, &arn, None, None).await;
        assert_eq!(
            (
                member(&neither, "BaseLensVersion"),
                member(&neither, "TargetLensVersion"),
                member(&neither, "LatestLensVersion")
            ),
            (latest, latest, latest),
            "case {case:?}: with neither version named both sides are the latest, got {neither:?}"
        );
        assert!(
            reported(&neither, case).is_empty(),
            "case {case:?}: the latest version cannot differ from itself, got {neither:?}"
        );
    }
}

/// Rule 4. The AWS-official lens this emulator ships content for differences against its
/// own shipped version, however the request names it.
#[tokio::test]
async fn the_official_lens_differences_against_its_shipped_version() {
    let client = Client::new();
    let shipped = state::FRAMEWORK_LENS_VERSION;
    // The ARN the service REPORTS for a lens AWS publishes: partition-global, with the
    // publisher in the account position. A caller may still NAME the lens by the
    // Region-qualified spelling the `LensAlias` doc shows, which resolves to the same lens.
    let reported_arn = format!(
        "arn:aws:wellarchitected::aws:lens/{}",
        state::FRAMEWORK_LENS_ALIAS
    );
    let regional_arn = format!(
        "arn:aws:wellarchitected:us-west-2::lens/{}",
        state::FRAMEWORK_LENS_ALIAS
    );

    // (case, how the request names the lens)
    let cases: Vec<(&str, String)> = vec![
        ("bare-alias", state::FRAMEWORK_LENS_ALIAS.to_string()),
        ("official-arn", reported_arn.clone()),
        ("regional-arn", regional_arn.clone()),
    ];

    for (case, alias) in &cases {
        // Named explicitly on both sides, and left to the default on both sides: the
        // emulator ships exactly one version of this lens, so either way the comparison
        // is that version against itself.
        for (sub_case, base, target) in [
            ("explicit", Some(shipped), Some(shipped)),
            ("implied", None, None),
        ] {
            let answer = difference(&client, alias, base, target).await;
            let case = format!("{case}/{sub_case}");
            assert!(
                reported(&answer, &case).is_empty(),
                "case {case:?}: the shipped version cannot differ from itself, got {answer:?}"
            );
            assert_eq!(
                (
                    member(&answer, "BaseLensVersion"),
                    member(&answer, "TargetLensVersion"),
                    member(&answer, "LatestLensVersion"),
                    member(&answer, "LensArn"),
                ),
                (shipped, shipped, shipped, reported_arn.as_str()),
                "case {case:?}: the official lens reports its shipped version and ARN, got \
                 {answer:?}"
            );
            assert_eq!(
                member(&answer, "LensAlias"),
                alias.as_str(),
                "case {case:?}: the difference must name the lens as the request did, got \
                 {answer:?}"
            );
        }

        // No other version of it exists to compare against.
        let absent = difference(&client, alias, Some(shipped), Some("1970-01-01")).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: an unshipped version of the official lens must not resolve, got \
             {absent:?}"
        );
        assert_eq!(
            absent.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {absent:?}"
        );
    }
}

/// Rule 5. A lens or a version there is no content to compare is a 404, and the modelled
/// `LensVersion` bounds are refused at the boundary. Rows differ along WHY there is
/// nothing to compare.
#[tokio::test]
async fn a_lens_or_version_with_no_content_to_compare_is_not_found() {
    let client = Client::new();
    let document = lens_document("Unpublished lens", None, &[("governance", 1, 2)]).to_string();
    let unpublished = imported_arn(
        &import(&client, &document, None, "import-unpublished", None).await,
        "unpublished",
    );
    // A custom ARN that agrees on the lens id and disagrees on the owning account names a
    // different lens, so it resolves to nothing.
    let other_account = unpublished.replacen(
        &format!(":{}:", engine::emulator_account_id()),
        ":210987654321:",
        1,
    );
    wait_for_the_import_to_land();

    let published = imported_arn(
        &import(&client, &document, None, "import-published", None).await,
        "published",
    );
    wait_for_the_import_to_land();
    let answer = create_version(&client, &published, "1.0", Some(true), "pub-published").await;
    assert_eq!(
        answer.status, 200,
        "the lens under test must have a published version, got {answer:?}"
    );

    // (case, alias, base, target)
    let cases: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
        // An Amazon Web Services official lens whose content this emulator's catalog does
        // not carry: the alias names a lens AWS publishes, and there is no version of it
        // to compare. (An official lens the catalog DOES carry has exactly one version and
        // is compared above.)
        (
            "official-lens-without-content",
            "mergersandacquisitions".to_string(),
            None,
            None,
        ),
        (
            "official-arn-without-content",
            "arn:aws:wellarchitected:us-west-2::lens/mergersandacquisitions".to_string(),
            None,
            None,
        ),
        // A bare alias that is not one Amazon Web Services publishes
        // (`state::AWS_OFFICIAL_LENS_ALIASES`) names no lens at all.
        ("not-an-official-alias", "notalens".to_string(), None, None),
        ("unknown-custom-arn", ABSENT_LENS_ARN.to_string(), None, None),
        ("another-account", other_account, None, None),
        // A lens whose content no version has been published from has no version to
        // compare at all.
        ("no-published-version", unpublished.clone(), None, None),
        (
            "no-published-version-named",
            unpublished.clone(),
            Some("1.0"),
            Some("2.0"),
        ),
        // A lens that HAS a published version, asked for one it does not carry.
        (
            "unpublished-base",
            published.clone(),
            Some("0.9"),
            Some("1.0"),
        ),
        (
            "unpublished-target",
            published.clone(),
            Some("1.0"),
            Some("1.1"),
        ),
    ];

    for (case, alias, base, target) in &cases {
        let answer = difference(&client, alias, *base, *target).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "LENS",
            "case {case:?}: expected ResourceType LENS, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            alias.as_str(),
            "case {case:?}: the refusal must name the lens as the request did, got {answer:?}"
        );
    }

    // `BaseLensVersion` and `TargetLensVersion` are `@length(min: 1, max: 32)`
    // (`smithy_query(query="operation", operation="GetLensVersionDifference")`): one
    // character past the ceiling is refused at the boundary, before the handler.
    let too_long = "9".repeat(33);
    for (case, base, target) in [
        ("base-one-past-the-ceiling", Some(too_long.as_str()), None),
        ("target-one-past-the-ceiling", None, Some(too_long.as_str())),
    ] {
        let answer = difference(&client, &published, base, target).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a LensVersion outside 1..32 must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}
