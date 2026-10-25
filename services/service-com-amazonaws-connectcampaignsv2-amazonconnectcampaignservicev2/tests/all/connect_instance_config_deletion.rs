// SPDX-License-Identifier: CC0-1.0

//! Deleting the Campaigns configuration of a Connect instance, end to end over the
//! wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rules under test:
//!
//! * `DeleteConnectInstanceConfig` REMOVES the configuration of the instance it names
//!   together with everything held on that configuration — the instance's
//!   onboarding record, its instance-wide communication limits and its integrations —
//!   so afterwards every instance-scoped read of it is HTTP 404
//!   `ResourceNotFoundException` and the instance is no longer a campaign target;
//! * the request's `campaignDeletionPolicy` decides what the delete enacts on the
//!   campaigns of that instance: `DELETE_ALL` removes them, `RETAIN_ALL` retains
//!   them, and a request naming no policy retains them too;
//! * the delete reaches one instance: another instance's configuration, integrations
//!   and campaigns are untouched whichever policy is named;
//! * an instance with no configuration at the instant of the call — never onboarded,
//!   onboarding not finished, or configuration already deleted — is HTTP 404 and
//!   nothing is removed;
//! * a `campaignDeletionPolicy` outside the model's catalog is HTTP 400
//!   `ValidationException` and removes nothing;
//! * onboarding the instance again gives it a fresh configuration: it holds no
//!   integrations and no communication limits from before the delete.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::CAMPAIGN_DELETION_POLICY_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// The region every case here signs for; nothing in an instance configuration is
/// region-dependent — the ARNs it holds are the caller's — so one is enough.
const REGION: &str = "us-east-1";

/// `StartInstanceOnboardingJob` for every instance in `instances`, then wait out the
/// span the clock owes their onboarding jobs once: an instance has a configuration,
/// and is a campaign target, only after its job has finished. Nothing in the emulator
/// sleeps for the transition — the record stores the due instant — so a case only has
/// to let the clock pass it.
async fn onboard_all(router: &MultiServiceRouter, instances: &[&str]) {
    for instance in instances {
        let (status, body) = call(
            router,
            REGION,
            "PUT",
            &format!("/v2/connect-instance/{instance}/onboarding"),
            &json!({"encryptionConfig": {"enabled": false}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {instance:?}: expected onboarding to start, got {status} {body}"
        );
    }
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// `DeleteConnectInstanceConfig` for `instance`, with `policy` carried as the
/// operation's `campaignDeletionPolicy` query parameter when there is one.
async fn delete_config(
    router: &MultiServiceRouter,
    instance: &str,
    policy: Option<&str>,
) -> (u16, Value) {
    let query = match policy {
        Some(policy) => format!("?campaignDeletionPolicy={policy}"),
        None => String::new(),
    };
    call(
        router,
        REGION,
        "DELETE",
        &format!("/v2/connect-instance/{instance}/config{query}"),
        &Value::Null,
    )
    .await
}

/// `GetConnectInstanceConfig` for `instance`.
async fn get_config(router: &MultiServiceRouter, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/config"),
        &Value::Null,
    )
    .await
}

/// `GetInstanceOnboardingJobStatus` for `instance`.
async fn get_onboarding(router: &MultiServiceRouter, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await
}

/// `GetInstanceCommunicationLimits` for `instance`.
async fn get_limits(router: &MultiServiceRouter, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &Value::Null,
    )
    .await
}

/// `ListConnectInstanceIntegrations` for `instance`.
async fn list_integrations(router: &MultiServiceRouter, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/integrations"),
        &Value::Null,
    )
    .await
}

/// `DescribeCampaign` for `id`.
async fn describe_campaign(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await
}

/// Configure `instance` the way a caller does before deleting it: one Lambda
/// integration and one instance-wide communication-limits config, both of which live
/// on the instance's configuration.
async fn configure(router: &MultiServiceRouter, instance: &str, case: &str) {
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/integrations"),
        &json!({"integrationConfig": {"lambda": {
            "functionArn":
                format!("arn:aws:lambda:us-east-1:000000000000:function:{instance}-handler"),
        }}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 putting an integration on {instance}, \
         got {status} {body}"
    );

    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &json!({"communicationLimitsConfig": {"allChannelSubtypes": {
            "communicationLimitsList": [
                {"maxCountPerRecipient": 3, "frequency": 1, "unit": "DAY"},
            ],
        }}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 putting communication limits on {instance}, \
         got {status} {body}"
    );
}

/// `CreateCampaign` for `instance` under `name`, returning the id the store minted.
async fn create_campaign(
    router: &MultiServiceRouter,
    instance: &str,
    name: &str,
    case: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            // `outboundMode` and `defaultOutboundConfig` are both required by the model.
            "channelSubtypeConfig": {"telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
            }},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating campaign {name:?} on {instance}, \
         got {status} {body}"
    );
    body.get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign answered no id: {body}"))
        .to_string()
}

/// Assert `status`/`body` is the modelled 404 for an instance this service holds no
/// configuration for.
fn assert_not_found(case: &str, what: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from {what}, got {status} {body}"
    );
    assert!(
        error_code(body).ends_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException from {what}, got {}",
        error_code(body)
    );
}

/// The policy a row names, the campaigns of the instance that row expects to survive
/// the delete, and nothing else: the table spans the whole `CampaignDeletionPolicy`
/// catalog plus the request that names no policy at all.
///
/// Catalog source: `CAMPAIGN_DELETION_POLICY_VALUES`, the model's own
/// `CampaignDeletionPolicy` catalog
/// (`smithy_query(query="shape", shape="CampaignDeletionPolicy")`), which the case
/// below asserts the table covers.
fn policy_cases() -> Vec<(&'static str, Option<&'static str>, bool)> {
    vec![
        ("DELETE_ALL", Some("DELETE_ALL"), false),
        ("RETAIN_ALL", Some("RETAIN_ALL"), true),
        ("no policy at all", None, true),
    ]
}

/// Deleting an instance's configuration removes the configuration and everything held
/// on it, and the request's campaign deletion policy decides the instance's campaigns.
///
/// The rows differ along the policy the request carries — every value the model's
/// catalog declares, and the absent member — which is what the rule generalises over.
/// Each row carries more than one campaign on the instance under test, so a policy is
/// judged on the SET of them, and a bystander instance with a configuration, an
/// integration and a campaign of its own, so the delete is shown to reach one
/// instance.
#[tokio::test]
async fn the_campaign_deletion_policy_decides_the_instances_campaigns() {
    // The table spans the model's catalog: every declared policy has a row.
    let cases = policy_cases();
    for value in CAMPAIGN_DELETION_POLICY_VALUES {
        assert!(
            cases.iter().any(|(_, policy, _)| *policy == Some(*value)),
            "the policy table must cover every CampaignDeletionPolicy value; {value} is missing"
        );
    }

    for (case, policy, campaigns_survive) in cases {
        let router = router();
        let instance = "deleted-instance";
        let bystander = "bystander-instance";
        onboard_all(&router, &[instance, bystander]).await;
        configure(&router, instance, case).await;
        configure(&router, bystander, case).await;

        // Two campaigns on the instance under test and one on the bystander.
        let mut owned = Vec::new();
        for name in ["first-wave", "second-wave"] {
            owned.push(create_campaign(&router, instance, name, case).await);
        }
        let other = create_campaign(&router, bystander, "bystander-wave", case).await;

        let (status, body) = delete_config(&router, instance, policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} {body}"
        );

        // The configuration and everything held on it are gone, so every
        // instance-scoped read of the instance reports it holds nothing.
        let (status, body) = get_config(&router, instance).await;
        assert_not_found(case, "GetConnectInstanceConfig", status, &body);
        let (status, body) = get_onboarding(&router, instance).await;
        assert_not_found(case, "GetInstanceOnboardingJobStatus", status, &body);
        let (status, body) = get_limits(&router, instance).await;
        assert_not_found(case, "GetInstanceCommunicationLimits", status, &body);
        let (status, body) = list_integrations(&router, instance).await;
        assert_not_found(case, "ListConnectInstanceIntegrations", status, &body);

        // The campaigns of the instance are decided by the policy, as a set.
        for id in &owned {
            let (status, body) = describe_campaign(&router, id).await;
            if campaigns_survive {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected campaign {id} to survive the delete, \
                     got {status} {body}"
                );
            } else {
                assert_not_found(case, &format!("DescribeCampaign {id}"), status, &body);
            }
        }

        // The delete reached one instance: the bystander keeps its configuration, its
        // integration and its campaign whatever the policy was.
        let (status, body) = get_config(&router, bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bystander instance to keep its configuration, \
             got {status} {body}"
        );
        let (status, body) = list_integrations(&router, bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bystander instance to keep its integrations, \
             got {status} {body}"
        );
        let held = body
            .get("integrationSummaryList")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, 1,
            "case {case:?}: expected the bystander instance to still hold one integration, \
             got {held}"
        );
        let (status, body) = describe_campaign(&router, &other).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bystander instance's campaign to survive, \
             got {status} {body}"
        );

        // A second delete finds no configuration to delete.
        let (status, body) = delete_config(&router, instance, policy).await;
        assert_not_found(case, "a repeated delete", status, &body);

        // Onboarding the instance again gives it a FRESH configuration: it holds no
        // integration and no communication limits from before the delete, so the
        // delete removed those records rather than hiding them.
        onboard_all(&router, &[instance]).await;
        let (status, body) = get_config(&router, instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-onboarded instance to have a configuration, \
             got {status} {body}"
        );
        let (status, body) = list_integrations(&router, instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 listing the re-onboarded instance's \
             integrations, got {status} {body}"
        );
        let restored = body
            .get("integrationSummaryList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            restored,
            Vec::<Value>::new(),
            "case {case:?}: expected the re-onboarded instance to hold no integration, \
             got {restored:?}"
        );
        let (status, body) = get_limits(&router, instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the re-onboarded instance's limits, \
             got {status} {body}"
        );
        assert!(
            body.get("communicationLimitsConfig")
                .is_none_or(Value::is_null),
            "case {case:?}: expected the re-onboarded instance to hold no communication \
             limits, got {body}"
        );
    }
}

/// An instance this service holds no configuration for at the instant of the call has
/// none to delete: the delete is HTTP 404 `ResourceNotFoundException` and removes
/// nothing.
///
/// The rows differ along WHY the instance holds no configuration and along the form of
/// the `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="DeleteConnectInstanceConfig")`): the
/// shortest admissible id, a uuid-form id, the longest admissible id, an id that only
/// looks like the configured one, and an instance whose onboarding job has started but
/// not finished.
#[tokio::test]
async fn an_instance_with_no_configuration_has_none_to_delete() {
    let router = router();

    // One configured instance, so the 404s below are the lookup failing rather than
    // the operation being broken.
    let configured = "configured-instance";
    onboard_all(&router, &[configured]).await;
    configure(&router, configured, "control instance").await;

    // An instance whose onboarding job has only just started has no configuration yet:
    // no wait here, so the job is still in progress when the delete runs.
    let in_progress = "in-progress-instance";
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{in_progress}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"onboarding in progress\": expected HTTP 200 starting the job, got {status} {body}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("shortest admissible id", "a".to_string()),
        (
            "uuid form",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("longest admissible id", "i".repeat(256)),
        (
            "an id that only looks like the configured one",
            format!("{configured}-copy"),
        ),
        ("onboarding in progress", in_progress.to_string()),
    ];

    for (case, instance) in cases {
        // Each row is tried with every policy form: the policy decides the campaigns,
        // never whether there is a configuration to delete.
        for (_, policy, _) in policy_cases() {
            let (status, body) = delete_config(&router, &instance, policy).await;
            assert_not_found(case, "the delete", status, &body);
        }
    }

    // Every refusal removed nothing: the configured instance is still configured and
    // the in-progress job is still in progress.
    let (status, body) = get_config(&router, configured).await;
    assert_eq!(
        status, 200,
        "case \"control instance\": a refused delete must remove nothing, got {status} {body}"
    );
    let (status, body) = get_onboarding(&router, in_progress).await;
    assert_eq!(
        status, 200,
        "case \"onboarding in progress\": a refused delete must remove nothing, \
         got {status} {body}"
    );
    assert_eq!(
        body.pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
            .and_then(Value::as_str),
        Some(in_progress),
        "case \"onboarding in progress\": expected the onboarding record to survive, got {body}"
    );
}

/// A `campaignDeletionPolicy` outside the model's `CampaignDeletionPolicy` catalog
/// names no policy, so the request is HTTP 400 `ValidationException` and removes
/// nothing.
///
/// The rows differ along how the value misses the catalog: the case of a declared
/// value, a prefix and a suffix of one, a value from another of the service's enums,
/// and two declared values at once.
#[tokio::test]
async fn a_policy_outside_the_catalog_is_refused() {
    let router = router();
    let instance = "policy-validation-instance";
    onboard_all(&router, &[instance]).await;
    configure(&router, instance, "policy validation").await;
    let campaign = create_campaign(&router, instance, "validation-wave", "policy validation").await;

    let cases: Vec<(&str, String)> = vec![
        ("a declared value in lower case", "delete_all".to_string()),
        ("a prefix of a declared value", "DELETE".to_string()),
        (
            "a declared value with a suffix",
            "RETAIN_ALL_CAMPAIGNS".to_string(),
        ),
        (
            "a value from another catalog",
            "ALL_CHANNEL_SUBTYPES".to_string(),
        ),
        (
            "two declared values at once",
            "DELETE_ALL,RETAIN_ALL".to_string(),
        ),
    ];

    for (case, policy) in cases {
        let (status, body) = delete_config(&router, instance, Some(&policy)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for policy {policy:?}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException for policy {policy:?}, got {}",
            error_code(&body)
        );

        // A refusal removes nothing: the configuration and the campaign are still there.
        let (status, body) = get_config(&router, instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the configuration, got {status} {body}"
        );
        let (status, body) = describe_campaign(&router, &campaign).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the campaign, got {status} {body}"
        );
    }
}
