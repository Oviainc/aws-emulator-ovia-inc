// SPDX-License-Identifier: CC0-1.0

//! The ONE reference rule both writers of a `Config` hold an `uplinkEchoConfig` to, driven
//! end to end over HTTP through `CreateConfig` and `UpdateConfig` — the two operations that
//! used to carry their own copy of it.
//!
//! The RULE under test: an `uplinkEchoConfig`'s `antennaUplinkConfigArn` must resolve to a
//! config of capability type
//! [`state::UPLINK_ECHO_REFERENCED_TYPE`][referenced] this account holds, an ARN resolving
//! to no config being a missing resource (`ResourceNotFoundException`, HTTP 434) and one
//! resolving to a config of ANOTHER capability type a bad parameter
//! (`InvalidParameterException`, HTTP 431) naming the member that carried it; the create and
//! the update answer it identically, and a refused write stores nothing.
//!
//! Asserted over a TABLE rather than one call: every capability type a `ConfigTypeData`
//! selects is created and then named as an echo config's reference, so the one admissible
//! type and the seven refused ones come from the same table, crossed with both writers; the
//! absent case is crossed with a catalogue of well-formed ARNs that name nothing. Every
//! request goes through the real front door — routing, deserialization, constraint
//! validation, the handler and serialization — on the harness `scheduling_loop.rs` stands
//! up.
//!
//! [referenced]: service_com_amazonaws_groundstation_groundstation::state::UPLINK_ECHO_REFERENCED_TYPE

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    typed_reference_absent_message, typed_reference_wrong_type_message,
    UPLINK_ECHO_REFERENCED_TYPE, UPLINK_ECHO_REFERENCE_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, harness, send, Reply, REGION,
};

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that name no config: a
/// run of zeros, a run of `f`s and a mixed one, so an assertion about the absent answer
/// cannot be passing on one id's shape.
const ABSENT_CONFIG_IDS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
];

/// A `configData` naming `arn` as the antenna uplink config an echo config echoes.
fn echo_of(arn: &str) -> Value {
    json!({"uplinkEchoConfig": {"enabled": true, "antennaUplinkConfigArn": arn}})
}

/// `POST /config` — `CreateConfig`, without asserting the answer.
async fn create_echo(router: &MultiServiceRouter, name: &str, arn: &str) -> Reply {
    send(
        router,
        "POST",
        "/config",
        Some(json!({"name": name, "configData": echo_of(arn)})),
    )
    .await
}

/// `PUT /config/{configType}/{configId}` — `UpdateConfig`, retargeting an echo config at
/// `arn`, without asserting the answer.
async fn update_echo(
    router: &MultiServiceRouter,
    config_type: &str,
    config_id: &str,
    name: &str,
    arn: &str,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/config/{config_type}/{config_id}"),
        Some(json!({"name": name, "configData": echo_of(arn)})),
    )
    .await
}

/// `GET /config/{configType}/{configId}` — `GetConfig`, the read a write is proven through.
async fn get_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// What a write answered, projected onto the members the reference rule decides — so the
/// create's and the update's answers are comparable even though the ids they report are
/// each their own resource's.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_code: String,
    message: Option<String>,
    parameter_name: Option<String>,
}

fn answer(reply: &Reply) -> Answer {
    let member = |name: &str| {
        reply
            .body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Answer {
        status: reply.status,
        error_code: reply.error_code(),
        message: member("message"),
        parameter_name: member("parameterName"),
    }
}

/// The answer a reference to a config of a capability type other than the admissible one
/// earns, whichever writer took it.
fn wrong_type_answer() -> Answer {
    Answer {
        status: 431,
        error_code: "InvalidParameterException".to_string(),
        message: Some(typed_reference_wrong_type_message(
            UPLINK_ECHO_REFERENCED_TYPE,
        )),
        parameter_name: Some(UPLINK_ECHO_REFERENCE_PARAMETER.to_string()),
    }
}

/// A config ARN of the admissible capability type carrying `config_id`, which this account
/// holds no config under.
fn absent_arn(config_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:config/{UPLINK_ECHO_REFERENCED_TYPE}/{config_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

#[tokio::test]
async fn an_echo_config_reference_is_held_to_one_rule_by_the_create_and_the_update() {
    let router = harness();

    // The config every well-formed echo reference names, and a baseline echo config the
    // update path retargets: it starts out pointing at that config, so every refused
    // update below is observable as a config that did not move.
    let (_, _, uplink_arn) = create_config(
        &router,
        "echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;
    let (echo_id, echo_type, _) =
        create_config(&router, "echo to retarget", echo_of(&uplink_arn), None).await;

    // Every capability type a `ConfigTypeData` selects, each created and then named as an
    // echo config's reference: exactly one of them is the type the member admits.
    for (config_type, config_data) in config_data_table(&uplink_arn) {
        let (_, _, target_arn) = create_config(
            &router,
            &format!("echo target {config_type}"),
            config_data,
            None,
        )
        .await;

        let created = create_echo(
            &router,
            &format!("echo of a {config_type} config"),
            &target_arn,
        )
        .await;
        let updated = update_echo(
            &router,
            &echo_type,
            &echo_id,
            "echo to retarget",
            &target_arn,
        )
        .await;
        assert_eq!(
            answer(&created),
            answer(&updated),
            "case {config_type}: the create and the update must answer one rule, got create \
             {} and update {}",
            created.body,
            updated.body
        );

        if config_type == UPLINK_ECHO_REFERENCED_TYPE {
            assert_eq!(
                answer(&created),
                Answer {
                    status: 200,
                    error_code: String::new(),
                    message: None,
                    parameter_name: None,
                },
                "case {config_type}: a reference to a config of the admitted type must be \
                 accepted, got {}",
                created.body
            );
            // And the accepted reference is what the store now holds, through both writers:
            // the created config and the retargeted one both read back naming it.
            let fresh = get_config(
                &router,
                &created.string("configType"),
                &created.string("configId"),
            )
            .await;
            assert_eq!(
                fresh.field("configData"),
                &echo_of(&target_arn),
                "case {config_type}: the created echo config must read back naming the \
                 config it echoes, got {}",
                fresh.field("configData")
            );
            let retargeted = get_config(&router, &echo_type, &echo_id).await;
            assert_eq!(
                retargeted.field("configData"),
                &echo_of(&target_arn),
                "case {config_type}: the updated echo config must read back naming the \
                 config it echoes, got {}",
                retargeted.field("configData")
            );
            // Put the baseline back, so a later row's refused update is still asserted
            // against a config that points somewhere else.
            let restored = update_echo(
                &router,
                &echo_type,
                &echo_id,
                "echo to retarget",
                &uplink_arn,
            )
            .await;
            assert_eq!(
                restored.status, 200,
                "case {config_type}: restoring the baseline echo config must be accepted, \
                 got {} {}",
                restored.status, restored.body
            );
            continue;
        }

        assert_eq!(
            answer(&created),
            wrong_type_answer(),
            "case {config_type}: a reference to a config of another capability type must be \
             refused as a bad parameter, got {}",
            created.body
        );
        let message = created.string("message");
        assert!(
            !message.contains(&target_arn),
            "case {config_type}: the refusal is about the type, not about the resource, got \
             {message:?}"
        );
        let unchanged = get_config(&router, &echo_type, &echo_id).await;
        assert_eq!(
            unchanged.field("configData"),
            &echo_of(&uplink_arn),
            "case {config_type}: a refused update must change nothing, got {}",
            unchanged.field("configData")
        );
    }

    // The OTHER fault, crossed with ids that name nothing: an ARN of the admitted type
    // resolving to no config is a missing resource, with the not-found text naming the
    // capability the member required and the ARN the request gave.
    for config_id in ABSENT_CONFIG_IDS {
        let arn = absent_arn(config_id);
        let expected = Answer {
            status: 434,
            error_code: "ResourceNotFoundException".to_string(),
            message: Some(typed_reference_absent_message(
                UPLINK_ECHO_REFERENCED_TYPE,
                &arn,
            )),
            parameter_name: None,
        };
        let created = create_echo(&router, "echo of nothing", &arn).await;
        let updated = update_echo(&router, &echo_type, &echo_id, "echo to retarget", &arn).await;
        assert_eq!(
            answer(&created),
            answer(&updated),
            "case absent {config_id}: the create and the update must answer one rule, got \
             create {} and update {}",
            created.body,
            updated.body
        );
        assert_eq!(
            answer(&created),
            expected,
            "case absent {config_id}: a reference to no config must be refused as a missing \
             resource, got {}",
            created.body
        );
        let unchanged = get_config(&router, &echo_type, &echo_id).await;
        assert_eq!(
            unchanged.field("configData"),
            &echo_of(&uplink_arn),
            "case absent {config_id}: a refused update must change nothing, got {}",
            unchanged.field("configData")
        );
    }
}
