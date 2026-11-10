// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetDeploymentPatternVersion`: the deployment-pattern VERSION
//! view of the seeded catalogue in `crate::state`, driven over HTTP through the same
//! router the production emulator serves with (the harness in `tests/all/harness.rs`).
//!
//! Every assertion here is about a RULE rather than one recorded call, so each one runs
//! over a table: EVERY workload of the catalogue, every deployment pattern of each and
//! every version each pattern publishes, plus all three arms of the not-found contract
//! (an unknown workload, an unknown pattern of a known workload, and a well-formed
//! version a known pattern does not publish — in each of the two forms the modelled
//! `DeploymentPatternVersionName` allows), each published bound of that shape's length
//! and one past it, and the versions an `UpdateDeployment` may name.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    CATALOGUE_PATTERN_VERSIONS, DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE,
};

use crate::deployment_lifecycle::create;
use crate::deployment_update::{
    conforming_specifications, error_message, events, every_pattern, last_update_description,
    one_pattern_per_workload, pattern_data, update_body, with_member,
};
use crate::harness::Harness;

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s it walks, plus slack): a deployment whose create is still
/// in flight refuses an update.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// Long enough for the one step an UPDATE schedules to come due, after which the
/// deployment takes another update.
const UPDATE_SETTLE: Duration = Duration::from_millis(1_200);

/// One workload's version suite: a deployment per published version (each accepted
/// update leaves its own deployment mid-update, a state that takes no further update),
/// plus the one the refusals and the "keeps its version" pair run against.
struct VersionSuite {
    case: String,
    workload: String,
    pattern: String,
    specifications: Map<String, Value>,
    per_version: Vec<String>,
    kept: String,
}

/// The route the model binds `GetDeploymentPatternVersion` to:
/// `POST /getDeploymentPatternVersion`.
const PATH: &str = "/getDeploymentPatternVersion";

/// A `GetDeploymentPatternVersion` request body: every member of the input is required.
pub(crate) fn version_body(workload: &str, pattern: &str, version: &str) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "deploymentPatternVersionName": version,
    })
}

/// The `DeploymentPatternVersionDataSummary` the service reports for one version of one
/// pattern, requiring HTTP 200.
pub(crate) async fn get_version(
    harness: &Harness,
    case: &str,
    workload: &str,
    pattern: &str,
    version: &str,
) -> Value {
    let body = harness
        .ok(case, PATH, version_body(workload, pattern, version))
        .await;
    let summary = body["deploymentPatternVersion"].clone();
    assert!(
        summary.is_object(),
        "case {case:?}: GetDeploymentPatternVersion must report a deploymentPatternVersion, \
         got {body}"
    );
    summary
}

/// The description of the last `DeploymentUpdateInProgress` event of `id`, read through
/// `ListDeploymentEvents` — where the version a deployment deploys is observable, since
/// no output shape in this model carries it.
async fn update_description(harness: &Harness, case: &str, id: &str) -> String {
    let described = last_update_description(&events(harness, case, id).await);
    assert!(
        !described.is_empty(),
        "case {case:?}: the update appended no in-progress event"
    );
    described
}

#[tokio::test]
async fn every_published_version_of_every_pattern_is_reported_from_state() {
    let harness = Harness::new();
    // The catalogue publishes several versions per pattern, which is what makes this a
    // rule over a set rather than a single lookup.
    assert!(
        CATALOGUE_PATTERN_VERSIONS.len() >= 2,
        "the catalogue must publish enough versions per pattern to span the rule, got {:?}",
        CATALOGUE_PATTERN_VERSIONS
    );
    for (workload, pattern) in every_pattern(&harness).await {
        let mut descriptions: BTreeSet<String> = BTreeSet::new();
        for version in CATALOGUE_PATTERN_VERSIONS {
            let case = format!("{workload}/{pattern}@{version}");
            let summary = get_version(&harness, &case, &workload, &pattern, version).await;

            // The version reported is the version asked for, stated as a version of the
            // pattern it was found in.
            assert_eq!(
                summary["deploymentPatternVersionName"],
                json!(version),
                "case {case:?}: expected the requested version name, got {}",
                summary["deploymentPatternVersionName"]
            );
            assert_eq!(
                summary["workloadName"],
                json!(workload),
                "case {case:?}: expected workloadName {workload:?}, got {}",
                summary["workloadName"]
            );
            assert_eq!(
                summary["deploymentPatternName"],
                json!(pattern),
                "case {case:?}: expected deploymentPatternName {pattern:?}, got {}",
                summary["deploymentPatternName"]
            );
            let description = summary["description"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !description.is_empty(),
                "case {case:?}: a reported version must describe itself, got {}",
                summary["description"]
            );
            // The catalogue publishes no document per VERSION — one per application,
            // which the workload's own view reports — so the member is left out rather
            // than filled from another record (the rule itself lives in
            // `catalogue_published_urls.rs`).
            assert!(
                summary
                    .get("documentationUrl")
                    .is_none_or(Value::is_null),
                "case {case:?}: a version publishes no document of its own, got {}",
                summary["documentationUrl"]
            );
            descriptions.insert(description);

            // A read changes nothing a later caller sees: the same request answers the
            // same version.
            let again = get_version(&harness, &case, &workload, &pattern, version).await;
            assert_eq!(
                again, summary,
                "case {case:?}: reading a version must not change what a later caller sees"
            );
        }
        // Each version of one pattern describes ITSELF, so the versions are told apart
        // rather than answered with one canned record.
        assert_eq!(
            descriptions.len(),
            CATALOGUE_PATTERN_VERSIONS.len(),
            "case {workload}/{pattern}: each published version must carry its own \
             description, got {descriptions:?}"
        );
    }
}

#[tokio::test]
async fn the_version_a_pattern_reports_is_the_newest_one_the_version_read_resolves() {
    let harness = Harness::new();
    let newest = CATALOGUE_PATTERN_VERSIONS
        .first()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");
    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("{workload}/{pattern}");
        // What `GetWorkloadDeploymentPattern` reports as the pattern's current version.
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let current = data["deploymentPatternVersionName"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the pattern reports no deploymentPatternVersionName")
            })
            .to_string();
        assert_eq!(
            current, newest,
            "case {case:?}: a pattern reports the NEWEST version it publishes, got {current:?}"
        );

        // The two views read the same records, so the version a pattern reports is one
        // the version read resolves.
        let summary = get_version(&harness, &case, &workload, &pattern, &current).await;
        assert_eq!(
            summary["deploymentPatternVersionName"],
            json!(current),
            "case {case:?}: the version read must report the version the pattern names, got {}",
            summary["deploymentPatternVersionName"]
        );
        assert_eq!(
            summary["deploymentPatternName"],
            json!(pattern),
            "case {case:?}: the version must name its pattern, got {}",
            summary["deploymentPatternName"]
        );
    }
}

#[tokio::test]
async fn an_absent_workload_pattern_or_version_is_a_404_that_echoes_nothing() {
    let harness = Harness::new();
    let published = CATALOGUE_PATTERN_VERSIONS
        .last()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        // One row per way the request can name something the catalogue does not hold:
        // the workload, the pattern, or the version — the last in each of the two forms
        // `DeploymentPatternVersionName` allows (dotted and dashed) and at the shape's
        // length ceiling.
        let rows: Vec<(&str, String, String, String)> = vec![
            (
                "unknown workload",
                "NoSuchWorkloadXyz".to_string(),
                pattern.clone(),
                published.to_string(),
            ),
            (
                "workload that looks like a known one",
                format!("{workload}x"),
                pattern.clone(),
                published.to_string(),
            ),
            (
                "unknown pattern",
                workload.clone(),
                "NoSuchPattern".to_string(),
                published.to_string(),
            ),
            (
                "pattern that looks like a known one",
                workload.clone(),
                format!("{pattern}X"),
                published.to_string(),
            ),
            (
                "absent dotted version",
                workload.clone(),
                pattern.clone(),
                "9.9.9".to_string(),
            ),
            (
                "absent dashed version",
                workload.clone(),
                pattern.clone(),
                "no-such-version".to_string(),
            ),
            (
                "absent version at the length ceiling",
                workload.clone(),
                pattern.clone(),
                "v".repeat(30),
            ),
        ];
        for (label, asked_workload, asked_pattern, asked_version) in rows {
            let case = format!("{label}: {asked_workload}/{asked_pattern}@{asked_version}");
            let body = harness
                .refused(
                    &case,
                    PATH,
                    version_body(&asked_workload, &asked_pattern, &asked_version),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
            let message = error_message(&body);
            assert!(
                !message.is_empty(),
                "case {case:?}: a not-found must carry a message, got {body}"
            );
            // The message states WHICH KIND of resource was not found and never the
            // caller's own input, so the catalogue cannot be probed through the error
            // channel.
            for asked in [&asked_workload, &asked_pattern, &asked_version] {
                assert!(
                    !message.contains(asked.as_str()),
                    "case {case:?}: the not-found message must not echo {asked:?}, got \
                     {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_version_name_outside_the_modelled_shape_is_refused_at_the_boundary() {
    let harness = Harness::new();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        // `DeploymentPatternVersionName` is @length(min: 5, max: 30) with
        // @pattern ^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$ — from
        // smithy_query(query="shape", shape="DeploymentPatternVersionName"). A name
        // INSIDE the shape reaches the handler, so an absent one is a 404 and never a
        // 400; both published length bounds are inside it.
        for inside in ["abcde".to_string(), "v".repeat(30)] {
            let case = format!("{workload}/{pattern}@{inside} (inside the shape)");
            harness
                .refused(
                    &case,
                    PATH,
                    version_body(&workload, &pattern, &inside),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
        }
        // One past each bound, and each way the pattern can be broken, is refused at the
        // boundary instead.
        for outside in [
            "abcd".to_string(),
            "v".repeat(31),
            "has space".to_string(),
            "-leading".to_string(),
            "1.0".to_string(),
            "1.0.0.0".to_string(),
        ] {
            let case = format!("{workload}/{pattern}@{outside} (outside the shape)");
            harness
                .refused(
                    &case,
                    PATH,
                    version_body(&workload, &pattern, &outside),
                    400,
                    "ValidationException",
                )
                .await;
        }
        // Every member of the input is @required, so a request that omits one is
        // refused the same way, whichever one it omits.
        let published = CATALOGUE_PATTERN_VERSIONS
            .last()
            .copied()
            .expect("the catalogue publishes at least one version per pattern");
        let whole = version_body(&workload, &pattern, published);
        for omitted in [
            "workloadName",
            "deploymentPatternName",
            "deploymentPatternVersionName",
        ] {
            let mut object = whole
                .as_object()
                .expect("a request body is an object")
                .clone();
            object.remove(omitted);
            let case = format!("{workload}/{pattern} without {omitted}");
            harness
                .refused(
                    &case,
                    PATH,
                    Value::Object(object),
                    400,
                    "ValidationException",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn a_version_the_read_reports_is_a_version_an_update_can_name() {
    let harness = Harness::new();
    let oldest = CATALOGUE_PATTERN_VERSIONS
        .last()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");
    let newest = CATALOGUE_PATTERN_VERSIONS
        .first()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");
    // Each accepted update needs a deployment whose lifecycle is at rest, so every row
    // gets its own and all of them are created FIRST, then waited out with ONE sleep.
    let mut suites: Vec<VersionSuite> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, false);
        let mut per_version = Vec::new();
        for (row, _) in CATALOGUE_PATTERN_VERSIONS.iter().enumerate() {
            per_version.push(
                create(
                    &harness,
                    &case,
                    &workload,
                    &pattern,
                    &format!("ver-{index}-{row}"),
                )
                .await,
            );
        }
        // The deployment the refusals and the "keeps its version" pair run against.
        let kept = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("ver-{index}-kept"),
        )
        .await;
        suites.push(VersionSuite {
            case,
            workload,
            pattern,
            specifications,
            per_version,
            kept,
        });
    }
    std::thread::sleep(CREATE_SETTLE);

    for VersionSuite {
        case,
        workload,
        pattern,
        specifications,
        per_version,
        kept,
    } in suites
    {
        // Every version the version read reports is a version a deployment that is not
        // already on it can be moved onto, and the event the update appends records the
        // one it now deploys. The one version of the published set a deployment is
        // ALREADY on is refused instead, because moving it there moves no version at all
        // — the rule `tests/all/update_deployment_version_targets.rs` tables.
        for (row, version) in CATALOGUE_PATTERN_VERSIONS.iter().enumerate() {
            let id = &per_version[row];
            let summary = get_version(&harness, &case, &workload, &pattern, version).await;
            let name = summary["deploymentPatternVersionName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let case = format!("{case}@{name}");
            let body = with_member(
                &update_body(id, &specifications),
                "deploymentPatternVersionName",
                json!(name),
            );
            // Every one of these deployments was created against the pattern's CURRENT
            // version, so that is the version each of them already deploys.
            if name == newest {
                let refused = harness
                    .refused(&case, "/updateDeployment", body, 400, "ValidationException")
                    .await;
                let message = error_message(&refused);
                assert_eq!(
                    message, DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE,
                    "case {case:?}: the version a deployment already deploys is not one an \
                     update can name, expected the fixed refusal, got {message:?}"
                );
                continue;
            }
            harness.ok(&case, "/updateDeployment", body).await;
            let described = update_description(&harness, &case, id).await;
            assert!(
                described.contains(&format!("'{name}'")),
                "case {case:?}: the update event must name the version the deployment now \
                 deploys, got {described:?}"
            );
        }

        // A version the version read answers 404 for is one no update can name either:
        // the two reads resolve a version against the same published list.
        for absent in ["9.9.9", "no-such-version"] {
            let case = format!("{case}@{absent} (absent)");
            harness
                .refused(
                    &case,
                    PATH,
                    version_body(&workload, &pattern, absent),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
            harness
                .refused(
                    &case,
                    "/updateDeployment",
                    with_member(
                        &update_body(&kept, &specifications),
                        "deploymentPatternVersionName",
                        json!(absent),
                    ),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
        }

        // Moved back onto the OLDEST published version, an update that names no version
        // at all leaves it there: an update applies settings and does not silently move
        // a deployment onto the pattern's newest version.
        harness
            .ok(
                &format!("{case}@{oldest} (back to the oldest)"),
                "/updateDeployment",
                with_member(
                    &update_body(&kept, &specifications),
                    "deploymentPatternVersionName",
                    json!(oldest),
                ),
            )
            .await;
        // The second update of the same deployment waits for the first to finish: a
        // deployment that is still applying an update does not take another.
        std::thread::sleep(UPDATE_SETTLE);
        harness
            .ok(
                &format!("{case} with no version named"),
                "/updateDeployment",
                update_body(&kept, &specifications),
            )
            .await;
        let described = update_description(&harness, &case, &kept).await;
        // The clause names the DEPLOYMENT PATTERN version, which is where the two
        // version members of one catalogue entry differ: the workload version an entry
        // is published at is its newest pattern version, so the bare version name is not
        // enough to tell the two clauses apart.
        assert!(
            described.contains(&format!("deployment pattern version '{oldest}'")),
            "case {case:?}: an update naming no version must keep version {oldest:?}, got \
             {described:?}"
        );
        assert!(
            !described.contains(&format!("deployment pattern version '{newest}'")),
            "case {case:?}: an update naming no version must not move the deployment to \
             {newest:?}, got {described:?}"
        );
    }
}
