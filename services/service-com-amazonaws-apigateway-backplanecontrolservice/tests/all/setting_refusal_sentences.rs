// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentences API Gateway refuses a SETTING's own illegal value
//! with, and for the one it refuses a domain name ARN it cannot read with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` / `UpdateAccount` / `CreateUsagePlan`
//! helpers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `stage_lifecycle`, `account_settings`, `usage_plan_lifecycle` and
//! `domain_name_access_associations`.
//!
//! The RULES under test, each over a TABLE of inputs rather than one recorded call:
//!
//! * where API Gateway publishes a refusal sentence for a SETTING, that sentence is the
//!   answer for every value the setting cannot take — not the JSON Pointer the value
//!   happened to arrive on: a method's cache time-to-live outside its documented range
//!   states the rule the value broke, a method's logging level outside its documented
//!   values names the level that was asked for, and a usage plan's quota period outside
//!   the modelled set enumerates the periods a plan meters by;
//! * a value on a member typed as the ARN of a private custom domain name that this
//!   service cannot read as one earns ONE fixed sentence, whichever member carried it and
//!   whatever the value was — which is a different answer from a readable pair that names
//!   no association, so a caller can tell a request it must fix from a resource that is
//!   not there;
//! * every refusal leaves the record exactly as it was, and each setting's legal values —
//!   each published bound included — still apply and read back.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::QUOTA_PERIOD_TYPE_VALUES;

use crate::account_settings::{patch_account, role_arn};
use crate::domain_name_access_associations::{
    association_body, create_association, private_domain,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};
use crate::tag_reads_by_arn::{apigateway_arn, encode_label};
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan};

/// The modelled refusal classes these cases assert.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The stage every stage row is deployed to.
const STAGE: &str = "prod";

/// The documented maximum of `MethodSetting$cacheTtlInSeconds`: API Gateway publishes a
/// maximum time-to-live of 3600 seconds for a method cache (and a default of 300).
const MAX_CACHE_TTL_SECONDS: i32 = 3600;

/// The sentence API Gateway refuses a cache time-to-live the setting cannot take with:
/// fixed text stating the rule, with the documented maximum.
fn invalid_cache_ttl() -> String {
    format!("Invalid time-to-live setting, must be an integer no greater than {MAX_CACHE_TTL_SECONDS}")
}

/// The sentence API Gateway refuses a logging level outside the documented set with: fixed
/// text naming the setting, then the level the request asked for.
fn invalid_logging_level(level: &str) -> String {
    format!("Invalid logging level specified: {level}")
}

/// The sentence API Gateway refuses a quota period outside the modelled set with: entirely
/// fixed text, enumerating the periods in the order the model declares them
/// (`smithy_query(query="shape", shape="QuotaPeriodType")`: DAY, WEEK, MONTH).
const INVALID_QUOTA_PERIOD: &str =
    "Invalid Usage Plan quota period specified. Must be one of [DAY, WEEK, MONTH]";

/// The sentence API Gateway refuses a value it cannot read as a private custom domain
/// name's ARN with: entirely fixed text, carrying neither the value nor the member.
const INVALID_DOMAIN_NAME_ARN: &str = "Invalid domain name ARN specified.";

/// The sentence a readable pair naming no access association of the domain name named is
/// refused with.
const NO_SUCH_ASSOCIATION: &str =
    "This private custom domain name doesn't have any domain name access associations.";

/// One `replace` of `path` with `value`.
fn replace(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// `UpdateStage` for [`STAGE`] of `api` with one operation.
async fn update_stage(gateway: &Gateway, api: &str, operation: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({ "patchOperations": [operation] })),
        )
        .await
}

/// `UpdateUsagePlan` for `plan` with one operation.
async fn update_usage_plan(gateway: &Gateway, plan: &str, operation: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{plan}"),
            Some(json!({ "patchOperations": [operation] })),
        )
        .await
}

/// An API deployed to [`STAGE`], answering the API id.
async fn deployed_api(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    create_deployed_stage(gateway, &api, STAGE).await;
    api
}

/// Assert `answer` is the modelled failure `expected` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// The two method scopes every method-setting row is driven under: the stage-wide wildcard
/// and one named method, so a refusal is about the SETTING and not about the scope.
const METHOD_SCOPES: [&str; 2] = ["/*/*", "/~1pets/GET"];

#[tokio::test]
async fn a_method_cache_ttl_the_setting_cannot_take_earns_one_sentence() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "cache-ttl-sentence").await;
    let before = get_stage(&gateway, &api, STAGE).await;

    // Rows differ along HOW the value breaks the rule — one past the documented maximum,
    // far past it, negative, far negative, past the modelled integer, not a whole number,
    // no number at all, and an empty value — because the sentence states the whole rule
    // ("an integer no greater than the maximum") rather than one bound.
    let values = [
        "3601",
        "86400",
        "-1",
        "-3600",
        "2147483648",
        "60.5",
        "forever",
        "",
    ];
    for scope in METHOD_SCOPES {
        for value in values {
            let case = format!("replace {scope}/caching/ttlInSeconds with {value:?}");
            let answer = update_stage(
                &gateway,
                &api,
                replace(&format!("{scope}/caching/ttlInSeconds"), value),
            )
            .await;
            assert_refusal(&answer, BAD_REQUEST, &invalid_cache_ttl(), &case);
            // A refused patch writes nothing: the stage is exactly as it was.
            let read = get_stage(&gateway, &api, STAGE).await;
            assert_eq!(
                read.body, before.body,
                "case {case}: expected the stage left at {:?}, got {:?}",
                before.body, read.body
            );
        }
    }

    // Each published bound of the range, and one value inside it, applies and reads back —
    // so every refusal above is about the value and not about the path.
    for (value, expected) in [("0", 0), (&MAX_CACHE_TTL_SECONDS.to_string()[..], 3600), ("300", 300)]
    {
        let case = format!("replace /*/*/caching/ttlInSeconds with {value:?}");
        let answer = update_stage(&gateway, &api, replace("/*/*/caching/ttlInSeconds", value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        let actual = &read.body["methodSettings"]["*/*"]["cacheTtlInSeconds"];
        assert_eq!(
            actual,
            &json!(expected),
            "case {case}: expected the stage to report {expected}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_method_logging_level_outside_its_documented_values_names_the_level() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "logging-level-sentence").await;
    let before = get_stage(&gateway, &api, STAGE).await;

    // Rows differ along the FORM of the rejected level — an unknown word, another unknown
    // word, a documented value in the wrong case, one carrying trailing space, one
    // carrying a numeric level, and an empty value — and each is named back exactly as the
    // request spelled it.
    let levels = ["NOT_A_LEVEL", "VERBOSE", "info", "OFF ", "3", ""];
    for scope in METHOD_SCOPES {
        for level in levels {
            let case = format!("replace {scope}/logging/loglevel with {level:?}");
            let answer = update_stage(
                &gateway,
                &api,
                replace(&format!("{scope}/logging/loglevel"), level),
            )
            .await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                &invalid_logging_level(level),
                &case,
            );
            let read = get_stage(&gateway, &api, STAGE).await;
            assert_eq!(
                read.body, before.body,
                "case {case}: expected the stage left at {:?}, got {:?}",
                before.body, read.body
            );
        }
    }

    // Every documented value of the setting ("Valid values are OFF, ERROR, and INFO",
    // `MethodSetting$loggingLevel`) still applies and reads back. The two that turn
    // execution logging ON need the account's CloudWatch Logs role, which this account is
    // given first, so the rows differ only in the level.
    patch_account(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some(&role_arn("apigw-logs-levels")),
    )
    .await;
    for level in ["OFF", "ERROR", "INFO"] {
        let case = format!("replace /*/*/logging/loglevel with {level:?}");
        let answer = update_stage(&gateway, &api, replace("/*/*/logging/loglevel", level)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        let actual = &read.body["methodSettings"]["*/*"]["loggingLevel"];
        assert_eq!(
            actual,
            &json!(level),
            "case {case}: expected the stage to report {level:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_quota_period_outside_the_modelled_set_earns_one_fixed_sentence() {
    let gateway = Gateway::new();
    let plan = create_usage_plan(
        &gateway,
        "CreateUsagePlan(quota period sentence)",
        json!({
            "name": "quota-period-sentence",
            "quota": { "limit": 1000, "offset": 0, "period": "DAY" },
        }),
    )
    .await;
    let plan = string_member(&plan, "id", "CreateUsagePlan(quota period sentence)");

    // Rows differ along the FORM of the rejected period — an unknown period, a declared
    // value in the wrong case, one carrying trailing space, a near-miss word, and an empty
    // value — and the sentence is byte-for-byte the same for each: it quotes neither the
    // value nor the path that carried it.
    for period in ["YEAR", "day", "MONTH ", "MONTHLY", ""] {
        let case = format!("replace /quota/period with {period:?}");
        let answer = update_usage_plan(&gateway, &plan, replace("/quota/period", period)).await;
        assert_refusal(&answer, BAD_REQUEST, INVALID_QUOTA_PERIOD, &case);
        // The refused patch wrote nothing: the plan still meters by the period it had.
        let read = get_usage_plan(&gateway, &plan).await;
        assert_eq!(
            read.body["quota"]["period"].as_str(),
            Some("DAY"),
            "case {case}: expected the stored period untouched, got {:?}",
            read.body["quota"]
        );
    }

    // Every value the modelled enum declares still applies and reads back, so the refusals
    // above are about the values outside the set.
    for period in QUOTA_PERIOD_TYPE_VALUES {
        let case = format!("replace /quota/period with {period:?}");
        let answer = update_usage_plan(&gateway, &plan, replace("/quota/period", period)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_usage_plan(&gateway, &plan).await;
        assert_eq!(
            read.body["quota"]["period"].as_str(),
            Some(*period),
            "case {case}: expected the plan to report {period:?}, got {:?}",
            read.body["quota"]
        );
    }
}

#[tokio::test]
async fn a_domain_name_arn_this_service_cannot_read_earns_one_fixed_sentence() {
    let gateway = Gateway::new();

    // Rows differ along the FORM of the unreadable value: no ARN at all, a bare domain
    // name, an ARN of too few fields, an ARN of another vendor, an API Gateway ARN of
    // another kind of resource, an API Gateway ARN whose resource portion names no
    // resource, and an access-association ARN whose resource portion is a domain name
    // reference rather than an identifier this service minted.
    let unreadable: Vec<String> = vec![
        "not-a-valid-arn".to_string(),
        "api.example.com".to_string(),
        "arn:aws:apigateway".to_string(),
        "arn:aws:s3:::my-bucket".to_string(),
        apigateway_arn("/restapis/abcde12345"),
        apigateway_arn("/"),
        apigateway_arn(
            "/domainnameaccessassociations/domainname/absent.example.com+abcd12/vpcesource/vpce-0123456789abcdef0",
        ),
        String::new(),
    ];

    // …and along the MEMBER the value rides on: each of the two ARNs
    // `RejectDomainNameAccessAssociation` requires, and the `domainNameArn`
    // `CreateDomainNameAccessAssociation` takes.
    let readable_association = apigateway_arn("/domainnameaccessassociations/dnaa1a2b3c");
    let readable_domain = apigateway_arn("/domainnames/api.example.com");
    for value in &unreadable {
        let on_association = reject(&gateway, value, &readable_domain).await;
        assert_refusal(
            &on_association,
            BAD_REQUEST,
            INVALID_DOMAIN_NAME_ARN,
            &format!("RejectDomainNameAccessAssociation(domainNameAccessAssociationArn={value:?})"),
        );
        let on_domain = reject(&gateway, &readable_association, value).await;
        assert_refusal(
            &on_domain,
            BAD_REQUEST,
            INVALID_DOMAIN_NAME_ARN,
            &format!("RejectDomainNameAccessAssociation(domainNameArn={value:?})"),
        );
        let on_create =
            create_association(&gateway, association_body(value, "vpce-0123456789abcdef0")).await;
        assert_refusal(
            &on_create,
            BAD_REQUEST,
            INVALID_DOMAIN_NAME_ARN,
            &format!("CreateDomainNameAccessAssociation(domainNameArn={value:?})"),
        );
    }

    // A READABLE pair is a different answer: nothing holds either record, which is a
    // resource that is not there and not a request to fix.
    let absent = reject(&gateway, &readable_association, &readable_domain).await;
    assert_refusal(
        &absent,
        NOT_FOUND,
        NO_SUCH_ASSOCIATION,
        "RejectDomainNameAccessAssociation(a readable pair naming no record)",
    );

    // And a readable `domainNameArn` of a STORED private custom domain name is served, so
    // the refusals above are about the values and not about the calls.
    let stored = private_domain(&gateway, "readable.example.com").await;
    let created =
        create_association(&gateway, association_body(&stored, "vpce-0123456789abcdef0")).await;
    assert_eq!(
        created.status, 201,
        "case CreateDomainNameAccessAssociation(a stored private domain name): expected 201, \
         got {} with {:?}",
        created.status, created.body
    );
}

/// `RejectDomainNameAccessAssociation` for the pair `(association, domain_name)`, both
/// carried in the query string the way the model binds them.
async fn reject(gateway: &Gateway, association: &str, domain_name: &str) -> Answer {
    gateway
        .call(
            "POST",
            &format!(
                "/rejectdomainnameaccessassociations\
                 ?domainNameAccessAssociationArn={}&domainNameArn={}",
                encode_label(association),
                encode_label(domain_name)
            ),
            None,
        )
        .await
}
