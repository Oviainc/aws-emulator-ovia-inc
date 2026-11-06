// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three refusals and one outcome the service states in its own
//! vocabulary rather than in the model's: `UpgradeDomain`'s target-version
//! eligibility, the terminal status of a node maintenance action, and
//! `DissociatePackage`/`DissociatePackages` on a link a domain does not hold.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`], the upgrade calls from [`super::upgrade_history`], the
//! published upgrade relation from [`super::compatible_versions`], the maintenance
//! calls from [`super::domain_maintenance`] and the package calls from
//! [`super::package_associations`] / [`super::package_dissociation`]; this suite adds
//! no second copy of any of them.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one call:
//!
//! * an upgrade goes only to a version the domain's current one can be upgraded to —
//!   the relation `GetCompatibleVersions` publishes — and a target outside that set
//!   names an upgrade the service does not offer: it is refused on the service's
//!   GENERIC envelope, stating the version the domain runs and the one asked for, and
//!   the domain's `EngineVersion` does not move. The rows span the ways a target can
//!   be outside the set (the version the domain already runs, an earlier release of
//!   its own line, the oldest published release, a release past the line it can
//!   reach, and a pattern-valid release the catalog does not publish at all), over
//!   source versions in the middle of a line, at the end of a line and at the end of
//!   the newest line, and with and without `PerformCheckOnly`;
//! * that eligibility is decided from the request and the version the domain runs
//!   alone, so it is answered the same way whatever the domain is busy with — while
//!   the activity gates still answer an ELIGIBLE target in a busy domain in their own
//!   words;
//! * a maintenance action that finishes is reported with the service's terminal
//!   status, by `GetDomainMaintenanceStatus` and by `ListDomainMaintenances` alike,
//!   over every `MaintenanceType` value the model publishes;
//! * dissociating a package the named domain is not holding names a relationship that
//!   cannot be dissolved, so it is refused as a bad REQUEST — over the ways the link
//!   can be absent (never made, made with another domain, already removed) and over
//!   both wire forms.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    MAINTAINABLE_DATA_NODES, MAINTENANCE_STATUS_SUCCESS, NODE_TYPE_DATA,
    SUPPORTED_ENGINE_VERSIONS,
};

use super::compatible_versions::{compatible, entries, target_versions};
use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, SETTLE_WAIT};
use super::domain_maintenance::{census, create, every_role, list, start, status, MAINTENANCE_TYPES};
use super::package_associations::{available_packages, stored_status};
use super::package_dissociation::{dissociate_many, dissociate_one, with_associations};
use super::upgrade_history::{settled_domain, upgrade};

/// A `VersionString` the catalog does not publish, inside the member's
/// `@pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`
/// and its `@length(min: 14, max: 18)` — so it is the eligibility rule that refuses
/// it, not the request boundary.
const UNPUBLISHED_VERSION: &str = "OpenSearch_99.9";

/// The source versions the eligibility table is asserted from: a release in the middle
/// of its upgrade line, the release at the END of a line (whose targets cross into the
/// next one), and the newest published release, which has nothing to upgrade to at
/// all.
const SOURCES: [&str; 3] = ["OpenSearch_2.11", "Elasticsearch_7.10", "OpenSearch_3.7"];

/// The sentence the service refuses a dissociation of a package the domain is not
/// holding with: fixed text carrying no value of the request. Spelled out here rather
/// than taken from the crate, so a change to either one fails this suite.
const PACKAGE_NOT_ASSOCIATED: &str = "Package is not associated to this domain.";

/// The message the service states an ineligible target with: the two versions the
/// eligibility was decided from, and nothing else of the request.
///
/// The two are rendered differently, as the service renders them: the version the domain
/// RUNS keeps the engine prefix its record declares it with, while the target is stated as
/// its bare release — the `X.Y` part of the `VersionString` the request carried.
fn not_supported(source: &str, target: &str) -> String {
    let bare = target.split_once('_').map_or(target, |(_, release)| release);
    format!("Failed to submit upgrade. Upgrade from {source} to {bare} not supported.")
}

/// The `EngineVersion` `DescribeDomain` reports for `name`.
async fn engine_version(emulator: &Emulator, name: &str, case: &str) -> String {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeDomain to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body["DomainStatus"], "EngineVersion", case)
}

/// The targets `GetCompatibleVersions` advertises for the one version `name` runs.
async fn advertised(emulator: &Emulator, name: &str, case: &str) -> Vec<String> {
    let answer = compatible(emulator, Some(name)).await;
    let rows = entries(&answer, case);
    assert_eq!(
        rows.len(),
        1,
        "case {case}: expected the one row for the version the domain runs, got {rows:?}"
    );
    target_versions(&rows[0], case)
}

/// The targets of `source` that the published relation does NOT offer, spanning the
/// ways a target can fall outside it.
fn ineligible_targets(source: &str, offered: &[String]) -> Vec<&'static str> {
    let position = SUPPORTED_ENGINE_VERSIONS
        .iter()
        .position(|version| *version == source)
        .expect("every source of the table is a published version");
    let mut candidates: Vec<&'static str> = vec![
        // The version the domain already runs: an upgrade only goes forwards, so no
        // version is among its own targets.
        SUPPORTED_ENGINE_VERSIONS[position],
        // The published release immediately before it — a downgrade.
        SUPPORTED_ENGINE_VERSIONS[position.saturating_sub(1)],
        // The oldest published release of all.
        SUPPORTED_ENGINE_VERSIONS[0],
        // The newest published release: reachable only from the end of the line below
        // it, so for every source but one it is past what that source can reach.
        SUPPORTED_ENGINE_VERSIONS[SUPPORTED_ENGINE_VERSIONS.len() - 1],
        UNPUBLISHED_VERSION,
    ];
    candidates.retain(|candidate| !offered.iter().any(|target| target == candidate));
    candidates.dedup();
    candidates
}

/// Assert `answer` is the refusal `case` expects: the status, the wire error code and
/// the message the service states for the condition, all three.
fn assert_refusal(answer: &Answer, status: u16, code: &str, message: &str, case: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (status, code),
        "case {case}: expected {status} {code}, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the service's wording {message:?}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_target_the_published_relation_does_not_offer_is_refused_and_moves_nothing() {
    let emulator = Emulator::new();

    for (index, source) in SOURCES.iter().enumerate() {
        let name = format!("upgrade-target-{index}");
        settled_domain(&emulator, &name, source).await;
        let offered = advertised(&emulator, &name, source).await;

        for target in ineligible_targets(source, &offered) {
            for check_only in [true, false] {
                let case = format!("{source} -> {target} (PerformCheckOnly {check_only})");
                let answer = upgrade(&emulator, &name, target, check_only).await;
                assert_refusal(
                    &answer,
                    400,
                    "BaseException",
                    &not_supported(source, target),
                    &case,
                );
                // A refused upgrade starts nothing, so the version the domain runs is
                // the one it was created on however many targets were refused.
                assert_eq!(
                    engine_version(&emulator, &name, &case).await,
                    *source,
                    "case {case}: a refused upgrade leaves the domain's EngineVersion alone"
                );
            }
        }

        // The boundary of the same rule: every target the relation DOES offer is
        // accepted. A check performs no upgrade, so the domain is left on its own
        // version and the next row judges the same source.
        for target in [offered.first(), offered.last()].into_iter().flatten() {
            let case = format!("{source} -> {target} (offered)");
            let accepted = upgrade(&emulator, &name, target, true).await;
            assert_eq!(
                accepted.status, 200,
                "case {case}: expected the offered target to be accepted, got {} with {:?}",
                accepted.status, accepted.body
            );
            assert_eq!(
                string_member(&accepted.body, "TargetVersion", &case),
                *target,
                "case {case}: expected the requested TargetVersion echoed, got {:?}",
                accepted.body
            );
            // The check takes the span the clock owes it; the next call is made on a
            // domain with nothing in flight.
            tokio::time::sleep(SETTLE_WAIT).await;
            assert_eq!(
                engine_version(&emulator, &name, &case).await,
                *source,
                "case {case}: a check performs no upgrade, so the domain stays where it was"
            );
        }
    }
}

#[tokio::test]
async fn an_ineligible_target_is_refused_the_same_way_whatever_the_domain_is_busy_with() {
    let emulator = Emulator::new();
    let source = "OpenSearch_2.11";
    let name = "upgrade-target-busy";
    settled_domain(&emulator, name, source).await;
    let offered = advertised(&emulator, name, "busy domain").await;
    let eligible = offered
        .first()
        .cloned()
        .expect("the source of this case has somewhere to go");

    // A configuration change in flight, then an upgrade in flight: two kinds of work
    // the domain is busy with. In each, the ineligible target is refused for being
    // ineligible, while the eligible one is refused in the words that state what the
    // domain is busy with.
    let changing = emulator
        .update_config(name, json!({ "ClusterConfig": { "InstanceCount": 3 } }))
        .await;
    assert_eq!(
        changing.status, 200,
        "case a change in flight: expected the update to be accepted, got {} with {:?}",
        changing.status, changing.body
    );
    assert_refusal(
        &upgrade(&emulator, name, UNPUBLISHED_VERSION, false).await,
        400,
        "BaseException",
        &not_supported(source, UNPUBLISHED_VERSION),
        "a change in flight / an ineligible target",
    );
    assert_refusal(
        &upgrade(&emulator, name, &eligible, false).await,
        400,
        "BaseException",
        "Upgrade failed. Domain has background tasks in progress",
        "a change in flight / an offered target",
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let started = upgrade(&emulator, name, &eligible, false).await;
    assert_eq!(
        started.status, 200,
        "case an upgrade in flight: expected the upgrade to start, got {} with {:?}",
        started.status, started.body
    );
    assert_refusal(
        &upgrade(&emulator, name, UNPUBLISHED_VERSION, true).await,
        400,
        "BaseException",
        &not_supported(source, UNPUBLISHED_VERSION),
        "an upgrade in flight / an ineligible target",
    );
    let racing = upgrade(&emulator, name, &eligible, false).await;
    assert_eq!(
        (racing.status, error_code(&racing).as_str()),
        (409, "ResourceAlreadyExistsException"),
        "case an upgrade in flight / an offered target: expected the run in progress to be \
         named, got {} with {:?}",
        racing.status,
        racing.body
    );

    // The upgrade the refusals raced still lands on its own span, and it is the only
    // thing that moved the domain.
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        engine_version(&emulator, name, "settled").await,
        eligible,
        "case settled: the one accepted upgrade is the one the domain runs"
    );
}

#[tokio::test]
async fn a_finished_maintenance_action_is_reported_with_the_services_terminal_status() {
    let emulator = Emulator::new();
    let domain = "maintenance-outcome";
    create(&emulator, domain, every_role(MAINTAINABLE_DATA_NODES)).await;
    let nodes: Vec<String> = census(&emulator, domain)
        .await
        .into_iter()
        .filter(|(_, node_type)| node_type == NODE_TYPE_DATA)
        .map(|(id, _)| id)
        .collect();

    // One action per published `MaintenanceType`, each on a data node of the domain.
    let mut started: Vec<(String, &str)> = Vec::new();
    for (index, action) in MAINTENANCE_TYPES.iter().enumerate() {
        let node_id = nodes[index % nodes.len()].clone();
        let answer = start(&emulator, domain, action, Some(node_id.as_str())).await;
        assert_eq!(
            answer.status, 200,
            "case StartDomainMaintenance({action}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        started.push((
            string_member(&answer.body, "MaintenanceId", action),
            action,
        ));
    }

    // The clock finishes every action, with no call having asked for the move.
    tokio::time::sleep(SETTLE_WAIT).await;
    let listed = list(&emulator, domain, "").await;
    assert_eq!(
        listed.status, 200,
        "case ListDomainMaintenances: expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    let rows = listed.body["DomainMaintenances"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case ListDomainMaintenances: expected a list, got {:?}",
                listed.body
            )
        })
        .clone();

    for (maintenance_id, action) in &started {
        let case = format!("{action} / {maintenance_id}");
        let read = status(&emulator, domain, maintenance_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["Status"].as_str(),
            Some(MAINTENANCE_STATUS_SUCCESS),
            "case {case}: expected the terminal status of a finished action, got {:?}",
            read.body
        );
        // The listing reports the SAME record, so the two read paths cannot disagree.
        let row = rows
            .iter()
            .find(|row: &&Value| row["MaintenanceId"].as_str() == Some(maintenance_id.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected the listing to report it, {rows:?}"));
        assert_eq!(
            row["Status"].as_str(),
            read.body["Status"].as_str(),
            "case {case}: the listing and the read report one status, got {row:?} and {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn dissociating_a_link_the_domain_does_not_hold_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "dissoc-not-associated";
    let sibling = "dissoc-not-associated-other";
    with_associations(&emulator, domain, &ids, &[0, 1]).await;
    with_associations(&emulator, sibling, &ids, &[2]).await;

    // The link the domain once held is taken off and the removal is let land, so the
    // third row below names a relationship that was dissolved rather than one that
    // never existed.
    let removed = dissociate_one(&emulator, domain, &ids[1]).await;
    assert_eq!(
        removed.status, 200,
        "case the first removal: expected 200, got {} with {:?}",
        removed.status, removed.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    // One row per way the link can be absent, each asserted through BOTH wire forms:
    // the one that unlinks several packages and the one that unlinks a single package.
    for (case, package_id) in [
        ("a stored package the domain never had", &ids[3]),
        ("a package associated with another domain", &ids[2]),
        ("a link this domain held and has had removed", &ids[1]),
    ] {
        let expected = PACKAGE_NOT_ASSOCIATED;
        for (form, answer) in [
            (
                "DissociatePackages",
                dissociate_many(
                    &emulator,
                    json!({ "DomainName": domain, "PackageList": [package_id] }),
                )
                .await,
            ),
            (
                "DissociatePackage",
                dissociate_one(&emulator, domain, package_id).await,
            ),
        ] {
            assert_refusal(
                &answer,
                400,
                "ValidationException",
                expected,
                &format!("{form} / {case}"),
            );
        }
        // A refused request dissociates nothing: the one association the domain still
        // holds is in place, and the sibling's is too.
        assert!(
            stored_status(&emulator, domain, &ids[0]).is_some(),
            "case {case}: the association the refused call did not name is as it was"
        );
        assert!(
            stored_status(&emulator, sibling, &ids[2]).is_some(),
            "case {case}: the sibling domain's association is as it was"
        );
    }

    // The same batch refused by ONE absent link leaves the links it also named alone,
    // so a refused batch is all-or-nothing.
    let batch = dissociate_many(
        &emulator,
        json!({ "DomainName": domain, "PackageList": [ids[0], ids[3]] }),
    )
    .await;
    assert_refusal(
        &batch,
        400,
        "ValidationException",
        PACKAGE_NOT_ASSOCIATED,
        "a batch whose last link is absent",
    );
    assert!(
        stored_status(&emulator, domain, &ids[0]).is_some(),
        "case a batch whose last link is absent: the link the batch also named is as it was"
    );
}
