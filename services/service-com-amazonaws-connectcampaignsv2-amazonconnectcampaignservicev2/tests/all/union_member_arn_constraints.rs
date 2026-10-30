// SPDX-License-Identifier: CC0-1.0

//! The ARN constraints that sit BEHIND a union, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rule under test, over every union of this service
//! that carries an ARN — `Source` (`CreateCampaign`, `UpdateCampaignSource`),
//! `IntegrationConfig` (`PutConnectInstanceIntegration`), `IntegrationIdentifier`
//! (`DeleteConnectInstanceIntegration`) and `ChannelSubtypeParameters`
//! (`PutOutboundRequestBatch`):
//!
//! * an ARN-valued member behind a union member is held to the `@length` and
//!   `@pattern` of the shape it targets — `Arn` (len 20..500) for all of them but a
//!   Lambda integration's function, which targets `LambdaArn` (len 1..140) — so a
//!   value outside those bounds is HTTP 400 `ValidationException` naming the member,
//!   and the call stores nothing;
//! * every value inside them is accepted and read back by the operation that reports
//!   it, including at each published bound of the `@length`.
//!
//! The bounds this suite asserts are read back out of `smithy-model/` by
//! [`the_arn_rules_this_crate_restates_are_the_models`], so no row below can be
//! asserting a bound the model no longer declares.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, create_campaign_with, describe_campaign, error_code, padded_arn, router,
    router_with_engine, Harness,
};
use super::connect_instance_integrations::{
    list_integrations, object_type_names, put_integration, summaries,
};

/// The region every case here signs for: one region holds every record a case reads,
/// and nothing in the rule under test is region-dependent.
const REGION: &str = "us-east-1";

/// `Arn`'s published `@length` bounds, from
/// `smithy_query(query="shape", shape="Arn")`, and pinned to `smithy-model/` by
/// [`the_arn_rules_this_crate_restates_are_the_models`].
const ARN_MIN_LEN: usize = 20;
const ARN_MAX_LEN: usize = 500;

/// `LambdaArn`'s published `@length` bounds, from
/// `smithy_query(query="shape", shape="LambdaArn")` — the shape a Lambda
/// integration's `functionArn` targets, which no other member of these unions does.
const LAMBDA_ARN_MIN_LEN: usize = 1;
const LAMBDA_ARN_MAX_LEN: usize = 140;

/// Well-formed ARN heads [`padded_arn`] grows a value of an exact length from.
const PROFILE_ARN_HEAD: &str = "arn:aws:profile:us-east-1:000000000000:domains/";
const LAMBDA_ARN_HEAD: &str = "arn:aws:lambda:us-east-1:000000000000:function:";

/// A value a shape forbids, as a row of the tables below: the case name, the value,
/// and the requirement clause the refusal must state for it.
pub(crate) type BadValue = (&'static str, String, String);

/// How one case builds the value under test from an ARN.
type Build = fn(&str) -> Value;

/// One ARN-valued member behind a union: the member, how a value carrying it is
/// built, and the path (relative to the operation's own member) a refusal names it
/// by.
type UnionMember = (&'static str, Build, &'static str);

/// A [`UnionMember`] together with the values the shape IT targets forbids — a Lambda
/// function's ARN targets a different shape from every other member of these unions.
type UnionMemberCases = (&'static str, Build, &'static str, Vec<BadValue>);

/// The shortest value `Arn` admits: exactly [`ARN_MIN_LEN`] characters, and inside
/// the shape's `@pattern`. Hand-written, because 20 characters is shorter than any
/// ARN head [`padded_arn`] builds from.
const SHORTEST_ARN: &str = "arn:a:a:aa-aa-1:a:xx";

/// One character shorter than `Arn` admits, and otherwise well-formed — so the
/// refusal it draws is the length bound and nothing else.
const TOO_SHORT_ARN: &str = "arn:a:a:aa-aa-1:a:x";

/// The requirement clause a refusal states for a value below a `@length` minimum.
fn min_length(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement clause a refusal states for a value past a `@length` maximum.
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement clause a refusal states for a value outside a `@pattern`.
fn pattern() -> String {
    "Member must satisfy regular expression pattern".to_string()
}

/// Values no `Arn` admits, each with the requirement it breaks.
///
/// The rows differ along what the shape's bounds generalise over: the empty value, a
/// single character, one character below the published minimum, one character past
/// the published maximum, and three values of an admitted LENGTH that the `@pattern`
/// refuses — no `arn:` prefix at all, a region field the pattern does not spell, and
/// a service field carrying a character class it excludes.
pub(crate) fn arns_outside_arns_bounds() -> Vec<BadValue> {
    vec![
        ("the empty value", String::new(), min_length(ARN_MIN_LEN)),
        ("a single character", "x".to_string(), min_length(ARN_MIN_LEN)),
        (
            "one character below the length minimum",
            TOO_SHORT_ARN.to_string(),
            min_length(ARN_MIN_LEN),
        ),
        (
            "one character past the length maximum",
            padded_arn(PROFILE_ARN_HEAD, ARN_MAX_LEN + 1),
            max_length(ARN_MAX_LEN),
        ),
        (
            "long enough, but not an ARN at all",
            "not-an-arn-but-long-enough".to_string(),
            pattern(),
        ),
        (
            "an ARN whose region field is not a region",
            "arn:aws:profile:useast1:000000000000:domains/d-1".to_string(),
            pattern(),
        ),
        (
            "an ARN whose service field carries an excluded character",
            "arn:aws:pro file:us-east-1:000000000000:domains/d-1".to_string(),
            pattern(),
        ),
        (
            // The `@pattern` spells the region field `[a-z]{2}-[a-z]+-\d{1,2}`, so a
            // GovCloud region's second hyphen group is outside it even though the
            // partition is one the pattern admits.
            "an ARN whose region carries a second hyphen group",
            "arn:aws-us-gov:profile:us-gov-west-1:000000000000:domains/d-6/segments/s-6"
                .to_string(),
            pattern(),
        ),
    ]
}

/// Values no `LambdaArn` admits, each with the requirement it breaks.
///
/// The rows differ along the same axes as [`arns_outside_arns_bounds`], read against
/// this shape's own bounds: below its 1-character minimum, past its 140-character
/// maximum, a well-formed ARN of another service (its `@pattern` admits a Lambda
/// function ARN alone), and a Lambda ARN whose account field is not twelve digits.
pub(crate) fn arns_outside_lambda_arns_bounds() -> Vec<BadValue> {
    vec![
        (
            "the empty value",
            String::new(),
            min_length(LAMBDA_ARN_MIN_LEN),
        ),
        (
            "one character past the length maximum",
            padded_arn(LAMBDA_ARN_HEAD, LAMBDA_ARN_MAX_LEN + 1),
            max_length(LAMBDA_ARN_MAX_LEN),
        ),
        (
            "a well-formed ARN of another service",
            "arn:aws:profile:us-east-1:000000000000:domains/d-1".to_string(),
            pattern(),
        ),
        (
            "a Lambda ARN whose account is not twelve digits",
            "arn:aws:lambda:us-east-1:123:function:short-account".to_string(),
            pattern(),
        ),
    ]
}

/// Assert `response` is the modelled refusal of the member at `path`, stating
/// `requirement`.
pub(crate) fn assert_refused(
    case: &str,
    status: u16,
    response: &Value,
    path: &str,
    requirement: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {response}"
    );
    assert!(
        error_code(response).ends_with("ValidationException"),
        "case {case:?}: expected ValidationException, got {}",
        error_code(response)
    );
    let message = response["message"].as_str().unwrap_or("<no message>");
    assert!(
        message.contains(&format!("at '{path}'")),
        "case {case:?}: expected the refusal to name the member at {path:?}, got {message:?}"
    );
    assert!(
        message.contains(requirement),
        "case {case:?}: expected the refusal to state {requirement:?}, got {message:?}"
    );
}

/// A `CreateCampaign` body for `name` on `instance`, sourced from `source` — the
/// members the model requires, plus the one under test. Built here rather than through
/// the shared fixture because every case below expects the create to be REFUSED, and
/// the fixture asserts it succeeded.
fn create_body(name: &str, instance: &str, source: &Value) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            }
        },
        "source": source,
    })
}

/// `UpdateCampaignSource` for `id`.
async fn update_source(
    router: &MultiServiceRouter,
    id: &str,
    source: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/source"),
        &json!({"source": source}),
    )
    .await
}

/// How many campaigns this region holds, as `ListCampaigns` reports them — how a case
/// asserts a refused create minted nothing.
async fn campaign_count(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, body) = call(router, REGION, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body.get("campaignSummaryList")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// `DeleteConnectInstanceIntegration` for `instance_id` with `identifier`.
async fn delete_integration(
    router: &MultiServiceRouter,
    instance_id: &str,
    identifier: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/connect-instance/{instance_id}/integrations/delete"),
        &json!({"integrationIdentifier": identifier}),
    )
    .await
}

/// Onboard `instance` so it is a target for integrations, through the shared campaign
/// fixture — which onboards the instance before it creates anything.
async fn onboarded_campaign(router: &Harness, instance: &str, name: &str) -> String {
    create_campaign_with(router, REGION, instance, name, json!({})).await
}

/// The bounds the emulator judges these ARNs by are the model's.
///
/// Every ARN behind a union but one is judged straight off its structure's GENERATED
/// schema, which carries the model's own constraint traits. The exception is
/// `Source$customerProfilesSegmentArn`, a bare string with no structure schema to
/// walk, whose bounds the crate restates once (`state::ARN_MIN_LEN` and friends).
/// This pins that restatement — and this suite's own bounds — to `smithy-model/`, so
/// a model refresh that moved a bound fails here instead of leaving the emulator
/// judging by a stale one.
#[test]
fn the_arn_rules_this_crate_restates_are_the_models() {
    use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json"
    );
    let text = std::fs::read_to_string(path).expect("the service's Smithy model is readable");
    let model: Value = serde_json::from_str(&text).expect("the service's Smithy model is JSON");
    let traits = |shape: &str| {
        model["shapes"][format!("com.amazonaws.connectcampaignsv2#{shape}")]["traits"].clone()
    };

    let arn = traits("Arn");
    assert_eq!(
        arn["smithy.api#length"]["min"],
        json!(state::ARN_MIN_LEN),
        "case \"Arn @length min\": the modelled floor must be the one the handlers \
         enforce, got {arn}"
    );
    assert_eq!(
        arn["smithy.api#length"]["max"],
        json!(state::ARN_MAX_LEN),
        "case \"Arn @length max\": the modelled ceiling must be the one the handlers \
         enforce, got {arn}"
    );
    assert_eq!(
        arn["smithy.api#pattern"],
        json!(state::ARN_PATTERN),
        "case \"Arn @pattern\": the modelled pattern must be the one the handlers \
         enforce, got {arn}"
    );
    assert_eq!(
        (state::ARN_MIN_LEN as usize, state::ARN_MAX_LEN as usize),
        (ARN_MIN_LEN, ARN_MAX_LEN),
        "case \"this suite's Arn bounds\": the rows must be built from the bounds the \
         handlers enforce"
    );

    // `LambdaArn`'s bounds are never restated — the framework's walk reads them off
    // the generated schema — but this suite's rows are built from them.
    let lambda_arn = traits("LambdaArn");
    assert_eq!(
        lambda_arn["smithy.api#length"]["min"],
        json!(LAMBDA_ARN_MIN_LEN),
        "case \"LambdaArn @length min\": this suite's floor must be the modelled one, \
         got {lambda_arn}"
    );
    assert_eq!(
        lambda_arn["smithy.api#length"]["max"],
        json!(LAMBDA_ARN_MAX_LEN),
        "case \"LambdaArn @length max\": this suite's ceiling must be the modelled one, \
         got {lambda_arn}"
    );
}

/// An ARN behind the `Source` union that the shape forbids is refused by both paths
/// that set a source, and neither stores it.
///
/// The rows are every ARN-valued member the union reaches (source:
/// `smithy_query(query="shape", shape="Source")` and `shape="EventTrigger"`) crossed
/// with [`arns_outside_arns_bounds`].
#[tokio::test]
async fn a_source_arn_outside_arns_bounds_is_refused_and_stores_nothing() {
    let router = router();
    let instance = "source-arn-bounds";
    let stored = json!({"customerProfilesSegmentArn":
        "arn:aws:profile:us-east-1:000000000000:domains/d-1/segments/kept"});
    let id = create_campaign_with(
        &router,
        REGION,
        instance,
        "sourced",
        json!({"source": stored.clone()}),
    )
    .await;
    let before = campaign_count(&router, "the fixture campaign").await;

    // (the member, the source value carrying `arn`, the path a refusal names)
    let members: &[UnionMember] = &[
        (
            "customerProfilesSegmentArn",
            |arn| json!({"customerProfilesSegmentArn": arn}),
            "source.customerProfilesSegmentArn",
        ),
        (
            "eventTrigger.customerProfilesDomainArn",
            |arn| json!({"eventTrigger": {"customerProfilesDomainArn": arn}}),
            "source.eventTrigger.customerProfilesDomainArn",
        ),
    ];

    for (member, build, path) in members {
        for (index, (value_case, value, requirement)) in
            arns_outside_arns_bounds().into_iter().enumerate()
        {
            let source = build(&value);

            let case = format!("UpdateCampaignSource: {member} sent {value_case}");
            let (status, body) = update_source(&router, &id, &source).await;
            assert_refused(&case, status, &body, path, &requirement);
            let campaign = describe_campaign(&router, REGION, &id, &case).await;
            assert_eq!(
                campaign["source"], stored,
                "case {case:?}: expected the refusal to leave {stored} stored, got {:?}",
                campaign["source"]
            );

            let case = format!("CreateCampaign: {member} sent {value_case}");
            let (status, body) = call(
                &router,
                REGION,
                "PUT",
                "/v2/campaigns",
                &create_body(&format!("born-{member}-{index}"), instance, &source),
            )
            .await;
            assert_refused(&case, status, &body, path, &requirement);
            let count = campaign_count(&router, &case).await;
            assert_eq!(
                count, before,
                "case {case:?}: expected the refused create to mint nothing, got {count} \
                 campaigns (was {before})"
            );
        }
    }
}

/// An ARN behind the `Source` union that the shape admits is accepted and read back,
/// at both published bounds of the `@length` and over the forms an ARN takes.
///
/// This is the other side of the rule: the check refuses what the shape forbids and
/// nothing else.
#[tokio::test]
async fn a_source_arn_arn_admits_is_accepted_and_read_back() {
    let router = router();
    let instance = "source-arn-accepted";
    let id = onboarded_campaign(&router, instance, "accepting").await;

    // (case, the source to set)
    let cases: Vec<(&str, Value)> = vec![
        (
            "the shortest ARN the length minimum admits",
            json!({"customerProfilesSegmentArn": SHORTEST_ARN}),
        ),
        (
            "an ARN of exactly the length maximum",
            json!({"customerProfilesSegmentArn": padded_arn(PROFILE_ARN_HEAD, ARN_MAX_LEN)}),
        ),
        (
            "a nested resource path with a qualifier",
            json!({"customerProfilesSegmentArn":
                "arn:aws:profile:us-east-1:000000000000:domains/d-1/segments/s-1:v2"}),
        ),
        (
            "another partition behind the event trigger",
            json!({"eventTrigger": {"customerProfilesDomainArn":
                "arn:aws-cn:profile:cn-north-1:000000000000:domains/d-2"}}),
        ),
        (
            "an event trigger carrying no ARN at all",
            json!({"eventTrigger": {}}),
        ),
        (
            "the shortest ARN behind the event trigger",
            json!({"eventTrigger": {"customerProfilesDomainArn": SHORTEST_ARN}}),
        ),
    ];

    for (case, source) in &cases {
        let (status, body) = update_source(&router, &id, source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the source to be accepted, got {status} {body}"
        );
        let campaign = describe_campaign(&router, REGION, &id, case).await;
        assert_eq!(
            campaign["source"], *source,
            "case {case:?}: expected the stored source {source}, got {:?}",
            campaign["source"]
        );
    }

    // The create path stores the same values, and `DescribeCampaign` projects them.
    for (index, (case, source)) in cases.iter().enumerate() {
        let created = create_campaign_with(
            &router,
            REGION,
            instance,
            &format!("created-{index}"),
            json!({"source": source}),
        )
        .await;
        let campaign = describe_campaign(&router, REGION, &created, case).await;
        assert_eq!(
            campaign["source"], *source,
            "case {case:?}: expected the create to store {source}, got {:?}",
            campaign["source"]
        );
    }
}

/// An ARN behind the integration unions that its shape forbids is refused by the put
/// and by the delete, and neither changes what the instance holds.
///
/// The rows are every ARN-valued member of `IntegrationConfig` /
/// `IntegrationIdentifier` (source: `smithy_query(query="shape",
/// shape="IntegrationConfig")`) crossed with the bad values of the shape each targets
/// — `Arn` for the Customer Profiles domain and the Q Connect knowledge base,
/// `LambdaArn` for the Lambda function.
#[tokio::test]
async fn an_integration_arn_outside_its_shapes_bounds_is_refused_and_changes_nothing() {
    let router = router();
    let instance = "integration-arn-bounds";
    onboarded_campaign(&router, instance, "integration-fixture").await;

    // One integration the instance really holds, so each refusal below is weighed
    // against a store that is not empty.
    let held = json!({"lambda": {
        "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:held-handler"
    }});
    let (status, body) = put_integration(&router, instance, &held).await;
    assert_eq!(
        status, 200,
        "case \"the held integration\": expected HTTP 200, got {status} {body}"
    );
    let (status, body) = list_integrations(&router, instance, "").await;
    assert_eq!(
        status, 200,
        "case \"the held integration\": expected HTTP 200 from the list, got {status} {body}"
    );
    let before = summaries(&body);

    // (the union member, the config/identifier carrying `arn`, the path a refusal
    // names, the bad values of the shape that member targets)
    let members: Vec<UnionMemberCases> = vec![
        (
            "customerProfiles.domainArn",
            |arn| json!({"customerProfiles": {"domainArn": arn, "objectTypeNames": {}}}),
            "customerProfiles.domainArn",
            arns_outside_arns_bounds(),
        ),
        (
            "qConnect.knowledgeBaseArn",
            |arn| json!({"qConnect": {"knowledgeBaseArn": arn}}),
            "qConnect.knowledgeBaseArn",
            arns_outside_arns_bounds(),
        ),
        (
            "lambda.functionArn",
            |arn| json!({"lambda": {"functionArn": arn}}),
            "lambda.functionArn",
            arns_outside_lambda_arns_bounds(),
        ),
    ];

    for (member, build, path, bad_values) in &members {
        for (value_case, value, requirement) in bad_values {
            let payload = build(value);

            let case = format!("PutConnectInstanceIntegration: {member} sent {value_case}");
            let (status, body) = put_integration(&router, instance, &payload).await;
            assert_refused(
                &case,
                status,
                &body,
                &format!("integrationConfig.{path}"),
                requirement,
            );

            // The identifier carries the same ARN member, minus the object-type map
            // the config requires beside it.
            let identifier = match *member {
                "customerProfiles.domainArn" => json!({"customerProfiles": {"domainArn": value}}),
                other => {
                    assert!(
                        other == "qConnect.knowledgeBaseArn" || other == "lambda.functionArn",
                        "case {case:?}: unexpected member {other:?}"
                    );
                    payload.clone()
                }
            };
            let case = format!("DeleteConnectInstanceIntegration: {member} sent {value_case}");
            let (status, body) = delete_integration(&router, instance, &identifier).await;
            assert_refused(
                &case,
                status,
                &body,
                &format!("integrationIdentifier.{path}"),
                requirement,
            );

            let (status, body) = list_integrations(&router, instance, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
            );
            let after = summaries(&body);
            assert_eq!(
                after, before,
                "case {case:?}: expected the refusals to leave the integrations alone, \
                 got {after:?} (was {before:?})"
            );
        }
    }
}

/// An ARN behind the integration unions that its shape admits is stored, listed and
/// deleted — including at each published bound of the `@length` of the shape it
/// targets.
#[tokio::test]
async fn an_integration_arn_its_shape_admits_is_stored_listed_and_deleted() {
    let router = router();
    let instance = "integration-arn-accepted";
    onboarded_campaign(&router, instance, "integration-accepted-fixture").await;

    // (case, the config, the identifier that names it)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a Customer Profiles domain at the shortest ARN the minimum admits",
            json!({"customerProfiles": {
                "domainArn": SHORTEST_ARN,
                "objectTypeNames": object_type_names(1),
            }}),
            json!({"customerProfiles": {"domainArn": SHORTEST_ARN}}),
        ),
        (
            "a Q Connect knowledge base at exactly the length maximum",
            json!({"qConnect": {
                "knowledgeBaseArn": padded_arn(PROFILE_ARN_HEAD, ARN_MAX_LEN),
            }}),
            json!({"qConnect": {
                "knowledgeBaseArn": padded_arn(PROFILE_ARN_HEAD, ARN_MAX_LEN),
            }}),
        ),
        (
            "a Lambda function at exactly LambdaArn's length maximum",
            json!({"lambda": {
                "functionArn": padded_arn(LAMBDA_ARN_HEAD, LAMBDA_ARN_MAX_LEN),
            }}),
            json!({"lambda": {
                "functionArn": padded_arn(LAMBDA_ARN_HEAD, LAMBDA_ARN_MAX_LEN),
            }}),
        ),
        (
            "a Lambda function ARN qualified with an alias, in another partition",
            json!({"lambda": {
                "functionArn": "arn:aws-cn:lambda:cn-north-1:000000000000:function:handler:live",
            }}),
            json!({"lambda": {
                "functionArn": "arn:aws-cn:lambda:cn-north-1:000000000000:function:handler:live",
            }}),
        ),
    ];

    for (case, config, identifier) in &cases {
        let (status, body) = put_integration(&router, instance, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the integration to be accepted, got {status} {body}"
        );

        let (status, body) = list_integrations(&router, instance, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
        );
        let listed = summaries(&body);
        assert!(
            listed.iter().any(|summary| summary == config),
            "case {case:?}: expected the list to report {config}, got {listed:?}"
        );

        // The identifier path takes the same ARN, so the record it names is removed.
        let (status, body) = delete_integration(&router, instance, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the integration to be deleted, got {status} {body}"
        );
        let (status, body) = list_integrations(&router, instance, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
        );
        let listed = summaries(&body);
        assert!(
            !listed.iter().any(|summary| summary == config),
            "case {case:?}: expected {config} gone from the list, got {listed:?}"
        );
    }
}

/// An ARN behind the `channelSubtypeParameters` union that `Arn` forbids refuses the
/// WHOLE batch, before any request of it is taken: a constraint violation is a bad
/// request, not a per-request outcome.
///
/// The rows are every ARN-valued member the union reaches (source:
/// `smithy_query(query="shape", shape="SmsChannelSubtypeParameters")` and its email
/// and WhatsApp siblings) crossed with [`arns_outside_arns_bounds`], each sent beside
/// a well-formed request the campaign's channel config does serve.
#[tokio::test]
async fn a_batch_carrying_an_arn_outside_arns_bounds_is_refused_whole() {
    let (router, engine) = router_with_engine();
    let instance = "batch-arn-bounds";
    let id = create_campaign_with(
        &router,
        REGION,
        instance,
        "batched",
        json!({"channelSubtypeConfig": {
            "sms": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourcePhoneNumberArn":
                        "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                    "wisdomTemplateArn":
                        "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
                }
            }
        }}),
    )
    .await;
    let (status, body) = call(
        &router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"start the campaign\": expected HTTP 200, got {status} {body}"
    );

    /// The requests the campaign's buffer holds, straight out of the store — how a
    /// case asserts a refused batch took nothing.
    fn buffered(engine: &engine::InMemoryEngine, id: &str) -> BTreeSet<String> {
        engine
            .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
            .lock()
            .campaign(REGION, id)
            .expect("the campaign under test")
            .outbound_requests
            .keys()
            .cloned()
            .collect()
    }

    // (the channel and member under test, the parameters carrying `arn`)
    let members: &[(&str, Build)] = &[
        ("sms.connectSourcePhoneNumberArn", |arn| {
            json!({"sms": {
                "destinationPhoneNumber": "+15555550101",
                "connectSourcePhoneNumberArn": arn,
                "templateParameters": {},
            }})
        }),
        ("sms.templateArn", |arn| {
            json!({"sms": {
                "destinationPhoneNumber": "+15555550101",
                "templateArn": arn,
                "templateParameters": {},
            }})
        }),
        ("email.templateArn", |arn| {
            json!({"email": {
                "destinationEmailAddress": "someone@example.com",
                "templateArn": arn,
                "templateParameters": {},
            }})
        }),
        ("whatsApp.connectSourcePhoneNumberArn", |arn| {
            json!({"whatsApp": {
                "destinationPhoneNumber": "+15555550102",
                "connectSourcePhoneNumberArn": arn,
                "templateParameters": {},
            }})
        }),
        ("whatsApp.templateArn", |arn| {
            json!({"whatsApp": {
                "destinationPhoneNumber": "+15555550102",
                "templateArn": arn,
                "templateParameters": {},
            }})
        }),
    ];

    let before = buffered(&engine, &id);
    for (member, build) in members {
        for (index, (value_case, value, requirement)) in
            arns_outside_arns_bounds().into_iter().enumerate()
        {
            let case = format!("{member} sent {value_case}");
            // A request the campaign does serve, first in the list, so the refusal is
            // the batch's and not that request's.
            let well_formed = json!({
                "clientToken": format!("well-formed-{member}-{index}"),
                "expirationTime": "2099-01-01T00:00:00Z",
                "channelSubtypeParameters": {"sms": {
                    "destinationPhoneNumber": "+15555550100",
                    "templateParameters": {},
                }},
            });
            let offending = json!({
                "clientToken": format!("offending-{member}-{index}"),
                "expirationTime": "2099-01-01T00:00:00Z",
                "channelSubtypeParameters": build(&value),
            });
            let (status, body) = call(
                &router,
                REGION,
                "PUT",
                &format!("/v2/campaigns/{id}/outbound-requests"),
                &json!({"outboundRequests": [well_formed, offending]}),
            )
            .await;
            // The offending request is the SECOND element, which the refusal names
            // the way the boundary walk spells a list element.
            assert_refused(
                &case,
                status,
                &body,
                &format!("outboundRequests.2.member.channelSubtypeParameters.{member}"),
                &requirement,
            );
            let after = buffered(&engine, &id);
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused batch to take nothing, got {after:?} \
                 (was {before:?})"
            );
        }
    }

    // The same batch without the offending ARN goes through, so the refusals above
    // are the constraint and not the batch path being broken.
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        &format!("/v2/campaigns/{id}/outbound-requests"),
        &json!({"outboundRequests": [{
            "clientToken": "accepted",
            "expirationTime": "2099-01-01T00:00:00Z",
            "channelSubtypeParameters": {"sms": {
                "destinationPhoneNumber": "+15555550100",
                "connectSourcePhoneNumberArn": SHORTEST_ARN,
                "templateArn": padded_arn(PROFILE_ARN_HEAD, ARN_MAX_LEN),
                "templateParameters": {},
            }},
        }]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ARNs inside the bounds\": expected HTTP 200, got {status} {body}"
    );
    let taken = buffered(&engine, &id);
    assert!(
        taken.contains("accepted"),
        "case \"ARNs inside the bounds\": expected the request to be taken, got {taken:?}"
    );
}
