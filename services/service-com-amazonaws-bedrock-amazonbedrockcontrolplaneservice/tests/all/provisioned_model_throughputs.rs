// SPDX-License-Identifier: CC0-1.0

//! The Provisioned Throughput resource end-to-end: `CreateProvisionedModelThroughput`
//! buys it, `GetProvisionedModelThroughput` reads it back,
//! `ListProvisionedModelThroughputs` reports it and
//! `DeleteProvisionedModelThroughput` removes it, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A listing reports the stored Provisioned Throughputs of the signing region**,
//!    ordered by `creationTime` in `sortOrder` (`Ascending` by default), filtered by every
//!    filter the request carries (`statusEquals`, `modelArnEquals`, `nameContains`,
//!    `creationTimeAfter`, `creationTimeBefore`), paged at `maxResults` with a `nextToken`
//!    that partitions the listing, and each summary agreeing with what a read of that one
//!    Provisioned Throughput reports.
//! 2. **A bought Provisioned Throughput round-trips**: every member a read reports comes
//!    out of the stored record, for rows differing in base model, `modelId` spelling (bare
//!    id and model ARN), model units, commitment term and name length (both modelled
//!    bounds), addressed by either `ProvisionedModelId` spelling (its ARN or its name).
//! 3. **Model units are bought out of the quota the account has been GRANTED for them**,
//!    and which quota applies depends on whether the request carries a
//!    `commitmentDuration`: a no-commitment create over a model AWS offers a no-commitment
//!    purchase for is refused with `ServiceQuotaExceededException` naming the model it
//!    asked for and handing over the documented service-limit-increase support case form
//!    for the region it addressed, and a committed one is admitted up to the granted units
//!    and refused past them.
//! 4. **`modelId` must name a base model the account's catalog offers with Provisioned
//!    Throughput**: a model the catalog does not carry is refused as the spelling it IS —
//!    400 `ValidationException` for the bare model-id spelling, which is no ARN to report
//!    missing, and 404 `ResourceNotFoundException` for an ARN of one of this service's own
//!    model resources — while one it carries but offers on demand only is 400
//!    `ValidationException` about the purchase.
//! 5. **`status` is non-terminal first and settles to `InService`**: `Creating` straight
//!    after a create, `InService` once `PROVISIONED_MODEL_SETTLE_AFTER` has passed — and
//!    the settle is owed by the clock, not by the read.
//! 6. **A delete removes exactly one Provisioned Throughput, once**: the identifier that
//!    named it then names nothing (404 for a read, a delete and a listing alike), so of two
//!    deletes racing for one Provisioned Throughput exactly one wins; and the name it held
//!    is free again.
//! 7. **A name is unique per account per region** while it is live, and a replayed
//!    `clientRequestToken` buys nothing a second time.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateProvisionedModelThroughput,GetProvisionedModelThroughput,ListProvisionedModelThroughputs,DeleteProvisionedModelThroughput")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model and the published catalog admit for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// Base models this account's catalog offers WITH Provisioned Throughput: rows of
/// `crate::support`'s service whose `inferenceTypesSupported` carries `PROVISIONED`
/// (`service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS`,
/// which `ListFoundationModels` reports).
const PROVISIONABLE_MODELS: &[&str] = &[
    "amazon.titan-text-lite-v1",
    "anthropic.claude-3-haiku-20240307-v1:0",
    "cohere.command-text-v14",
    "meta.llama3-70b-instruct-v1:0",
];

/// Base models the same catalog offers with Provisioned Throughput and NO commitment term:
/// the ids
/// `service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::NO_COMMITMENT_PROVISIONED_MODELS`
/// carries, which is where a no-commitment purchase reaches the account's no-commitment
/// model-unit quota at all. Over a model offered only UNDER a term there is no such
/// purchase to make, which is its own refusal (asserted in
/// `tests/all/foundation_model_catalogue_resolution.rs`).
const NO_COMMITMENT_MODELS: &[&str] = &[
    "amazon.titan-text-lite-v1",
    "cohere.command-text-v14",
    "cohere.command-light-text-v14",
    "amazon.titan-image-generator-v1",
];

/// Base models the same catalog offers ON DEMAND ONLY, so no Provisioned Throughput can be
/// bought over them.
const ON_DEMAND_ONLY_MODELS: &[&str] = &[
    "amazon.nova-canvas-v1:0",
    "amazon.titan-text-premier-v1:0",
    "ai21.jamba-1-5-mini-v1:0",
    "meta.llama3-1-8b-instruct-v1:0",
];

/// `CommitmentDuration` is `[OneMonth, SixMonths]`
/// (`smithy_query(query="shape",shape="CommitmentDuration")`).
const COMMITMENTS: &[&str] = &["OneMonth", "SixMonths"];

/// The two fixed sentences a `modelId` that resolves to no model is refused with, written
/// out here rather than read off the crate so this suite pins the wording a client reads.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";
const CUSTOM_MODEL_NOT_FOUND: &str = "Custom model not found.";

/// `ProvisionedModelStatus` is `[Creating, InService, Updating, Failed]`
/// (`smithy_query(query="shape",shape="ProvisionedModelStatus")`).
const CREATING: &str = "Creating";
const IN_SERVICE: &str = "InService";
const UPDATING: &str = "Updating";
const FAILED: &str = "Failed";

/// The model units this account is granted for a committed Provisioned Throughput, as the
/// service states it.
const GRANTED_COMMITTED_UNITS: i32 =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_COMMITTED_MODEL_UNITS;

/// How long after a create the `Creating` status is owed its move to `InService`, as the
/// service states it.
const SETTLE_AFTER: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::PROVISIONED_MODEL_SETTLE_AFTER;

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// A create body: `modelUnits`, `provisionedModelName` and `modelId` are required, and
/// `commitmentDuration` is carried only when the row buys a commitment.
fn create_body(name: &str, model_id: &str, units: i32, commitment: Option<&str>) -> Value {
    let mut body = json!({
        "provisionedModelName": name,
        "modelId": model_id,
        "modelUnits": units,
    });
    if let Some(commitment) = commitment {
        body["commitmentDuration"] = json!(commitment);
    }
    body
}

/// A create body for a Provisioned Throughput this account can actually buy: one model
/// unit, with a commitment (the quota a no-commitment create draws on is not granted).
fn committed_body(name: &str, model_id: &str) -> Value {
    create_body(name, model_id, 1, Some(COMMITMENTS[0]))
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    create_in_region(emu, REGION, body).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/provisioned-model-throughput", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/provisioned-model-throughput/{}", label(identifier)),
        None,
    )
    .await
}

async fn delete(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/provisioned-model-throughput/{}", label(identifier)),
        None,
    )
    .await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/provisioned-model-throughputs".to_string()
    } else {
        format!("/provisioned-model-throughputs?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// Buy one Provisioned Throughput and hand back its ARN, failing the case by name when the
/// create is refused.
async fn buy(emu: &Emulator, case: &str, body: Value) -> String {
    let reply = create(emu, body).await;
    assert_eq!(
        reply.status, 201,
        "case {case}: CreateProvisionedModelThroughput must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("provisionedModelArn")
}

// ---------------------------------------------------------------------------------------
// Response helpers.
// ---------------------------------------------------------------------------------------

/// The summaries a listing reported, in the order it reported them.
fn listed(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("provisionedModelSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: provisionedModelSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The ARNs a listing reported, in order.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    listed(case, reply)
        .iter()
        .map(|row| text(case, row, "provisionedModelArn"))
        .collect()
}

fn member(case: &str, value: &Value, name: &str) -> Value {
    value
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the response has no member {name}: {value:?}"))
}

fn text(case: &str, value: &Value, name: &str) -> String {
    let member = member(case, value, name);
    member
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: member {name} must be a string, got {member:?}"))
        .to_string()
}

fn number(case: &str, value: &Value, name: &str) -> i64 {
    let member = member(case, value, name);
    member
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: member {name} must be an integer, got {member:?}"))
}

/// A `Timestamp` member in epoch seconds. Every one of them is modelled
/// `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING.
fn instant(case: &str, value: &Value, name: &str) -> f64 {
    let member = member(case, value, name);
    let raw = member.as_str().unwrap_or_else(|| {
        panic!(
            "case {case}: member {name} is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string, got {member}"
        )
    });
    date_time_seconds(case, raw)
}

/// `seconds` since the epoch as the RFC 3339 date-time the two `creationTime` filters
/// carry on the wire, read with the same codec the wire uses.
fn date_time_query(seconds: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant the date-time format can render")
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// The ARN of the base model `model_id` in `region`, as `crate::support` spells it: one
// spelling across the suites that compare a response's ARN against it.
use crate::support::foundation_model_arn as model_arn;

/// Whether `arn` is a `ProvisionedModelArn`:
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$`.
fn is_provisioned_model_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:provisioned-model/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    id.len() == 12
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

// ---------------------------------------------------------------------------------------
// 1. A bought Provisioned Throughput round-trips through GetProvisionedModelThroughput.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_bought_provisioned_throughput_round_trips() {
    let emu = emulator();

    // `provisionedModelName` is `@length(min: 1, max: 63)`, so the rows include both
    // bounds; `modelId` admits the bare model id and the model's ARN, so both spellings
    // appear; `modelUnits` is `range 1..`, bounded here by what the account is granted.
    let max_length_name = "a".repeat(63);
    let rows: Vec<(&str, &str, &str, i32, Option<&str>)> = vec![
        (
            "one unit, a one-month commitment, a bare model id",
            "pt-minimal",
            PROVISIONABLE_MODELS[0],
            1,
            Some(COMMITMENTS[0]),
        ),
        (
            "several units and a six-month commitment",
            "pt_several_units",
            PROVISIONABLE_MODELS[1],
            4,
            Some(COMMITMENTS[1]),
        ),
        (
            "every granted unit",
            "pt-granted-units",
            PROVISIONABLE_MODELS[2],
            GRANTED_COMMITTED_UNITS,
            Some(COMMITMENTS[0]),
        ),
        (
            "a shortest-possible name",
            "p",
            PROVISIONABLE_MODELS[3],
            2,
            Some(COMMITMENTS[1]),
        ),
        (
            "a longest-possible name",
            max_length_name.as_str(),
            PROVISIONABLE_MODELS[0],
            3,
            Some(COMMITMENTS[0]),
        ),
    ];

    for (case, name, model_id, units, commitment) in rows {
        // Half the rows name the model by its ARN rather than by its bare id: both are
        // `ModelIdentifier` spellings of the same base model.
        let spelled = if units % 2 == 0 {
            model_arn(REGION, model_id)
        } else {
            model_id.to_string()
        };
        let arn = buy(
            &emu,
            case,
            create_body(name, &spelled, units, commitment),
        )
        .await;
        assert!(
            is_provisioned_model_arn(&arn, REGION),
            "case {case}: {arn:?} is not a ProvisionedModelArn of {REGION}"
        );

        // Both `ProvisionedModelId` spellings address the same Provisioned Throughput, and
        // report the same record.
        let by_arn = get(&emu, &arn).await;
        assert_eq!(
            by_arn.status, 200,
            "case {case}: a read of {arn} must answer 200, got {} {:?}",
            by_arn.status, by_arn.body
        );
        let by_name = get(&emu, name).await;
        assert_eq!(
            by_name.body, by_arn.body,
            "case {case}: a read by name must report the same record as a read by ARN, got \
             {:?} and {:?}",
            by_name.body, by_arn.body
        );

        let read = &by_arn.body;
        assert_eq!(
            text(case, read, "provisionedModelName"),
            name,
            "case {case}: the read must report the name the create carried"
        );
        assert_eq!(
            text(case, read, "provisionedModelArn"),
            arn,
            "case {case}: the read must report the ARN the create minted"
        );
        for reported in ["modelArn", "desiredModelArn", "foundationModelArn"] {
            assert_eq!(
                text(case, read, reported),
                model_arn(REGION, model_id),
                "case {case}: {reported} must be the ARN of the base model the create named"
            );
        }
        for reported in ["modelUnits", "desiredModelUnits"] {
            assert_eq!(
                number(case, read, reported),
                i64::from(units),
                "case {case}: {reported} must be the model units the create bought"
            );
        }
        assert_eq!(
            text(case, read, "status"),
            CREATING,
            "case {case}: a Provisioned Throughput is still being brought up right after the \
             create"
        );
        assert_eq!(
            member(case, read, "commitmentDuration"),
            json!(commitment),
            "case {case}: commitmentDuration must be the term the create bought"
        );
        // The commitment expires one term after the Provisioned Throughput was created:
        // `OneMonth` is one month and `SixMonths` is six of them.
        let created = instant(case, read, "creationTime");
        let months = if commitment == Some(COMMITMENTS[1]) { 6.0 } else { 1.0 };
        let month_seconds = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::COMMITMENT_MONTH
            .as_secs_f64();
        let expires = instant(case, read, "commitmentExpirationTime");
        assert!(
            (expires - (created + months * month_seconds)).abs() < 1.0,
            "case {case}: commitmentExpirationTime must be {months} month(s) after \
             creationTime, got {expires} against {created}"
        );
        assert_eq!(
            instant(case, read, "lastModifiedTime"),
            created,
            "case {case}: nothing has changed the Provisioned Throughput yet, so \
             lastModifiedTime is its creation instant"
        );
        assert_eq!(
            read.get("failureMessage"),
            None,
            "case {case}: an admitted Provisioned Throughput has no failure to report"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. Model units come out of the quota the account has been granted for them.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn model_units_are_bought_out_of_the_granted_quota() {
    let emu = emulator();

    // A create with NO commitment draws on the no-commitment model-unit quota, which an
    // account holds none of until an increase is granted — so every unit count it can ask
    // for (`modelUnits` is `range 1..`) is refused, and the refusal names the model the
    // request asked for.
    let refused: Vec<(&str, &str, i32)> = vec![
        ("one unit, no commitment", NO_COMMITMENT_MODELS[0], 1),
        ("two units, no commitment", NO_COMMITMENT_MODELS[1], 2),
        (
            "every granted committed unit, but no commitment",
            NO_COMMITMENT_MODELS[2],
            GRANTED_COMMITTED_UNITS,
        ),
        (
            "more units than any quota grants, no commitment",
            NO_COMMITMENT_MODELS[3],
            GRANTED_COMMITTED_UNITS + 1,
        ),
    ];
    for (case, model_id, units) in refused {
        let reply = create(&emu, create_body("pt-no-commitment", model_id, units, None)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the create must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: the create must answer ServiceQuotaExceededException, got {:?}",
            reply.error
        );
        let expected = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::no_commitment_model_units_message(
            model_id, REGION,
        );
        let stated = refusal_message(&reply);
        assert_eq!(
            stated, expected,
            "case {case}: the refusal must name the model units the account lacks for \
             {model_id}, got {stated:?}"
        );
    }

    // And a refused create bought nothing: neither the name nor any listing knows it.
    let read = get(&emu, "pt-no-commitment").await;
    assert_eq!(
        read.status, 404,
        "a refused create must store nothing, got {} {:?}",
        read.status, read.body
    );
    assert!(
        listed_arns("after the refused creates", &list(&emu, "").await).is_empty(),
        "a refused create must leave the listing empty"
    );

    // A create WITH a commitment draws on the commitment quota instead, which is granted
    // up to `GRANTED_COMMITTED_UNITS`: the bound itself is admitted and one past it is not.
    let committed: Vec<(&str, i32, bool)> = vec![
        ("one committed unit", 1, true),
        ("every granted committed unit", GRANTED_COMMITTED_UNITS, true),
        (
            "one unit past the granted units",
            GRANTED_COMMITTED_UNITS + 1,
            false,
        ),
        (
            "far past the granted units",
            GRANTED_COMMITTED_UNITS * 10,
            false,
        ),
    ];
    for (case, units, admitted) in committed {
        let name = format!("pt-committed-{units}");
        let reply = create(
            &emu,
            create_body(&name, PROVISIONABLE_MODELS[0], units, Some(COMMITMENTS[0])),
        )
        .await;
        if admitted {
            assert_eq!(
                reply.status, 201,
                "case {case}: the create must answer 201, got {} {:?}",
                reply.status, reply.body
            );
            let read = get(&emu, &name).await;
            assert_eq!(
                number(case, &read.body, "modelUnits"),
                i64::from(units),
                "case {case}: the stored Provisioned Throughput must hold the units it bought"
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case}: the create must answer 400, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ServiceQuotaExceededException"),
                "case {case}: the create must answer ServiceQuotaExceededException, got {:?}",
                reply.error
            );
            assert_eq!(
                get(&emu, &name).await.status,
                404,
                "case {case}: a refused create must store nothing"
            );
        }
    }
}

/// The support-center case form the model's own `modelUnits` documentation hands a caller
/// who has no model units granted, written out HERE rather than taken from the service so
/// the wording and the link form are pinned by the test and not by the code under test.
///
/// Source: `smithy_query(query="doc", shape="CreateProvisionedModelThroughputRequest",
/// member="modelUnits")` — "By default, your account has no model units for purchasing
/// Provisioned Throughputs with commitment. You must first visit the
/// <a href=\"https://console.aws.amazon.com/support/home#/case/create?issueType=service-limit-increase\">Amazon
/// Web Services support center</a> to request MUs." The `issueType=service-limit-increase`
/// selector is what opens the quota-increase form, and the region the request addressed is
/// carried so the form opens against it.
fn expected_no_commitment_refusal(base_model_id: &str, region: &str) -> String {
    format!(
        "Your account does not currently have any no commitment model units reserved for \
         {base_model_id}. Please see \
         https://support.console.aws.amazon.com/support/home?region={region}\
         #/case/create?issueType=service-limit-increase to request a service quota increase."
    )
}

/// A no-commitment create is refused for want of granted model units, and the refusal
/// hands the caller the documented service-limit-increase case form for the region it
/// addressed, naming the BASE model the request asked for whichever `ModelIdentifier`
/// spelling named it.
///
/// Rows differ along everything the rule generalises over — base model (including models
/// the recorded case never named), `modelId` spelling (bare id and model ARN), the region
/// the request signs for, and the unit count asked for (`modelUnits` is `range 1..`) — so
/// the only constant across them is the remedy clause itself.
#[tokio::test]
async fn the_no_commitment_quota_refusal_hands_over_the_service_limit_increase_case_form() {
    let emu = emulator();

    let rows: Vec<(&str, String, &str, &str, i32)> = vec![
        (
            "a bare model id in the suite's region, one unit",
            NO_COMMITMENT_MODELS[0].to_string(),
            NO_COMMITMENT_MODELS[0],
            REGION,
            1,
        ),
        (
            "the model ARN spelling, a second region, several units",
            model_arn("us-east-1", NO_COMMITMENT_MODELS[1]),
            NO_COMMITMENT_MODELS[1],
            "us-east-1",
            4,
        ),
        (
            "a third base model in a third region, every granted committed unit",
            NO_COMMITMENT_MODELS[2].to_string(),
            NO_COMMITMENT_MODELS[2],
            "eu-central-1",
            GRANTED_COMMITTED_UNITS,
        ),
        (
            "a fourth base model, ARN spelling, past any granted units",
            model_arn("ap-northeast-1", NO_COMMITMENT_MODELS[3]),
            NO_COMMITMENT_MODELS[3],
            "ap-northeast-1",
            GRANTED_COMMITTED_UNITS + 1,
        ),
    ];

    for (index, (case, model_id, base_model_id, region, units)) in rows.into_iter().enumerate() {
        let name = format!("pt-remedy-{index}");
        let reply =
            create_in_region(&emu, region, create_body(&name, &model_id, units, None)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the refused create must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: the refused create must answer ServiceQuotaExceededException, got {:?}",
            reply.error
        );
        let stated = refusal_message(&reply);
        let expected = expected_no_commitment_refusal(base_model_id, region);
        assert_eq!(
            stated, expected,
            "case {case}: the refusal must name the base model and hand over the \
             service-limit-increase case form for {region}, got {stated:?}"
        );
        assert!(
            stated.contains("?issueType=service-limit-increase"),
            "case {case}: the remedy link must carry the documented issueType selector, \
             got {stated:?}"
        );
        assert_eq!(
            emu.call_in_region(
                region,
                "GET",
                &format!("/provisioned-model-throughput/{}", label(&name)),
                None
            )
            .await
            .status,
            404,
            "case {case}: a refused create must store nothing in {region}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. modelId must name a base model the catalog offers with Provisioned Throughput.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_model_the_catalog_does_not_offer_for_provisioning_is_refused() {
    let emu = emulator();

    // A model the published catalog does not carry is a model this service cannot resolve,
    // and the `ModelIdentifier` spelling that named it decides HOW that is reported: an ARN
    // of one of this service's own model resources names a resource to report missing, while
    // the bare model-id spelling is no ARN at all and is un-usable input.
    let unknown: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "a bare id no row carries",
            "amazon.titan-not-a-model".to_string(),
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER.to_string(),
        ),
        (
            "the ARN of a model no row carries",
            model_arn(REGION, "amazon.titan-not-a-model"),
            404,
            "ResourceNotFoundException",
            format!(
                "Could not resolve the model from the provided model identifier: {}",
                model_arn(REGION, "amazon.titan-not-a-model")
            ),
        ),
        (
            "a live model id with a suffix",
            format!("{}-2", PROVISIONABLE_MODELS[0]),
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER.to_string(),
        ),
        (
            "a custom model ARN this account holds no model for",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/amazon.titan-text-lite-v1/abcdefghijkl"),
            404,
            "ResourceNotFoundException",
            CUSTOM_MODEL_NOT_FOUND.to_string(),
        ),
    ];
    for (case, model_id, status, code, sentence) in unknown {
        let reply = create(&emu, committed_body("pt-unknown-model", &model_id)).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (status, Some(code)),
            "case {case}: the create must answer {status} {code}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            refusal_message(&reply),
            sentence,
            "case {case}: the refusal states the sentence of its class, got {:?}",
            refusal_message(&reply)
        );
    }

    // A model the catalog carries but offers ON DEMAND ONLY cannot back a Provisioned
    // Throughput at all.
    for model_id in ON_DEMAND_ONLY_MODELS {
        let reply = create(&emu, committed_body("pt-on-demand-only", model_id)).await;
        assert_eq!(
            reply.status, 400,
            "case {model_id}: the create must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {model_id}: the create must answer ValidationException, got {:?}",
            reply.error
        );
    }

    assert!(
        listed_arns("after the refused creates", &list(&emu, "").await).is_empty(),
        "a refused create must leave the listing empty"
    );
}

// ---------------------------------------------------------------------------------------
// 4. The status is non-terminal first and settles to InService on the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_status_settles_to_in_service_on_the_clock() {
    let emu = emulator();

    let name = "pt-settling";
    let arn = buy(
        &emu,
        "the settling Provisioned Throughput",
        committed_body(name, PROVISIONABLE_MODELS[0]),
    )
    .await;

    // Straight after the create, both a read and a listing report it creating: the caller
    // observes the transient status the real service reports.
    let read = get(&emu, &arn).await;
    assert_eq!(
        text("straight after the create", &read.body, "status"),
        CREATING,
        "the read must report the creating status"
    );
    let created = instant("straight after the create", &read.body, "creationTime");
    let rows = listed("straight after the create", &list(&emu, "").await);
    assert_eq!(
        text("straight after the create", &rows[0], "status"),
        CREATING,
        "the listing must report the same status the read does"
    );

    // The move is owed by the CLOCK: once the span has passed the next call that touches
    // the record reports it serving, and reports the due instant as its last modification.
    tokio::time::sleep(SETTLE_AFTER + std::time::Duration::from_millis(150)).await;
    let settled = get(&emu, &arn).await;
    assert_eq!(
        text("after the settle span", &settled.body, "status"),
        IN_SERVICE,
        "the read must report the serving status once the span has passed"
    );
    let modified = instant("after the settle span", &settled.body, "lastModifiedTime");
    assert!(
        modified >= created + SETTLE_AFTER.as_secs_f64() - 0.001,
        "the settle must be stamped at the instant it was owed ({}), got {modified} against a \
         creation at {created}",
        SETTLE_AFTER.as_secs_f64()
    );
    assert_eq!(
        instant("after the settle span", &settled.body, "creationTime"),
        created,
        "a settle must leave the creation instant alone"
    );

    // And `statusEquals` filters on the status each record holds NOW, so the serving
    // Provisioned Throughput is the one the serving filter reports.
    let serving = list(&emu, &format!("statusEquals={IN_SERVICE}")).await;
    assert_eq!(
        listed_arns("statusEquals=InService after the settle", &serving),
        vec![arn.clone()],
        "the serving filter must report the settled Provisioned Throughput"
    );
    let creating = list(&emu, &format!("statusEquals={CREATING}")).await;
    assert!(
        listed_arns("statusEquals=Creating after the settle", &creating).is_empty(),
        "nothing is creating once the settle is done, got {:?}",
        creating.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. The listing: order, filters, pages and agreement with a read.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_orders_filters_and_pages_the_stored_provisioned_throughputs() {
    let emu = emulator();

    assert!(
        listed_arns("an empty account", &list(&emu, "").await).is_empty(),
        "an account that has bought nothing lists nothing"
    );

    // Two groups, created one after the other so the creation order is known, differing in
    // the base model they are bought over and in their names.
    let first_group = ["alpha-one", "alpha-two", "alpha-three"];
    let second_group = ["beta-one", "beta-two"];
    let mut arns: Vec<String> = Vec::new();
    for name in first_group {
        arns.push(buy(&emu, name, committed_body(name, PROVISIONABLE_MODELS[0])).await);
    }
    // A clear gap, so a bound between the two groups is unambiguous.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    for name in second_group {
        arns.push(buy(&emu, name, committed_body(name, PROVISIONABLE_MODELS[1])).await);
    }

    // Order: `sortBy` admits only `CreationTime`, so the listing is in creation order,
    // ascending by default and reversed by `sortOrder=Descending`.
    let all = list(&emu, "").await;
    assert_eq!(
        listed_arns("the default listing", &all),
        arns,
        "the default listing must report every stored Provisioned Throughput in creation order"
    );
    assert_eq!(
        listed_arns("sortBy=CreationTime", &list(&emu, "sortBy=CreationTime").await),
        arns,
        "sortBy=CreationTime is the order an absent sortBy already takes"
    );
    assert_eq!(
        listed_arns(
            "sortOrder=Ascending",
            &list(&emu, "sortBy=CreationTime&sortOrder=Ascending").await
        ),
        arns,
        "sortOrder=Ascending is the order an absent sortOrder already takes"
    );
    let mut reversed = arns.clone();
    reversed.reverse();
    assert_eq!(
        listed_arns("sortOrder=Descending", &list(&emu, "sortOrder=Descending").await),
        reversed,
        "sortOrder=Descending must report the same listing newest first"
    );

    // Every summary agrees with what a read of that one Provisioned Throughput reports.
    for row in listed("the default listing", &all) {
        let arn = text("summary agreement", &row, "provisionedModelArn");
        let read = get(&emu, &arn).await;
        for shared in [
            "provisionedModelName",
            "provisionedModelArn",
            "modelArn",
            "desiredModelArn",
            "foundationModelArn",
            "status",
            "commitmentDuration",
            "commitmentExpirationTime",
            "creationTime",
            "lastModifiedTime",
        ] {
            assert_eq!(
                member("summary agreement", &row, shared),
                member("summary agreement", &read.body, shared),
                "case {arn}: the summary's {shared} must be what a read of it reports"
            );
        }
        for shared in ["modelUnits", "desiredModelUnits"] {
            assert_eq!(
                number("summary agreement", &row, shared),
                number("summary agreement", &read.body, shared),
                "case {arn}: the summary's {shared} must be what a read of it reports"
            );
        }
    }

    // `modelArnEquals`: the Provisioned Throughputs associated with that one model.
    let first_model = model_arn(REGION, PROVISIONABLE_MODELS[0]);
    assert_eq!(
        listed_arns(
            "modelArnEquals the first model",
            &list(&emu, &format!("modelArnEquals={}", label(&first_model))).await
        ),
        arns[..first_group.len()].to_vec(),
        "modelArnEquals must report only the Provisioned Throughputs over that model"
    );
    let unused_model = model_arn(REGION, PROVISIONABLE_MODELS[3]);
    assert!(
        listed_arns(
            "modelArnEquals a model nothing was bought over",
            &list(&emu, &format!("modelArnEquals={}", label(&unused_model))).await
        )
        .is_empty(),
        "modelArnEquals a model nothing was bought over must report nothing"
    );

    // `nameContains`: a substring of the name, not the whole name.
    assert_eq!(
        listed_arns("nameContains=alpha", &list(&emu, "nameContains=alpha").await),
        arns[..first_group.len()].to_vec(),
        "nameContains must report the Provisioned Throughputs whose names carry the fragment"
    );
    assert_eq!(
        listed_arns("nameContains=one", &list(&emu, "nameContains=one").await),
        vec![arns[0].clone(), arns[first_group.len()].clone()],
        "nameContains must match a fragment anywhere in the name"
    );
    assert!(
        listed_arns("nameContains=gamma", &list(&emu, "nameContains=gamma").await).is_empty(),
        "nameContains a fragment no name carries must report nothing"
    );

    // `statusEquals`: the statuses this emulator mints, and the ones no call produces.
    assert_eq!(
        listed_arns(
            "statusEquals=Creating",
            &list(&emu, &format!("statusEquals={CREATING}")).await
        ),
        arns,
        "every Provisioned Throughput is still creating this soon after its create"
    );
    for absent in [UPDATING, FAILED] {
        assert!(
            listed_arns(
                "a status no call produces",
                &list(&emu, &format!("statusEquals={absent}")).await
            )
            .is_empty(),
            "no call here leaves a Provisioned Throughput {absent}, so that filter reports none"
        );
    }

    // The two creation-time bounds, which the members word strictly: "created after" and
    // "created before" the instant given.
    let rows = listed("the default listing", &all);
    let last_of_first_group = instant("the bound rows", &rows[first_group.len() - 1], "creationTime");
    let first_of_second_group = instant("the bound rows", &rows[first_group.len()], "creationTime");
    let between = (last_of_first_group + first_of_second_group) / 2.0;
    let far_past = date_time_query(0.0);
    let far_future = date_time_query(4_102_444_800.0);
    let bounds: Vec<(&str, String, Vec<String>)> = vec![
        (
            "creationTimeAfter the epoch",
            format!("creationTimeAfter={}", label(&far_past)),
            arns.clone(),
        ),
        (
            "creationTimeAfter the far future",
            format!("creationTimeAfter={}", label(&far_future)),
            Vec::new(),
        ),
        (
            "creationTimeBefore the far future",
            format!("creationTimeBefore={}", label(&far_future)),
            arns.clone(),
        ),
        (
            "creationTimeBefore the epoch",
            format!("creationTimeBefore={}", label(&far_past)),
            Vec::new(),
        ),
        (
            "creationTimeAfter a bound between the two groups",
            format!("creationTimeAfter={}", label(&date_time_query(between))),
            arns[first_group.len()..].to_vec(),
        ),
        (
            "creationTimeBefore a bound between the two groups",
            format!("creationTimeBefore={}", label(&date_time_query(between))),
            arns[..first_group.len()].to_vec(),
        ),
        (
            "both bounds around the second group",
            format!(
                "creationTimeAfter={}&creationTimeBefore={}",
                label(&date_time_query(between)),
                label(&far_future)
            ),
            arns[first_group.len()..].to_vec(),
        ),
    ];
    for (case, query, expected) in bounds {
        assert_eq!(
            listed_arns(case, &list(&emu, &query).await),
            expected,
            "case {case}: the creation-time bounds must select exactly those rows"
        );
    }

    // Filters compose: a name fragment and a model that disagree select nothing.
    assert!(
        listed_arns(
            "nameContains and modelArnEquals that disagree",
            &list(
                &emu,
                &format!("nameContains=alpha&modelArnEquals={}", label(&model_arn(REGION, PROVISIONABLE_MODELS[1])))
            )
            .await
        )
        .is_empty(),
        "every filter a request carries must hold for a row to be listed"
    );

    // Pages: `maxResults` bounds the page and `nextToken` resumes the listing, so the pages
    // partition it with neither a gap nor a repeat.
    for page_size in [1_usize, 2, 3, arns.len(), arns.len() + 1] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={}", label(token)),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&emu, &query).await;
            let case = format!("maxResults={page_size} page {pages}");
            let reported = listed_arns(&case, &page);
            assert!(
                reported.len() <= page_size,
                "case {case}: a page must hold at most {page_size} rows, got {}",
                reported.len()
            );
            walked.extend(reported);
            token = page.get("nextToken").as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= arns.len() + 2,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, arns,
            "case maxResults={page_size}: the pages must partition the listing in order"
        );
    }

    // A token this listing never issued is refused rather than read as some other page.
    for token in [
        "not-a-cursor",
        "0:short",
        "00000000000000000000",
        "x0000000000000000000:arn",
    ] {
        let reply = list(&emu, &format!("nextToken={}", label(token))).await;
        assert_eq!(
            reply.status, 400,
            "case {token}: a foreign nextToken must be refused with 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {token}: a foreign nextToken must be refused with ValidationException, got {:?}",
            reply.error
        );
    }

    // The listing is per region: the same account in another region has bought nothing.
    assert!(
        listed_arns(
            "another region",
            &list_in_region(&emu, "eu-central-1", "").await
        )
        .is_empty(),
        "a Provisioned Throughput of one region is not listed in another"
    );
}

// ---------------------------------------------------------------------------------------
// 6. A delete removes exactly one Provisioned Throughput, once.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_removes_one_provisioned_throughput_and_only_one_delete_wins() {
    let emu = emulator();

    // An identifier that names nothing is refused, whichever spelling it is.
    let kept = "pt-kept";
    let kept_arn = buy(&emu, kept, committed_body(kept, PROVISIONABLE_MODELS[0])).await;
    let unknown: Vec<(&str, String)> = vec![
        ("a name no create ever used", "pt-absent".to_string()),
        ("a prefix of a live name", "pt-kep".to_string()),
        (
            "an ARN of this account that names nothing",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:provisioned-model/aaaaaaaaaaaa"),
        ),
        (
            "an ARN of another account",
            format!("arn:aws:bedrock:{REGION}:123456789012:provisioned-model/aaaaaaaaaaaa"),
        ),
    ];
    for (case, identifier) in unknown {
        for (operation, reply) in [
            ("a read", get(&emu, &identifier).await),
            ("a delete", delete(&emu, &identifier).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case}: {operation} of {identifier} must answer 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: {operation} of {identifier} must answer ResourceNotFoundException, \
                 got {:?}",
                reply.error
            );
        }
    }

    // A `provisioned-model` ARN of ANOTHER region never reaches the handler: this service
    // reads such an ARN itself, ahead of the handler, and words its own recorded sentence
    // for one that is not usable in the region the request addresses
    // (`crate::state::ARN_INVALID_FOR_SERVICE_REGION`, applied in `src/service.rs` to every
    // member carrying one of `crate::state::SELF_VALIDATED_RESOURCE_TYPES`).
    let foreign = kept_arn.replace(REGION, "eu-central-1");
    for (operation, reply) in [
        ("a read", get(&emu, &foreign).await),
        ("a delete", delete(&emu, &foreign).await),
    ] {
        assert_eq!(
            reply.status, 400,
            "case the live ARN in another region: {operation} must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case the live ARN in another region: {operation} must answer ValidationException, \
             got {:?}",
            reply.error
        );
        assert_eq!(
            refusal_message(&reply),
            service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ARN_INVALID_FOR_SERVICE_REGION,
            "case the live ARN in another region: {operation} must state this service's own \
             sentence for an ARN of another region"
        );
    }
    assert_eq!(
        get(&emu, &kept_arn).await.status,
        200,
        "the refusals must leave the live Provisioned Throughput alone"
    );

    // Two deletes racing for one Provisioned Throughput: exactly one removes it and the
    // other finds nothing left, by either spelling of the identifier.
    for (case, identifier) in [("by name", kept.to_string()), ("by ARN", kept_arn.clone())] {
        let name = format!("pt-raced-{}", case.replace(' ', "-"));
        let arn = buy(&emu, &name, committed_body(&name, PROVISIONABLE_MODELS[1])).await;
        let addressed = if identifier == kept_arn {
            arn.clone()
        } else {
            name.clone()
        };
        let (left, right) = tokio::join!(delete(&emu, &addressed), delete(&emu, &addressed));
        let statuses = [left.status, right.status];
        assert_eq!(
            statuses.iter().filter(|status| **status == 200).count(),
            1,
            "case {case}: exactly one of two racing deletes may win, got {statuses:?}"
        );
        assert_eq!(
            statuses.iter().filter(|status| **status == 404).count(),
            1,
            "case {case}: the delete that lost the race must answer 404, got {statuses:?}"
        );
        assert_eq!(
            get(&emu, &arn).await.status,
            404,
            "case {case}: a deleted Provisioned Throughput must be gone"
        );
        assert!(
            !listed_arns("after the race", &list(&emu, "").await).contains(&arn),
            "case {case}: a deleted Provisioned Throughput must leave the listing"
        );
    }

    // The name a deleted Provisioned Throughput held is free again, and the create that
    // takes it mints a NEW ARN.
    let delete_kept = delete(&emu, kept).await;
    assert_eq!(
        delete_kept.status, 200,
        "deleting the live Provisioned Throughput must answer 200, got {} {:?}",
        delete_kept.status, delete_kept.body
    );
    let reused_arn = buy(
        &emu,
        "the reused name",
        committed_body(kept, PROVISIONABLE_MODELS[2]),
    )
    .await;
    assert_ne!(
        reused_arn, kept_arn,
        "a create that reuses a freed name must mint a new ARN"
    );
    assert_eq!(
        listed_arns("after the name was reused", &list(&emu, "").await),
        vec![reused_arn],
        "the listing must report exactly the Provisioned Throughputs that are live"
    );
}

// ---------------------------------------------------------------------------------------
// 7. A live name is unique per region, and a replayed token buys nothing twice.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_live_name_is_unique_per_region_and_a_replayed_token_buys_once() {
    let emu = emulator();

    let name = "pt-unique";
    let arn = buy(&emu, name, committed_body(name, PROVISIONABLE_MODELS[0])).await;

    // A second create of a live name is refused: the name is the other spelling of
    // `ProvisionedModelId`, so two live Provisioned Throughputs could not both answer to it.
    for (case, body) in [
        (
            "the same request again",
            committed_body(name, PROVISIONABLE_MODELS[0]),
        ),
        (
            "the same name over another model",
            committed_body(name, PROVISIONABLE_MODELS[1]),
        ),
        (
            "the same name with other model units",
            create_body(name, PROVISIONABLE_MODELS[0], 2, Some(COMMITMENTS[1])),
        ),
    ] {
        let reply = create(&emu, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: a duplicate name must be refused with 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a duplicate name must be refused with ValidationException, got {:?}",
            reply.error
        );
    }

    // The same name in ANOTHER region is another Provisioned Throughput, with that region's
    // ARN.
    let elsewhere = create_in_region(
        &emu,
        "eu-central-1",
        committed_body(name, PROVISIONABLE_MODELS[0]),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "the same name in another region must be admitted, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere_arn = elsewhere.text("provisionedModelArn");
    assert!(
        is_provisioned_model_arn(&elsewhere_arn, "eu-central-1"),
        "{elsewhere_arn:?} is not a ProvisionedModelArn of eu-central-1"
    );
    assert_eq!(
        listed_arns("the signing region", &list(&emu, "").await),
        vec![arn.clone()],
        "the other region's Provisioned Throughput must not appear in this one's listing"
    );

    // `@idempotencyToken`: a replayed `clientRequestToken` answers with the ARN the first
    // request minted and buys nothing a second time; a fresh one buys another.
    let mut first = committed_body("pt-token", PROVISIONABLE_MODELS[0]);
    first["clientRequestToken"] = json!("token-one");
    let token_arn = buy(&emu, "the first tokened create", first.clone()).await;
    let replay = create(&emu, first.clone()).await;
    assert_eq!(
        replay.status, 201,
        "a replayed token must answer the first create's success, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("provisionedModelArn"),
        token_arn,
        "a replayed token must answer with the ARN the first request minted"
    );
    assert_eq!(
        listed_arns("after the replay", &list(&emu, "").await),
        vec![arn.clone(), token_arn.clone()],
        "a replayed token must buy nothing a second time"
    );

    let mut fresh = committed_body("pt-token-two", PROVISIONABLE_MODELS[1]);
    fresh["clientRequestToken"] = json!("token-two");
    let fresh_arn = buy(&emu, "a fresh token", fresh).await;
    assert_ne!(
        fresh_arn, token_arn,
        "a fresh token must buy its own Provisioned Throughput"
    );
    assert_eq!(
        listed_arns("after the fresh token", &list(&emu, "").await),
        vec![arn, token_arn, fresh_arn],
        "the listing must report every Provisioned Throughput that was bought"
    );
}
