// SPDX-License-Identifier: CC0-1.0

//! The versions an `UpdateDeployment` may move a deployment ONTO, driven over HTTP
//! through the same router the production emulator serves with (the harness in
//! `tests/all/harness.rs`).
//!
//! The rule under test: a pattern publishes several versions and an update may name any
//! of them EXCEPT the one its deployment already deploys, because moving a deployment
//! onto the version it is already on moves no version at all. So, for every deployment
//! pattern the catalogue holds:
//!
//! 1. an update naming the version its deployment already deploys is refused with the
//!    modelled `ValidationException` (HTTP 400) and its fixed message, whatever else the
//!    request carries — more or fewer settings, the workload version alongside it,
//!    `force` set, or the `dryRun` pre-flight flag set — and nothing about the
//!    deployment moves;
//! 2. the versions an update ACCEPTS are exactly the ones
//!    `ListDeploymentPatternVersions` reports for that deployment's current version
//!    under its `updateFromVersion` filter, so the read that answers "what can this
//!    deployment be updated onto" and the operation that performs the move cannot
//!    disagree about one version;
//! 3. which version is refused follows the DEPLOYMENT rather than the catalogue: once an
//!    update has moved a deployment onto another published version, that version is the
//!    refused one and the version it started on has become a target.
//!
//! No workload, pattern, setting or version is written out here: each is read from what
//! the service itself reports.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE;

use crate::deployment_lifecycle::{create, get};
use crate::deployment_update::{
    conforming_specifications, error_message, events, one_pattern_per_workload, pattern_data,
    update_body, update_targets, with_member,
};
use crate::harness::Harness;

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s a create walks, plus slack): a deployment whose create is
/// still in flight refuses an update outright. The emulator never sleeps; this is the
/// test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// Long enough for the one step an UPDATE schedules to come due, after which the
/// deployment is at rest again and takes another update.
const UPDATE_SETTLE: Duration = Duration::from_millis(1_200);

/// The wire error code and HTTP status the refusal of rule 1 carries
/// (`ValidationException`, `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// The `DeploymentStatus` an accepted update reports, and the resting one a refused
/// update leaves the deployment in. Both are members of the model's `DeploymentStatus`
/// enum.
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// See [`UPDATE_IN_PROGRESS`].
const AT_REST: &str = "COMPLETED";

/// Every version `ListDeploymentPatternVersions` reports for one pattern when no filter
/// narrows the page — the published set the rule selects from.
async fn published_versions(
    harness: &Harness,
    case: &str,
    workload: &str,
    pattern: &str,
) -> Vec<String> {
    let body = harness
        .ok(
            case,
            "/listDeploymentPatternVersions",
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
        )
        .await;
    body["deploymentPatternVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no deploymentPatternVersions reported: {body}"))
        .iter()
        .map(|summary| {
            summary["deploymentPatternVersionName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a reported version carries no name: {summary}")
                })
                .to_string()
        })
        .collect()
}

/// The `deploymentPatternVersionName` a pattern currently deploys — the version a
/// deployment created against it starts on.
fn current_version(case: &str, data: &Value) -> String {
    data["deploymentPatternVersionName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no deploymentPatternVersionName reported: {data}"))
        .to_string()
}

/// One row of rule 1's table: a deployment at rest, and the request form that names the
/// version it already deploys.
struct RefusalRow {
    case: String,
    id: String,
    /// The pattern's own schema filled in, so the refusal is about the VERSION rather
    /// than about a setting.
    specifications: Map<String, Value>,
    deployed: String,
    workload_version: String,
}

#[tokio::test]
async fn the_version_a_deployment_already_deploys_is_refused_however_the_request_is_framed() {
    let harness = Harness::new();
    // Every deployment is created FIRST and the create lifecycles are waited out with
    // ONE sleep: an update is taken only by a deployment whose lifecycle is at rest.
    // One deployment serves every row of a pattern, because a REFUSED update persists
    // nothing and so leaves it at rest for the next row.
    let mut rows: Vec<RefusalRow> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let id = create(&harness, &case, &workload, &pattern, &format!("tgt-{index}")).await;
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        rows.push(RefusalRow {
            deployed: current_version(&case, &data),
            workload_version: data["workloadVersionName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no workloadVersionName reported"))
                .to_string(),
            // Alternating sizes, so the rule is exercised over settings maps that differ
            // in how many of the pattern's fields they carry.
            specifications: conforming_specifications(&case, &data, index % 2 == 0),
            case,
            id,
        });
    }
    std::thread::sleep(CREATE_SETTLE);

    for RefusalRow {
        case,
        id,
        specifications,
        deployed,
        workload_version,
    } in rows
    {
        let before = get(&harness, &case, &id).await;
        let before_events = events(&harness, &case, &id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(AT_REST),
            "case {case:?}: the row must start from a deployment at rest, got {before}"
        );
        let named = with_member(
            &update_body(&id, &specifications),
            "deploymentPatternVersionName",
            json!(deployed),
        );
        // The rows differ in the OTHER members the request carries: the version member
        // alone, the workload version alongside it, the member that applies an update
        // despite validation warnings, and the pre-flight flag that checks a request
        // without making it. None of them is permission to move a deployment onto the
        // version it is already on.
        let forms = [
            ("the version alone", named.clone()),
            (
                "the version and the workload version",
                with_member(&named, "workloadVersionName", json!(workload_version)),
            ),
            ("with force set", with_member(&named, "force", json!(true))),
            (
                "with force clear",
                with_member(&named, "force", json!(false)),
            ),
            (
                "as a dry run",
                with_member(&named, "dryRun", json!(true)),
            ),
        ];
        for (label, body) in forms {
            let case = format!("{case}: {label}");
            let refused = harness
                .refused(&case, "/updateDeployment", body, STATUS, CODE)
                .await;
            let message = error_message(&refused);
            assert_eq!(
                message, DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE,
                "case {case:?}: expected the fixed refusal of the already-deployed version, \
                 got {message:?}"
            );
            // The refusal states the rule rather than reflecting the request back.
            for echoed in [deployed.as_str(), id.as_str()] {
                assert!(
                    !message.contains(echoed),
                    "case {case:?}: the message must not render {echoed:?}, got {message:?}"
                );
            }
            // Nothing moved: not the record a reader reports, not its event log.
            let after = get(&harness, &case, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must leave the record untouched, got {after}"
            );
            let after_events = events(&harness, &case, &id).await;
            assert_eq!(
                after_events, before_events,
                "case {case:?}: a refused update must append no event, got {after_events:?}"
            );
        }
    }
}

/// One row of rule 2's table: the published version an update submits, and the
/// deployment of its own it submits it against.
struct TargetRow {
    version: String,
    id: String,
}

/// One pattern's rule-2 suite: the deployment table, the version those deployments
/// start on and the settings every row submits.
struct TargetSuite {
    case: String,
    workload: String,
    pattern: String,
    specifications: Map<String, Value>,
    deployed: String,
    table: Vec<TargetRow>,
}

#[tokio::test]
async fn the_versions_an_update_accepts_are_the_ones_the_read_reports_as_update_targets() {
    let harness = Harness::new();
    // One deployment per published version, all created FIRST and waited out with ONE
    // sleep: an ACCEPTED update leaves its deployment mid-update, which takes no further
    // update, so a row that may be accepted needs a deployment of its own.
    let mut suites: Vec<TargetSuite> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, true);
        let deployed = current_version(&case, &data);
        let mut table = Vec::new();
        for (row, version) in published_versions(&harness, &case, &workload, &pattern)
            .await
            .into_iter()
            .enumerate()
        {
            table.push(TargetRow {
                version,
                id: create(
                    &harness,
                    &case,
                    &workload,
                    &pattern,
                    &format!("acc-{index}-{row}"),
                )
                .await,
            });
        }
        assert!(
            table.len() > 1,
            "case {case:?}: the pattern must publish more than one version for the rule to \
             select over, got {}",
            table.len()
        );
        suites.push(TargetSuite {
            case,
            workload,
            pattern,
            specifications,
            deployed,
            table,
        });
    }
    std::thread::sleep(CREATE_SETTLE);

    for TargetSuite {
        case,
        workload,
        pattern,
        specifications,
        deployed,
        table,
    } in suites
    {
        let reported: BTreeSet<String> =
            update_targets(&harness, &case, &workload, &pattern, &deployed)
                .await
                .into_iter()
                .collect();
        let mut accepted: BTreeSet<String> = BTreeSet::new();
        for TargetRow { version, id } in &table {
            let case = format!("{case}@{version}");
            let body = with_member(
                &update_body(id, &specifications),
                "deploymentPatternVersionName",
                json!(version),
            );
            let reply = harness.call("/updateDeployment", &body).await;
            if reply.status == 200 {
                accepted.insert(version.clone());
                assert_eq!(
                    reply.body["deployment"]["status"].as_str(),
                    Some(UPDATE_IN_PROGRESS),
                    "case {case:?}: an accepted update reports the status the model declares \
                     for it, got {}",
                    reply.body
                );
                // The move is observable on the record the next reader sees: the event
                // the update appended names the version the deployment now deploys.
                let described = crate::deployment_update::last_update_description(
                    &events(&harness, &case, id).await,
                );
                assert!(
                    described.contains(&format!("'{version}'")),
                    "case {case:?}: the update event must name the version the deployment now \
                     deploys, got {described:?}"
                );
            } else {
                assert_eq!(
                    (reply.status, reply.error_code.as_deref()),
                    (STATUS, Some(CODE)),
                    "case {case:?}: a version an update cannot name is refused with the \
                     modelled client error, got {} with body {}",
                    reply.status,
                    reply.body
                );
                let message = error_message(&reply.body);
                assert_eq!(
                    message, DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE,
                    "case {case:?}: expected the fixed refusal of the already-deployed \
                     version, got {message:?}"
                );
            }
        }
        assert_eq!(
            accepted, reported,
            "case {case:?}: the versions an update accepts must be the ones the read reports \
             as this deployment's update targets {reported:?}, got {accepted:?}"
        );
        assert!(
            !accepted.contains(&deployed),
            "case {case:?}: the version a deployment already deploys is not a target, got \
             {accepted:?}"
        );
    }
}

#[tokio::test]
async fn the_refused_version_follows_the_deployment_rather_than_the_catalogue() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern} moved");
    let id = create(&harness, &case, &workload, &pattern, "tgt-moved").await;
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let specifications = conforming_specifications(&case, &data, true);
    let started_on = current_version(&case, &data);
    // The version this deployment is moved onto: one the read reports as a target of the
    // version it starts on.
    let moved_onto = update_targets(&harness, &case, &workload, &pattern, &started_on)
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the read must report an update target"));
    std::thread::sleep(CREATE_SETTLE);

    let body = |version: &str| {
        with_member(
            &update_body(&id, &specifications),
            "deploymentPatternVersionName",
            json!(version),
        )
    };

    // The move itself, and then the deployment is at rest on its NEW version.
    let answered = harness
        .ok(
            &format!("{case}: onto {moved_onto}"),
            "/updateDeployment",
            body(&moved_onto),
        )
        .await;
    assert_eq!(
        answered["deployment"]["status"].as_str(),
        Some(UPDATE_IN_PROGRESS),
        "case {case:?}: the move must be accepted, got {answered}"
    );
    std::thread::sleep(UPDATE_SETTLE);

    // What was a target is now the refused version...
    let refused = harness
        .refused(
            &format!("{case}: onto {moved_onto} again"),
            "/updateDeployment",
            body(&moved_onto),
            STATUS,
            CODE,
        )
        .await;
    let message = error_message(&refused);
    assert_eq!(
        message, DEPLOYMENT_PATTERN_VERSION_ALREADY_DEPLOYED_MESSAGE,
        "case {case:?}: the version the deployment now deploys must be refused, got \
         {message:?}"
    );
    // ...and the read reports the same pair of answers for the version it now deploys.
    let reported: BTreeSet<String> =
        update_targets(&harness, &case, &workload, &pattern, &moved_onto)
            .await
            .into_iter()
            .collect();
    assert!(
        !reported.contains(&moved_onto) && reported.contains(&started_on),
        "case {case:?}: the read must now omit {moved_onto:?} and report {started_on:?}, got \
         {reported:?}"
    );
    // ...while the version it started on, refused before the move, is accepted now.
    let answered = harness
        .ok(
            &format!("{case}: back onto {started_on}"),
            "/updateDeployment",
            body(&started_on),
        )
        .await;
    assert_eq!(
        answered["deployment"]["status"].as_str(),
        Some(UPDATE_IN_PROGRESS),
        "case {case:?}: the version the deployment left must be a target again, got {answered}"
    );
}
