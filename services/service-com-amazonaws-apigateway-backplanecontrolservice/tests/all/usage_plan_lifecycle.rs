// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the usage plan lifecycle: `CreateUsagePlan` and `GetUsagePlan`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper and the tag-call helpers are this crate's existing harness,
//! reused from `rest_api_lifecycle`, `stage_lifecycle` and `tag_reads_by_arn`.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * a created usage plan is PERSISTED and reported from the stored record, so what the
//!   create answers is what a later `GetUsagePlan` reports — over a table spanning which
//!   optional members the request carries, every `QuotaPeriodType` value, both bounds of
//!   the quota counts, how many API stages the plan is associated with and how many tags
//!   it carries;
//! * every throttle limit on the request — the plan's own and the per-method one on each
//!   `apiStages` entry — must be a non-negative number, and every quota count must be a
//!   non-negative integer, else the request is refused 400 `BadRequestException`;
//! * an `apiStages` entry must name a stored stage of a stored API, else the request is
//!   refused (404 `NotFoundException` for an identifier that resolves to nothing, 400
//!   `BadRequestException` for an entry that names no stage at all);
//! * a `usagePlanId` no stored plan answers to is 404 `NotFoundException`, and reading a
//!   plan changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::QUOTA_PERIOD_TYPE_VALUES;

use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};

/// The largest value the modelled `Integer` of `QuotaSettings$limit` / `$offset` carries,
/// which a quota must round-trip without losing precision.
const MAX_INTEGER: i64 = i32::MAX as i64;

/// `CreateUsagePlan` with `request`, asserting the modelled 201 and returning the body.
pub(crate) async fn create_usage_plan(gateway: &Gateway, case: &str, request: Value) -> Value {
    let answer = gateway.call("POST", "/usageplans", Some(request)).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `GetUsagePlan` for `id`.
pub(crate) async fn get_usage_plan(gateway: &Gateway, id: &str) -> Answer {
    gateway.call("GET", &format!("/usageplans/{id}"), None).await
}

/// A tag map of `count` entries. Shared with the sibling `usage_plan_creation_bounds`
/// suite, which drives the same map at the tag-count bound and one past it.
pub(crate) fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("k{n}"), Value::String(format!("v{n}"))))
            .collect(),
    )
}

/// Assert that `member` — a structure the request either carried or did not — is reported
/// with every value the request set for it, and is absent when the request set none.
///
/// Member-wise rather than whole-value, because the members `ThrottleSettings` and
/// `QuotaSettings` declare defaults for are reported whether or not the request set them:
/// what this pins is that nothing the caller SET is lost or altered.
fn assert_echoed_structure(body: &Value, request: &Value, member: &str, case: &str) {
    match request.get(member) {
        None => assert!(
            body[member].is_null(),
            "case {case}: a plan created with no {member} must report none, got {:?}",
            body[member]
        ),
        Some(sent) => {
            let sent = sent
                .as_object()
                .unwrap_or_else(|| panic!("case {case}: the request's {member} is a structure"));
            for (key, value) in sent {
                assert_eq!(
                    &body[member][key], value,
                    "case {case}: {member}.{key} must report the request's value, got {:?}",
                    body[member][key]
                );
            }
        }
    }
}

#[tokio::test]
async fn a_created_usage_plan_is_persisted_and_read_back_member_for_member() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "usage-plan-stages-one").await;
    let (second, _) = create_api(&gateway, "usage-plan-stages-two").await;
    // An API stage must name a stage, so each one these rows associate is deployed first
    // — over the wire, through `CreateDeployment`.
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &first, "beta").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    // Rows differ along what the rule generalises over: which optional members the
    // request carries, the quota period (every value the model declares), the quota
    // counts at both ends of their range, how many API stages the plan covers and whether
    // they belong to one API or several, whether a stage carries per-method throttling,
    // and how many tags the plan carries.
    let mut cases: Vec<(String, Value)> = vec![
        ("only the required name".to_string(), json!({ "name": "plan-minimal" })),
        (
            "named and described".to_string(),
            json!({ "name": "plan-described", "description": "the described plan" }),
        ),
        (
            "a throttle only".to_string(),
            json!({ "name": "plan-throttled", "throttle": { "burstLimit": 5, "rateLimit": 10.5 } }),
        ),
        (
            "a throttle at the bottom of its range".to_string(),
            json!({ "name": "plan-zero-throttle", "throttle": { "burstLimit": 0, "rateLimit": 0.0 } }),
        ),
        (
            "a quota with a positive offset".to_string(),
            // At the top of the window the `WEEK` period gives the offset: an offset falls
            // inside the initial period it shifts (`bound_quota_settings`).
            json!({ "name": "plan-offset", "quota": { "limit": 500, "offset": 6, "period": "WEEK" } }),
        ),
        (
            "a quota at the bottom of its range".to_string(),
            json!({ "name": "plan-zero-quota", "quota": { "limit": 0, "offset": 0, "period": "DAY" } }),
        ),
        (
            "a quota limit at the top of the modelled Integer, with the offset at the top of its period".to_string(),
            json!({
                "name": "plan-large-quota",
                "quota": { "limit": MAX_INTEGER, "offset": 27, "period": "MONTH" },
            }),
        ),
        (
            "a throttle and a quota together".to_string(),
            json!({
                "name": "plan-metered",
                "throttle": { "burstLimit": 200, "rateLimit": 100.0 },
                "quota": { "limit": 10_000, "offset": 0, "period": "MONTH" },
            }),
        ),
        (
            "one api stage".to_string(),
            json!({ "name": "plan-one-stage", "apiStages": [{ "apiId": first, "stage": "prod" }] }),
        ),
        (
            "two stages of one api".to_string(),
            json!({
                "name": "plan-two-stages",
                "apiStages": [
                    { "apiId": first, "stage": "prod" },
                    { "apiId": first, "stage": "beta" },
                ],
            }),
        ),
        (
            "stages of two apis".to_string(),
            json!({
                "name": "plan-two-apis",
                "apiStages": [
                    { "apiId": first, "stage": "prod" },
                    { "apiId": second, "stage": "canary" },
                ],
            }),
        ),
        (
            "an api stage with per-method throttling".to_string(),
            json!({
                "name": "plan-method-throttle",
                "apiStages": [{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 2, "rateLimit": 1.0 } },
                }],
            }),
        ),
        ("one tag".to_string(), json!({ "name": "plan-tagged-1", "tags": tag_map(1) })),
        ("three tags".to_string(), json!({ "name": "plan-tagged-3", "tags": tag_map(3) })),
    ];
    // Every period the model declares, so the catalog is exercised and not one recorded
    // value. Source: `smithy_query(query="shape", shape="QuotaPeriodType")`, re-exported
    // as `QUOTA_PERIOD_TYPE_VALUES`.
    for period in QUOTA_PERIOD_TYPE_VALUES {
        cases.push((
            format!("a quota per {period}"),
            json!({
                "name": format!("plan-quota-{period}"),
                "quota": { "limit": 1_000, "offset": 0, "period": period },
            }),
        ));
    }

    let mut ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let created = create_usage_plan(&gateway, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        assert!(
            id.len() == 10 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case}: expected a ten-character lowercase alphanumeric id, got {id:?}"
        );

        // Every echoed member reports what the request set, and nothing it did not.
        for member in ["name", "description"] {
            assert_eq!(
                created[member],
                request.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: {member} must report the request's value, got {:?}",
                created[member]
            );
        }
        assert_echoed_structure(&created, request, "throttle", case);
        assert_echoed_structure(&created, request, "quota", case);
        // The two collection members are always reported, empty when the plan holds
        // nothing: a `UsagePlan` carries a list of API stage associations and a map of
        // tags, which are empty rather than absent on a plan that has neither.
        assert_eq!(
            created["tags"],
            request.get("tags").cloned().unwrap_or(json!({})),
            "case {case}: tags must report the request's map, got {:?}",
            created["tags"]
        );
        let sent_stages = request
            .get("apiStages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let stages = created["apiStages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an apiStages list, got {created:?}"));
        assert_eq!(
            stages.len(),
            sent_stages.len(),
            "case {case}: expected {} api stages, got {:?}",
            sent_stages.len(),
            created["apiStages"]
        );
        for (position, sent) in sent_stages.iter().enumerate() {
            for member in ["apiId", "stage"] {
                assert_eq!(
                    stages[position][member], sent[member],
                    "case {case}: apiStages[{position}].{member} must report the request's \
                     value, got {:?}",
                    stages[position][member]
                );
            }
            // A per-method throttle entry the request set is reported with the values it
            // set, by the same rule the plan's own throttle follows.
            for (method, limits) in sent["throttle"].as_object().into_iter().flatten() {
                for (member, value) in limits.as_object().into_iter().flatten() {
                    assert_eq!(
                        &stages[position]["throttle"][method][member], value,
                        "case {case}: apiStages[{position}].throttle[{method}].{member} must \
                         report the request's value, got {:?}",
                        stages[position]["throttle"][method][member]
                    );
                }
            }
        }
        // Nothing of the Marketplace product code is invented: no member of this
        // operation's input sets it.
        assert!(
            created["productCode"].is_null(),
            "case {case}: a created plan carries no productCode, got {:?}",
            created["productCode"]
        );

        // Read back through GetUsagePlan: the create answered from the stored record, so
        // the read reports exactly the same body.
        let read = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetUsagePlan, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetUsagePlan must report the stored plan, got {:?} after {:?}",
            read.body, created
        );
        ids.push(id);
    }

    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "every created plan takes its own id, got {ids:?}"
    );
}

#[tokio::test]
async fn a_usage_plan_s_limits_must_be_values_api_gateway_can_meter() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "usage-plan-limit-bounds").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // A throttle limit is a request rate and a quota count is a number of requests, so
    // neither can be negative. Rows span both members of each structure, both places a
    // throttle rides on the request (the plan's own and a stage's per-method map), and
    // both ends of the range: the bottom value is ACCEPTED and one past it REFUSED.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a negative quota limit",
            json!({ "name": "p", "quota": { "limit": -1, "offset": 0, "period": "DAY" } }),
        ),
        (
            "the smallest negative quota limit",
            json!({ "name": "p", "quota": { "limit": -MAX_INTEGER, "offset": 0, "period": "MONTH" } }),
        ),
        (
            "a negative quota offset",
            json!({ "name": "p", "quota": { "limit": 100, "offset": -1, "period": "WEEK" } }),
        ),
        (
            "a negative throttle rate",
            json!({ "name": "p", "throttle": { "burstLimit": 1, "rateLimit": -1.0 } }),
        ),
        (
            "a negative throttle burst",
            json!({ "name": "p", "throttle": { "burstLimit": -1, "rateLimit": 1.0 } }),
        ),
        (
            "a negative per-method throttle rate on a stage",
            json!({
                "name": "p",
                "apiStages": [{
                    "apiId": api,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 1, "rateLimit": -0.5 } },
                }],
            }),
        ),
        (
            "a negative per-method throttle burst on a stage",
            json!({
                "name": "p",
                "apiStages": [{
                    "apiId": api,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": -2, "rateLimit": 1.0 } },
                }],
            }),
        ),
    ];
    for (case, request) in &refused {
        let answer = gateway
            .call("POST", "/usageplans", Some(request.clone()))
            .await;
        assert_failure(&answer, 400, "BadRequestException", case);
    }

    let accepted: Vec<(&str, Value)> = vec![
        (
            "a quota of zero requests",
            json!({ "name": "p-zero-quota", "quota": { "limit": 0, "offset": 0, "period": "DAY" } }),
        ),
        (
            "a throttle of zero",
            json!({ "name": "p-zero-throttle", "throttle": { "burstLimit": 0, "rateLimit": 0.0 } }),
        ),
        (
            "a fractional throttle rate",
            json!({ "name": "p-fractional", "throttle": { "burstLimit": 1, "rateLimit": 0.5 } }),
        ),
        (
            "a per-method throttle of zero on a stage",
            json!({
                "name": "p-zero-method-throttle",
                "apiStages": [{
                    "apiId": api,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 0, "rateLimit": 0.0 } },
                }],
            }),
        ),
    ];
    for (case, request) in &accepted {
        create_usage_plan(&gateway, case, request.clone()).await;
    }
}

#[tokio::test]
async fn a_quota_period_outside_the_modelled_value_set_is_refused() {
    let gateway = Gateway::new();
    // `QuotaSettings$period` targets the modelled `QuotaPeriodType` enum, so a value
    // outside that set is refused by the CONSTRAINT layer before the handler runs, under
    // that layer's own error class rather than the operation's `BadRequestException` (see
    // the `validation_message_rules` suite). Rows: a value of another period vocabulary,
    // the right value in the wrong case, and the empty string.
    for period in ["YEAR", "day", ""] {
        let case = format!("CreateUsagePlan(quota.period={period:?})");
        let answer = gateway
            .call(
                "POST",
                "/usageplans",
                Some(json!({
                    "name": "p",
                    "quota": { "limit": 10, "offset": 0, "period": period },
                })),
            )
            .await;
        assert_failure(&answer, 400, "ValidationException", &case);
    }
}

#[tokio::test]
async fn an_api_stage_must_name_a_stored_stage_of_a_stored_api() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "usage-plan-stage-refusals-one").await;
    let (second, _) = create_api(&gateway, "usage-plan-stage-refusals-two").await;
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    // An entry whose identifiers resolve to nothing is the modelled NotFoundException;
    // one that does not name a stage at all is a bad request. Rows put the offending
    // entry alone, beside a valid one, and in either order.
    let refused: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown apiId",
            json!([{ "apiId": "zzzzzzzzzz", "stage": "prod" }]),
            404,
            "NotFoundException",
        ),
        (
            "an unknown api beside a known stage",
            json!([
                { "apiId": first, "stage": "prod" },
                { "apiId": "zzzzzzzzzz", "stage": "prod" },
            ]),
            404,
            "NotFoundException",
        ),
        (
            "a stage no api has",
            json!([{ "apiId": first, "stage": "nosuchstage" }]),
            404,
            "NotFoundException",
        ),
        (
            "a stage of the other api",
            json!([{ "apiId": first, "stage": "canary" }]),
            404,
            "NotFoundException",
        ),
        (
            "an unknown stage before a known one",
            json!([
                { "apiId": second, "stage": "prod" },
                { "apiId": second, "stage": "canary" },
            ]),
            404,
            "NotFoundException",
        ),
        (
            "no stage",
            json!([{ "apiId": first }]),
            400,
            "BadRequestException",
        ),
        (
            "no apiId",
            json!([{ "stage": "prod" }]),
            400,
            "BadRequestException",
        ),
        (
            "an entry naming neither",
            json!([{}]),
            400,
            "BadRequestException",
        ),
    ];
    for (case, api_stages, status, code) in &refused {
        let answer = gateway
            .call(
                "POST",
                "/usageplans",
                Some(json!({ "name": "p", "apiStages": api_stages })),
            )
            .await;
        assert_failure(&answer, *status, code, case);
    }

    // The stages that DO resolve are associated and persisted, so the refusals above are
    // about the entry and not about the operation.
    let created = create_usage_plan(
        &gateway,
        "both apis' stages",
        json!({
            "name": "p-resolved",
            "apiStages": [
                { "apiId": first, "stage": "prod" },
                { "apiId": second, "stage": "canary" },
            ],
        }),
    )
    .await;
    let id = string_member(&created, "id", "both apis' stages");
    let read = get_usage_plan(&gateway, &id).await;
    assert_eq!(
        read.body["apiStages"], created["apiStages"],
        "case GetUsagePlan(both apis' stages): expected the stored stages, got {:?}",
        read.body["apiStages"]
    );
}

#[tokio::test]
async fn an_unknown_usage_plan_identifier_is_a_not_found() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        "existing",
        json!({ "name": "existing", "description": "the stored plan" }),
    )
    .await;
    let id = string_member(&created, "id", "existing");

    // Rows differ in the form of the identifier: one no plan ever had, the plan's NAME —
    // which is not the id the `usagePlanId` label takes — and an identifier that differs
    // from the stored one in a single character.
    let mut near_miss: Vec<char> = id.chars().collect();
    near_miss[0] = if near_miss[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();
    for identifier in ["zzzzzzzzzz", "existing", near_miss.as_str()] {
        let case = format!("GET /usageplans/{identifier}");
        let answer = get_usage_plan(&gateway, identifier).await;
        assert_failure(&answer, 404, "NotFoundException", &case);
    }

    // The stored plan is still there: a failed lookup changed nothing.
    let read = get_usage_plan(&gateway, &id).await;
    assert_eq!(
        read.status, 200,
        "case GetUsagePlan(existing): expected 200, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn reading_a_usage_plan_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        "stable",
        json!({
            "name": "stable",
            "throttle": { "burstLimit": 5, "rateLimit": 2.5 },
            "quota": { "limit": 100, "offset": 1, "period": "WEEK" },
            "tags": tag_map(2),
        }),
    )
    .await;
    let id = string_member(&created, "id", "stable");

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case GetUsagePlan(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    for round in 1..seen.len() {
        assert_eq!(
            seen[round], seen[0],
            "case GetUsagePlan(round {round}): a read must not change the record, got {:?} \
             after {:?}",
            seen[round], seen[0]
        );
    }
}

#[tokio::test]
async fn a_stored_usage_plan_s_tags_are_the_collection_its_arn_addresses() {
    let gateway = Gateway::new();
    // A usage plan is taggable — the modelled `UsagePlan` carries a `tags` member — and
    // its ARN's resource portion is the URI `GetUsagePlan` is served at, so now that a
    // plan is a stored record the tag operations resolve that record rather than
    // answering "no such resource". Rows differ in how many tags the create carried.
    for count in [0, 1, 3] {
        let case = format!("a plan created with {count} tags");
        let created =
            create_usage_plan(&gateway, &case, json!({ "name": "tagged", "tags": tag_map(count) }))
                .await;
        let id = string_member(&created, "id", &case);
        let arn = apigateway_arn(&format!("/usageplans/{id}"));

        let read = get_tags(&gateway, &arn).await;
        assert_eq!(
            read.status, 200,
            "case GetTags({case}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["tags"], created["tags"],
            "case GetTags({case}): expected the plan's stored tags, got {:?}",
            read.body["tags"]
        );

        // One record: a tag written by ARN is reported by the plan's own read.
        let tagged = gateway
            .call(
                "PUT",
                &format!("/tags/{}", encode_label(&arn)),
                Some(json!({ "tags": { "team": "api" } })),
            )
            .await;
        assert_eq!(
            tagged.status, 204,
            "case TagResource({case}): expected the modelled 204, got {} with {:?}",
            tagged.status, tagged.body
        );
        let after = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            after.body["tags"]["team"],
            json!("api"),
            "case GetUsagePlan({case}): expected the tag written by ARN, got {:?}",
            after.body["tags"]
        );
    }
}
