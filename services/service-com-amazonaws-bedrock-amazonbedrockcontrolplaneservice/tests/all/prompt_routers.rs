// SPDX-License-Identifier: CC0-1.0

//! The prompt router resource end-to-end: `CreatePromptRouter` writes it,
//! `GetPromptRouter` reads it back, over the same `MultiServiceRouter` the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! recorded request:
//!
//! 1. **`routingCriteria.responseQualityDifference` is accepted only at the multiples of
//!    5 inside its modelled `@range(0..100)`** — every other value in that span is 400
//!    `ValidationException`, and a value outside the span is refused by the modelled
//!    range itself. Rows span both published bounds, one step past each of them, whole
//!    and fractional non-multiples, and several accepted multiples in between.
//! 2. **A created router round-trips**: every member `GetPromptRouter` reports comes back
//!    out of the stored record, for configurations differing in model count, description,
//!    tags and routing criteria.
//! 3. **A name is unique per account per region**, so the second create of a name in one
//!    region conflicts while the same name in another region is minted.
//! 4. **A replayed `clientRequestToken` mints nothing new**, and two distinct tokens mint
//!    two routers.
//! 5. **`models` needs at least one entry**, which the model documents but declares no
//!    `@length` for.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreatePromptRouter,GetPromptRouter")` /
//! `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, error_code, label, Emulator, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// Foundation-model ARNs `PromptRouterTargetModel$modelArn` admits
/// (`smithy_query(query="shape",shape="PromptRouterTargetModel")`: the member targets
/// `PromptRouterTargetModelArn`, whose pattern admits
/// `arn:aws:bedrock:<region>::foundation-model/<vendor>.<model>[:<n>]`).
const MODEL_ARNS: &[&str] = &[
    "arn:aws:bedrock:us-west-2::foundation-model/anthropic.claude-3-haiku-20240307-v1:0",
    "arn:aws:bedrock:us-west-2::foundation-model/anthropic.claude-3-sonnet-20240229-v1:0",
    "arn:aws:bedrock:us-west-2::foundation-model/meta.llama3-1-8b-instruct-v1:0",
    "arn:aws:bedrock:us-west-2::foundation-model/amazon.nova-lite-v1:0",
];

/// The step `responseQualityDifference` is accepted at, as the handler states it.
const STEP: f64 = 5.0;

/// The refusal a non-multiple gets, built from that rule.
const NOT_A_MULTIPLE: &str = "Response quality difference must be a value multiple of 5";

/// The ARN prefix every router minted for [`REGION`] carries
/// (`PromptRouterArn`'s pattern: `…:bedrock:<region>:<account>:default-prompt-router/…`).
fn arn_prefix(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:default-prompt-router/")
}

/// A create body with only the required members: one model, the fallback model and a
/// routing criteria the rule accepts.
fn create_body(name: &str, quality_difference: f64) -> Value {
    json!({
        "promptRouterName": name,
        "models": [{ "modelArn": MODEL_ARNS[0] }],
        "fallbackModel": { "modelArn": MODEL_ARNS[1] },
        "routingCriteria": { "responseQualityDifference": quality_difference },
    })
}

async fn create(emu: &Emulator, body: Value) -> crate::support::Reply {
    emu.call("POST", "/prompt-routers", Some(body)).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> crate::support::Reply {
    emu.call_in_region(region, "POST", "/prompt-routers", Some(body))
        .await
}

async fn get(emu: &Emulator, arn: &str) -> crate::support::Reply {
    emu.call("GET", &format!("/prompt-routers/{}", label(arn)), None)
        .await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> crate::support::Reply {
    let path = if query.is_empty() {
        "/prompt-routers".to_string()
    } else {
        format!("/prompt-routers?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

/// The ARNs a listing reported, in the order it reported them.
fn listed_arns(reply: &crate::support::Reply) -> Vec<String> {
    reply
        .get("promptRouterSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!("promptRouterSummaries must be a present array, got {:?}", reply.body)
        })
        .iter()
        .map(|row| {
            row.get("promptRouterArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// 1. The granularity rule, over the whole modelled range.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn response_quality_difference_is_accepted_only_at_multiples_of_the_step() {
    // (value, accepted) — the modelled `@range(min: 0, max: 100)`
    // (`smithy_query(query="shape",shape="RoutingCriteria")`) spans these rows: both
    // published bounds, whole and fractional non-multiples inside the span, and the
    // multiple the issue's recorded request did NOT carry.
    let rows: &[(f64, bool)] = &[
        (0.0, true),
        (5.0, true),
        (35.0, true),
        (95.0, true),
        (100.0, true),
        (1.0, false),
        (2.5, false),
        (7.0, false),
        (12.5, false),
        (42.0, false),
        (99.0, false),
        (99.9, false),
    ];

    let emu = emulator();
    for (index, (value, accepted)) in rows.iter().enumerate() {
        let case = format!("responseQualityDifference={value}");
        let name = format!("granularity {index}");
        let reply = create(&emu, create_body(&name, *value)).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a multiple of {STEP} must be accepted, got {} {:?}",
                reply.status, reply.body
            );
            let arn = reply.text("promptRouterArn");
            assert!(
                arn.starts_with(&arn_prefix(REGION)),
                "case {case:?}: expected an ARN under {:?}, got {arn:?}",
                arn_prefix(REGION)
            );
            // The accepted value is what a read reports, not a value snapped to a step.
            let read = get(&emu, &arn).await;
            assert_eq!(
                read.get("routingCriteria"),
                json!({ "responseQualityDifference": value }),
                "case {case:?}: GetPromptRouter must report the stored criteria, got {:?}",
                read.get("routingCriteria")
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a value that is not a multiple of {STEP} must be refused, \
                 got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?}",
                reply.error
            );
            assert_eq!(
                reply.text("message"),
                NOT_A_MULTIPLE,
                "case {case:?}: expected {NOT_A_MULTIPLE:?}, got {:?}",
                reply.text("message")
            );
        }
    }
}

#[tokio::test]
async fn response_quality_difference_outside_the_modelled_range_is_refused() {
    // One step past each published bound of `@range(min: 0, max: 100)`. Both are
    // multiples of the step, so only the modelled range can refuse them — and it must,
    // ahead of the granularity rule.
    let rows: &[f64] = &[-5.0, 105.0];

    let emu = emulator();
    for (index, value) in rows.iter().enumerate() {
        let case = format!("responseQualityDifference={value}");
        let reply = create(&emu, create_body(&format!("range {index}"), *value)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value outside 0..100 must be refused, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            reply.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. What a create stores is what a read reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_router_round_trips_through_get() {
    // Rows differ along what the create carries: how many target models, whether a
    // description and tags are present, and which accepted routing criteria.
    struct Row {
        name: &'static str,
        models: Vec<&'static str>,
        fallback: &'static str,
        description: Option<&'static str>,
        tags: Vec<(&'static str, &'static str)>,
        quality_difference: f64,
    }

    let rows = vec![
        Row {
            name: "minimal router",
            models: vec![MODEL_ARNS[0]],
            fallback: MODEL_ARNS[0],
            description: None,
            tags: Vec::new(),
            quality_difference: 0.0,
        },
        Row {
            name: "two model router",
            models: vec![MODEL_ARNS[1], MODEL_ARNS[2]],
            fallback: MODEL_ARNS[0],
            description: Some("routes between two models"),
            tags: vec![("team", "search")],
            quality_difference: 10.0,
        },
        Row {
            name: "four_model-router 3",
            models: vec![MODEL_ARNS[0], MODEL_ARNS[1], MODEL_ARNS[2], MODEL_ARNS[3]],
            fallback: MODEL_ARNS[3],
            description: Some("routes.between four models"),
            tags: vec![("team", "search"), ("env", "test")],
            quality_difference: 100.0,
        },
    ];

    let emu = emulator();
    for row in &rows {
        let case = row.name;
        let mut body = json!({
            "promptRouterName": row.name,
            "models": row.models
                .iter()
                .map(|arn| json!({ "modelArn": arn }))
                .collect::<Vec<Value>>(),
            "fallbackModel": { "modelArn": row.fallback },
            "routingCriteria": { "responseQualityDifference": row.quality_difference },
        });
        if let Some(description) = row.description {
            body["description"] = json!(description);
        }
        if !row.tags.is_empty() {
            body["tags"] = Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }

        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePromptRouter must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = created.text("promptRouterArn");
        let id = arn
            .strip_prefix(&arn_prefix(REGION))
            .unwrap_or_else(|| panic!("case {case:?}: ARN {arn:?} is not under this region"));
        assert!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':' || c == '.'),
            "case {case:?}: the ARN resource id must be one PromptRouterArn admits, got {id:?}"
        );

        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetPromptRouter must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("promptRouterArn"),
            arn,
            "case {case:?}: the read must report the ARN the create minted"
        );
        assert_eq!(
            read.text("promptRouterName"),
            row.name,
            "case {case:?}: the read must report the stored name"
        );
        assert_eq!(
            read.get("models"),
            Value::Array(
                row.models
                    .iter()
                    .map(|arn| json!({ "modelArn": arn }))
                    .collect()
            ),
            "case {case:?}: the read must report the stored models in request order, got {:?}",
            read.get("models")
        );
        assert_eq!(
            read.get("fallbackModel"),
            json!({ "modelArn": row.fallback }),
            "case {case:?}: the read must report the stored fallback model, got {:?}",
            read.get("fallbackModel")
        );
        assert_eq!(
            read.get("routingCriteria"),
            json!({ "responseQualityDifference": row.quality_difference }),
            "case {case:?}: the read must report the stored criteria, got {:?}",
            read.get("routingCriteria")
        );
        assert_eq!(
            read.get("description"),
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: the read must report the stored description, got {:?}",
            read.get("description")
        );
        // `PromptRouterStatus` carries exactly one value and `PromptRouterType` two, of
        // which `default` names the routers Amazon Bedrock itself publishes
        // (`smithy_query(query="shape",shape="PromptRouterStatus,PromptRouterType")`).
        assert_eq!(
            read.text("status"),
            "AVAILABLE",
            "case {case:?}: a minted router is the one PromptRouterStatus value"
        );
        assert_eq!(
            read.text("type"),
            "custom",
            "case {case:?}: a router a caller created is PromptRouterType custom"
        );
        // Both instants are modelled `@timestampFormat("date-time")`, so each must be an
        // RFC 3339 string, and a router nothing has changed reports them equal.
        let created_at = read.instant("createdAt");
        let updated_at = read.instant("updatedAt");
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {case:?}: an unchanged router reports updatedAt == createdAt, \
             got {created_at} and {updated_at}"
        );
    }
}

/// The two instants `GetPromptRouterResponse` carries target `Timestamp`, which the model
/// marks `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="Timestamp")`), so each member schema must report
/// that format — it is what makes the codec write the RFC 3339 string the round-trip test
/// reads rather than the protocol's default epoch number.
#[test]
fn the_date_time_members_of_a_router_read_carry_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::GetPromptRouterOutput;

    let rows = [
        ("GetPromptRouterResponse$createdAt", &GetPromptRouterOutput::M_CREATED_AT),
        ("GetPromptRouterResponse$updatedAt", &GetPromptRouterOutput::M_UPDATED_AT),
    ];
    for (case, schema) in rows {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), \
             got {format:?}"
        );
    }

    // The control row: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let status = GetPromptRouterOutput::M_STATUS
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        status.is_none(),
        "case GetPromptRouterResponse$status: the model gives this member no \
         @timestampFormat, got {status:?}"
    );
}

#[tokio::test]
async fn reading_a_router_twice_reports_the_same_record() {
    // A read must not change what a later caller sees: GetPromptRouter is `@readonly`
    // (`smithy_query(query="operation",operation="GetPromptRouter")`).
    let emu = emulator();
    let arn = create(&emu, create_body("stable router", 20.0))
        .await
        .text("promptRouterArn");

    let first = get(&emu, &arn).await;
    let second = get(&emu, &arn).await;
    assert_eq!(
        first.body, second.body,
        "case {arn:?}: two reads of one router must report the same record, got {:?} then {:?}",
        first.body, second.body
    );
}

// ---------------------------------------------------------------------------------------
// 3. The name index, and 4. idempotency.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_name_is_unique_per_region() {
    // `promptRouterName` is documented as unique "within your Amazon Web Services account
    // in the current region"
    // (`smithy_query(query="operation",operation="CreatePromptRouter")` + the member doc),
    // so the region a request addresses is part of the key.
    let emu = emulator();
    let other_region = "eu-central-1";

    for name in ["shared name", "shared_name-2"] {
        let first = create_in_region(&emu, REGION, create_body(name, 5.0)).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first create must be admitted, got {} {:?}",
            first.status, first.body
        );

        let again = create_in_region(&emu, REGION, create_body(name, 5.0)).await;
        assert_eq!(
            again.status, 400,
            "case {name:?}: a duplicate name in one region must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error.as_deref(),
            Some("ConflictException"),
            "case {name:?}: expected ConflictException, got {:?}",
            again.error
        );

        let elsewhere = create_in_region(&emu, other_region, create_body(name, 5.0)).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: the same name in another region names another router, got {} {:?}",
            elsewhere.status, elsewhere.body
        );
        let elsewhere_arn = elsewhere.text("promptRouterArn");
        assert!(
            elsewhere_arn.starts_with(&arn_prefix(other_region)),
            "case {name:?}: expected an ARN under {:?}, got {elsewhere_arn:?}",
            arn_prefix(other_region)
        );
    }
}

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing_new() {
    // `clientRequestToken` is `@idempotencyToken`, so the replay answers with the router
    // the first request minted and a fresh token mints another.
    let emu = emulator();
    let rows = ["Token1", "token-2-abc"];
    let mut minted = Vec::new();

    for (index, token) in rows.iter().enumerate() {
        let mut body = create_body(&format!("idempotent {index}"), 15.0);
        body["clientRequestToken"] = json!(token);

        let first = create(&emu, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: the first create must be admitted, got {} {:?}",
            first.status, first.body
        );
        let arn = first.text("promptRouterArn");

        // The same token, the same body: the SAME router, and no ConflictException on the
        // name it already took.
        let replay = create(&emu, body).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: a replay must be admitted, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("promptRouterArn"),
            arn,
            "case {token:?}: a replay must answer with the router the token minted"
        );
        minted.push(arn);
    }

    assert_ne!(
        minted[0], minted[1],
        "two distinct tokens must mint two routers, got {minted:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 5. `models`, and the reader's refusals.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn models_must_carry_at_least_one_entry() {
    // `models` is documented "At least one model must be specified" while
    // `PromptRouterTargetModels` declares no `@length`
    // (`smithy_query(query="shape",shape="PromptRouterTargetModels")`), so the empty list
    // is refused by the service rather than at the boundary.
    let emu = emulator();
    let mut body = create_body("no models", 5.0);
    body["models"] = json!([]);

    let reply = create(&emu, body).await;
    assert_eq!(
        reply.status, 400,
        "case \"models=[]\": an empty model list must be refused, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("ValidationException"),
        "case \"models=[]\": expected ValidationException, got {:?}",
        reply.error
    );
}

#[tokio::test]
async fn getting_a_router_that_does_not_exist_is_not_found() {
    let emu = emulator();
    let created = create(&emu, create_body("findable router", 25.0)).await;
    let arn = created.text("promptRouterArn");
    let id = arn
        .strip_prefix(&arn_prefix(REGION))
        .expect("the minted ARN is under this region")
        .to_string();

    // An id nothing minted, and the SAME id in another region: the ARN a router is keyed
    // by carries the region the create addressed.
    let rows = [
        format!("{}00000000000z", arn_prefix(REGION)),
        format!("{}{id}", arn_prefix("eu-central-1")),
    ];

    for arn in rows {
        let reply = get(&emu, &arn).await;
        assert_eq!(
            reply.status, 404,
            "case {arn:?}: an unknown router must be 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {arn:?}: expected ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}

#[tokio::test]
async fn a_router_carrying_more_tags_than_a_resource_admits_is_refused() {
    // `TooManyTagsException` documents the bound itself: "The request contains more tags
    // than can be associated with a resource (50 tags per resource)"
    // (`smithy_query(query="operation",operation="CreatePromptRouter")`).
    let emu = emulator();
    // (tag count, admitted)
    let rows: &[(usize, bool)] = &[(0, true), (1, true), (50, true), (51, false)];

    for (index, (count, admitted)) in rows.iter().enumerate() {
        let case = format!("{count} tags");
        let mut body = create_body(&format!("tagged {index}"), 5.0);
        body["tags"] = Value::Array(
            (0..*count)
                .map(|n| json!({ "key": format!("key{n}"), "value": format!("value{n}") }))
                .collect(),
        );

        let reply = create(&emu, body).await;
        if *admitted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: must be admitted, got {} {:?}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: more tags than a resource admits must be refused, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref().map(error_code),
                Some("TooManyTagsException"),
                "case {case:?}: expected TooManyTagsException, got {:?}",
                reply.error
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 6. The listing: scoped by region and by `type`, paginated in ARN order.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_routers_of_the_region_and_type_asked_for() {
    // `type` is modelled `@default("default")`
    // (`smithy_query(query="operation",operation="ListPromptRouters")`), and a router a
    // caller created is `custom`, so the rows span: the type the created routers carry,
    // the type the request defaults to, that same default named explicitly, and the
    // listing of another region.
    let emu = emulator();
    let other_region = "eu-central-1";

    let mut here = Vec::new();
    for index in 0..3 {
        let reply = create_in_region(&emu, REGION, create_body(&format!("listed {index}"), 5.0))
            .await;
        here.push(reply.text("promptRouterArn"));
    }
    let elsewhere = create_in_region(&emu, other_region, create_body("listed elsewhere", 5.0))
        .await
        .text("promptRouterArn");
    here.sort();

    // (case, region, query, the ARNs the listing must report)
    let rows: Vec<(&str, &str, &str, Vec<String>)> = vec![
        ("type=custom", REGION, "type=custom", here.clone()),
        ("no type, so the default", REGION, "", Vec::new()),
        ("type=default", REGION, "type=default", Vec::new()),
        (
            "another region, type=custom",
            other_region,
            "type=custom",
            vec![elsewhere.clone()],
        ),
    ];

    for (case, region, query, expected) in rows {
        let reply = list_in_region(&emu, region, query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListPromptRouters must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            listed_arns(&reply),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_arns(&reply)
        );
        assert_eq!(
            reply.get("nextToken"),
            Value::Null,
            "case {case:?}: a listing that fits one page carries no nextToken, got {:?}",
            reply.get("nextToken")
        );
    }

    // A summary carries the same record the read does, so the listing is a projection of
    // the stored router rather than a second copy of it.
    let listed = list_in_region(&emu, REGION, "type=custom").await;
    let summaries = listed.get("promptRouterSummaries");
    let summary = summaries
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .expect("the listing carries the first router");
    let read = get(&emu, &here[0]).await;
    for member in [
        "promptRouterName",
        "promptRouterArn",
        "routingCriteria",
        "models",
        "fallbackModel",
        "status",
        "type",
        "createdAt",
        "updatedAt",
    ] {
        assert_eq!(
            summary.get(member).cloned().unwrap_or(Value::Null),
            read.get(member),
            "case {member:?}: the summary and the read must report the same value"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_through_every_router_exactly_once() {
    // `maxResults` is modelled `@range(min: 1, max: 1000)`, so the rows span its published
    // minimum, a size between, a size at or above the whole listing, and the absent member.
    let emu = emulator();
    let mut all = Vec::new();
    for index in 0..5 {
        all.push(
            create(&emu, create_body(&format!("paged {index}"), 5.0))
                .await
                .text("promptRouterArn"),
        );
    }
    all.sort();

    let rows: &[(&str, Option<i32>)] = &[
        ("maxResults=1", Some(1)),
        ("maxResults=2", Some(2)),
        ("maxResults=5", Some(5)),
        ("maxResults=1000", Some(1000)),
        ("no maxResults", None),
    ];

    for (case, max_results) in rows {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match max_results {
                Some(max) => format!("type=custom&maxResults={max}"),
                None => "type=custom".to_string(),
            };
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={}", label(token)));
            }
            let reply = list_in_region(&emu, REGION, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: ListPromptRouters must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = listed_arns(&reply);
            if let Some(max) = max_results {
                assert!(
                    page.len() <= usize::try_from(*max).unwrap_or(usize::MAX),
                    "case {case:?}: a page must hold at most maxResults rows, got {}",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: the listing must terminate, got {pages} pages"
            );
            token = reply
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: the pages must partition the listing in ARN order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emu = emulator();
    create(&emu, create_body("token owner", 5.0)).await;

    // Rows: a token of another shape entirely, and one that looks like an id but names no
    // ARN — neither is a cursor this listing issued.
    for token in ["not-a-cursor", "000000000000"] {
        let reply = list_in_region(
            &emu,
            REGION,
            &format!("type=custom&nextToken={}", label(token)),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: a foreign nextToken must be refused, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {token:?}: expected ValidationException, got {:?}",
            reply.error
        );
    }
}
