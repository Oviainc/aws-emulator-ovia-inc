// SPDX-License-Identifier: CC0-1.0

//! `UpdateMatchmakingConfiguration` and the backfill method, over the wire: the operation
//! stores and reports the `BackfillMode` and `FlexMatchMode` a request supplies in EVERY
//! combination of the two enums, because neither member constrains the other — the backfill
//! note "Automatic backfill is not available when FlexMatchMode is set to STANDALONE"
//! describes what the feature does in that mode, not a request that fails. A combination
//! whose documentation DOES state that the request fails (more than one queue, a queue
//! under the standalone mode) is still refused, and writes nothing.
//!
//! The harness (`GameLift`) and the matchmaker fixtures are the ones `hosting_loop` and
//! `resource_updates` already drive the production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::GameLift;
use super::resource_updates::{create_matchmaker, described_configuration, only_changed};

/// Every value of `UpdateMatchmakingConfigurationInput$BackfillMode` (`smithy_query
/// shape=BackfillMode`: AUTOMATIC | MANUAL), plus the member left out — an update supplies
/// it or keeps what the configuration carries.
const BACKFILL_MODES: [Option<&str>; 3] = [Some("AUTOMATIC"), Some("MANUAL"), None];

/// Every value of `UpdateMatchmakingConfigurationInput$FlexMatchMode` (`smithy_query
/// shape=FlexMatchMode`: STANDALONE | WITH_QUEUE), plus the member left out.
const FLEX_MATCH_MODES: [Option<&str>; 3] = [Some("STANDALONE"), Some("WITH_QUEUE"), None];

/// Every combination of the two enums (and of each one left out) is accepted on a stored
/// standalone configuration: the update answers 200, reports the configuration it stored,
/// and an independent `DescribeMatchmakingConfigurations` serves exactly the supplied
/// values back with every other setting untouched.
#[tokio::test]
async fn update_matchmaking_configuration_stores_every_backfill_and_flexmatch_combination() {
    let gamelift = GameLift::new();

    for (index, (flex_match_mode, backfill_mode)) in FLEX_MATCH_MODES
        .iter()
        .flat_map(|flex| BACKFILL_MODES.iter().map(move |backfill| (*flex, *backfill)))
        .enumerate()
    {
        let case = format!("FlexMatchMode={flex_match_mode:?}, BackfillMode={backfill_mode:?}");
        // The fixture is a STANDALONE configuration whose backfill method is the service's
        // default, so every row below is the standalone case the note talks about.
        let name = create_matchmaker(&gamelift, &format!("backfill-matrix-{index}")).await;
        let before = described_configuration(&gamelift, &name).await;
        assert_eq!(
            (before["FlexMatchMode"].clone(), before["BackfillMode"].clone()),
            (json!("STANDALONE"), json!("MANUAL")),
            "case {case:?}: the fixture must start standalone with manual backfill, got {before}"
        );

        let mut request = json!({"Name": &name});
        let mut expected: Vec<(&str, Value)> = Vec::new();
        if let Some(flex_match_mode) = flex_match_mode {
            request["FlexMatchMode"] = json!(flex_match_mode);
            expected.push(("FlexMatchMode", json!(flex_match_mode)));
        }
        if let Some(backfill_mode) = backfill_mode {
            request["BackfillMode"] = json!(backfill_mode);
            expected.push(("BackfillMode", json!(backfill_mode)));
        }

        let updated = gamelift
            .ok("UpdateMatchmakingConfiguration", request)
            .await;
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            updated["Configuration"], after,
            "case {case:?}: the update must report the configuration the next describe serves, got {}",
            updated["Configuration"]
        );
        assert_eq!(
            after,
            only_changed(&before, &expected),
            "case {case:?}: the update must store exactly the supplied settings, got {after}"
        );
    }
}

/// A standalone configuration that already backfills automatically stays updatable: every
/// unrelated setting a later request supplies is written through, and the backfill method
/// it is not asked about keeps the value it carries.
///
/// The rows differ along which settings the request supplies and the type of each one
/// (string, integer at each modelled bound, boolean, list), down to a request that supplies
/// no setting at all.
#[tokio::test]
async fn an_automatic_standalone_configuration_still_takes_every_unrelated_update() {
    struct Case {
        label: &'static str,
        update: Value,
        expected: Vec<(&'static str, Value)>,
    }

    let cases = [
        Case {
            label: "the description alone",
            update: json!({"Description": "a standalone matchmaker that backfills itself"}),
            expected: vec![(
                "Description",
                json!("a standalone matchmaker that backfills itself"),
            )],
        },
        Case {
            // `MatchmakingRequestTimeoutInteger`, range 1..43200 (`smithy_query
            // operation=UpdateMatchmakingConfiguration`).
            label: "the request timeout at the modelled minimum",
            update: json!({"RequestTimeoutSeconds": 1}),
            expected: vec![("RequestTimeoutSeconds", json!(1))],
        },
        Case {
            label: "the request timeout at the modelled maximum",
            update: json!({"RequestTimeoutSeconds": 43_200}),
            expected: vec![("RequestTimeoutSeconds", json!(43_200))],
        },
        Case {
            label: "player acceptance turned on with the time players are given",
            update: json!({"AcceptanceRequired": true, "AcceptanceTimeoutSeconds": 600}),
            expected: vec![
                ("AcceptanceRequired", json!(true)),
                ("AcceptanceTimeoutSeconds", json!(600)),
            ],
        },
        Case {
            label: "two game properties, a list setting",
            update: json!({
                "GameProperties": [
                    {"Key": "difficulty", "Value": "novice"},
                    {"Key": "map", "Value": "arena"},
                ],
            }),
            expected: vec![(
                "GameProperties",
                json!([
                    {"Key": "difficulty", "Value": "novice"},
                    {"Key": "map", "Value": "arena"},
                ]),
            )],
        },
        Case {
            label: "no setting at all",
            update: json!({}),
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = create_matchmaker(&gamelift, &format!("automatic-standalone-{index}")).await;
        gamelift
            .ok(
                "UpdateMatchmakingConfiguration",
                json!({"Name": &name, "BackfillMode": "AUTOMATIC"}),
            )
            .await;
        let before = described_configuration(&gamelift, &name).await;
        assert_eq!(
            (before["FlexMatchMode"].clone(), before["BackfillMode"].clone()),
            (json!("STANDALONE"), json!("AUTOMATIC")),
            "case {label:?}: the fixture must stand on automatic backfill, got {before}"
        );

        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let updated = gamelift
            .ok("UpdateMatchmakingConfiguration", request)
            .await;
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            updated["Configuration"], after,
            "case {label:?}: the update must report the stored configuration, got {}",
            updated["Configuration"]
        );
        assert_eq!(
            after,
            only_changed(&before, &case.expected),
            "case {label:?}: the update must change only the supplied settings, got {after}"
        );
    }
}

/// The combinations whose documentation states the request FAILS are still refused, even in
/// a request that also names the backfill method: "a matchmaking configuration supports only
/// one queue; if you specify more than one ARN, the request fails with an
/// InvalidRequestException. If FlexMatchMode is set to STANDALONE, do not set this
/// parameter."
#[tokio::test]
async fn update_matchmaking_configuration_still_refuses_the_queue_combinations_documented_to_fail()
{
    let gamelift = GameLift::new();
    let name = create_matchmaker(&gamelift, "queue-combinations").await;
    let before = described_configuration(&gamelift, &name).await;

    let refused: Vec<(&str, Value)> = vec![
        (
            "two queues, where a configuration supports only one",
            json!({
                "FlexMatchMode": "WITH_QUEUE",
                "BackfillMode": "AUTOMATIC",
                "GameSessionQueueArns": [
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one",
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/two",
                ],
            }),
        ),
        (
            "a queue on a configuration that stays standalone",
            json!({
                "BackfillMode": "AUTOMATIC",
                "GameSessionQueueArns": [
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one",
                ],
            }),
        ),
    ];

    for (label, update) in &refused {
        let mut request = update.clone();
        request["Name"] = json!(&name);
        let (status, code) = gamelift
            .err("UpdateMatchmakingConfiguration", request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the update must be refused, got {status} {code}"
        );
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused update must write nothing, got {after}"
        );
    }
}
