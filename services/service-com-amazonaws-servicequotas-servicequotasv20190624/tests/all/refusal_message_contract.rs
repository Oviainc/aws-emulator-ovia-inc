// SPDX-License-Identifier: CC0-1.0

//! The WORDS four more of Service Quotas' refusals are stated in, and WHICH validator
//! states them, over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine. The harness (`emulator`, `client`,
//! `Client`) is `quota_workflow.rs`'s, and `refusal_wording.rs` spans the sentences for
//! the `RequestId` lookup and the quota-value bounds.
//!
//! The rules asserted, each over a TABLE of inputs rather than one recorded call:
//!
//! * a refusal states the service's own sentence for its error class — FIXED text that
//!   interpolates nothing from the request and nothing this service minted — so two
//!   calls that fail the same way read back the same string. That is spanned here for
//!   three facts: a `NextToken` that names no position (on EVERY paginated listing and
//!   the utilization report), a quota AWS publishes as not adjustable, and a quota plane
//!   that already has a request awaiting a decision;
//! * the ONE refusal of this group whose sentence has variable parts is an Automatic
//!   Management exclusion naming a quota a published service does not hold: it is a bad
//!   ARGUMENT in the map the caller sent, so it is the modelled
//!   `IllegalArgumentException` and names the entry — the service code and the limit
//!   code — the caller has to correct;
//! * a value outside an ENUMERATED type's value set is a malformed request, refused by
//!   the request-validation front end as `ValidationException` whichever member carries
//!   it, with AWS's `N validation error(s) detected: ` count envelope around the clause
//!   naming the member as the wire spells it and the value set it admits. The count
//!   follows the number of clauses, so the single- and multi-violation forms stay
//!   distinct.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    APPLIED_LEVEL_ENUM_VALUES, OPT_IN_LEVEL_VALUES, OPT_IN_TYPE_VALUES, REQUEST_STATUS_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::{
    excluded_quota_absent_message, INVALID_PAGINATION_TOKEN_MESSAGE, ONE_OPEN_REQUEST_MESSAGE,
    QUOTA_NOT_ADJUSTABLE_MESSAGE,
};

use crate::quota_workflow::{base64_of, client, default_quotas, emulator, service_inventory, Client};

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// The history listing, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The sentence a `NextToken` that names no position is refused with, as recorded.
/// Spelled here so the suite pins the WORDING as well as its single sourcing: comparing
/// a refusal against the crate's constant alone would stay green if that constant's own
/// sentence drifted.
const PAGINATION_WORDING: &str = "You specified an invalid value for nextToken. You must get \
                                  the value from the response to the previous API call.";

/// The sentence a quota AWS publishes as not adjustable is refused with, as recorded.
const NOT_ADJUSTABLE_WORDING: &str =
    "The request failed because the specified quota is not adjustable.";

/// The sentence a second request on one quota plane is refused with, as recorded.
const ONE_OPEN_REQUEST_WORDING: &str =
    "Only one open service quota increase request is allowed per quota.";

/// The status, error code and message of one call: `auto_management.rs`'s reader, which
/// already reports all three and reads the message member either validator wrote.
use crate::auto_management::refused as answer;

/// One quota of the catalog as the tables below name one: `(ServiceCode, QuotaCode, its
/// AWS default value)`.
type Quota = (String, String, f64);

/// `wanted` quotas the catalog publishes with `adjustable` as their `Adjustable`, each
/// from a different service where the catalog has one, read over the wire (`ListServices`
/// + `ListAWSDefaultServiceQuotas`) so the rows span quotas no issue cited.
///
/// A quota is kept only when the account-wide read of it is served, because a quota that
/// is adjustable for an individual resource only has no account plane for a write to land
/// on and is refused for that instead of for the fact under test.
async fn quotas_with_adjustable(client: &Client, adjustable: bool, wanted: usize) -> Vec<Quota> {
    let mut picked: Vec<Quota> = Vec::new();
    for service in service_inventory(client).await {
        if picked.len() == wanted {
            break;
        }
        let Some(service_code) = service.get("ServiceCode").and_then(Value::as_str) else {
            continue;
        };
        for quota in default_quotas(client, service_code).await {
            if picked.len() == wanted {
                break;
            }
            let (Some(quota_code), Some(value)) = (
                quota.get("QuotaCode").and_then(Value::as_str),
                quota.get("Value").and_then(Value::as_f64),
            ) else {
                continue;
            };
            if quota.get("Adjustable") != Some(&json!(adjustable)) {
                continue;
            }
            let (status, _) = client
                .call(
                    "GetServiceQuota",
                    &json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            if status == 200 {
                picked.push((service_code.to_string(), quota_code.to_string(), value));
                // One quota per service, so the rows span the catalog rather than one
                // service's listing.
                break;
            }
        }
    }
    assert!(
        picked.len() == wanted,
        "case setup: expected {wanted} catalog quotas with Adjustable={adjustable} and an \
         account-wide plane, got {picked:?}"
    );
    picked
}

/// A `NextToken` that names no position in the listing it is presented to is refused with
/// the service's ONE fixed sentence for that fact, by every listing that pages — and by
/// the utilization report, which pages the same way.
#[tokio::test]
async fn an_unusable_pagination_token_states_one_fixed_sentence_on_every_listing() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // One quota of the catalog, read over the wire, so the listings that are scoped to a
    // service and a quota are driven against names this service really publishes.
    let (service_code, quota_code, _) = quotas_with_adjustable(&caller, true, 1).await[0].clone();
    // A report of this Region, so the report row resolves its `ReportId` and the token is
    // what it has to refuse.
    let started = caller
        .ok("StartQuotaUtilizationReport", json!({}))
        .await;
    let report_id = started["ReportId"]
        .as_str()
        .unwrap_or_else(|| panic!("case setup: expected a started report to carry a ReportId, got {started}"))
        .to_string();

    // Every operation of this service that carries a `NextToken`, each with a request
    // that is otherwise well formed.
    let listings: Vec<(&str, Value)> = vec![
        ("ListServices", json!({})),
        (
            "ListServiceQuotas",
            json!({"ServiceCode": service_code.clone()}),
        ),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": service_code.clone()}),
        ),
        ("ListRequestedServiceQuotaChangeHistory", json!({})),
        (
            HISTORY,
            json!({"ServiceCode": service_code.clone(), "QuotaCode": quota_code.clone()}),
        ),
        (
            "GetQuotaUtilizationReport",
            json!({"ReportId": report_id.clone()}),
        ),
    ];

    // The tokens differ in FORM while all satisfying the member's own
    // `@length 0..2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$`
    // (`smithy_query(query="shape", shape="NextToken")`), so each one is refused on
    // RESOLUTION rather than on syntax: base64 of text this service never issued, a
    // padded run of one letter, a token naming a listing that does not exist, and the
    // longest token the member admits.
    let padded = format!("{}==", "A".repeat(40));
    let at_the_maximum = "Z".repeat(2_048);
    // Built the way this service builds one, for a listing label nothing serves.
    let other_listing = base64_of("NoSuchListing:0");
    let tokens = [
        ("base64 of text no listing issued", "QUJDREVGRw=="),
        ("a padded run of one letter", padded.as_str()),
        (
            "a token naming a listing that does not exist",
            other_listing.as_str(),
        ),
        ("a token at the length maximum", at_the_maximum.as_str()),
    ];

    for (operation, base) in &listings {
        for (why, token) in &tokens {
            let case = format!("{operation} with {why}");
            let mut body = base.clone();
            body["NextToken"] = json!(token);
            let (status, code, message) = answer(&caller, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "InvalidPaginationTokenException"),
                "case {case}: expected HTTP {CLIENT_FAULT} InvalidPaginationTokenException, \
                 got {status} {code} with {message:?}"
            );
            assert_eq!(
                message, INVALID_PAGINATION_TOKEN_MESSAGE,
                "case {case}: expected the crate's one sentence for the fact, got {message:?}"
            );
            assert_eq!(
                message, PAGINATION_WORDING,
                "case {case}: expected the sentence {PAGINATION_WORDING:?}, got {message:?}"
            );
            assert!(
                !message.contains(*token) && !message.contains(operation),
                "case {case}: the refusal must carry nothing of the request, got {message:?}"
            );
        }
    }
}

/// A quota AWS publishes as not adjustable is refused with the service's ONE fixed
/// sentence for that fact, whichever quota of whichever service was named.
#[tokio::test]
async fn a_quota_that_is_not_adjustable_states_one_fixed_sentence() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    // Three quotas AWS publishes as not adjustable, from three different services, read
    // out of the catalog: the sentence must not follow the quota.
    let quotas = quotas_with_adjustable(&caller, false, 3).await;
    let mut stated: Vec<String> = Vec::new();

    for (service_code, quota_code, value) in &quotas {
        // The desired values differ along how far above the quota's value they sit, so
        // the refusal is seen to be about the quota rather than about the value.
        for (why, desired) in [
            ("one above the value in force", value + 1.0),
            ("far above the value in force", value * 4.0 + 100.0),
        ] {
            let case = format!("{service_code}/{quota_code}, {why} ({desired})");
            let (status, code, message) = answer(
                &caller,
                "RequestServiceQuotaIncrease",
                &json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "IllegalArgumentException"),
                "case {case}: expected HTTP {CLIENT_FAULT} IllegalArgumentException, got \
                 {status} {code} with {message:?}"
            );
            assert_eq!(
                message, QUOTA_NOT_ADJUSTABLE_MESSAGE,
                "case {case}: expected the crate's one sentence for the fact, got {message:?}"
            );
            assert_eq!(
                message, NOT_ADJUSTABLE_WORDING,
                "case {case}: expected the sentence {NOT_ADJUSTABLE_WORDING:?}, got {message:?}"
            );
            assert!(
                !message.contains(quota_code.as_str())
                    && !message.contains(service_code.as_str())
                    && !message.contains(&format!("{desired}")),
                "case {case}: the refusal must carry nothing of the request, got {message:?}"
            );
            stated.push(message);
        }
    }

    // Six calls over three quotas and two values: ONE sentence.
    stated.sort();
    stated.dedup();
    assert_eq!(
        stated.len(),
        1,
        "case the table as a whole: expected one sentence for the fact, got {stated:?}"
    );
}

/// A quota plane that already has a request awaiting a decision refuses a second one with
/// the service's ONE fixed sentence for that fact — which names neither the quota nor the
/// id of the request this service minted for the first call.
#[tokio::test]
async fn a_second_open_request_on_a_plane_states_one_fixed_sentence() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    // Two adjustable quotas, from two services, so the sentence is seen not to follow
    // the quota — and each is asked for twice over, with a second desired value that is
    // itself a valid increase, so what is refused is the OPEN REQUEST and not the value.
    for (service_code, quota_code, value) in quotas_with_adjustable(&caller, true, 2).await {
        let first = caller
            .ok(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": value + 1.0,
                }),
            )
            .await;
        let minted = first["RequestedQuota"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {service_code}/{quota_code}: expected an Id, got {first}"))
            .to_string();

        for (why, desired) in [
            ("the same value again", value + 1.0),
            ("a higher value", value + 2.0),
        ] {
            let case = format!("{service_code}/{quota_code}, {why} ({desired})");
            let (status, code, message) = answer(
                &caller,
                "RequestServiceQuotaIncrease",
                &json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "ResourceAlreadyExistsException"),
                "case {case}: expected HTTP {CLIENT_FAULT} ResourceAlreadyExistsException, \
                 got {status} {code} with {message:?}"
            );
            assert_eq!(
                message, ONE_OPEN_REQUEST_MESSAGE,
                "case {case}: expected the crate's one sentence for the fact, got {message:?}"
            );
            assert_eq!(
                message, ONE_OPEN_REQUEST_WORDING,
                "case {case}: expected the sentence {ONE_OPEN_REQUEST_WORDING:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(minted.as_str()) && !message.contains(quota_code.as_str()),
                "case {case}: the refusal must name neither the open request {minted} nor \
                 the quota, got {message:?}"
            );
        }
    }
}

/// A value outside an ENUMERATED type's value set is refused by the request-validation
/// front end — `ValidationException`, inside AWS's count envelope, naming the member as
/// the wire spells it and the value set the model declares for it — whichever member of
/// whichever operation carries it.
#[tokio::test]
async fn an_enum_value_outside_its_set_is_refused_by_the_request_validation_front_end() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    let (service_code, quota_code, _) = quotas_with_adjustable(&caller, true, 1).await[0].clone();

    // The level a configuration is started on, so the UPDATE row has a running
    // configuration to be refused against: the model's own only member.
    let account_level = OPT_IN_LEVEL_VALUES[0];
    caller
        .ok(
            "StartAutoManagement",
            json!({"OptInLevel": account_level, "OptInType": OPT_IN_TYPE_VALUES[0]}),
        )
        .await;

    // Each row: why it is invalid, the operation, the request, the member the WIRE spells
    // in the clause, and the value set the MODEL declares for it — taken from the
    // generated value lists (`ops::types::*_VALUES`) rather than written out here, so a
    // member the model gains or loses is covered without editing this table. The rows
    // span four enumerated types over five operations, including two members of one
    // request and the two writers of one member.
    let rows: Vec<(&str, &str, Value, &str, &[&str])> = vec![
        (
            "an opt-in level outside its value set",
            "StartAutoManagement",
            json!({"OptInLevel": "REGION", "OptInType": OPT_IN_TYPE_VALUES[0]}),
            "optInLevel",
            OPT_IN_LEVEL_VALUES,
        ),
        (
            "an opt-in type outside its value set",
            "StartAutoManagement",
            json!({"OptInLevel": account_level, "OptInType": "NotifyLater"}),
            "optInType",
            OPT_IN_TYPE_VALUES,
        ),
        (
            "an opt-in type outside its value set, on the partial writer",
            "UpdateAutoManagement",
            json!({"OptInType": "NotifyLater"}),
            "optInType",
            OPT_IN_TYPE_VALUES,
        ),
        (
            "a request status outside its value set",
            HISTORY,
            json!({
                "ServiceCode": service_code.clone(),
                "QuotaCode": quota_code.clone(),
                "Status": "NOT_A_STATUS",
            }),
            "status",
            REQUEST_STATUS_VALUES,
        ),
        (
            "a request status outside its value set, on the account-wide history",
            "ListRequestedServiceQuotaChangeHistory",
            json!({"Status": "NOT_A_STATUS"}),
            "status",
            REQUEST_STATUS_VALUES,
        ),
        (
            "an applied level outside its value set",
            "ListServiceQuotas",
            json!({
                "ServiceCode": service_code.clone(),
                "QuotaAppliedAtLevel": "REGIONAL",
            }),
            "quotaAppliedAtLevel",
            APPLIED_LEVEL_ENUM_VALUES,
        ),
    ];

    for (why, operation, body, member, values) in rows {
        let case = format!("{why} ({operation})");
        let (status, code, message) = answer(&caller, operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_FAULT, "ValidationException"),
            "case {case}: expected HTTP {CLIENT_FAULT} ValidationException, got {status} \
             {code} with {message:?}"
        );
        let prefix = format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must satisfy enum value set: ["
        );
        assert!(
            message.starts_with(&prefix) && message.ends_with(']'),
            "case {case}: expected a message {prefix:?}…], got {message:?}"
        );
        // The value set the clause reports is the member's own, whatever order the model
        // declares its values in (an order neither the model nor a recording fixes).
        let mut reported: Vec<&str> = message[prefix.len()..message.len() - 1]
            .split(", ")
            .collect();
        let mut declared: Vec<&str> = values.to_vec();
        reported.sort_unstable();
        declared.sort_unstable();
        assert_eq!(
            reported, declared,
            "case {case}: expected the model's value set {declared:?}, got {reported:?} in \
             {message:?}"
        );
    }

    // The count follows the number of clauses, so a request that breaks TWO rules is
    // reported in the plural form with both clauses — which is what makes the envelope
    // worth reading. `MaxResults` is `@range 1..100`, so 0 breaks it.
    let (status, code, message) = answer(
        &caller,
        HISTORY,
        &json!({
            "ServiceCode": service_code,
            "QuotaCode": quota_code,
            "Status": "NOT_A_STATUS",
            "MaxResults": 0,
        }),
    )
    .await;
    let case = "two violations in one request";
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_FAULT, "ValidationException"),
        "case {case}: expected HTTP {CLIENT_FAULT} ValidationException, got {status} {code} \
         with {message:?}"
    );
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case}: expected the plural count envelope, got {message:?}"
    );
    for clause in [
        "Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [",
        "Value at 'maxResults' failed to satisfy constraint: Member must have value greater \
         than or equal to 1",
    ] {
        assert!(
            message.contains(clause),
            "case {case}: expected the envelope to carry {clause:?}, got {message:?}"
        );
    }
}

/// An Automatic Management exclusion naming a quota a published service does not hold is
/// a bad ARGUMENT in the map the caller sent: the modelled `IllegalArgumentException`,
/// naming the service code and the limit code of the entry that resolved to nothing — the
/// one refusal of this suite whose sentence has variable parts, and they come from the
/// request because the entry is what the caller has to correct.
#[tokio::test]
async fn an_exclusion_naming_no_quota_is_a_bad_argument_naming_the_entry() {
    let engine = emulator();
    let caller = client(&engine, "ap-southeast-2");

    // Three services the catalog publishes, read over the wire, each named with a limit
    // code it holds no quota under: the entries differ in the service AND in the code, so
    // the two variable parts are seen to follow the request.
    let services: Vec<String> = quotas_with_adjustable(&caller, true, 3)
        .await
        .into_iter()
        .map(|(service_code, _, _)| service_code)
        .collect();
    // The codes differ in FORM while satisfying the map's own
    // `@pattern ^[A-Za-z0-9-_ /]{1,128}$` (`smithy_query(query="shape",
    // shape="ExclusionList")`), so each is refused on the LOOKUP rather than on syntax.
    let codes = ["L-00000000", "NotAQuotaCode", "L-FFFFFFFF"];
    let mut stated: Vec<String> = Vec::new();

    for (service_code, limit_code) in services.iter().zip(codes) {
        let case = format!("{service_code} excluding {limit_code}");
        let request = json!({
            "OptInLevel": OPT_IN_LEVEL_VALUES[0],
            "OptInType": OPT_IN_TYPE_VALUES[0],
            "ExclusionList": { service_code.as_str(): [limit_code] },
        });
        let (status, code, message) = answer(&caller, "StartAutoManagement", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_FAULT, "IllegalArgumentException"),
            "case {case}: expected HTTP {CLIENT_FAULT} IllegalArgumentException, got {status} \
             {code} with {message:?}"
        );
        let expected = format!("Service code '{service_code}' with limit code '{limit_code}' does not exist");
        assert_eq!(
            message, expected,
            "case {case}: expected the refusal to state {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            excluded_quota_absent_message(service_code, limit_code),
            "case {case}: expected the crate's one sentence for the fact, got {message:?}"
        );

        // The rejection is side-effect-free: the account still has no configuration.
        let read = caller.ok("GetAutoManagementConfiguration", json!({})).await;
        assert_eq!(
            read.get("ExclusionList"),
            None,
            "case {case}: a refused start must record no exclusion, got {read}"
        );
        stated.push(message);
    }

    // Three entries, three sentences: the two variable parts are the entry's, not a
    // constant.
    stated.sort();
    stated.dedup();
    assert_eq!(
        stated.len(),
        services.len(),
        "case the table as a whole: expected one sentence per entry, got {stated:?}"
    );
}
