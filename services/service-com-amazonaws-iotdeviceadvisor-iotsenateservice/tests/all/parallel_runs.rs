// SPDX-License-Identifier: CC0-1.0

//! `suiteRunConfiguration.parallelRun`, over the wire.
//!
//! The member the model documents as making "multiple test suites run in parallel" is what
//! decides whether a second run of one suite definition is accepted while an earlier run
//! of it is still in progress: two runs share a definition only when BOTH asked for a
//! parallel run, and otherwise the earlier one holds the definition and the next start is
//! refused with the modelled `ConflictException`.
//!
//! The table spans the class the rule generalises over — every combination of the member's
//! three wire forms (absent, `false`, `true`) on the run already in progress and on the
//! request — rather than the one body the issue cited, and the cap on runs that DO share a
//! definition is asserted at its bound and one past it. Every case drives real HTTP
//! requests through the same `MultiServiceRouter` the emulator serves on a socket (the
//! harness `suite_lifecycle` owns).

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{run_configuration, thing_arn, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// The status-transition span this service promises callers, as the state module fixes it.
fn span() -> Duration {
    iotdeviceadvisor::state::STATUS_TRANSITION
}

/// Create one suite definition and hand back its id.
async fn definition(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": name,
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": thing_arn(0) }],
            }),
            REGION,
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: creating {name:?} failed with {} {}",
        created.status, created.body
    );
    created.string("suiteDefinitionId")
}

/// A second run of a definition is accepted while the first is in progress exactly when
/// both configurations ask for a parallel run; every other combination of the member's
/// three wire forms is the modelled conflict.
#[tokio::test]
async fn parallel_run_decides_whether_a_concurrent_start_is_accepted() {
    // Every combination of the member on the run already in progress and on the request.
    // `None` is the ABSENT member, which the model leaves optional.
    let forms: [Option<bool>; 3] = [None, Some(false), Some(true)];

    for first in forms {
        for second in forms {
            let case = format!("in progress parallelRun={first:?}, request parallelRun={second:?}");
            let emulator = Emulator::new();
            let definition_id = definition(&emulator, "parallel-suite").await;
            let runs_path = format!("/suiteDefinitions/{definition_id}/suiteRuns");
            let both_parallel = first == Some(true) && second == Some(true);

            let first_configuration = run_configuration(&thing_arn(1), None, first);
            let started = emulator
                .call(
                    "POST",
                    &runs_path,
                    REGION,
                    Some(json!({ "suiteRunConfiguration": first_configuration })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: the first start must be accepted, got {} {}",
                started.status, started.body
            );
            let first_id = started.string("suiteRunId");

            let second_configuration = run_configuration(&thing_arn(2), None, second);
            let answer = emulator
                .call(
                    "POST",
                    &runs_path,
                    REGION,
                    Some(json!({ "suiteRunConfiguration": second_configuration })),
                )
                .await;

            if !both_parallel {
                assert_eq!(
                    (answer.status, answer.error_code.as_deref()),
                    (400, Some("ConflictException")),
                    "case {case:?}: a concurrent start only both configurations asked for \
                     must be refused, got {} {}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.member("message").as_str(),
                    Some(
                        format!(
                            "Suite run {first_id} is still in progress for suite definition \
                             {definition_id}"
                        )
                        .as_str()
                    ),
                    "case {case:?}: the refusal must name the run holding the definition and \
                     the definition, got {}",
                    answer.body
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: two runs that both asked for a parallel run must both be \
                 accepted, got {} {}",
                answer.status, answer.body
            );
            let second_id = answer.string("suiteRunId");
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a second accepted start must mint a second run"
            );

            // Round trip: both runs are stored, readable, and each echoes its own
            // configuration — the second start did not replace or disturb the first.
            for (which, run_id, configuration) in [
                ("first", &first_id, &first_configuration),
                ("second", &second_id, &second_configuration),
            ] {
                let fetched = emulator
                    .call("GET", &format!("{runs_path}/{run_id}"), REGION, None)
                    .await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: the {which} parallel run must be readable, got {} {}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.string("status"),
                    "PENDING",
                    "case {case:?}: the {which} parallel run is still PENDING straight after \
                     the start, got {}",
                    fetched.body
                );
                assert_eq!(
                    fetched.member("suiteRunConfiguration"),
                    configuration,
                    "case {case:?}: the {which} parallel run must echo its own stored \
                     configuration, got {}",
                    fetched.body
                );
            }

            let listed = emulator
                .call(
                    "GET",
                    &format!("/suiteRuns?suiteDefinitionId={definition_id}"),
                    REGION,
                    None,
                )
                .await;
            let mut ids: Vec<String> = listed
                .member("suiteRunsList")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry["suiteRunId"].as_str().unwrap_or_default().to_string())
                .collect();
            ids.sort();
            let mut expected = vec![first_id.clone(), second_id.clone()];
            expected.sort();
            assert_eq!(
                ids, expected,
                "case {case:?}: the listing must report both runs in progress, got {}",
                listed.body
            );
        }
    }
}

/// Runs that share a definition are capped: starts up to the bound are accepted, the one
/// past it is refused with the modelled conflict, and the bound is on OCCUPANCY — once the
/// runs have finished another start is accepted again.
#[tokio::test]
async fn runs_sharing_a_definition_are_capped() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "parallel-cap-suite").await;
    let runs_path = format!("/suiteDefinitions/{definition_id}/suiteRuns");
    let cap = iotdeviceadvisor::state::MAX_PARALLEL_RUNS_PER_DEFINITION;
    let start = |index: usize| {
        let runs_path = runs_path.clone();
        let emulator = &emulator;
        async move {
            emulator
                .call(
                    "POST",
                    &runs_path,
                    REGION,
                    Some(json!({
                        "suiteRunConfiguration":
                            run_configuration(&thing_arn(index), None, Some(true)),
                    })),
                )
                .await
        }
    };

    for index in 0..cap {
        let answer = start(index).await;
        assert_eq!(
            answer.status, 200,
            "case \"parallel run {index} of the cap\": expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
    }

    let refused = start(cap).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ConflictException")),
        "case \"one parallel run past the cap\": expected a 400 ConflictException, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.member("message").as_str(),
        Some(
            format!(
                "Suite definition {definition_id} already has {cap} suite runs in progress, \
                 which is the maximum number that can run in parallel"
            )
            .as_str()
        ),
        "case \"one parallel run past the cap\": the refusal must say how many runs hold the \
         definition, got {}",
        refused.body
    );

    // Two transition spans take every one of those runs to a terminal status, which frees
    // the definition: the cap bounds how many runs are in progress at once, it is not a
    // lifetime quota.
    tokio::time::sleep(span() * 5 / 2).await;
    let accepted = start(cap).await;
    assert_eq!(
        accepted.status, 200,
        "case \"a parallel run once the earlier ones finished\": expected HTTP 200, got {} {}",
        accepted.status, accepted.body
    );
}
