// SPDX-License-Identifier: CC0-1.0

//! What decommissioning a landing zone DELETES, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rule under test, asserted over a TABLE of deployments rather than one case:
//! decommissioning "deletes Amazon Web Services Control Tower resources deployed in
//! accounts managed by Amazon Web Services Control Tower", so the resources this service
//! deploys under a landing zone — every control enablement, whether a caller enabled it or
//! the service applied it to a target it governs, and every baseline enablement — do not
//! outlive it: applying the decommissioning removes them, and the tags attached to them,
//! with the landing zone itself, and the control and baseline reporting operations then
//! report none of them. Two things it does NOT remove: anything at all before the
//! decommissioning is due (it is asynchronous), and anything at all when the
//! decommissioning did not happen (its own operation FAILED). The operation logs are not
//! resources and outlive both, so every identifier a caller was handed still resolves.
//!
//! The rows differ along what the rule generalises over: how many enablements the landing
//! zone has under it, of which kind, whether the service applied some itself, and whether
//! they carry tags.
//!
//! Nothing writes a control, a baseline or a version out by hand: the controls come from
//! `state::CONTROL_CATALOG` (the enablable entries for what a caller enables, the
//! non-enablable ones for what the service applies to a target it governs) and the
//! baselines and their versions from `state::BASELINE_CATALOG`, so the cases cover entries
//! beyond the ones the issue cited and no row can drift from the catalog it is about. The
//! paging bounds come from `smithy_query(query="operation",
//! operation="ListEnabledControls,ListEnabledBaselines")` (`maxResults` 1..200 and 5..100)
//! and the refusals from `smithy_query(query="operation",
//! operation="GetEnabledControl,GetEnabledBaseline")` (`ResourceNotFoundException` is HTTP
//! 404) on this crate's `smithy-model/`.

use std::collections::BTreeMap;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, CatalogControl, BASELINE_CATALOG, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_catalog::served_arn;
use crate::enabled_control_lifecycle::{control, entries, ou};
use crate::governed_target_mandatory_controls::mandatory;
use crate::landing_zone_lifecycle::{
    call, emulator, emulator_with_engine, manifest_governing, past_the_settle_span, Answer,
};
use crate::landing_zone_pending_transitions::fail_the_operation;
use crate::resource_tags::{read_tags, reported_tags, tags};

/// The landing zone version every row deploys: one the catalog offers.
const VERSION: &str = "3.3";

/// The regions the landing zone every row deploys governs, so a control's reported
/// `targetRegions` is a governed set rather than the enabling request's own region.
const GOVERNED: &[&str] = &["us-west-2", "eu-west-1"];

/// Which enablement store a resource lives in — the two kinds of resource this service
/// deploys under a landing zone, and the reporting calls that resolve each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Control,
    Baseline,
}

impl Kind {
    /// The reporting call that resolves ONE enablement of this kind.
    fn get_path(self) -> &'static str {
        match self {
            Kind::Control => "/get-enabled-control",
            Kind::Baseline => "/get-enabled-baseline",
        }
    }

    /// The input member an enablement's identifier travels in.
    fn identifier_member(self) -> &'static str {
        match self {
            Kind::Control => "enabledControlIdentifier",
            Kind::Baseline => "enabledBaselineIdentifier",
        }
    }

    /// The listing that reports every enablement of this kind.
    fn list_path(self) -> &'static str {
        match self {
            Kind::Control => "/list-enabled-controls",
            Kind::Baseline => "/list-enabled-baselines",
        }
    }

    /// The output member that listing carries its entries in.
    fn list_member(self) -> &'static str {
        match self {
            Kind::Control => "enabledControls",
            Kind::Baseline => "enabledBaselines",
        }
    }

    /// A page big enough for everything any row enables, inside the listing's own
    /// `maxResults` range: 1..200 for controls, 5..100 for baselines.
    fn page_size(self) -> u32 {
        match self {
            Kind::Control => 200,
            Kind::Baseline => 100,
        }
    }

    /// The call that resolves a record of the operation log this kind's calls append to.
    fn operation_path(self) -> &'static str {
        match self {
            Kind::Control => "/get-control-operation",
            Kind::Baseline => "/get-baseline-operation",
        }
    }
}

/// One enablement a row deployed under the landing zone.
struct Enablement {
    /// What it is, for the assertion messages.
    what: String,
    kind: Kind,
    /// The ARN this service minted for it.
    arn: String,
    /// The `operationIdentifier` of the call that created it, so a case can assert the log
    /// outlives the resource.
    created_by: String,
}

/// One row of the decommissioning table: what is deployed under the landing zone when the
/// decommissioning starts.
struct Decommission {
    case: &'static str,
    /// How many of the catalog's ENABLABLE controls the caller enables, each on its own
    /// organizational unit.
    controls: usize,
    /// How many of the catalog's baselines that establish no governance the caller enables,
    /// each at its newest offered version on its own organizational unit.
    baselines: usize,
    /// Whether the caller also enables the baseline that ESTABLISHES GOVERNANCE, which
    /// makes the service apply every mandatory control to that target itself — enablements
    /// no caller created.
    governance: bool,
    /// The tags every enablement the row creates carries.
    tags: BTreeMap<String, String>,
}

/// The decommissioning table.
fn decommissionings() -> Vec<Decommission> {
    vec![
        Decommission {
            case: "nothing deployed under the landing zone",
            controls: 0,
            baselines: 0,
            governance: false,
            tags: tags(&[]),
        },
        Decommission {
            case: "one control a caller enabled",
            controls: 1,
            baselines: 0,
            governance: false,
            tags: tags(&[]),
        },
        Decommission {
            case: "every control of the catalog a caller may enable, each on its own target",
            controls: enablable_controls().len(),
            baselines: 0,
            governance: false,
            tags: tags(&[]),
        },
        Decommission {
            case: "baseline enablements only",
            controls: 0,
            baselines: ungoverning_baselines().len(),
            governance: false,
            tags: tags(&[]),
        },
        Decommission {
            case: "the governance baseline, so the service applies the mandatory controls itself",
            controls: 0,
            baselines: 0,
            governance: true,
            tags: tags(&[]),
        },
        Decommission {
            case: "controls, baselines and governance together, every enablement tagged",
            controls: 2,
            baselines: 2,
            governance: true,
            tags: tags(&[("Owner", "platform"), ("Cost-Centre", "4711")]),
        },
    ]
}

/// The catalog controls a CALLER may enable.
fn enablable_controls() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect()
}

/// The catalog baselines whose enablement establishes no governance, so enabling one
/// deploys a baseline enablement and nothing else.
fn ungoverning_baselines() -> Vec<&'static CatalogBaseline> {
    BASELINE_CATALOG
        .iter()
        .filter(|baseline| !baseline.establishes_governance)
        .collect()
}

/// The one catalog baseline whose enablement brings a target under governance.
fn governing_baseline() -> &'static CatalogBaseline {
    BASELINE_CATALOG
        .iter()
        .find(|baseline| baseline.establishes_governance)
        .expect("the catalog must publish a baseline that establishes governance")
}

/// The newest version a baseline offers: its version list is oldest first.
fn newest_version(baseline: &CatalogBaseline) -> &'static str {
    baseline
        .versions
        .last()
        .expect("every catalog baseline offers at least one version")
}

/// The number of control enablements a row leaves in the store: the ones the caller enabled
/// plus, when it established governance, the mandatory controls the service applies itself.
fn deployed_controls(row: &Decommission) -> usize {
    row.controls + if row.governance { mandatory().len() } else { 0 }
}

/// The number of baseline enablements a row leaves in the store.
fn deployed_baselines(row: &Decommission) -> usize {
    row.baselines + usize::from(row.governance)
}

/// Create the account's one landing zone, governing [`GOVERNED`], and wait for its `CREATE`
/// to become due so the decommissioning is not refused for an operation in flight.
async fn settled_landing_zone(router: &MultiServiceRouter, case: &str) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": VERSION, "manifest": manifest_governing(GOVERNED) }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: create-landingzone expected 200, got {} body {:?}",
        created.status, created.body
    );
    // Nothing sleeps server-side: the clock makes the create due, and the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// The `tags` member a row's enable calls carry, omitted when the row attaches none.
fn tag_member(attached: &BTreeMap<String, String>) -> Option<serde_json::Value> {
    (!attached.is_empty()).then(|| {
        serde_json::Value::Object(
            attached
                .iter()
                .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
                .collect(),
        )
    })
}

/// Deploy everything `row` names under the landing zone, answering the enablements the
/// CALLER created (the service's own are read back off the listing).
async fn deploy(router: &MultiServiceRouter, row: &Decommission) -> Vec<Enablement> {
    let case = row.case;
    let attached = tag_member(&row.tags);
    let mut created: Vec<Enablement> = Vec::new();

    for (index, entry) in enablable_controls().into_iter().take(row.controls).enumerate() {
        let mut body = serde_json::json!({
            "controlIdentifier": control(entry.name),
            "targetIdentifier": ou(&format!("dcc{index}")),
        });
        if let Some(attached) = &attached {
            body["tags"] = attached.clone();
        }
        let answer = call(router, "/enable-control", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: enable-control {} expected 200, got {} body {:?}",
            entry.name, answer.status, answer.body
        );
        created.push(Enablement {
            what: format!("the control {} a caller enabled", entry.name),
            kind: Kind::Control,
            arn: answer.string("arn"),
            created_by: answer.string("operationIdentifier"),
        });
    }

    let baselines = ungoverning_baselines()
        .into_iter()
        .take(row.baselines)
        .map(|baseline| (baseline, format!("dcb{}", baseline.id.to_lowercase())))
        .chain(
            row.governance
                .then(|| (governing_baseline(), "dcg".to_string())),
        );
    for (baseline, target) in baselines {
        let mut body = serde_json::json!({
            "baselineIdentifier": served_arn(baseline),
            "baselineVersion": newest_version(baseline),
            "targetIdentifier": ou(&target),
        });
        if let Some(attached) = &attached {
            body["tags"] = attached.clone();
        }
        let answer = call(router, "/enable-baseline", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: enable-baseline {} expected 200, got {} body {:?}",
            baseline.name, answer.status, answer.body
        );
        created.push(Enablement {
            what: format!("the baseline {} a caller enabled", baseline.name),
            kind: Kind::Baseline,
            arn: answer.string("arn"),
            created_by: answer.string("operationIdentifier"),
        });
    }

    created
}

/// The reporting call that resolves one enablement of `kind`, whatever it answers.
async fn get_enabled(router: &MultiServiceRouter, kind: Kind, arn: &str) -> Answer {
    let mut body = serde_json::Map::new();
    body.insert(
        kind.identifier_member().to_string(),
        serde_json::Value::String(arn.to_string()),
    );
    call(router, kind.get_path(), serde_json::Value::Object(body)).await
}

/// Every enablement of `kind` the listing reports, by ARN, sorted — so a case compares SETS
/// rather than depending on the minted-ARN order.
async fn listed(router: &MultiServiceRouter, case: &str, kind: Kind) -> Vec<String> {
    let listed = call(
        router,
        kind.list_path(),
        serde_json::json!({ "maxResults": kind.page_size() }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: {} expected 200, got {} body {:?}",
        kind.list_path(),
        listed.status,
        listed.body
    );
    let mut arns: Vec<String> = entries(listed.get(kind.list_member()))
        .iter()
        .filter_map(|entry| {
            entry
                .get("arn")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    arns.sort();
    arns
}

/// The `targetRegions` `GetEnabledControl` reports for the control `arn` names.
async fn target_regions(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<serde_json::Value> {
    let reported = get_enabled(router, Kind::Control, arn).await;
    assert_eq!(
        reported.status, 200,
        "case {case:?}: get-enabled-control {arn} expected 200, got {} body {:?}",
        reported.status, reported.body
    );
    entries(
        reported
            .get("enabledControlDetails")
            .get("targetRegions")
            .unwrap_or(&serde_json::Value::Null),
    )
}

/// The regions a control reports while the landing zone governs [`GOVERNED`].
fn governed_regions() -> Vec<serde_json::Value> {
    GOVERNED
        .iter()
        .map(|name| serde_json::json!({ "name": name }))
        .collect()
}

/// `GetLandingZone` for `identifier`, whatever it answers.
async fn get_landing_zone(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await
}

/// Start the decommissioning of `arn`, answering the `operationIdentifier` it returned.
async fn start_decommissioning(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let deleted = call(
        router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: delete-landingzone expected 200, got {} body {:?}",
        deleted.status, deleted.body
    );
    deleted.string("operationIdentifier")
}

/// Assert every enablement `created` names is still reported, still listed and still
/// carries `attached` — what the store holds while nothing has been deleted.
async fn assert_still_deployed(
    router: &MultiServiceRouter,
    case: &str,
    when: &str,
    created: &[Enablement],
    attached: &BTreeMap<String, String>,
) {
    for enablement in created {
        let reported = get_enabled(router, enablement.kind, &enablement.arn).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?} {when}: expected {} to still be reported, got {} body {:?}",
            enablement.what, reported.status, reported.body
        );
        let tagged = read_tags(router, &enablement.arn).await;
        assert_eq!(
            (tagged.status, reported_tags(&tagged)),
            (200, attached.clone()),
            "case {case:?} {when}: expected {} to still carry its tags, got {:?} body {:?}",
            enablement.what,
            tagged.status,
            tagged.body
        );
    }
}

/// RULE: applying a decommissioning deletes the Control Tower resources deployed under the
/// landing zone — every control enablement, the service's own as well as a caller's, and
/// every baseline enablement — together with their tags, and nothing is deleted while the
/// decommissioning is still in flight.
#[tokio::test]
async fn decommissioning_deletes_what_was_deployed_under_the_landing_zone() {
    for row in decommissionings() {
        let case = row.case;
        // The account holds one landing zone, and every row decommissions it.
        let router = emulator();
        let landing_zone = settled_landing_zone(&router, case).await;
        let created = deploy(&router, &row).await;
        // The enables are asynchronous too: wait for the clock to make them due, so what
        // the decommissioning removes is a deployed, settled set.
        tokio::time::sleep(past_the_settle_span()).await;

        // What is deployed under the landing zone, the service's own enablements included.
        let controls = listed(&router, case, Kind::Control).await;
        let baselines = listed(&router, case, Kind::Baseline).await;
        assert_eq!(
            (controls.len(), baselines.len()),
            (deployed_controls(&row), deployed_baselines(&row)),
            "case {case:?}: expected {} control and {} baseline enablements deployed, got \
             controls {controls:?} baselines {baselines:?}",
            deployed_controls(&row),
            deployed_baselines(&row)
        );
        assert_still_deployed(&router, case, "before the delete", &created, &row.tags).await;
        for arn in &controls {
            let regions = target_regions(&router, case, arn).await;
            assert_eq!(
                regions,
                governed_regions(),
                "case {case:?}: expected {arn} to be deployed into the governed regions {:?}, \
                 got {regions:?}",
                governed_regions()
            );
        }

        // The decommissioning is asynchronous: until the clock makes it due it has deleted
        // nothing.
        let operation = start_decommissioning(&router, case, &landing_zone).await;
        assert_still_deployed(&router, case, "during the delete", &created, &row.tags).await;
        assert_eq!(
            (
                listed(&router, case, Kind::Control).await,
                listed(&router, case, Kind::Baseline).await,
            ),
            (controls.clone(), baselines.clone()),
            "case {case:?}: expected the in-flight decommissioning to delete nothing, \
             expected controls {controls:?} baselines {baselines:?}"
        );

        tokio::time::sleep(past_the_settle_span()).await;

        // The landing zone is decommissioned, and so is everything deployed under it.
        let gone = get_landing_zone(&router, &landing_zone).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the landing zone to be decommissioned, got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
        for kind in [Kind::Control, Kind::Baseline] {
            let left = listed(&router, case, kind).await;
            assert!(
                left.is_empty(),
                "case {case:?}: expected {} to report nothing once the landing zone is \
                 decommissioned, got {left:?}",
                kind.list_path()
            );
        }
        for enablement in &created {
            let refused = get_enabled(&router, enablement.kind, &enablement.arn).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected {} to be gone with the landing zone, got {:?} body \
                 {:?}",
                enablement.what,
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            let untagged = read_tags(&router, &enablement.arn).await;
            assert_eq!(
                (untagged.status, untagged.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the tags of {} to go with it, got {:?} body {:?}",
                enablement.what,
                (untagged.status, untagged.error_code.as_deref()),
                untagged.body
            );
            // A log record is not a resource: the identifier the enabling call handed back
            // still resolves, exactly as the decommissioning's own does.
            let logged = call(
                &router,
                enablement.kind.operation_path(),
                serde_json::json!({ "operationIdentifier": enablement.created_by }),
            )
            .await;
            assert_eq!(
                logged.status, 200,
                "case {case:?}: expected the operation that created {} to still resolve, got \
                 {} body {:?}",
                enablement.what, logged.status, logged.body
            );
        }
        let decommissioning = call(
            &router,
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": operation }),
        )
        .await;
        assert_eq!(
            decommissioning.status, 200,
            "case {case:?}: expected the DELETE record to outlive the landing zone, got {} \
             body {:?}",
            decommissioning.status, decommissioning.body
        );
    }
}

/// RULE: a decommissioning that did not happen deletes nothing — a `DELETE` whose own
/// operation FAILED leaves the landing zone AND everything deployed under it where they
/// are, and the same call, allowed to succeed, deletes all of it.
#[tokio::test]
async fn a_failed_decommissioning_deletes_nothing_it_was_going_to_delete() {
    let rows = decommissionings()
        .into_iter()
        .filter(|row| deployed_controls(row) + deployed_baselines(row) > 0);

    for row in rows {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        let landing_zone = settled_landing_zone(&router, case).await;
        let created = deploy(&router, &row).await;
        tokio::time::sleep(past_the_settle_span()).await;

        let controls = listed(&router, case, Kind::Control).await;
        let baselines = listed(&router, case, Kind::Baseline).await;

        // Nothing in this crate fails a landing-zone operation, so the failure is seeded
        // into the log the way the sibling landing-zone suite seeds it.
        let failed = start_decommissioning(&router, case, &landing_zone).await;
        fail_the_operation(&engine, &failed);

        tokio::time::sleep(past_the_settle_span()).await;

        let survived = get_landing_zone(&router, &landing_zone).await;
        assert_eq!(
            survived.status, 200,
            "case {case:?}: expected the landing zone to survive the failed decommissioning, \
             got {} body {:?}",
            survived.status, survived.body
        );
        assert_still_deployed(
            &router,
            case,
            "after the failed decommissioning",
            &created,
            &row.tags,
        )
        .await;
        assert_eq!(
            (
                listed(&router, case, Kind::Control).await,
                listed(&router, case, Kind::Baseline).await,
            ),
            (controls.clone(), baselines.clone()),
            "case {case:?}: expected the failed decommissioning to delete nothing, expected \
             controls {controls:?} baselines {baselines:?}"
        );

        // Not vacuous: the same call, with its operation left to succeed, deletes exactly
        // what the failed one did not.
        let succeeding = start_decommissioning(&router, case, &landing_zone).await;
        assert_ne!(
            succeeding, failed,
            "case {case:?}: expected the repeated delete to start a FRESH operation"
        );
        tokio::time::sleep(past_the_settle_span()).await;

        for kind in [Kind::Control, Kind::Baseline] {
            let left = listed(&router, case, kind).await;
            assert!(
                left.is_empty(),
                "case {case:?}: expected {} to report nothing once the decommissioning \
                 succeeded, got {left:?}",
                kind.list_path()
            );
        }
    }
}
