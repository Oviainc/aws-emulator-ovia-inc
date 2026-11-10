// SPDX-License-Identifier: CC0-1.0

//! The Amazon Connect instance registry gate, over every operation of this service
//! that merely requires a USABLE Connect instance, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test:
//!
//! * an operation whose own resource is not the instance's configuration, and which
//!   only requires the Connect instance it names to be usable by Campaigns, is refused
//!   with HTTP 404 `ResourceNotFoundException` carrying the service's one fixed text
//!   for that class whenever EITHER half of usability fails — the account's Amazon
//!   Connect registry does not hold the instance, or no finished onboarding job has
//!   enabled it for Campaigns in the request's region — and persists nothing;
//! * the registry owns whether an instance exists, so its current contents decide the
//!   answer: an id removed from it stops being usable however this service's own
//!   onboarding record reads, and an id put back into it is usable again;
//! * every operation of the class answers that refusal identically, so two of them
//!   cannot disagree about a request they both refuse.
//!
//! The rows differ along what the rule generalises over: which operation of the class
//! is called (all five, spanning a write that mints a resource, a write that configures
//! the instance, a delete and a pure read), how the instance came to be missing from
//! the registry (removed after onboarding, or never held at all), the form of the
//! instance id, and the region the request is signed for.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::EncryptionConfig;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, INSTANCE_NOT_FOUND_MESSAGE,
    ONBOARDING_JOB_DURATION,
};

use super::common::{call, error_code, router_with_engine, Harness};

/// The region the cases sign for unless they are about more than one.
const REGION: &str = "us-east-1";

/// A second region, for the rows that assert the gate is the same wherever the request
/// is signed for: the instance registry is the ACCOUNT's, so it answers the same
/// question at every regional endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// The instance the per-operation rows work on.
const INSTANCE: &str = "registry-gate-instance";

/// Two Lambda function ARNs, so a row has one integration to act on in each phase: the
/// request driven while the registry holds the instance names the first, the one driven
/// after it is gone names the second — which is still stored, so a refusal of it can
/// only be about the instance.
const FIRST_FUNCTION: &str = "arn:aws:lambda:us-east-1:000000000000:function:registry-gate-first";
const SECOND_FUNCTION: &str = "arn:aws:lambda:us-east-1:000000000000:function:registry-gate-second";

/// One request: its HTTP method, its path, and its body (`Value::Null` for a request
/// that carries none).
type Request = (&'static str, String, Value);

/// A `lambda` `IntegrationConfig` for `function_arn` (`LambdaIntegrationConfig$functionArn`,
/// from `smithy_query(query="shape", shape="IntegrationConfig")`).
fn lambda_config(function_arn: &str) -> Value {
    json!({"lambda": {"functionArn": function_arn}})
}

/// The `IntegrationIdentifier` naming the integration [`lambda_config`] configures
/// (`LambdaIntegrationIdentifier$functionArn`, from
/// `smithy_query(query="shape", shape="IntegrationIdentifier")`).
fn lambda_identifier(function_arn: &str) -> Value {
    json!({"lambda": {"functionArn": function_arn}})
}

/// A minimal telephony `CreateCampaign` body for `instance` under `name`: both
/// `outboundMode` and `defaultOutboundConfig` are required by the model.
fn campaign_body(instance: &str, name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": {"telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
        }},
    })
}

/// Every operation of this refusal class, each with TWO requests for `instance`: one to
/// drive while the account's registry holds the instance, which must be served, and one
/// to drive once it does not, which must be refused. Both name resources the fixture
/// really set up, so the registry is the only difference between the two phases.
///
/// The class is the one [`INSTANCE_NOT_FOUND_MESSAGE`] describes: starting an onboarding
/// job, creating a campaign, and the instance-integration operations.
fn class_operations(instance: &str) -> Vec<(&'static str, Request, Request)> {
    let onboarding = format!("/v2/connect-instance/{instance}/onboarding");
    let integrations = format!("/v2/connect-instance/{instance}/integrations");
    let onboarding_body = json!({"encryptionConfig": {"enabled": false}});
    vec![
        (
            "StartInstanceOnboardingJob",
            ("PUT", onboarding.clone(), onboarding_body.clone()),
            ("PUT", onboarding, onboarding_body),
        ),
        (
            "CreateCampaign",
            (
                "PUT",
                "/v2/campaigns".to_string(),
                campaign_body(instance, "while-the-account-holds-the-instance"),
            ),
            (
                "PUT",
                "/v2/campaigns".to_string(),
                campaign_body(instance, "once-the-account-does-not"),
            ),
        ),
        (
            "PutConnectInstanceIntegration",
            (
                "PUT",
                integrations.clone(),
                json!({"integrationConfig": lambda_config(FIRST_FUNCTION)}),
            ),
            (
                "PUT",
                integrations.clone(),
                json!({"integrationConfig": lambda_config(SECOND_FUNCTION)}),
            ),
        ),
        (
            "DeleteConnectInstanceIntegration",
            (
                "POST",
                format!("{integrations}/delete"),
                json!({"integrationIdentifier": lambda_identifier(FIRST_FUNCTION)}),
            ),
            (
                "POST",
                format!("{integrations}/delete"),
                json!({"integrationIdentifier": lambda_identifier(SECOND_FUNCTION)}),
            ),
        ),
        (
            "ListConnectInstanceIntegrations",
            ("GET", integrations.clone(), Value::Null),
            ("GET", integrations, Value::Null),
        ),
    ]
}

/// Drive one [`Request`] of `region`'s endpoint.
async fn send(router: &Harness, region: &str, request: &Request) -> (u16, Value) {
    let (method, path, body) = request;
    call(router, region, method, path, body).await
}

/// Register `instance` in the account's Connect registry, onboard it to Campaigns in
/// `region` and let the clock reach the instant its job is due at, then give it both
/// integrations the rows act on — so every operation of the class has a resource to act
/// on and both halves of usability hold.
async fn prepare(router: &Harness, region: &str, instance: &str) {
    router.register_connect_instance(region, instance);
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "fixture: onboarding {instance:?} in {region:?} expected HTTP 200, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(150)).await;

    for function_arn in [FIRST_FUNCTION, SECOND_FUNCTION] {
        let (status, body) = call(
            router,
            region,
            "PUT",
            &format!("/v2/connect-instance/{instance}/integrations"),
            &json!({"integrationConfig": lambda_config(function_arn)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "fixture: integrating {function_arn:?} expected HTTP 200, got {status} {body}"
        );
    }
}

/// The refusal class as a comparable triple: the status, the error code and the message.
fn refusal(body: &Value, status: u16) -> (u16, String, String) {
    (
        status,
        error_code(body).to_string(),
        body.get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string(),
    )
}

/// Assert `(status, body)` is the modelled 404 for an instance that is not usable by
/// Campaigns: the service's FIXED text for that class, which carries nothing from the
/// request.
fn assert_instance_not_found(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 for an instance the account does not hold, \
         got {status} {body}"
    );
    assert!(
        error_code(body).ends_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        error_code(body)
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(INSTANCE_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the service's fixed text for an instance not usable by \
         Campaigns, got {body}"
    );
}

/// The campaigns `region` lists, by name.
async fn campaign_names(router: &Harness, region: &str, case: &str) -> Vec<String> {
    let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected HTTP 200, got {status} {body}"
    );
    body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .filter_map(|summary| summary["name"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn every_operation_needing_a_usable_instance_is_refused_once_the_registry_loses_it() {
    let mut refusals: Vec<(&str, (u16, String, String))> = Vec::new();

    for (operation, served, refused) in class_operations(INSTANCE) {
        let (router, _engine) = router_with_engine();
        prepare(&router, REGION, INSTANCE).await;

        // Served while the account holds the instance, so the row's own request is
        // one this service answers: whatever the second phase answers is about the
        // registry and nothing else.
        let (status, body) = send(&router, REGION, &served).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected HTTP 200 while the account holds the instance, \
             got {status} {body}"
        );

        assert!(
            router.deregister_connect_instance(INSTANCE),
            "case {operation:?}: the fixture registered the instance, so removing it \
             removes a record"
        );

        let (status, body) = send(&router, REGION, &refused).await;
        assert_instance_not_found(operation, status, &body);
        refusals.push((operation, refusal(&body, status)));
    }

    // The class is ONE refusal: every operation of it answers the same status, code and
    // text, so two of them cannot disagree about a request they both refuse.
    let (first_operation, first) = refusals
        .first()
        .cloned()
        .expect("the class has operations");
    for (operation, answered) in &refusals {
        assert_eq!(
            answered, &first,
            "case {operation:?}: expected the same refusal {first_operation:?} gives \
             ({first:?}), got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_create_mints_nothing_and_a_refused_put_stores_nothing() {
    let (router, _engine) = router_with_engine();
    prepare(&router, REGION, INSTANCE).await;

    // What the region holds before the registry loses the instance.
    let before = campaign_names(&router, REGION, "baseline").await;
    let (status, body) = call(
        &router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{INSTANCE}/integrations"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "baseline: listing the integrations expected HTTP 200, got {status} {body}"
    );
    let integrations_before = body["integrationSummaryList"].clone();

    assert!(
        router.deregister_connect_instance(INSTANCE),
        "the fixture registered the instance"
    );

    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &campaign_body(INSTANCE, "never-minted"),
    )
    .await;
    assert_instance_not_found("CreateCampaign stores nothing", status, &body);
    assert!(
        body.get("id").is_none() && body.get("arn").is_none(),
        "case \"CreateCampaign stores nothing\": a refused create mints no id or ARN, got {body}"
    );

    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{INSTANCE}/integrations"),
        &json!({"integrationConfig": lambda_config(
            "arn:aws:lambda:us-east-1:000000000000:function:never-integrated"
        )}),
    )
    .await;
    assert_instance_not_found("PutConnectInstanceIntegration stores nothing", status, &body);

    // Putting the instance back into the registry makes it usable again — the gate
    // reads the registry's current contents rather than latching — and what it holds is
    // exactly what it held before the refusals.
    router.register_connect_instance(REGION, INSTANCE);
    let after = campaign_names(&router, REGION, "after the refusals").await;
    assert_eq!(
        after, before,
        "expected the refused create to have minted nothing, got {after:?} where {before:?} \
         was held"
    );
    let (status, body) = call(
        &router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{INSTANCE}/integrations"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        (status, &body["integrationSummaryList"]),
        (200, &integrations_before),
        "expected the refused put to have stored nothing, got {status} {body}"
    );

    // And a create for the re-registered instance is served again.
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &campaign_body(INSTANCE, "minted-once-the-account-holds-it-again"),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the re-registered instance to be a campaign target again, \
         got {status} {body}"
    );
}

#[tokio::test]
async fn an_onboarding_record_alone_does_not_make_an_instance_usable() {
    // The other way the halves can disagree: this service holds a FINISHED onboarding
    // record for an id the account's registry never held at all. The registry owns
    // whether the instance exists, so every operation of the class refuses it.
    let instance = "onboarded-but-never-registered";

    for (operation, _served, refused) in class_operations(instance) {
        let (router, engine) = router_with_engine();
        let state =
            engine.state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
        let started = SystemTime::now() - (ONBOARDING_JOB_DURATION + Duration::from_secs(1));
        {
            let mut store = state.lock();
            store
                .start_instance_onboarding(
                    REGION,
                    instance,
                    EncryptionConfig::default(),
                    // A job whose encryption config asks for the service's own key
                    // needs no key of the caller's, so it settles SUCCEEDED.
                    None,
                    started,
                )
                .expect("room for one onboarding record");
            assert!(
                store.is_onboarded(REGION, instance, SystemTime::now()),
                "case {operation:?}: the fixture's onboarding record is due to have finished, \
                 so the onboarding half of usability holds"
            );
        }

        let (status, body) = send(&router, REGION, &refused).await;
        assert_instance_not_found(operation, status, &body);
    }
}

#[tokio::test]
async fn the_gate_follows_the_registry_for_every_id_form_and_region() {
    // Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
    // `smithy_query(query="operation", operation="CreateCampaign")`): the shortest id,
    // the uuid form a Connect instance id takes, every punctuation character the pattern
    // admits, and the longest id the length bound allows.
    let ids: Vec<(&str, String)> = vec![
        ("one character (length minimum)", "a".to_string()),
        (
            "uuid form",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("256 characters (length maximum)", "i".repeat(256)),
    ];

    for (form, instance) in ids {
        for region in [REGION, OTHER_REGION] {
            let case = format!("{form} in {region}");
            let (router, _engine) = router_with_engine();
            router.register_connect_instance(region, &instance);
            let (status, body) = call(
                &router,
                region,
                "PUT",
                &format!("/v2/connect-instance/{instance}/onboarding"),
                &json!({"encryptionConfig": {"enabled": false}}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: onboarding the registered instance expected HTTP 200, \
                 got {status} {body}"
            );
            tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(150)).await;

            let (status, body) = call(
                &router,
                region,
                "PUT",
                "/v2/campaigns",
                &campaign_body(&instance, "target-while-registered"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the onboarded, registered instance to be a campaign \
                 target, got {status} {body}"
            );

            // The registry loses the instance: it stops being a campaign target at the
            // same endpoint, with the same record still in this service's own store.
            assert!(
                router.deregister_connect_instance(&instance),
                "case {case:?}: the registered instance has a record to remove"
            );
            let (status, body) = call(
                &router,
                region,
                "PUT",
                "/v2/campaigns",
                &campaign_body(&instance, "target-once-deregistered"),
            )
            .await;
            assert_instance_not_found(&case, status, &body);

            // Put back, and it is a target again.
            router.register_connect_instance(region, &instance);
            let (status, body) = call(
                &router,
                region,
                "PUT",
                "/v2/campaigns",
                &campaign_body(&instance, "target-once-registered-again"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the re-registered instance to be a campaign target \
                 again, got {status} {body}"
            );
        }
    }
}
