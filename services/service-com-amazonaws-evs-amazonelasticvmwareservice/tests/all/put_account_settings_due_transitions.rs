// SPDX-License-Identifier: CC0-1.0

//! The lifecycle transitions a call that RECOMPUTES the environment health checks has to
//! materialise first, end to end over awsJson1_0.
//!
//! THE RULE under test: `Environment$checks` is derived from the stored environments —
//! each one's `EnvironmentState`, the hosts and connectors it holds and the account's own
//! settings (`smithy_query(query="shape", shape="Check,CheckType,CheckResult")` and the
//! `Environment` members) — so a call that recomputes those checks derives them from the
//! states that are OWED at that instant: it materialises the lifecycle transitions the
//! clock has already made due before recomputing, whether or not any earlier call did.
//! What a call leaves in the store is therefore what the next read reports, and no call
//! decides a check against a state the clock has already moved past.
//!
//! `PutAccountSettings` is a call of that class, because "The total number of VCF license
//! cores ported to Amazon EVS for the account in that Region" (`AccountSetting$name`) is a
//! `KEY_COVERAGE` input of every environment of the Region, so writing a setting changes
//! what the checks of all of them decide.
//!
//! The rows differ along what the rule generalises over: what has happened to the
//! environment's own transition when the call lands (overdue with nothing having called
//! since, already materialised, or not owed at all); whether the put writes a check input
//! or an empty list of settings; the ported core count it writes, over both sides of the
//! `KEY_COVERAGE` comparison and its exact boundary; and WHICH transition is overdue — an
//! environment's own create transition in the first table, and another environment's
//! deletion, which frees the VCF license key it carried, in the second.
//!
//! The harness (`emulator`, `call`, `settle`) is the one `environment_lifecycle` owns and
//! the check readers are `environment_check_input_recompute`'s, so every test drives the
//! service the way a client does: `POST /` with `X-Amz-Target:
//! AmazonElasticVMwareService.<Operation>`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::environment_check_input_recompute::{
    cores, get_environment, license_info, reported_check_results, result_of,
    stored_check_results, INSTANCE_TYPE,
};
use crate::environment_lifecycle::{
    call, emulator, error_type, settle, Emulator, PAST_THE_TRANSITION,
};
use crate::list_while_initializing::force_state;
use crate::tagging_resource_arn_resolution::stored_state;

/// The `CheckResult` values, written out from the model's own enumeration
/// (`smithy_query(query="shape", shape="CheckResult")`), so this suite states the results
/// instead of reading them back out of the code under test.
const PASSED: &str = "PASSED";
/// See [`PASSED`].
const FAILED: &str = "FAILED";
/// See [`PASSED`].
const UNKNOWN: &str = "UNKNOWN";

/// The `EnvironmentState` values this suite names, from the model's own enumeration
/// (`smithy_query(query="shape", shape="EnvironmentState")`): the state a create mints an
/// environment in, and the one the transition it owes moves it to.
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const CREATED: &str = "CREATED";

/// The checks a settled environment of this suite carries, as `(type, result)`, for the
/// `KEY_COVERAGE` result one row decides: it carries one VCF license no other environment
/// of its store carries (`KEY_REUSE` passes), is deployed with the minimum host count
/// (`HOST_COUNT` passes) and holds no connector (`CONNECTOR_HEALTH` aggregates nothing).
fn settled_checks(key_coverage: &str) -> BTreeMap<String, String> {
    [
        (evs::CHECK_TYPE_KEY_REUSE, PASSED),
        (evs::CHECK_TYPE_KEY_COVERAGE, key_coverage),
        (evs::CHECK_TYPE_HOST_COUNT, PASSED),
        (evs::CHECK_TYPE_CONNECTOR_HEALTH, UNKNOWN),
    ]
    .iter()
    .map(|(check_type, result)| (check_type.to_string(), result.to_string()))
    .collect()
}

/// Create an environment carrying one VCF license and `host_count` hosts, and return its
/// id — WITHOUT settling the transition the clock owes it, because every row here times
/// its call against that pending transition.
///
/// The hosts are named after the environment, because "DNS hostnames for hosts must be
/// unique across Amazon EVS environments" (`HostInfoForCreate$hostName` doc).
async fn create_unsettled(
    router: &Emulator,
    case: &str,
    name: &str,
    host_count: usize,
) -> String {
    let mut request = router.create_request(Some(name));
    request["licenseInfo"] = license_info();
    if host_count > 0 {
        request["hosts"] = (0..host_count)
            .map(|ordinal| {
                json!({
                    "hostName": format!("{name}-{ordinal}"),
                    "keyName": "evs-key",
                    "instanceType": INSTANCE_TYPE,
                })
            })
            .collect();
    }
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create {name}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: create {name}: no environmentId in {body}"))
        .to_string()
}

/// Call `PutAccountSettings` with `settings`, failing the test if it is refused.
async fn put_account_settings(router: &Emulator, case: &str, settings: Value) {
    let (status, body) = call(
        router,
        "PutAccountSettings",
        &json!({ "settings": settings }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the put expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
}

/// What has happened to the environment's own create transition by the time the put lands.
#[derive(Debug)]
enum Stage {
    /// The transition has come due and NOTHING has called since, so the put is the first
    /// call that can materialise it.
    OverdueAndUntouched,
    /// An earlier call already materialised it.
    AlreadyMaterialised,
    /// The clock owes the environment nothing, so there is no transition to materialise
    /// and it stays where it is.
    NothingDue,
}

/// One row of the first table.
struct Row {
    case: &'static str,
    stage: Stage,
    /// The ported core count the put writes, as a delta from the cores the environment's
    /// deployed hosts need — both sides of the `KEY_COVERAGE` comparison and its exact
    /// boundary — or `None` for a put that writes no setting at all, which `settings`
    /// (`len 0..100`) models.
    ported_delta: Option<i64>,
    /// The `environmentState` the put must leave in the store.
    expected_state: &'static str,
    /// The `KEY_COVERAGE` of the checks the put must leave in the store, or `None` when it
    /// must leave none at all: no check of an environment that is still being created has
    /// run.
    expected_key_coverage: Option<&'static str>,
}

#[tokio::test]
async fn a_put_recomputes_the_checks_against_the_transitions_the_clock_has_made_due() {
    let rows = [
        Row {
            case: "the first call after the create transition came due, the hosts' cores \
                   exactly ported",
            stage: Stage::OverdueAndUntouched,
            ported_delta: Some(0),
            expected_state: CREATED,
            expected_key_coverage: Some(PASSED),
        },
        Row {
            case: "the first call after the create transition came due, one core short of \
                   the hosts",
            stage: Stage::OverdueAndUntouched,
            ported_delta: Some(-1),
            expected_state: CREATED,
            expected_key_coverage: Some(FAILED),
        },
        Row {
            case: "the first call after the create transition came due, one core more than \
                   the hosts need",
            stage: Stage::OverdueAndUntouched,
            ported_delta: Some(1),
            expected_state: CREATED,
            expected_key_coverage: Some(PASSED),
        },
        Row {
            case: "the first call after the create transition came due, far more cores \
                   ported than the hosts need",
            stage: Stage::OverdueAndUntouched,
            ported_delta: Some(4096),
            expected_state: CREATED,
            expected_key_coverage: Some(PASSED),
        },
        Row {
            case: "the first call after the create transition came due writes no setting \
                   at all",
            stage: Stage::OverdueAndUntouched,
            ported_delta: None,
            // No ported core count has been set, so how many cores the account's licenses
            // allocate is not known and the check is undecidable.
            expected_state: CREATED,
            expected_key_coverage: Some(UNKNOWN),
        },
        Row {
            case: "an earlier call already materialised the transition, the hosts' cores \
                   exactly ported",
            stage: Stage::AlreadyMaterialised,
            ported_delta: Some(0),
            expected_state: CREATED,
            expected_key_coverage: Some(PASSED),
        },
        Row {
            case: "an earlier call already materialised the transition, one core short of \
                   the hosts",
            stage: Stage::AlreadyMaterialised,
            ported_delta: Some(-1),
            expected_state: CREATED,
            expected_key_coverage: Some(FAILED),
        },
        Row {
            case: "the environment is still being created and owes no transition",
            stage: Stage::NothingDue,
            ported_delta: Some(0),
            expected_state: CREATING,
            expected_key_coverage: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let Row {
            case,
            stage,
            ported_delta,
            expected_state,
            expected_key_coverage,
        } = row;
        // One emulator per row: the account's settings and the license keys the store
        // holds are the check inputs under test, so no row inherits another's.
        let router = emulator();
        let name = format!("due-transition-{index}");
        let environment_id =
            create_unsettled(&router, case, &name, evs::MINIMUM_HOST_COUNT).await;
        match stage {
            // The clock really passes the due instant, with no call in between: the put is
            // the first call that can materialise what it made due.
            Stage::OverdueAndUntouched => tokio::time::sleep(PAST_THE_TRANSITION).await,
            Stage::AlreadyMaterialised => settle(&router),
            // Left in the state it is in, with the transition it owed dropped.
            Stage::NothingDue => force_state(&router, &environment_id, CREATING),
        }

        // The cores the environment's deployed hosts run on: what `KEY_COVERAGE` compares
        // the account's ported cores against.
        let required = i64::try_from(
            cores(INSTANCE_TYPE) * u64::try_from(evs::MINIMUM_HOST_COUNT).unwrap(),
        )
        .expect("the cores of four hosts fit in an i64");
        let settings = match ported_delta {
            Some(delta) => json!([{
                "name": evs::VCF_PORTED_CORE_COUNT,
                "value": (required + delta).to_string(),
            }]),
            None => json!([]),
        };
        put_account_settings(&router, case, settings).await;

        // What the put LEFT in the store, captured before any read recomputes it.
        let left_behind = stored_check_results(&router, &environment_id);
        let state_left_behind = stored_state(&router, &environment_id);
        assert_eq!(
            state_left_behind, *expected_state,
            "case {case:?}: after {stage:?} the put must leave the environment in \
             {expected_state}, got {state_left_behind}"
        );
        let stored: BTreeMap<String, String> = left_behind.iter().cloned().collect();
        let expected = match expected_key_coverage {
            Some(key_coverage) => settled_checks(key_coverage),
            None => BTreeMap::new(),
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: after {stage:?} the put must leave the checks {expected:?}, got \
             {stored:?}"
        );

        // The invariant: the checks a call leaves in the store are the checks the next
        // read reports, because the state the clock owed is accounted for once.
        let after = get_environment(&router, case, &environment_id).await;
        let reported = reported_check_results(&after);
        assert_eq!(
            left_behind, reported,
            "case {case:?}: the stored checks {left_behind:?} must be the checks the next \
             read reports, got {reported:?}"
        );
        assert_eq!(
            after["environmentState"],
            json!(expected_state),
            "case {case:?}: the read must report {expected_state}, got {}",
            after["environmentState"]
        );
    }
}

#[tokio::test]
async fn a_put_recomputes_key_reuse_against_a_peers_deletion_the_clock_has_made_due() {
    // Rows: whether the peer environment's deletion transition has come due, with nothing
    // having called since, when the put lands — and the `KEY_REUSE` the surviving
    // environment must carry. "Verifies that the VCF license key is not used by another
    // Amazon EVS environment" (`Check$type`), and an environment that has reached DELETED
    // carries its license key no longer, so the key is free for the survivor again.
    let rows = [
        (
            "the peer's deletion came due and nothing has called since",
            true,
            PASSED,
        ),
        ("the peer is still being deleted", false, FAILED),
    ];
    for (case, deletion_came_due, expected) in rows {
        let router = emulator();
        // Both environments carry the SAME VCF license, so each is the other's reuse.
        let survivor =
            create_unsettled(&router, case, "reuse-survivor", evs::MINIMUM_HOST_COUNT).await;
        // The peer holds no host, because "Amazon EVS environments will only be enabled
        // for deletion once the hosts are deleted" (`DeleteEnvironment` doc).
        let peer = create_unsettled(&router, case, "reuse-peer", 0).await;
        settle(&router);

        let (status, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": peer }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete the peer: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        if deletion_came_due {
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }
        put_account_settings(
            &router,
            case,
            json!([{ "name": evs::VCF_PORTED_CORE_COUNT, "value": "1" }]),
        )
        .await;

        // What the put LEFT in the store for the SURVIVOR, captured before any read
        // recomputes it.
        let left_behind = stored_check_results(&router, &survivor);
        let actual = result_of(&left_behind, evs::CHECK_TYPE_KEY_REUSE);
        assert_eq!(
            actual, expected,
            "case {case:?}: the put must leave {} {expected}, got {actual} in {left_behind:?}",
            evs::CHECK_TYPE_KEY_REUSE
        );

        let after = get_environment(&router, case, &survivor).await;
        let reported = reported_check_results(&after);
        assert_eq!(
            left_behind, reported,
            "case {case:?}: the stored checks {left_behind:?} must be the checks the next \
             read reports, got {reported:?}"
        );
    }
}
