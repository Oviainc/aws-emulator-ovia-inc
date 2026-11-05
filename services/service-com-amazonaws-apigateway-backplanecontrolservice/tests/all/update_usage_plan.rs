// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateUsagePlan`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the usage plan
//! create/read helpers, the deployed-stage helper and the PATCH wrappers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `usage_plan_lifecycle`,
//! `stage_lifecycle` and `patch_updates`.
//!
//! The RULE under test: `UpdateUsagePlan` applies every entry of `patchOperations` to the
//! stored `UsagePlan` record the `usagePlanId` names, in request order, and reports the
//! patched record — which a later `GetUsagePlan` reads back member for member — while a
//! patch the service cannot apply is refused with a modelled error and leaves the stored
//! record exactly as it was.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! of members, ops, starting records, nesting depths and pointer spellings; every value of
//! the quota period catalog; the published bottom of each limit's range and one past it;
//! the character bound of a patchable string member and one past it; the ops and paths the
//! resource does not support; repeated application of one list; and the forms an id that
//! names no stored plan takes.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::QUOTA_PERIOD_TYPE_VALUES;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::patch_updates::{op, patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan, tag_map};

/// The `UpdateUsagePlan`/`GetUsagePlan` URI for one plan of the account.
fn plan_uri(usage_plan_id: &str) -> String {
    format!("/usageplans/{usage_plan_id}")
}

/// `UpdateUsagePlan` on `usage_plan_id`, answering whatever the service said.
async fn update(gateway: &Gateway, usage_plan_id: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &plan_uri(usage_plan_id), operations).await
}

/// Create a plan from `request` and answer its id.
async fn baseline(gateway: &Gateway, case: &str, request: Value) -> String {
    let created = create_usage_plan(gateway, case, request).await;
    string_member(&created, "id", case)
}

/// Assert every `(member, expected)` pair holds in `body`; a `Value::Null` expectation is
/// the member being ABSENT, which is what a cleared optional member looks like on the
/// wire.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

/// One row of the patch table: the case, the plan the row starts from, the
/// `patchOperations` it sends, and the members the patched plan must report afterwards.
type Row = (String, Value, Vec<Value>, Vec<(&'static str, Value)>);

#[tokio::test]
async fn a_patched_usage_plan_member_is_stored_and_read_back() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "update-usage-plan-one").await;
    let (second, _) = create_api(&gateway, "update-usage-plan-two").await;
    // An API stage association must name a stored stage, so each stage a row associates is
    // deployed first — over the wire, through `CreateDeployment`.
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &first, "beta").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    let described = || json!({ "name": "plan-described", "description": "initial description" });
    let metered = || {
        json!({
            "name": "plan-metered",
            "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
            // The offset is nonzero, so the period is one whose window admits a shift:
            // an offset must be zero in the `DAY` period (`bound_quota_settings`).
            "quota": { "limit": 100, "offset": 1, "period": "WEEK" },
        })
    };
    let staged = || {
        json!({
            "name": "plan-staged",
            "apiStages": [{ "apiId": first, "stage": "prod" }],
        })
    };

    // Rows differ along what the rule generalises over: which member the path names, how
    // deep into the record it reaches, whether the record started with that member set,
    // which op the row uses, how many ops one request carries, whether two ops target one
    // member, the form and size of the value, and which pointer spelling addresses one
    // entry of a collection or map.
    let mut rows: Vec<Row> = vec![
        (
            "replace the name".to_string(),
            described(),
            vec![replace("/name", "renamed")],
            vec![
                ("name", json!("renamed")),
                ("description", json!("initial description")),
            ],
        ),
        (
            "replace the description".to_string(),
            described(),
            vec![replace("/description", "a newer description")],
            vec![
                ("name", json!("plan-described")),
                ("description", json!("a newer description")),
            ],
        ),
        (
            "add a description the plan had none of".to_string(),
            json!({ "name": "plan-undescribed" }),
            vec![op("add", "/description", Some("first description"))],
            vec![("description", json!("first description"))],
        ),
        (
            "remove the description".to_string(),
            described(),
            vec![op("remove", "/description", None)],
            vec![
                ("name", json!("plan-described")),
                ("description", Value::Null),
            ],
        ),
        (
            "add on the name sets it just as replace does".to_string(),
            described(),
            vec![op("add", "/name", Some("added-name"))],
            vec![("name", json!("added-name"))],
        ),
        (
            "a name outside ASCII".to_string(),
            described(),
            vec![replace("/name", "план-✓")],
            vec![("name", json!("план-✓"))],
        ),
        (
            "a name at the character bound".to_string(),
            described(),
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS))],
            vec![("name", json!("n".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "a description at the character bound".to_string(),
            described(),
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS))],
            vec![("description", json!("d".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "associate a product code the plan had none of".to_string(),
            described(),
            vec![op("add", "/productCode", Some("ABCDEFG1234"))],
            vec![("productCode", json!("ABCDEFG1234"))],
        ),
        (
            "replace and then clear the product code".to_string(),
            described(),
            vec![
                replace("/productCode", "FIRSTCODE01"),
                replace("/productCode", "SECONDCODE2"),
                op("remove", "/productCode", None),
            ],
            vec![("productCode", Value::Null)],
        ),
        (
            "replace one throttle limit and keep the other".to_string(),
            metered(),
            vec![replace("/throttle/rateLimit", "7.5")],
            vec![(
                "throttle",
                json!({ "burstLimit": 10, "rateLimit": 7.5 }),
            )],
        ),
        (
            "replace both throttle limits in one request".to_string(),
            metered(),
            vec![
                replace("/throttle/burstLimit", "20"),
                replace("/throttle/rateLimit", "0.5"),
            ],
            vec![(
                "throttle",
                json!({ "burstLimit": 20, "rateLimit": 0.5 }),
            )],
        ),
        (
            "a throttle limit at the bottom of its range".to_string(),
            metered(),
            vec![
                replace("/throttle/burstLimit", "0"),
                replace("/throttle/rateLimit", "0"),
            ],
            vec![(
                "throttle",
                json!({ "burstLimit": 0, "rateLimit": 0.0 }),
            )],
        ),
        (
            "a throttle on a plan that had none takes the modelled defaults".to_string(),
            described(),
            vec![op("add", "/throttle/burstLimit", Some("3"))],
            vec![("throttle", json!({ "burstLimit": 3, "rateLimit": 0.0 }))],
        ),
        (
            "remove the whole throttle".to_string(),
            metered(),
            vec![op("remove", "/throttle", None)],
            vec![
                ("throttle", Value::Null),
                (
                    "quota",
                    json!({ "limit": 100, "offset": 1, "period": "WEEK" }),
                ),
            ],
        ),
        (
            "replace the quota limit".to_string(),
            metered(),
            vec![replace("/quota/limit", "250")],
            vec![(
                "quota",
                json!({ "limit": 250, "offset": 1, "period": "WEEK" }),
            )],
        ),
        (
            "replace the quota offset".to_string(),
            metered(),
            vec![replace("/quota/offset", "0")],
            vec![(
                "quota",
                json!({ "limit": 100, "offset": 0, "period": "WEEK" }),
            )],
        ),
        (
            "a quota count at the top of the modelled Integer".to_string(),
            metered(),
            vec![replace("/quota/limit", &i32::MAX.to_string())],
            vec![(
                "quota",
                json!({ "limit": i32::MAX, "offset": 1, "period": "WEEK" }),
            )],
        ),
        (
            "a quota on a plan that had none takes the modelled defaults".to_string(),
            described(),
            vec![replace("/quota/limit", "42")],
            vec![("quota", json!({ "limit": 42, "offset": 0 }))],
        ),
        (
            "remove the whole quota".to_string(),
            metered(),
            vec![op("remove", "/quota", None)],
            vec![
                ("quota", Value::Null),
                (
                    "throttle",
                    json!({ "burstLimit": 10, "rateLimit": 5.0 }),
                ),
            ],
        ),
        (
            "associate a stage named by the op's value".to_string(),
            described(),
            vec![op("add", "/apiStages", Some(&format!("{first}:prod")))],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "prod" }]),
            )],
        ),
        (
            "associate a stage named by the path".to_string(),
            described(),
            vec![op("add", &format!("/apiStages/{first}:beta"), None)],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "beta" }]),
            )],
        ),
        (
            "associate stages of two apis".to_string(),
            described(),
            vec![
                op("add", "/apiStages", Some(&format!("{first}:prod"))),
                op("add", "/apiStages", Some(&format!("{second}:canary"))),
            ],
            vec![(
                "apiStages",
                json!([
                    { "apiId": first, "stage": "prod" },
                    { "apiId": second, "stage": "canary" },
                ]),
            )],
        ),
        (
            "associating one stage twice associates it once".to_string(),
            staged(),
            vec![
                op("add", "/apiStages", Some(&format!("{first}:prod"))),
                op("add", &format!("/apiStages/{first}:prod"), None),
            ],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "prod" }]),
            )],
        ),
        (
            "disassociate the plan's only stage".to_string(),
            staged(),
            vec![op("remove", &format!("/apiStages/{first}:prod"), None)],
            vec![("apiStages", json!([]))],
        ),
        (
            "disassociate one stage of two".to_string(),
            json!({
                "name": "plan-two-stages",
                "apiStages": [
                    { "apiId": first, "stage": "prod" },
                    { "apiId": first, "stage": "beta" },
                ],
            }),
            vec![op("remove", "/apiStages", Some(&format!("{first}:prod")))],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "beta" }]),
            )],
        ),
        (
            "a stage disassociated and then associated again".to_string(),
            staged(),
            vec![
                op("remove", &format!("/apiStages/{first}:prod"), None),
                op("add", "/apiStages", Some(&format!("{first}:prod"))),
            ],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "prod" }]),
            )],
        ),
        (
            "a per-method throttle on an associated stage".to_string(),
            staged(),
            vec![
                op(
                    "add",
                    &format!("/apiStages/{first}:prod/throttle/~1pets/GET/burstLimit"),
                    Some("4"),
                ),
                op(
                    "add",
                    &format!("/apiStages/{first}:prod/throttle/~1pets/GET/rateLimit"),
                    Some("2.5"),
                ),
            ],
            vec![(
                "apiStages",
                json!([{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 4, "rateLimit": 2.5 } },
                }]),
            )],
        ),
        (
            "the escaped method spelling addresses the same map entry".to_string(),
            json!({
                "name": "plan-method-throttled",
                "apiStages": [{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 1, "rateLimit": 1.0 } },
                }],
            }),
            vec![op(
                "replace",
                &format!("/apiStages/{first}:prod/throttle/~1pets~1GET/burstLimit"),
                Some("9"),
            )],
            vec![(
                "apiStages",
                json!([{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 9, "rateLimit": 1.0 } },
                }]),
            )],
        ),
        (
            "two methods throttled on one stage".to_string(),
            staged(),
            vec![
                op(
                    "add",
                    &format!("/apiStages/{first}:prod/throttle/~1pets/GET/rateLimit"),
                    Some("1"),
                ),
                op(
                    "add",
                    &format!("/apiStages/{first}:prod/throttle/~1pets~1{{petId}}/DELETE/rateLimit"),
                    Some("2"),
                ),
            ],
            vec![(
                "apiStages",
                json!([{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": {
                        "/pets/GET": { "burstLimit": 0, "rateLimit": 1.0 },
                        "/pets/{petId}/DELETE": { "burstLimit": 0, "rateLimit": 2.0 },
                    },
                }]),
            )],
        ),
        (
            "remove one method's throttle entry".to_string(),
            json!({
                "name": "plan-two-methods",
                "apiStages": [{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": {
                        "/pets/GET": { "burstLimit": 1, "rateLimit": 1.0 },
                        "/pets/POST": { "burstLimit": 2, "rateLimit": 2.0 },
                    },
                }],
            }),
            vec![op(
                "remove",
                &format!("/apiStages/{first}:prod/throttle/~1pets~1GET"),
                None,
            )],
            vec![(
                "apiStages",
                json!([{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/POST": { "burstLimit": 2, "rateLimit": 2.0 } },
                }]),
            )],
        ),
        (
            "remove every per-method throttle of one stage".to_string(),
            json!({
                "name": "plan-method-cleared",
                "apiStages": [{
                    "apiId": first,
                    "stage": "prod",
                    "throttle": { "/pets/GET": { "burstLimit": 1, "rateLimit": 1.0 } },
                }],
            }),
            vec![op(
                "remove",
                &format!("/apiStages/{first}:prod/throttle"),
                None,
            )],
            vec![(
                "apiStages",
                json!([{ "apiId": first, "stage": "prod" }]),
            )],
        ),
        (
            "two ops on one member apply in request order".to_string(),
            described(),
            vec![replace("/name", "first"), replace("/name", "second")],
            vec![("name", json!("second"))],
        ),
        (
            "members of several structures in one request".to_string(),
            metered(),
            vec![
                replace("/name", "all-at-once"),
                replace("/description", "patched throughout"),
                replace("/throttle/burstLimit", "11"),
                replace("/quota/period", "MONTH"),
                op("add", "/apiStages", Some(&format!("{second}:canary"))),
            ],
            vec![
                ("name", json!("all-at-once")),
                ("description", json!("patched throughout")),
                (
                    "throttle",
                    json!({ "burstLimit": 11, "rateLimit": 5.0 }),
                ),
                (
                    "quota",
                    json!({ "limit": 100, "offset": 1, "period": "MONTH" }),
                ),
                (
                    "apiStages",
                    json!([{ "apiId": second, "stage": "canary" }]),
                ),
            ],
        ),
        (
            "an empty list reports the record unchanged".to_string(),
            metered(),
            vec![],
            vec![
                ("name", json!("plan-metered")),
                (
                    "throttle",
                    json!({ "burstLimit": 10, "rateLimit": 5.0 }),
                ),
            ],
        ),
        (
            "a tagged plan keeps the tags another operation owns".to_string(),
            json!({ "name": "plan-tagged", "tags": tag_map(2) }),
            vec![replace("/name", "tagged-and-renamed")],
            vec![
                ("name", json!("tagged-and-renamed")),
                ("tags", tag_map(2)),
            ],
        ),
    ];
    // Every period the model declares, so the catalog is exercised and not one recorded
    // value. Source: `smithy_query(query="shape", shape="QuotaPeriodType")`, re-exported
    // as `QUOTA_PERIOD_TYPE_VALUES`. The plan under the row carries a ZERO offset, the one
    // offset every period's window admits, so the row is about the period and not about
    // the offset's window (`bound_quota_settings`).
    let unshifted = || {
        json!({
            "name": "plan-metered",
            "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
            "quota": { "limit": 100, "offset": 0, "period": "WEEK" },
        })
    };
    for period in QUOTA_PERIOD_TYPE_VALUES {
        rows.push((
            format!("replace the quota period with {period}"),
            unshifted(),
            vec![replace("/quota/period", period)],
            vec![(
                "quota",
                json!({ "limit": 100, "offset": 0, "period": period }),
            )],
        ));
    }

    for (case, request, operations, expected) in rows {
        let id = baseline(&gateway, &case, request).await;
        let updated = patch_ok(&gateway, &case, &plan_uri(&id), operations).await;
        assert_members(&updated, &expected, &case, "UpdateUsagePlan");
        assert_eq!(
            updated["id"],
            json!(id),
            "case {case}: the update must report the record's own id, got {:?}",
            updated["id"]
        );

        // The patched record is what is STORED, not just what the update answered.
        let read = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetUsagePlan, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated,
            "case {case}: GetUsagePlan must read back the patched record, got {:?} after {:?}",
            read.body, updated
        );
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "update-usage-plan-refusals").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // Rows differ along the KIND of refusal: an op no member supports, an op missing its
    // value, a path naming a member the service assigns or that another operation owns, a
    // path that is not a JSON Pointer, a path reaching into a single-valued member, a path
    // naming a structure a single op cannot set, a value outside a declared value set or a
    // published range, a value that is not a number at all, a malformed collection entry,
    // a value one character past the bound, and a list whose LAST op is refused (so the
    // earlier ops must not be kept either).
    let cases: Vec<(String, Vec<Value>, u16, &str)> = vec![
        (
            "move is not supported".to_string(),
            vec![op("move", "/name", None)],
            400,
            "BadRequestException",
        ),
        (
            "copy is not supported".to_string(),
            vec![json!({ "op": "copy", "path": "/name", "from": "/name" })],
            400,
            "BadRequestException",
        ),
        (
            "test is not supported".to_string(),
            vec![op("test", "/name", None)],
            400,
            "BadRequestException",
        ),
        (
            "a replace with no value".to_string(),
            vec![op("replace", "/name", None)],
            400,
            "BadRequestException",
        ),
        (
            "an entry with no path".to_string(),
            vec![json!({ "op": "replace" })],
            400,
            "BadRequestException",
        ),
        (
            "an entry with no op".to_string(),
            vec![json!({ "path": "/name" })],
            400,
            "BadRequestException",
        ),
        (
            "the id is reported but minted by the service".to_string(),
            vec![replace("/id", "zzzzzzzzzz")],
            400,
            "BadRequestException",
        ),
        (
            "tags are changed through the tagging operations".to_string(),
            vec![replace("/tags/k0", "other")],
            400,
            "BadRequestException",
        ),
        (
            "a path naming no updateable member".to_string(),
            vec![replace("/nosuchmember", "x")],
            400,
            "BadRequestException",
        ),
        (
            "a path that is not a JSON Pointer".to_string(),
            vec![replace("name", "x")],
            400,
            "BadRequestException",
        ),
        (
            "an empty path".to_string(),
            vec![replace("", "x")],
            400,
            "BadRequestException",
        ),
        (
            "a path reaching into a single-valued member".to_string(),
            vec![replace("/name/first", "x")],
            400,
            "BadRequestException",
        ),
        (
            "remove of a name a plan always reports".to_string(),
            vec![op("remove", "/name", None)],
            400,
            "BadRequestException",
        ),
        (
            "an empty name".to_string(),
            vec![replace("/name", "")],
            400,
            "BadRequestException",
        ),
        (
            "a name one character past the bound".to_string(),
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS + 1))],
            400,
            "BadRequestException",
        ),
        (
            "a description one character past the bound".to_string(),
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS + 1))],
            400,
            "BadRequestException",
        ),
        (
            "a replace of the whole throttle structure".to_string(),
            vec![replace("/throttle", "10")],
            400,
            "BadRequestException",
        ),
        (
            "an add of the whole quota structure".to_string(),
            vec![op("add", "/quota", Some("100"))],
            400,
            "BadRequestException",
        ),
        (
            "a throttle member the shape does not declare".to_string(),
            vec![replace("/throttle/nosuchlimit", "1")],
            400,
            "BadRequestException",
        ),
        (
            "a negative throttle rate".to_string(),
            vec![replace("/throttle/rateLimit", "-0.5")],
            400,
            "BadRequestException",
        ),
        (
            "a negative throttle burst".to_string(),
            vec![replace("/throttle/burstLimit", "-1")],
            400,
            "BadRequestException",
        ),
        (
            "a throttle rate that is not a number".to_string(),
            vec![replace("/throttle/rateLimit", "fast")],
            400,
            "BadRequestException",
        ),
        (
            "a negative quota limit".to_string(),
            vec![replace("/quota/limit", "-1")],
            400,
            "BadRequestException",
        ),
        (
            "a negative quota offset".to_string(),
            vec![replace("/quota/offset", "-1")],
            400,
            "BadRequestException",
        ),
        (
            "a quota period outside the declared value set".to_string(),
            vec![replace("/quota/period", "YEAR")],
            400,
            "BadRequestException",
        ),
        (
            "a quota period in the wrong case".to_string(),
            vec![replace("/quota/period", "day")],
            400,
            "BadRequestException",
        ),
        (
            "a replace of one api stage association".to_string(),
            vec![replace(&format!("/apiStages/{api}:prod"), "x")],
            400,
            "BadRequestException",
        ),
        (
            "an api stage entry naming no stage".to_string(),
            vec![op("add", "/apiStages", Some(&api))],
            400,
            "BadRequestException",
        ),
        (
            "an api stage entry naming no api".to_string(),
            vec![op("add", "/apiStages", Some(":prod"))],
            400,
            "BadRequestException",
        ),
        (
            "an add of an api stage with no value".to_string(),
            vec![op("add", "/apiStages", None)],
            400,
            "BadRequestException",
        ),
        (
            "an api stage member the shape does not declare".to_string(),
            vec![replace(&format!("/apiStages/{api}:prod/stage"), "beta")],
            400,
            "BadRequestException",
        ),
        (
            "an api that names no stored api".to_string(),
            vec![op("add", "/apiStages", Some("zzzzzzzzzz:prod"))],
            404,
            "NotFoundException",
        ),
        (
            "a stage the named api does not have".to_string(),
            vec![op("add", "/apiStages", Some(&format!("{api}:nosuchstage")))],
            404,
            "NotFoundException",
        ),
        (
            "a per-method throttle on a stage the plan is not associated with".to_string(),
            vec![op(
                "add",
                &format!("/apiStages/{api}:prod/throttle/~1pets/GET/rateLimit"),
                Some("1"),
            )],
            404,
            "NotFoundException",
        ),
        (
            "a list whose last op is refused".to_string(),
            vec![
                replace("/name", "never stored"),
                replace("/nosuchmember", "x"),
            ],
            400,
            "BadRequestException",
        ),
    ];

    let baseline_members = vec![
        ("name", json!("plan-baseline")),
        ("description", json!("initial description")),
        ("throttle", json!({ "burstLimit": 10, "rateLimit": 5.0 })),
        (
            "quota",
            json!({ "limit": 100, "offset": 1, "period": "WEEK" }),
        ),
        ("apiStages", json!([])),
        ("tags", tag_map(1)),
    ];
    // ONE stored plan for every row: a refused patch must leave the record exactly as it
    // was, so each row both checks the refusal and re-checks the record the previous rows
    // must not have changed either.
    let id = baseline(
        &gateway,
        "the plan every row patches",
        json!({
            "name": "plan-baseline",
            "description": "initial description",
            "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
            // See the `metered` plan above for why a nonzero offset names `WEEK`.
            "quota": { "limit": 100, "offset": 1, "period": "WEEK" },
            "tags": tag_map(1),
        }),
    )
    .await;

    for (case, operations, status, code) in &cases {
        let answer = update(&gateway, &id, operations.clone()).await;
        assert_failure(&answer, *status, code, case);

        // Nothing was applied: the stored record is the one the create left behind.
        let read = get_usage_plan(&gateway, &id).await;
        assert_members(&read.body, &baseline_members, case, "GetUsagePlan");
    }

    // A path the resource has no updateable member for is refused with the path quoted, the
    // `op` of the same entry and the paths a plan publishes — the wording the
    // `patch_path_refusal_templates` suite pins for every operation that enumerates them.
    let answer = update(&gateway, &id, vec![replace("/id", "zzzzzzzzzz")]).await;
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message,
        "Invalid patch path  '/id' specified for op 'replace'. Must be one of: [/name, \
         /description, /productCode, /quota/period, /quota/limit, /quota/offset, \
         /throttle/rateLimit, /throttle/burstLimit, \
         /apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/rateLimit, \
         /apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/burstLimit, \
         /apiStages/<apidId:stageName>/throttle]",
        "case the message of an unsupported path: expected the path, the op and the plan's \
         own paths, got {message:?}"
    );
}

#[tokio::test]
async fn an_id_no_stored_plan_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let stored = baseline(
        &gateway,
        "the stored plan",
        json!({ "name": "plan-stored", "description": "the stored plan" }),
    )
    .await;
    let operations = vec![replace("/name", "never applied")];

    // Rows differ along the form of the id: an id of the shape this service mints, a
    // shorter and a longer one, the plan's NAME rather than its id, the stored id in
    // another case, and the stored id with one character stepped on.
    let mut rows: Vec<(String, String)> = vec![
        ("an unminted id".to_string(), "zzzzz99999".to_string()),
        ("a short id".to_string(), "abc".to_string()),
        (
            "a long id".to_string(),
            "abcdefghij0123456789".to_string(),
        ),
        (
            "the plan's name, not its id".to_string(),
            "plan-stored".to_string(),
        ),
        (
            "the stored id upper-cased".to_string(),
            stored.to_uppercase(),
        ),
    ];
    let mut near_miss: Vec<char> = stored.chars().collect();
    if let Some(last) = near_miss.pop() {
        near_miss.push(if last == 'a' { 'b' } else { 'a' });
    }
    rows.push((
        "the stored id with one character changed".to_string(),
        near_miss.into_iter().collect(),
    ));

    for (case, id) in &rows {
        assert_failure(
            &update(&gateway, id, operations.clone()).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // None of those refusals touched the stored record.
    let read = get_usage_plan(&gateway, &stored).await;
    assert_members(
        &read.body,
        &[
            ("name", json!("plan-stored")),
            ("description", json!("the stored plan")),
        ],
        "after the refusals",
        "GetUsagePlan",
    );
}

#[tokio::test]
async fn one_plan_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "update-usage-plan-siblings").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let target = baseline(
        &gateway,
        "the plan to patch",
        json!({ "name": "plan-target", "description": "initial description" }),
    )
    .await;
    let sibling = baseline(
        &gateway,
        "the sibling plan",
        json!({
            "name": "plan-sibling",
            "description": "the sibling's own description",
            "throttle": { "burstLimit": 1, "rateLimit": 1.0 },
        }),
    )
    .await;

    // Applying one list repeatedly lands on the same record: the patch sets members and
    // associates a stage, it does not accumulate.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateUsagePlan(round {round})");
        seen.push(
            patch_ok(
                &gateway,
                &case,
                &plan_uri(&target),
                vec![
                    replace("/name", "settled"),
                    replace("/quota/limit", "7"),
                    op("add", "/apiStages", Some(&format!("{api}:prod"))),
                ],
            )
            .await,
        );
    }
    for round in 1..seen.len() {
        assert_eq!(
            seen[round], seen[0],
            "case UpdateUsagePlan(round {round}): the same list must land on the same record, \
             got {:?} after {:?}",
            seen[round], seen[0]
        );
    }

    // The sibling is the record its own create left behind.
    let read = get_usage_plan(&gateway, &sibling).await;
    assert_members(
        &read.body,
        &[
            ("name", json!("plan-sibling")),
            ("description", json!("the sibling's own description")),
            ("throttle", json!({ "burstLimit": 1, "rateLimit": 1.0 })),
            ("apiStages", json!([])),
        ],
        "the sibling plan",
        "GetUsagePlan",
    );
    assert_eq!(
        read.body["id"],
        json!(sibling),
        "case the sibling plan: expected its own id, got {:?}",
        read.body["id"]
    );
}
