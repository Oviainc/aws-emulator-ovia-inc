// SPDX-License-Identifier: CC0-1.0

//! Onboarding, tagging and the outbound-request data plane, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * a Connect instance is a campaign target only once its onboarding job has
//!   SUCCEEDED: `StartInstanceOnboardingJob` answers `IN_PROGRESS`, the clock owes the
//!   move to `SUCCEEDED` one [`ONBOARDING_JOB_DURATION`] later, and until then
//!   `GetConnectInstanceConfig` reports no config;
//! * `CreateCampaign` mints a campaign only for an onboarded instance, and refuses
//!   every other one with HTTP 404 `ResourceNotFoundException` storing nothing;
//! * a tag operation resolves the ARN's resource path FIRST — an ARN that names no
//!   connect-campaigns campaign is HTTP 400 `BadRequestException` — and only then asks
//!   whether that campaign exists (HTTP 404 `NotFoundException`);
//! * tagging MERGES into the one tag map `ListTagsForResource` and `DescribeCampaign`
//!   both read;
//! * `PutOutboundRequestBatch` resolves the campaign (404), then its state (409 unless
//!   `Running`), then reports each request of the batch on its own, and is idempotent
//!   in `clientToken`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
use smithy_server_runtime::MultiServiceRouter;

// One harness per crate: the routing header, the router under test and the
// request/response plumbing live in `tests/all/common.rs`.
use super::common::{call, error_code, router, router_with_kms, Harness};

/// `StartInstanceOnboardingJob` for `instance`, returning its status and body — on an
/// instance registered in the account's Connect registry first, since onboarding
/// resolves the `connectInstanceId` against it.
async fn start_onboarding(
    router: &Harness,
    region: &str,
    instance: &str,
    encryption_config: &Value,
) -> (u16, Value) {
    router.register_connect_instance(region, instance);
    call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": encryption_config}),
    )
    .await
}

/// Onboard `instance` and wait out the span the clock owes the job, so the instance is
/// a campaign target by the time this returns. Nothing in the emulator sleeps for that
/// span — only a caller that wants the settled state does.
async fn onboard(router: &Harness, region: &str, instance: &str) {
    let (status, body) = start_onboarding(router, region, instance, &json!({"enabled": false})).await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(100)).await;
}

/// `CreateCampaign` on an onboarded instance, asserting it succeeded, and returning the
/// response body.
async fn create_campaign(router: &MultiServiceRouter, region: &str, body: &Value) -> Value {
    let (status, response) = call(router, region, "PUT", "/v2/campaigns", body).await;
    assert_eq!(
        status, 200,
        "CreateCampaign {body}: expected 200, got {status} {response}"
    );
    response
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

/// The `{arn}` path label of `/v2/tags/{arn}`, percent-encoded as a single segment.
fn encoded(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The tag map a `ListTagsForResource` response carries, as an ordered map so a
/// mismatch prints deterministically.
fn tags_of(response: &Value) -> BTreeMap<String, String> {
    response
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One outbound request body: the members `OutboundRequest` requires, for telephony.
fn telephony_request(client_token: &str, phone: &str, attributes: Value) -> Value {
    json!({
        "clientToken": client_token,
        "expirationTime": "2099-01-01T00:00:00Z",
        "channelSubtypeParameters": {
            "telephony": {"destinationPhoneNumber": phone, "attributes": attributes}
        }
    })
}

/// The onboarding job's status code, or a marker naming what the response carried
/// instead.
fn job_status(response: &Value) -> &str {
    response
        .pointer("/connectInstanceOnboardingJobStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// An instance's onboarding job moves IN_PROGRESS -> SUCCEEDED on the clock, the config
/// appears only once it has, and the config that appears is the one the onboarding call
/// asked for.
///
/// The rows differ along what the rule generalises over: the shape of the instance id
/// (every character class `InstanceId`'s pattern `^[-_.a-zA-Z0-9]+$` admits, and the
/// 1-character minimum of its 1..256 length bound) and the encryption config, enabled
/// with a KMS key and disabled. The enabled row is put on a key AWS KMS really holds,
/// minted through KMS's own `CreateKey`, because a job whose key resolves to nothing
/// settles FAILED instead — which is `onboarding_job_failure`'s rule.
#[tokio::test]
async fn an_onboarding_job_settles_on_the_clock_and_then_publishes_the_config() {
    let region = "us-west-2";
    let router = router_with_kms();
    let (status, created) = router.kms(region, "CreateKey", &json!({})).await;
    assert_eq!(
        status, 200,
        "the fixture's KMS key must be created, got {status} {created}"
    );
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .expect("CreateKey reports the new key's ARN")
        .to_string();

    // (case, instance id, encryptionConfig)
    let cases: &[(&str, &str, Value)] = &[
        ("hyphenated id", "instance-1", json!({"enabled": false})),
        (
            "underscored and dotted id",
            "instance_2.a",
            json!({
                "enabled": true,
                "encryptionType": "KMS",
                "keyArn": key_arn
            }),
        ),
        ("a single-character id", "i", json!({"enabled": false})),
        (
            "a uuid-shaped id",
            "11111111-2222-3333-4444-555555555555",
            json!({"enabled": false}),
        ),
    ];

    for (case, instance, encryption_config) in cases {
        // Before onboarding there is neither a config nor a job.
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/config"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an instance with no onboarding record has no config, got \
             {status} {body}"
        );
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/onboarding"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an instance that was never onboarded has no job, got \
             {status} {body}"
        );

        // Starting the job answers the transient status, and the config is still
        // absent while it is in progress.
        let (status, started) = start_onboarding(&router, region, instance, encryption_config).await;
        assert_eq!(
            (status, job_status(&started)),
            (200, "IN_PROGRESS"),
            "case {case:?}: expected a job that is still in progress, got {status} {started}"
        );
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/config"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an instance whose job is still in progress has no config \
             yet, got {status} {body}"
        );

        // Once the clock owes it, the job has succeeded and the config is readable.
        tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(100)).await;
        let (status, settled) = call(
            &router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/onboarding"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, job_status(&settled)),
            (200, "SUCCEEDED"),
            "case {case:?}: expected the job the clock already owed, got {status} {settled}"
        );
        let (status, config) = call(
            &router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/config"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the config of an onboarded instance, got {status} {config}"
        );
        assert_eq!(
            config.pointer("/connectInstanceConfig/connectInstanceId"),
            Some(&json!(instance)),
            "case {case:?}: the config must name the instance it belongs to, got {config}"
        );
        assert_eq!(
            config.pointer("/connectInstanceConfig/encryptionConfig"),
            Some(encryption_config),
            "case {case:?}: the config must carry the encryption config onboarding was \
             given, got {config}"
        );
        let role = config
            .pointer("/connectInstanceConfig/serviceLinkedRoleArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            role.starts_with("arn:aws:iam::"),
            "case {case:?}: the config must name the Campaigns service-linked role, got \
             {config}"
        );

        // Repeating the call is idempotent: the job already finished stays finished.
        let (status, repeated) = start_onboarding(&router, region, instance, encryption_config).await;
        assert_eq!(
            (status, job_status(&repeated)),
            (200, "SUCCEEDED"),
            "case {case:?}: a repeated onboarding must not restart the job, got {status} \
             {repeated}"
        );
    }
}

/// `CreateCampaign` mints a campaign only for an instance that is onboarded to
/// Campaigns. The rows differ along the instance's onboarding state — never onboarded,
/// onboarding still in progress, onboarded — over several id shapes, and a refusal
/// stores nothing.
#[tokio::test]
async fn create_campaign_refuses_an_instance_that_is_not_onboarded() {
    let region = "ap-south-1";
    let router = router();

    // An instance whose job is still in progress: onboarded, but not waited out.
    let (status, body) = start_onboarding(&router, region, "pending-1", &json!({"enabled": false})).await;
    assert_eq!(
        status, 200,
        "expected onboarding to start for the in-progress case, got {status} {body}"
    );

    // (case, connectInstanceId, expected status)
    let cases: &[(&str, &str, u16)] = &[
        ("an instance that was never onboarded", "absent-1", 404),
        (
            "a uuid-shaped instance that was never onboarded",
            "11111111-2222-3333-4444-555555555555",
            404,
        ),
        ("an instance whose onboarding is in progress", "pending-1", 404),
        ("an onboarded instance", "onboarded-1", 200),
        ("another onboarded instance", "onboarded_2.b", 200),
    ];

    for (case, instance, expected) in cases {
        if *expected == 200 {
            onboard(&router, region, instance).await;
        }
        let request = json!({
            "name": format!("campaign for {instance}"),
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
        });
        let (status, body) = call(&router, region, "PUT", "/v2/campaigns", &request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled not-found error, got {body}"
            );
            continue;
        }
        let id = body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a minted campaign id, got {body}"
        );
        // The campaign that was minted is the one the store holds.
        let (status, described) =
            call(&router, region, "GET", &format!("/v2/campaigns/{id}"), &Value::Null).await;
        assert_eq!(
            (
                status,
                described
                    .pointer("/campaign/connectInstanceId")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
            ),
            (200, *instance),
            "case {case:?}: expected the stored campaign back, got {status} {described}"
        );
    }

    // Nothing was stored for any refused create: only the two accepted ones exist.
    let (status, listed) = call(
        &router,
        region,
        "POST",
        "/v2/campaigns-summary",
        &Value::Null,
    )
    .await;
    let stored = listed
        .get("campaignSummaryList")
        .and_then(Value::as_array)
        .map_or(usize::MAX, Vec::len);
    assert_eq!(
        (status, stored),
        (200, 2),
        "expected exactly the campaigns of onboarded instances, got {status} {listed}"
    );

    // And once the in-progress instance's job settles, the same create is accepted —
    // the refusal was about the onboarding state, not about the id.
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(100)).await;
    let (status, body) = call(
        &router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({"name": "after settling", "connectInstanceId": "pending-1"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the create to be accepted once onboarding settled, got \
         {status} {body}",
        "an instance whose onboarding has settled"
    );
}

/// A tag operation resolves the ARN's RESOURCE PATH before it asks about existence: an
/// ARN that names no connect-campaigns campaign is a bad request, a campaign ARN that
/// matches no stored campaign is not found, and only a stored campaign's own ARN
/// resolves.
///
/// The rows differ along what the rule generalises over: the resource type, the
/// presence of a resource id, the service the ARN names, and the region and account it
/// is rooted in. Every ARN here satisfies `Arn`'s own 20..500 length and pattern, so
/// each case reaches the handler rather than the boundary validator.
#[tokio::test]
async fn an_arn_that_names_no_campaign_is_refused_before_any_existence_check() {
    let region = "us-east-1";
    let router = router();
    onboard(&router, region, "instance-1").await;
    let created = create_campaign(
        &router,
        region,
        &json!({"name": "tagged", "connectInstanceId": "instance-1"}),
    )
    .await;
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .expect("a minted campaign ARN")
        .to_string();

    // (case, arn, expected status, expected error code)
    let cases: &[(&str, String, u16, &str)] = &[
        (
            "a resource path with no resource type",
            "arn:aws:connect-campaigns:us-east-1:000000000000:not-a-real-resource-path".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a resource type this service does not mint",
            "arn:aws:connect-campaigns:us-east-1:000000000000:instance/000000000000".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a campaign resource type with no id",
            "arn:aws:connect-campaigns:us-east-1:000000000000:campaign/".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "another service's ARN",
            "arn:aws:connect:us-east-1:000000000000:instance/000000000000".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a campaign id the store does not hold",
            "arn:aws:connect-campaigns:us-east-1:000000000000:campaign/999999999999".to_string(),
            404,
            "NotFoundException",
        ),
        (
            "a stored campaign's id in another region",
            arn.replace("us-east-1", "eu-west-1"),
            404,
            "NotFoundException",
        ),
        (
            "a stored campaign's id in another account",
            arn.replace("000000000000", "123456789012"),
            404,
            "NotFoundException",
        ),
    ];

    for (case, candidate, expected_status, expected_code) in cases {
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/tags/{}", encoded(candidate)),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (*expected_status, *expected_code),
            "case {case:?} (ListTagsForResource {candidate}): expected HTTP \
             {expected_status} {expected_code}, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/tags/{}", encoded(candidate)),
            &json!({"tags": {"owner": "x"}}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (*expected_status, *expected_code),
            "case {case:?} (TagResource {candidate}): expected HTTP {expected_status} \
             {expected_code}, got {status} {body}"
        );
    }

    // The stored campaign's own ARN resolves, and nothing above tagged it.
    let (status, body) = call(
        &router,
        region,
        "GET",
        &format!("/v2/tags/{}", encoded(&arn)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        (status, tags_of(&body)),
        (200, BTreeMap::new()),
        "case {:?}: a campaign nothing has tagged reports no tags, got {status} {body}",
        "the stored campaign's own ARN"
    );
}

/// Tagging merges into the one tag map a campaign holds: `ListTagsForResource` and
/// `DescribeCampaign` report the same map, a repeated key takes the new value, and the
/// keys a call does not name are left alone.
///
/// The rows differ along the number of keys a call carries (none, one, several), whether
/// they are new or already present, and the tag values.
#[tokio::test]
async fn tagging_merges_into_the_one_map_every_reader_sees() {
    let region = "ca-central-1";
    let router = router();
    onboard(&router, region, "instance-1").await;
    let created = create_campaign(
        &router,
        region,
        &json!({
            "name": "merge target",
            "connectInstanceId": "instance-1",
            "tags": {"env": "test"},
        }),
    )
    .await;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .expect("a minted ARN")
        .to_string();

    /// (case, the tags one call carries, the whole map expected afterwards)
    type Case = (&'static str, Value, &'static [(&'static str, &'static str)]);
    let cases: &[Case] = &[
        (
            "the tags CreateCampaign was given",
            Value::Null,
            &[("env", "test")],
        ),
        (
            "one new key",
            json!({"owner": "campaigns"}),
            &[("env", "test"), ("owner", "campaigns")],
        ),
        (
            "several new keys at once",
            json!({"team": "voice", "tier": "gold"}),
            &[
                ("env", "test"),
                ("owner", "campaigns"),
                ("team", "voice"),
                ("tier", "gold"),
            ],
        ),
        (
            "a key already present takes the new value",
            json!({"env": "prod"}),
            &[
                ("env", "prod"),
                ("owner", "campaigns"),
                ("team", "voice"),
                ("tier", "gold"),
            ],
        ),
        (
            "an empty tag map changes nothing",
            json!({}),
            &[
                ("env", "prod"),
                ("owner", "campaigns"),
                ("team", "voice"),
                ("tier", "gold"),
            ],
        ),
        (
            "an empty value is a value",
            json!({"note": ""}),
            &[
                ("env", "prod"),
                ("note", ""),
                ("owner", "campaigns"),
                ("team", "voice"),
                ("tier", "gold"),
            ],
        ),
    ];

    for (case, tags, expected) in cases {
        if !tags.is_null() {
            let (status, body) = call(
                &router,
                region,
                "POST",
                &format!("/v2/tags/{}", encoded(&arn)),
                &json!({"tags": tags}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the tagging to succeed, got {status} {body}"
            );
        }
        let wanted: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let (status, listed) = call(
            &router,
            region,
            "GET",
            &format!("/v2/tags/{}", encoded(&arn)),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (status, tags_of(&listed)),
            (200, wanted.clone()),
            "case {case:?}: ListTagsForResource must report the merged map, got {status} \
             {listed}"
        );

        let (status, described) =
            call(&router, region, "GET", &format!("/v2/campaigns/{id}"), &Value::Null).await;
        let described_tags = described
            .pointer("/campaign")
            .map(tags_of)
            .unwrap_or_default();
        assert_eq!(
            (status, described_tags),
            (200, wanted),
            "case {case:?}: DescribeCampaign must report the same map, got {status} \
             {described}"
        );
    }
}

/// `PutOutboundRequestBatch` resolves the campaign before anything else, then its
/// state, and only then takes the batch: an unknown id is HTTP 404
/// `ResourceNotFoundException`, a campaign that is not `Running` is HTTP 409
/// `InvalidCampaignStateException` naming that state.
///
/// The rows differ along the id (ids no store holds, of several shapes) and along the
/// state a stored campaign is in at the time of the call.
#[tokio::test]
async fn an_outbound_batch_resolves_the_campaign_and_its_state_first() {
    let region = "us-east-2";
    let router = router();
    onboard(&router, region, "instance-1").await;
    let created = create_campaign(
        &router,
        region,
        &json!({
            "name": "dialler",
            "connectInstanceId": "instance-1",
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();

    let batch = json!({
        "outboundRequests": [telephony_request(
            "11111111-2222-3333-4444-555555555555",
            "+15555550100",
            json!({}),
        )]
    });

    // An id the store does not hold — including the recorded shape of this batch, a
    // uuid client token, a future expiration time and an EMPTY attributes map, which
    // the model permits.
    for unknown in ["999999999999", "11111111-2222-3333-4444-555555555555", "nope"] {
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{unknown}/outbound-requests"),
            &batch,
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown campaign id must reach the lookup and 404, got \
             {status} {body}"
        );
    }

    // (case, the lifecycle action to take first, the state the campaign is then in,
    //  expected status)
    let cases: &[(&str, Option<&str>, &str, u16)] = &[
        ("a campaign that was only just created", None, "Initialized", 409),
        ("a running campaign", Some("start"), "Running", 200),
        ("a paused campaign", Some("pause"), "Paused", 409),
        ("a resumed campaign", Some("start"), "Running", 200),
        ("a stopped campaign", Some("stop"), "Stopped", 409),
    ];

    for (case, action, state, expected) in cases {
        if let Some(action) = action {
            let (status, body) = call(
                &router,
                region,
                "POST",
                &format!("/v2/campaigns/{id}/{action}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {action} to succeed, got {status} {body}"
            );
        }
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{id}/outbound-requests"),
            &json!({
                "outboundRequests": [telephony_request(
                    &format!("token-for-{state}"),
                    "+15555550100",
                    json!({}),
                )]
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 409 {
            assert_eq!(
                (error_code(&body), body.get("state")),
                ("InvalidCampaignStateException", Some(&json!(state))),
                "case {case:?}: the refusal must name the state the campaign is in, got \
                 {body}"
            );
        }
    }
}

/// Each request of a batch is reported on its own, the call is idempotent in
/// `clientToken`, and a channel the campaign is not configured for fails that request
/// alone.
///
/// The rows differ along batch size, the client tokens (fresh and already used), the
/// destination phone number's form, the attributes map (empty and populated) and the
/// channel asked for.
#[tokio::test]
async fn every_request_of_a_batch_is_reported_on_its_own_and_a_repeat_is_idempotent() {
    let region = "eu-central-1";
    let router = router();
    onboard(&router, region, "instance-1").await;
    let created = create_campaign(
        &router,
        region,
        &json!({
            "name": "dialler",
            "connectInstanceId": "instance-1",
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();
    let (status, body) = call(
        &router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(status, 200, "expected the campaign to start, got {status} {body}");

    /// Submit a batch and return `(clientToken, dial request id)` pairs for the
    /// successful requests and `(clientToken, failureCode)` for the failed ones.
    async fn submit(
        router: &MultiServiceRouter,
        region: &str,
        id: &str,
        requests: Vec<Value>,
    ) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let (status, body) = call(
            router,
            region,
            "PUT",
            &format!("/v2/campaigns/{id}/outbound-requests"),
            &json!({"outboundRequests": requests}),
        )
        .await;
        assert_eq!(status, 200, "expected the batch to be taken, got {status} {body}");
        let pairs = |member: &str, second: &str| -> Vec<(String, String)> {
            body.get(member)
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| {
                            (
                                entry
                                    .get("clientToken")
                                    .and_then(Value::as_str)
                                    .unwrap_or("<absent>")
                                    .to_string(),
                                entry
                                    .get(second)
                                    .and_then(Value::as_str)
                                    .unwrap_or("<absent>")
                                    .to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        (
            pairs("successfulRequests", "id"),
            pairs("failedRequests", "failureCode"),
        )
    }

    // A batch spanning phone-number forms and attribute maps: every request is taken,
    // each under its own dial request id.
    let first = vec![
        telephony_request("token.one", "+15555550100", json!({})),
        telephony_request("token-two", "15555550101", json!({"CustomerId": "c-1"})),
        telephony_request(
            "token_three",
            "1-555-555-0102",
            json!({"CustomerId": "c-2", "Region": "us"}),
        ),
    ];
    let (successful, failed) = submit(&router, region, &id, first.clone()).await;
    assert_eq!(
        successful.len(),
        3,
        "case {:?}: expected every request taken, got {successful:?} / {failed:?}",
        "a batch of three telephony requests"
    );
    assert!(
        failed.is_empty(),
        "case {:?}: expected no failures, got {failed:?}",
        "a batch of three telephony requests"
    );
    let mut ids: Vec<&String> = successful.iter().map(|(_, id)| id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "case {:?}: every taken request needs its own dial request id, got {successful:?}",
        "a batch of three telephony requests"
    );

    // The same tokens again: idempotent, so the same ids come back.
    let (repeated, failed) = submit(&router, region, &id, first).await;
    assert_eq!(
        (repeated.clone(), failed.clone()),
        (successful.clone(), Vec::new()),
        "case {:?}: a repeated batch must answer the ids it first minted, got \
         {repeated:?} / {failed:?}",
        "the same batch submitted twice"
    );

    // One token already used, one fresh: the fresh one gets a new id, the used one its
    // original.
    let mixed = vec![
        telephony_request("token.one", "+15555550100", json!({})),
        telephony_request("token-four", "+15555550103", json!({})),
    ];
    let (mixed_successful, failed) = submit(&router, region, &id, mixed).await;
    assert!(
        failed.is_empty(),
        "case {:?}: expected no failures, got {failed:?}",
        "a batch mixing a used and a fresh token"
    );
    let first_id = successful
        .iter()
        .find(|(token, _)| token == "token.one")
        .map(|(_, id)| id.clone())
        .unwrap_or_default();
    assert_eq!(
        mixed_successful
            .iter()
            .find(|(token, _)| token == "token.one")
            .map(|(_, id)| id.clone()),
        Some(first_id.clone()),
        "case {:?}: a token already taken keeps its id, got {mixed_successful:?}",
        "a batch mixing a used and a fresh token"
    );
    let fresh_id = mixed_successful
        .iter()
        .find(|(token, _)| token == "token-four")
        .map(|(_, id)| id.clone())
        .unwrap_or_default();
    assert_ne!(
        fresh_id, first_id,
        "case {:?}: a fresh token must be given its own id, got {mixed_successful:?}",
        "a batch mixing a used and a fresh token"
    );

    // A channel this campaign is not configured for fails that request alone, with the
    // modelled per-request failure code, and the request beside it is still taken.
    let other_channels = vec![
        json!({
            "clientToken": "token-sms",
            "expirationTime": "2099-01-01T00:00:00Z",
            "channelSubtypeParameters": {
                "sms": {"destinationPhoneNumber": "+15555550104", "templateParameters": {}}
            }
        }),
        json!({
            "clientToken": "token-email",
            "expirationTime": "2099-01-01T00:00:00Z",
            "channelSubtypeParameters": {
                "email": {
                    "destinationEmailAddress": "someone@example.com",
                    "templateParameters": {}
                }
            }
        }),
        telephony_request("token-five", "+15555550105", json!({})),
    ];
    let (taken, failed) = submit(&router, region, &id, other_channels).await;
    assert_eq!(
        taken.iter().map(|(token, _)| token.as_str()).collect::<Vec<_>>(),
        vec!["token-five"],
        "case {:?}: only the configured channel is taken, got {taken:?} / {failed:?}",
        "a batch asking for channels the campaign is not configured for"
    );
    assert_eq!(
        failed,
        vec![
            ("token-sms".to_string(), "InvalidInput".to_string()),
            ("token-email".to_string(), "InvalidInput".to_string()),
        ],
        "case {:?}: an unconfigured channel fails that request with the modelled code, \
         got {failed:?}",
        "a batch asking for channels the campaign is not configured for"
    );
}

/// A `clientToken` outside its modelled pattern or length is refused at the boundary,
/// while every token the model admits reaches the campaign lookup — so a client can
/// tell a malformed request from an unknown campaign.
///
/// The rows differ along what the pattern and the length bound generalise over: the
/// character classes `^[a-zA-Z0-9_\-.]*$` admits and ones it does not, and both ends of
/// `ClientToken`'s 0..200 length bound plus one past it.
#[tokio::test]
async fn a_client_token_outside_its_pattern_is_refused_and_a_valid_one_reaches_the_lookup() {
    let region = "us-east-1";
    let router = router();
    // No campaign is created: every row below asks about an id the store does not hold,
    // so a token the model admits must reach the lookup and 404 while a token it does
    // not must be refused before the handler runs.
    let unknown = "999999999999";

    // (case, clientToken, expected status)
    let cases: &[(&str, String, u16)] = &[
        (
            "a uuid, as an SDK mints one",
            "11111111-2222-3333-4444-555555555555".to_string(),
            404,
        ),
        (
            "every character class the pattern admits",
            "aZ9_-.".to_string(),
            404,
        ),
        ("the empty token the 0 minimum admits", String::new(), 404),
        ("the 200-character maximum", "t".repeat(200), 404),
        ("one character past the maximum", "t".repeat(201), 400),
        ("a space", "token with space".to_string(), 400),
        ("a slash", "token/slash".to_string(), 400),
        ("a plus", "token+plus".to_string(), 400),
        ("a colon", "token:colon".to_string(), 400),
    ];

    for (case, client_token, expected) in cases {
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{unknown}/outbound-requests"),
            &json!({
                "outboundRequests": [telephony_request(client_token, "+15555550100", json!({}))]
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?} (clientToken {client_token:?}): expected HTTP {expected}, got \
             {status} {body}"
        );
        let expected_code = if *expected == 400 {
            "ValidationException"
        } else {
            "ResourceNotFoundException"
        };
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// EVERY `date-time` member this service's model declares reports
/// `@timestampFormat("date-time")` on its own member schema — the one thing the JSON
/// codec consults when it reads or writes a timestamp (`Schema::timestamp_format()`),
/// and whose absence made the codec fall back to epoch seconds and REFUSE the ISO-8601
/// instant every SDK sends.
///
/// The catalogue is every member of every shape of this service's model whose target is
/// `TimeStamp` — the only timestamp shape it declares, and one the model marks
/// `@timestampFormat("date-time")` — from
/// `smithy_query(query="shape", shape="Schedule,ProfileOutboundRequest")` and
/// `smithy_query(query="closure", operation="PutOutboundRequestBatch")`. It includes the
/// two `Schedule` members the issue did not cite, which `CreateCampaign` and
/// `DescribeCampaign` carry.
#[test]
fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    use aws_smithy_schema::traits::TimestampFormat;
    use aws_smithy_schema::Schema;
    use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
        OutboundRequest, ProfileOutboundRequest, Schedule,
    };

    let date_time_members: &[(&str, &Schema)] = &[
        (
            "OutboundRequest$expirationTime",
            &OutboundRequest::M_EXPIRATION_TIME,
        ),
        ("Schedule$startTime", &Schedule::M_START_TIME),
        ("Schedule$endTime", &Schedule::M_END_TIME),
        (
            "ProfileOutboundRequest$expirationTime",
            &ProfileOutboundRequest::M_EXPIRATION_TIME,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|carried| carried.format());
        assert_eq!(
            format,
            Some(TimestampFormat::DateTime),
            "case {case:?}: the member targets TimeStamp, which the model marks \
             @timestampFormat(\"date-time\"); its member schema reported {format:?}"
        );
    }
}

/// A campaign configured for several channels takes a request on each of them: the
/// channel gate is the campaign's own `channelSubtypeConfig`, not a fixed channel.
///
/// The rows are the four members of the `ChannelSubtypeParameters` union, two of which
/// this campaign configures and two of which it does not.
#[tokio::test]
async fn a_multi_channel_campaign_takes_a_request_on_every_channel_it_configures() {
    let region = "us-west-1";
    let router = router();
    onboard(&router, region, "instance-1").await;
    let created = create_campaign(
        &router,
        region,
        &json!({
            "name": "multi",
            "connectInstanceId": "instance-1",
            "channelSubtypeConfig": {
                "telephony": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                },
                "sms": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {
                        "connectSourcePhoneNumberArn":
                            "arn:aws:connect:us-west-1:000000000000:phone-number/p-1",
                        "wisdomTemplateArn":
                            "arn:aws:wisdom:us-west-1:000000000000:template/t-1"
                    }
                }
            },
        }),
    )
    .await;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();
    let (status, body) = call(
        &router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(status, 200, "expected the campaign to start, got {status} {body}");

    // (case, the channelSubtypeParameters union member, whether the campaign configures it)
    let cases: &[(&str, Value, bool)] = &[
        (
            "telephony, which it configures",
            json!({"telephony": {"destinationPhoneNumber": "+15555550100", "attributes": {}}}),
            true,
        ),
        (
            "sms, which it configures",
            json!({"sms": {"destinationPhoneNumber": "+15555550101", "templateParameters": {}}}),
            true,
        ),
        (
            "email, which it does not",
            json!({
                "email": {
                    "destinationEmailAddress": "someone@example.com",
                    "templateParameters": {}
                }
            }),
            false,
        ),
        (
            "whatsApp, which it does not",
            json!({
                "whatsApp": {
                    "destinationPhoneNumber": "+15555550102",
                    "templateParameters": {}
                }
            }),
            false,
        ),
    ];

    for (index, (case, parameters, configured)) in cases.iter().enumerate() {
        let mut request = Map::new();
        request.insert("clientToken".to_string(), json!(format!("token-{index}")));
        request.insert(
            "expirationTime".to_string(),
            json!("2099-01-01T00:00:00Z"),
        );
        request.insert("channelSubtypeParameters".to_string(), parameters.clone());
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{id}/outbound-requests"),
            &json!({"outboundRequests": [Value::Object(request)]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch to be taken, got {status} {body}"
        );
        let successful = body
            .get("successfulRequests")
            .and_then(Value::as_array)
            .map_or(usize::MAX, Vec::len);
        let failed = body
            .get("failedRequests")
            .and_then(Value::as_array)
            .map_or(usize::MAX, Vec::len);
        let expected = if *configured { (1, 0) } else { (0, 1) };
        assert_eq!(
            (successful, failed),
            expected,
            "case {case:?}: expected {expected:?} (successful, failed), got {status} {body}"
        );
    }
}
