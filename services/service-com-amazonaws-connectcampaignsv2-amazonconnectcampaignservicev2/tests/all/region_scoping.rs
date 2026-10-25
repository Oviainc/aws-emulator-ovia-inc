// SPDX-License-Identifier: CC0-1.0

//! Regional scoping of every resource this service holds, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The one rule under test: a campaign and a Connect instance
//! configuration belong to the region of the credential scope the request that created
//! them was signed for, so every lookup, listing and write resolves inside that region
//! alone — a resource created through one region's endpoint is absent from another
//! region's listings, its id and its ARN resolve to nothing there, and no call through
//! another region's endpoint changes it.
//!
//! Each case spans a TABLE of region pairs rather than one recorded pair, and the
//! operation tables below cover every implemented operation that addresses a campaign
//! by its `{id}` label, every one that addresses a Connect instance by its
//! `{connectInstanceId}` label, and both tag operations, which address a campaign by
//! ARN.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    MAX_REGIONS, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// The region pairs every case runs over: `(case, home, other)`, where `home` is the
/// region a resource is created in and `other` the region whose endpoint must not see
/// it. The rows differ along what a region name can differ along — continent, partition
/// geography, and a neighbour whose name differs only in its trailing digit — since the
/// rule is about the credential scope as a whole and not about any one region.
fn region_pairs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("different continents", "us-east-1", "eu-west-1"),
        ("europe and asia pacific", "eu-central-1", "ap-southeast-2"),
        ("same country, opposite coasts", "us-west-2", "us-east-2"),
        ("names differing in one digit", "us-east-1", "us-east-2"),
    ]
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// Start the onboarding job for `instance` in each of `regions`, then wait out the span
/// the clock owes those jobs once, so every instance is a campaign target in its region
/// by the time this returns. Nothing in the emulator sleeps for that span — only a
/// caller that wants the settled state does.
async fn onboard(router: &MultiServiceRouter, regions: &[&str], instance: &str) {
    for region in regions {
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
            "case {region:?}/{instance:?}: expected onboarding to start, got {status} {body}"
        );
    }
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(150)).await;
}

/// `CreateCampaign` in `region`, asserting it succeeded, and handing back
/// `(id, arn)`.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}/{name:?}: CreateCampaign expected 200, got {status} {body}"
    );
    (
        body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}/{name:?}: no minted id: {body}"))
            .to_string(),
        body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}/{name:?}: no minted arn: {body}"))
            .to_string(),
    )
}

/// The campaign ids `ListCampaigns` reports to a caller signing for `region`, sorted so
/// a mismatch prints deterministically.
async fn listed_ids(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<String> {
    let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns in {region} expected 200, got {status} {body}"
    );
    let mut ids: Vec<String> = body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or("<no id>")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The `state` `GetCampaignState` reports for `id` in `region`, asserting the read
/// succeeded.
async fn campaign_state(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> String {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}/state"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCampaignState in {region} expected 200, got {status} {body}"
    );
    body["state"].as_str().unwrap_or("<no state>").to_string()
}

/// The `campaign` object `DescribeCampaign` projects for `id` in `region`, asserting the
/// read succeeded.
async fn describe(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign in {region} expected 200, got {status} {body}"
    );
    body["campaign"].clone()
}

/// The `{arn}` path label of `/v2/tags/{arn}`, percent-encoded as a single segment.
fn encoded(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// Every implemented operation that addresses a campaign by its `{id}` path label, as
/// `(operation, method, path suffix, body)`.
///
/// `DeleteCampaign` comes last: it is the one row whose success would remove the
/// campaign, so a run that wrongly succeeds still leaves the earlier rows' evidence.
fn campaign_operations() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    let entry_limits = json!({"maxEntryCount": 4, "minEntryInterval": "PT45M"});
    let limits = json!({
        "allChannelSubtypes": {
            "communicationLimitsList": [
                {"maxCountPerRecipient": 3, "frequency": 2, "unit": "DAY"}
            ]
        }
    });
    let time_config = json!({
        "localTimeZoneConfig": {"defaultTimeZone": "America/New_York"},
        "telephony": {
            "openHours": {
                "dailyHours": {"MONDAY": [{"startTime": "T09:00", "endTime": "T17:00"}]}
            }
        }
    });
    vec![
        ("DescribeCampaign", "GET", "", Value::Null),
        ("GetCampaignState", "GET", "/state", Value::Null),
        ("StartCampaign", "POST", "/start", Value::Null),
        ("PauseCampaign", "POST", "/pause", Value::Null),
        ("ResumeCampaign", "POST", "/resume", Value::Null),
        ("StopCampaign", "POST", "/stop", Value::Null),
        (
            "UpdateCampaignFlowAssociation",
            "POST",
            "/flow",
            json!({
                "connectCampaignFlowArn":
                    "arn:aws:connect:us-east-1:000000000000:instance/i-1/flow/f-1"
            }),
        ),
        (
            "UpdateCampaignEntryLimits",
            "POST",
            "/entry-limits",
            json!({"entryLimitsConfig": entry_limits}),
        ),
        (
            "UpdateCampaignCommunicationLimits",
            "POST",
            "/communication-limits",
            json!({"communicationLimitsOverride": limits}),
        ),
        (
            "UpdateCampaignCommunicationTime",
            "POST",
            "/communication-time",
            json!({"communicationTimeConfig": time_config}),
        ),
        (
            "DeleteCampaignCommunicationLimits",
            "DELETE",
            // The route resolves by its required `config` query parameter, whose value
            // comes from the model's own catalog
            // (`COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES`).
            "/communication-limits?config=ALL_CHANNEL_SUBTYPES",
            Value::Null,
        ),
        (
            "DeleteCampaignCommunicationTime",
            "DELETE",
            // Same shape of route; the value comes from
            // `COMMUNICATION_TIME_CONFIG_TYPE_VALUES`.
            "/communication-time?config=TELEPHONY",
            Value::Null,
        ),
        (
            "PutOutboundRequestBatch",
            "PUT",
            "/outbound-requests",
            json!({"outboundRequests": [{
                "clientToken": "cross-region-token",
                "expirationTime": "2099-01-01T00:00:00Z",
                "channelSubtypeParameters": {
                    "telephony": {"destinationPhoneNumber": "+15555550100", "attributes": {}}
                }
            }]}),
        ),
        (
            "PutProfileOutboundRequestBatch",
            "PUT",
            "/profile-outbound-requests",
            json!({"profileOutboundRequests": [{
                "clientToken": "cross-region-profile-token",
                "profileId": "0123456789abcdef0123456789abcdef"
            }]}),
        ),
        ("DeleteCampaign", "DELETE", "", Value::Null),
    ]
}

/// Every implemented operation that addresses a Connect instance by its
/// `{connectInstanceId}` path label, as `(operation, method, path suffix, body)`.
fn instance_operations() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "GetInstanceOnboardingJobStatus",
            "GET",
            "/onboarding",
            Value::Null,
        ),
        ("GetConnectInstanceConfig", "GET", "/config", Value::Null),
        (
            "GetInstanceCommunicationLimits",
            "GET",
            "/communication-limits",
            Value::Null,
        ),
        (
            "PutInstanceCommunicationLimits",
            "PUT",
            "/communication-limits",
            json!({"communicationLimitsConfig": {
                "allChannelSubtypes": {
                    "communicationLimitsList": [
                        {"maxCountPerRecipient": 2, "frequency": 1, "unit": "DAY"}
                    ]
                }
            }}),
        ),
        (
            "ListConnectInstanceIntegrations",
            "GET",
            "/integrations",
            Value::Null,
        ),
        (
            "PutConnectInstanceIntegration",
            "PUT",
            "/integrations",
            json!({"integrationConfig": {"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:cross-region"
            }}}),
        ),
        (
            "DeleteConnectInstanceIntegration",
            "POST",
            "/integrations/delete",
            json!({"integrationIdentifier": {"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:cross-region"
            }}}),
        ),
        (
            "DeleteInstanceOnboardingJob",
            "DELETE",
            "/onboarding",
            Value::Null,
        ),
        (
            "DeleteConnectInstanceConfig",
            "DELETE",
            "/config",
            Value::Null,
        ),
    ]
}

/// A campaign created in one region is in that region's page and in no other's, and
/// each region's page is exactly what was created through its own endpoint.
#[tokio::test]
async fn a_campaign_is_listed_only_in_the_region_it_was_created_in() {
    for (case, home, other) in region_pairs() {
        let router = router();
        let instance = "listing-instance";
        onboard(&router, &[home, other], instance).await;

        let (home_id, _) = create_campaign(&router, home, instance, "at-home").await;

        // Before anything is created in `other`, its page is empty: the campaign the
        // home region holds is not a resource that endpoint serves.
        assert_eq!(
            listed_ids(&router, other, case).await,
            Vec::<String>::new(),
            "case {case:?}: expected {other} to list no campaign while only {home} holds one"
        );

        // And once `other` holds one of its own, each page carries exactly its own
        // region's campaign — never the pair of them.
        let (other_id, _) = create_campaign(&router, other, instance, "abroad").await;
        assert_eq!(
            listed_ids(&router, home, case).await,
            vec![home_id.clone()],
            "case {case:?}: expected {home} to list only its own campaign {home_id}"
        );
        assert_eq!(
            listed_ids(&router, other, case).await,
            vec![other_id.clone()],
            "case {case:?}: expected {other} to list only its own campaign {other_id}"
        );
        assert_ne!(
            home_id, other_id,
            "case {case:?}: the store mints one id per campaign, so two campaigns never \
             share one (got {home_id} twice)"
        );
    }
}

/// Every operation that addresses a campaign by id refuses an id another region holds,
/// with HTTP 404 `ResourceNotFoundException`, and leaves that campaign exactly as it
/// was — while the same call against the region's own campaign goes through.
#[tokio::test]
async fn every_campaign_operation_refuses_an_id_from_another_region() {
    for (case, home, other) in region_pairs() {
        let router = router();
        let instance = "cross-region-instance";
        onboard(&router, &[home, other], instance).await;

        let (home_id, _) = create_campaign(&router, home, instance, "at-home").await;
        // A campaign of `other`'s own, so the refusals below are the id failing to
        // resolve in that region and not an empty store answering everything.
        let (other_id, _) = create_campaign(&router, other, instance, "abroad").await;

        let before = describe(&router, home, &home_id, case).await;

        for (operation, method, suffix, body) in campaign_operations() {
            let path = format!("/v2/campaigns/{home_id}{suffix}");
            let (status, response) = call(&router, other, method, &path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} from {other} on a {home} campaign expected HTTP \
                 404, got {status} {response}"
            );
            assert!(
                error_code(&response).ends_with("ResourceNotFoundException"),
                "case {case:?}: {operation} from {other} expected ResourceNotFoundException, \
                 got {}",
                error_code(&response)
            );

            // The same call, against the campaign that region does hold, is served:
            // the refusal above is about the region the id belongs to and nothing else.
            let own = format!("/v2/campaigns/{other_id}{suffix}");
            let (status, response) = call(&router, other, method, &own, &body).await;
            assert_ne!(
                status, 404,
                "case {case:?}: {operation} from {other} on its OWN campaign {other_id} must \
                 resolve, got {status} {response}"
            );
        }

        // Nothing the other region attempted reached the home region's campaign: it is
        // still described exactly as before, and still in the state the create left it.
        assert_eq!(
            describe(&router, home, &home_id, case).await,
            before,
            "case {case:?}: expected the {home} campaign to be untouched by every call from \
             {other}"
        );
        assert_eq!(
            campaign_state(&router, home, &home_id, case).await,
            "Initialized",
            "case {case:?}: expected the {home} campaign to still be Initialized after every \
             lifecycle call from {other}"
        );
    }
}

/// A Connect instance configuration is reachable only through the endpoint of the
/// region its onboarding job ran in: every instance operation in another region answers
/// as not found, and so does `CreateCampaign`'s onboarding gate.
#[tokio::test]
async fn an_instance_configuration_is_reachable_only_where_it_was_onboarded() {
    for (case, home, other) in region_pairs() {
        let router = router();
        let instance = "onboarded-at-home";
        onboard(&router, &[home], instance).await;

        for (operation, method, suffix, body) in instance_operations() {
            let path = format!("/v2/connect-instance/{instance}{suffix}");
            let (status, response) = call(&router, other, method, &path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} from {other} on an instance onboarded in {home} \
                 expected HTTP 404, got {status} {response}"
            );
            assert!(
                error_code(&response).ends_with("ResourceNotFoundException"),
                "case {case:?}: {operation} from {other} expected ResourceNotFoundException, \
                 got {}",
                error_code(&response)
            );
        }

        // The same instance id is not a campaign target in the other region either:
        // the gate reads that region's onboarding records, which hold nothing for it.
        let (status, response) = call(
            &router,
            other,
            "PUT",
            "/v2/campaigns",
            &json!({
                "name": "abroad",
                "connectInstanceId": instance,
                "channelSubtypeConfig": telephony_config(),
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: CreateCampaign in {other} for an instance onboarded only in {home} \
             expected HTTP 404, got {status} {response}"
        );

        // And the home region still has the configuration every one of those calls
        // failed to reach: none of them deleted or changed it.
        let (status, response) = call(
            &router,
            home,
            "GET",
            &format!("/v2/connect-instance/{instance}/config"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {home} to still hold the instance configuration, got \
             {status} {response}"
        );
    }
}

/// A campaign ARN is resolved by the endpoint it is presented to: the tag operations
/// serve it in its own region and answer the not-found code anywhere else.
#[tokio::test]
async fn a_campaign_arn_is_tagged_only_through_its_own_regions_endpoint() {
    for (case, home, other) in region_pairs() {
        let router = router();
        let instance = "tagging-instance";
        onboard(&router, &[home, other], instance).await;
        let (_, arn) = create_campaign(&router, home, instance, "tagged").await;
        let path = format!("/v2/tags/{}", encoded(&arn));

        // (operation, method, body)
        let operations: [(&str, &str, Value); 2] = [
            ("ListTagsForResource", "GET", Value::Null),
            ("TagResource", "POST", json!({"tags": {"owner": "x"}})),
        ];
        for (operation, method, body) in &operations {
            let (status, response) = call(&router, other, method, &path, body).await;
            assert_eq!(
                (status, error_code(&response)),
                (404, "NotFoundException"),
                "case {case:?}: {operation} from {other} on a {home} ARN expected HTTP 404 \
                 NotFoundException, got {status} {response}"
            );
            let (status, response) = call(&router, home, method, &path, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} from {home} on its own ARN expected HTTP 200, got \
                 {status} {response}"
            );
        }

        // The refused tag call wrote nothing: the campaign carries only the tag its own
        // region's call put on it.
        let (status, response) = call(&router, home, "GET", &path, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource in {home} expected HTTP 200, got {status} \
             {response}"
        );
        assert_eq!(
            response["tags"],
            json!({"owner": "x"}),
            "case {case:?}: expected only the tag {home} put, got {response}"
        );
    }
}

/// The per-region stores do not grow without bound: the region of a request is the
/// credential scope its caller chose, so once the store holds [`MAX_REGIONS`] regions
/// a write that would give a further one its first record is refused and stores
/// nothing, while every region already in the store keeps serving.
#[tokio::test]
async fn a_write_past_the_region_ceiling_is_refused_and_stores_nothing() {
    let router = router();
    let instance = "ceiling-instance";

    // Regions are named off the index alone, so the table is the ceiling itself
    // rather than a list of real region names: the rule is about how many scopes the
    // store holds, not which.
    for index in 0..MAX_REGIONS {
        let region = format!("zz-test-{index}");
        let (status, body) = call(
            &router,
            &region,
            "PUT",
            &format!("/v2/connect-instance/{instance}/onboarding"),
            &json!({"encryptionConfig": {"enabled": false}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"region {index} of the ceiling\": expected HTTP 200, got {status} {body}"
        );
    }

    let past = format!("zz-test-{MAX_REGIONS}");
    let (status, body) = call(
        &router,
        &past,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"one region past the ceiling\": expected HTTP 409, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case \"one region past the ceiling\": expected ConflictException, got {}",
        error_code(&body)
    );

    // The refusal stored nothing: that region holds no onboarding job at all.
    let (status, body) = call(
        &router,
        &past,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"the refused region's job\": expected HTTP 404, got {status} {body}"
    );

    // And a region already in the store is untouched by the refusal.
    let (status, body) = call(
        &router,
        "zz-test-0",
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a region within the ceiling\": expected HTTP 200, got {status} {body}"
    );
}

/// its own, each instance holds the campaign names its own region created (a name is
/// unique within its instance AND its region), and deleting a campaign in one region
/// leaves the other region's alone.
#[tokio::test]
async fn each_region_holds_its_own_resources_under_the_same_names() {
    for (case, home, other) in region_pairs() {
        let router = router();
        let instance = "shared-instance-id";
        onboard(&router, &[home, other], instance).await;

        // One name, created in both regions: the name conflict `CreateCampaign`
        // refuses is per instance within one region, so neither create is a conflict
        // with the other.
        let (home_id, _) = create_campaign(&router, home, instance, "same-name").await;
        let (other_id, _) = create_campaign(&router, other, instance, "same-name").await;

        for (region, id) in [(home, &home_id), (other, &other_id)] {
            assert_eq!(
                describe(&router, region, id, case).await["name"],
                json!("same-name"),
                "case {case:?}: expected {region} to hold its own campaign named same-name"
            );
        }

        // Deleting one region's campaign is not a delete anywhere else.
        let (status, response) = call(
            &router,
            home,
            "DELETE",
            &format!("/v2/campaigns/{home_id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCampaign in {home} expected HTTP 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(&router, home, case).await,
            Vec::<String>::new(),
            "case {case:?}: expected {home} to list nothing after deleting its campaign"
        );
        assert_eq!(
            listed_ids(&router, other, case).await,
            vec![other_id.clone()],
            "case {case:?}: expected {other} to still hold its own campaign {other_id}"
        );
    }
}
