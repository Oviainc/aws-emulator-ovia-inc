// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfig` (`DELETE /config/{configType}/{configId}`) against the references other
//! resources of this service hold on the config, end to end over HTTP through the same
//! front door as the rest of this target's suites.
//!
//! The RULE under test: a config is removed only while NOTHING this service still holds
//! names it by ARN, whatever kind of resource holds the reference — a mission profile
//! through any of its `ConfigArn` members, or another config through a `ConfigArn` member
//! of its own `configData`, which is `uplinkEchoConfig$antennaUplinkConfigArn`
//! (`smithy_query(query="shape", shape="ConfigTypeData,UplinkEchoConfig")` — the only
//! `ConfigArn`-targeted member of the eight capability bodies). While such a reference is
//! live the delete is refused as a bad parameter (`InvalidParameterException`, HTTP 431)
//! naming the holders, and nothing is written; once the last holder has let the reference
//! go, the same delete goes through.
//!
//! The suites assert that over SETS of inputs rather than one call:
//!
//! * the holder KINDS, one table row per kind, each refusing the same delete and each
//!   naming its own holders;
//! * the reference FORMS and COUNTS an echoing config can hold: the referenced ARN whole
//!   and the same ARN with a node appended (both forms the modelled `ConfigArn` pattern
//!   admits), over one and several echoing configs;
//! * the guard reading LIVE state: repointing an echo with `UpdateConfig` frees the config
//!   it no longer names and holds the one it now does.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    config_in_use_by_configs_message, config_in_use_message,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, create_config, create_tracking_config, harness, send, Reply,
};

/// The node a reference may append to the ARN of the config it addresses: the modelled
/// `ConfigArn` pattern ends in `config/<type>/<uuid>(/.{1,256})?`
/// (`smithy_query(query="shape", shape="UplinkEchoConfig")` carries it on
/// `antennaUplinkConfigArn`), so this form addresses the same config as the bare ARN.
const REFERENCE_NODE: &str = "uplink-node-7";

/// How an echoing config names the config it echoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceForm {
    /// The referenced config's ARN, as a read of it reports it.
    WholeArn,
    /// That ARN with one node appended — the optional trailing segment above.
    ArnWithNode,
}

impl ReferenceForm {
    fn reference(self, config_arn: &str) -> String {
        match self {
            Self::WholeArn => config_arn.to_string(),
            Self::ArnWithNode => format!("{config_arn}/{REFERENCE_NODE}"),
        }
    }
}

/// `DELETE /config/{configType}/{configId}`.
async fn delete_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// `GET /config/{configType}/{configId}`.
async fn get_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// An `antenna-uplink` config — the one capability an `uplinkEchoConfig` may echo — and
/// its `(configId, configArn)`.
async fn create_uplink_config(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (config_id, _, config_arn) = create_config(
        router,
        name,
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;
    (config_id, config_arn)
}

/// An `uplink-echo` config echoing `reference`, and its `(configId, configArn)`.
async fn create_echo_config(
    router: &MultiServiceRouter,
    name: &str,
    reference: &str,
) -> (String, String) {
    let (config_id, _, config_arn) = create_config(
        router,
        name,
        json!({"uplinkEchoConfig": {"enabled": true, "antennaUplinkConfigArn": reference}}),
        None,
    )
    .await;
    (config_id, config_arn)
}

/// `PUT /config/uplink-echo/{configId}` — repoint an echo at `reference`.
async fn repoint_echo_config(
    router: &MultiServiceRouter,
    config_id: &str,
    name: &str,
    reference: &str,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/config/uplink-echo/{config_id}"),
        Some(json!({
            "name": name,
            "configData": {
                "uplinkEchoConfig": {"enabled": true, "antennaUplinkConfigArn": reference},
            },
        })),
    )
    .await
}

/// The ARN an echo config reports for the config it echoes.
fn echoed_arn(reply: &Reply) -> String {
    reply
        .field("configData")
        .get("uplinkEchoConfig")
        .and_then(|echo| echo.get("antennaUplinkConfigArn"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert `reply` is the in-use refusal carrying `expected_message`.
fn assert_in_use(case: &str, reply: &Reply, expected_message: &str) {
    assert_eq!(
        reply.status, 431,
        "case {case}: deleting a referenced config must answer 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterException",
        "case {case}: expected InvalidParameterException, got {}",
        reply.body
    );
    let message = reply.string("message");
    assert_eq!(
        message, expected_message,
        "case {case}: expected the refusal to read {expected_message:?}, got {message:?}"
    );
}

#[tokio::test]
async fn every_kind_of_resource_holding_a_config_reference_blocks_the_delete() {
    let router = harness();

    // The one table row per holder KIND: a config another CONFIG echoes, and a config a
    // MISSION PROFILE wires into a dataflow. Both are references to the same
    // antenna-uplink config, so the rows differ only in what holds the reference.
    for case in ["echoing config", "mission profile"] {
        let (uplink_id, uplink_arn) =
            create_uplink_config(&router, &format!("uplink held by an {case}")).await;

        let expected = match case {
            "echoing config" => {
                let (_, echo_arn) = create_echo_config(
                    &router,
                    &format!("echo of the uplink for {case}"),
                    &uplink_arn,
                )
                .await;
                config_in_use_by_configs_message(std::slice::from_ref(&echo_arn))
            }
            _ => {
                // A dataflow runs FROM a dataflow endpoint TO an antenna uplink
                // (`state::DATAFLOW_EDGE_CAPABILITY_PAIRS`), so the profile's graph is
                // what holds this config.
                let tracking_arn =
                    create_tracking_config(&router, &format!("tracking for {case}")).await;
                let (_, _, endpoint_arn) = create_config(
                    &router,
                    &format!("endpoint for {case}"),
                    json!({"dataflowEndpointConfig": {"dataflowEndpointName": "uplink-source"}}),
                    None,
                )
                .await;
                let created = send(
                    &router,
                    "POST",
                    "/missionprofile",
                    Some(json!({
                        "name": format!("profile for {case}"),
                        "minimumViableContactDurationSeconds": 60,
                        "trackingConfigArn": tracking_arn,
                        "dataflowEdges": [[endpoint_arn, uplink_arn]],
                    })),
                )
                .await;
                assert_eq!(
                    created.status, 200,
                    "case {case}: expected CreateMissionProfile 200, got {} {}",
                    created.status, created.body
                );
                let profile_id = created.string("missionProfileId");
                let profile_arn = send(
                    &router,
                    "GET",
                    &format!("/missionprofile/{profile_id}"),
                    None,
                )
                .await
                .string("missionProfileArn");
                config_in_use_message(std::slice::from_ref(&profile_arn))
            }
        };

        let refused = delete_config(&router, "antenna-uplink", &uplink_id).await;
        assert_in_use(case, &refused, &expected);

        // Nothing was written on the refused path: the config is still readable, so the
        // holder's reference still resolves.
        let still_there = get_config(&router, "antenna-uplink", &uplink_id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case}: a refused delete must leave the config readable, got {} {}",
            still_there.status, still_there.body
        );
    }
}

#[tokio::test]
async fn an_echoed_config_is_not_deleted_whatever_form_and_count_the_echoes_take() {
    let router = harness();

    // The reference forms the modelled `ConfigArn` admits, over one and several echoing
    // configs: the row is the list of forms, one echo config per entry.
    let rows: &[(&str, &[ReferenceForm])] = &[
        ("one echo naming the ARN", &[ReferenceForm::WholeArn]),
        ("one echo naming a node", &[ReferenceForm::ArnWithNode]),
        (
            "three echoes of mixed form",
            &[
                ReferenceForm::WholeArn,
                ReferenceForm::ArnWithNode,
                ReferenceForm::WholeArn,
            ],
        ),
    ];

    for (case, forms) in rows {
        let (uplink_id, uplink_arn) = create_uplink_config(&router, &format!("uplink {case}")).await;

        let mut echoes: Vec<(String, String)> = Vec::new();
        for (index, form) in forms.iter().enumerate() {
            let echo = create_echo_config(
                &router,
                &format!("echo {index} of {case}"),
                &form.reference(&uplink_arn),
            )
            .await;
            echoes.push(echo);
        }
        let mut echo_arns: Vec<String> = echoes.iter().map(|(_, arn)| arn.clone()).collect();
        echo_arns.sort();

        let refused = delete_config(&router, "antenna-uplink", &uplink_id).await;
        assert_in_use(case, &refused, &config_in_use_by_configs_message(&echo_arns));
        let message = refused.string("message");
        for arn in &echo_arns {
            assert!(
                message.contains(arn),
                "case {case}: the refusal must name every echoing config, missing {arn} in \
                 {message:?}"
            );
        }

        // The refused delete wrote nothing: the uplink config and every echo of it read
        // back as they were, each echo still resolving to the config it names.
        let uplink_still_there = get_config(&router, "antenna-uplink", &uplink_id).await;
        assert_eq!(
            uplink_still_there.status, 200,
            "case {case}: a refused delete must leave the uplink config readable, got {} {}",
            uplink_still_there.status, uplink_still_there.body
        );
        for (index, (echo_id, _)) in echoes.iter().enumerate() {
            let echo_read = get_config(&router, "uplink-echo", echo_id).await;
            let reported = echoed_arn(&echo_read);
            let expected = forms[index].reference(&uplink_arn);
            assert_eq!(
                reported, expected,
                "case {case}: echo {index} must still name {expected}, got {reported:?}"
            );
        }

        // Once every echo is gone the delete goes through — the refusal was about the
        // live reference, not about the config. The LAST echo standing still blocks it.
        for (index, (echo_id, _)) in echoes.iter().enumerate() {
            let removed = delete_config(&router, "uplink-echo", echo_id).await;
            assert_eq!(
                removed.status, 200,
                "case {case}: expected DeleteConfig 200 for echo {index}, got {} {}",
                removed.status, removed.body
            );
            let remaining: Vec<String> = echoes[index + 1..]
                .iter()
                .map(|(_, arn)| arn.clone())
                .collect();
            let mut remaining_sorted = remaining.clone();
            remaining_sorted.sort();
            let attempt = delete_config(&router, "antenna-uplink", &uplink_id).await;
            if remaining_sorted.is_empty() {
                assert_eq!(
                    attempt.status, 200,
                    "case {case}: an unreferenced config deletes, got {} {}",
                    attempt.status, attempt.body
                );
            } else {
                assert_in_use(
                    case,
                    &attempt,
                    &config_in_use_by_configs_message(&remaining_sorted),
                );
            }
        }

        let gone = get_config(&router, "antenna-uplink", &uplink_id).await;
        assert_eq!(
            gone.status, 434,
            "case {case}: the deleted config must read as absent, got {} {}",
            gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn repointing_an_echo_frees_the_config_it_no_longer_names() {
    let router = harness();
    let (first_id, first_arn) = create_uplink_config(&router, "first uplink").await;
    let (second_id, second_arn) = create_uplink_config(&router, "second uplink").await;
    let (echo_id, echo_arn) = create_echo_config(&router, "echo", &first_arn).await;

    // While the echo names the first config, that one is held and the second is free of
    // it — the guard counts the reference that is stored, not every config of the type.
    assert_in_use(
        "before the update",
        &delete_config(&router, "antenna-uplink", &first_id).await,
        &config_in_use_by_configs_message(std::slice::from_ref(&echo_arn)),
    );

    let repointed = repoint_echo_config(&router, &echo_id, "echo", &second_arn).await;
    assert_eq!(
        repointed.status, 200,
        "expected UpdateConfig 200, got {} {}",
        repointed.status, repointed.body
    );
    let read_back = echoed_arn(&get_config(&router, "uplink-echo", &echo_id).await);
    assert_eq!(
        read_back, second_arn,
        "the updated echo must name the second uplink config, got {read_back:?}"
    );

    // The reference moved with it: the config it no longer names deletes, the one it now
    // names does not.
    let freed = delete_config(&router, "antenna-uplink", &first_id).await;
    assert_eq!(
        freed.status, 200,
        "expected the no-longer-echoed config to delete, got {} {}",
        freed.status, freed.body
    );
    assert_in_use(
        "after the update",
        &delete_config(&router, "antenna-uplink", &second_id).await,
        &config_in_use_by_configs_message(std::slice::from_ref(&echo_arn)),
    );
}
