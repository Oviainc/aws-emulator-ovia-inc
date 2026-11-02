// SPDX-License-Identifier: CC0-1.0

//! One tenant's suppression-list preferences, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: a write of one tenant's `TenantSuppressionAttributes`
//! REPLACES both stored members with the ones the request carries, so a member the
//! request leaves out takes its documented default (`ACCOUNT` scope, and no reason at
//! all) rather than keeping what was there; a read of that tenant then reports exactly
//! the stored pair, and a request naming a value outside a member's modelled value set,
//! a `TenantName` outside the documented form, or a tenant the account does not hold is
//! refused with the modelled error for that class and stores nothing.
//!
//! The tables below span what the rule generalises over: how many reasons the request
//! names (none, one, every one, the same one twice), which ones and in which order, each
//! `SuppressionListScope` the model publishes, whether the scope is named at all, the
//! form of the tenant name (shortest allowed, at the documented bound), the write that
//! carries the preferences (the tenant's create and the preferences put), the sequence of
//! writes over one tenant, and each refusal leg.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    SUPPRESSION_LIST_REASON_VALUES, SUPPRESSION_LIST_SCOPE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_TENANT_NAME_CHARS;

use crate::configuration_sets::{Reply, Ses};

/// `CreateTenant`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="CreateTenant")`: `POST /v2/email/tenants`).
const CREATE_TENANT_PATH: &str = "/v2/email/tenants";

/// `GetTenant`'s route, which reads the stored preferences back
/// (`POST /v2/email/tenants/get`).
const GET_TENANT_PATH: &str = "/v2/email/tenants/get";

/// `PutTenantSuppressionAttributes`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="PutTenantSuppressionAttributes")`:
/// `POST /v2/email/tenant/suppression`).
const PUT_SUPPRESSION_PATH: &str = "/v2/email/tenant/suppression";

async fn create_tenant(ses: &Ses, body: Value) -> Reply {
    ses.call("POST", CREATE_TENANT_PATH, Some(body)).await
}

async fn get_tenant(ses: &Ses, name: &str) -> Reply {
    ses.call(
        "POST",
        GET_TENANT_PATH,
        Some(json!({"TenantName": name})),
    )
    .await
}

async fn put_suppression(ses: &Ses, body: Value) -> Reply {
    ses.call("POST", PUT_SUPPRESSION_PATH, Some(body)).await
}

/// The `SuppressionAttributes` the named tenant reports, or `Value::Null` when the read
/// reported no tenant at all.
async fn reported_attributes(ses: &Ses, name: &str) -> Value {
    get_tenant(ses, name)
        .await
        .member("Tenant")
        .get("SuppressionAttributes")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `TenantSuppressionAttributes` body a stored pair is reported as: the scope is
/// always present, and the reasons only when the tenant adds something automatically.
fn attributes(scope: &str, reasons: &[&str]) -> Value {
    let mut value = json!({"SuppressionScope": scope});
    if !reasons.is_empty() {
        value["SuppressedReasons"] = json!(reasons);
    }
    value
}

/// One accepted write of a tenant's preferences: the members the request carries and the
/// pair a read of the tenant must report afterwards.
struct Case {
    /// What this row varies, named in every assertion.
    case: String,
    /// The `TenantName` the row creates and writes.
    tenant: String,
    /// The `SuppressedReasons` the request carries, when it carries the member.
    reasons: Option<Vec<&'static str>>,
    /// The `SuppressionScope` the request carries, when it carries the member.
    scope: Option<&'static str>,
    /// The `TenantSuppressionAttributes` the stored tenant must report.
    expected: Value,
}

/// The accepted-write table, built from the model's own value sets so every published
/// value is covered rather than only the ones a caller happened to use.
fn cases() -> Vec<Case> {
    let every: Vec<&str> = SUPPRESSION_LIST_REASON_VALUES.to_vec();
    let reversed: Vec<&str> = SUPPRESSION_LIST_REASON_VALUES
        .iter()
        .rev()
        .copied()
        .collect();

    let mut cases = vec![
        Case {
            // `TenantSuppressionAttributes$SuppressionScope`: "if you don't specify a
            // suppression scope, the tenant defaults to ACCOUNT scope and uses the
            // account-level suppression list".
            case: "neither member carried".to_string(),
            tenant: "defaults".to_string(),
            reasons: None,
            scope: None,
            expected: attributes("ACCOUNT", &[]),
        },
        Case {
            case: "an empty reason list and no scope".to_string(),
            tenant: "a".to_string(),
            reasons: Some(Vec::new()),
            scope: None,
            expected: attributes("ACCOUNT", &[]),
        },
        Case {
            case: "tenant scope with no reason named".to_string(),
            tenant: format!("t{}", "x".repeat(MAX_TENANT_NAME_CHARS - 1)),
            reasons: None,
            scope: Some("TENANT"),
            expected: attributes("TENANT", &[]),
        },
        Case {
            case: "reasons without a scope, which keeps the documented default"
                .to_string(),
            tenant: "reasons_only".to_string(),
            reasons: Some(every.clone()),
            scope: None,
            expected: attributes("ACCOUNT", SUPPRESSION_LIST_REASON_VALUES),
        },
        Case {
            case: "every reason, reversed, which is the order it is reported in"
                .to_string(),
            tenant: "reversed-reasons".to_string(),
            reasons: Some(reversed.clone()),
            scope: Some("TENANT"),
            expected: attributes("TENANT", &reversed),
        },
        Case {
            // `SuppressionListReasons` is a list, not a set: the model binds no
            // `uniqueItems` on it
            // (`smithy_query(query="shape",shape="SuppressionListReasons")`), so a
            // repeated reason is stored as supplied.
            case: "the same reason twice".to_string(),
            tenant: "repeated-reason".to_string(),
            reasons: Some(vec!["BOUNCE", "BOUNCE"]),
            scope: Some("TENANT"),
            expected: attributes("TENANT", &["BOUNCE", "BOUNCE"]),
        },
    ];

    // One row per `SuppressionListReason` the model publishes, each on tenant scope.
    for reason in SUPPRESSION_LIST_REASON_VALUES.iter().copied() {
        cases.push(Case {
            case: format!("only {reason}"),
            tenant: format!("only-{}", reason.to_lowercase()),
            reasons: Some(vec![reason]),
            scope: Some("TENANT"),
            expected: attributes("TENANT", &[reason]),
        });
    }

    // One row per `SuppressionListScope` the model publishes, each naming every reason,
    // so the scope the request states is the one stored whatever the reasons are.
    for scope in SUPPRESSION_LIST_SCOPE_VALUES.iter().copied() {
        cases.push(Case {
            case: format!("{scope} scope with every reason"),
            tenant: format!("scope-{}", scope.to_lowercase()),
            reasons: Some(every.clone()),
            scope: Some(scope),
            expected: attributes(scope, SUPPRESSION_LIST_REASON_VALUES),
        });
    }

    cases
}

/// The `PutTenantSuppressionAttributes` body one row sends.
fn put_body(case: &Case) -> Value {
    let mut body = json!({"TenantName": case.tenant});
    if let Some(reasons) = &case.reasons {
        body["SuppressedReasons"] = json!(reasons);
    }
    if let Some(scope) = case.scope {
        body["SuppressionScope"] = json!(scope);
    }
    body
}

/// The `CreateTenant` body carrying the same preferences in its
/// `SuppressionAttributes`, so the row is driven through the other write of this pair
/// too.
fn create_body(case: &Case) -> Value {
    let mut suppression = json!({});
    if let Some(reasons) = &case.reasons {
        suppression["SuppressedReasons"] = json!(reasons);
    }
    if let Some(scope) = case.scope {
        suppression["SuppressionScope"] = json!(scope);
    }
    json!({"TenantName": case.tenant, "SuppressionAttributes": suppression})
}

#[tokio::test]
async fn a_preferences_put_persists_what_the_request_carries() {
    for case in cases() {
        let case_name = case.case.clone();
        // A fresh service per row, so each row is answered from the preferences it
        // wrote and nothing else.
        let ses = Ses::new();
        let created = create_tenant(&ses, json!({"TenantName": case.tenant})).await;
        assert_eq!(
            created.status, 200,
            "case {case_name:?}: expected 200 from CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );

        let put = put_suppression(&ses, put_body(&case)).await;
        assert_eq!(
            put.status, 200,
            "case {case_name:?}: expected 200 from the preferences put, got {} {:?}",
            put.status,
            put.message()
        );
        assert!(
            put.is_empty_body(),
            "case {case_name:?}: PutTenantSuppressionAttributesResponse models no \
             members, so the body must be empty, got {:?}",
            put.member("SuppressionAttributes")
        );

        let reported = reported_attributes(&ses, &case.tenant).await;
        assert_eq!(
            reported, case.expected,
            "case {case_name:?}: expected the tenant to report {:?}, got {reported:?}",
            case.expected
        );

        // Reading the tenant does not change what a later caller sees.
        let again = reported_attributes(&ses, &case.tenant).await;
        assert_eq!(
            again, case.expected,
            "case {case_name:?}: expected a second read to report {:?}, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn the_tenants_create_resolves_the_same_preferences_as_the_put() {
    for case in cases() {
        let case_name = case.case.clone();
        let ses = Ses::new();
        let created = create_tenant(&ses, create_body(&case)).await;
        assert_eq!(
            created.status, 200,
            "case {case_name:?}: expected 200 from CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );
        assert_eq!(
            created.member("SuppressionAttributes"),
            case.expected,
            "case {case_name:?}: expected CreateTenant to report {:?}, got {:?}",
            case.expected,
            created.member("SuppressionAttributes")
        );

        let reported = reported_attributes(&ses, &case.tenant).await;
        assert_eq!(
            reported, case.expected,
            "case {case_name:?}: expected the created tenant to report {:?}, got \
             {reported:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn each_put_replaces_the_previous_preferences() {
    let ses = Ses::new();
    let tenant = "marketing";
    let created = create_tenant(
        &ses,
        json!({
            "TenantName": tenant,
            "SuppressionAttributes": {"SuppressionScope": "TENANT", "SuppressedReasons": ["BOUNCE"]}
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"create\": expected 200 from CreateTenant, got {} {:?}",
        created.status,
        created.message()
    );

    // Each step replaces the whole pair, so the preferences after it are the ones that
    // step carried — never the union with an earlier step's.
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "widen to every reason on the tenant's own list",
            json!({
                "TenantName": tenant,
                "SuppressionScope": "TENANT",
                "SuppressedReasons": SUPPRESSION_LIST_REASON_VALUES,
            }),
            attributes("TENANT", SUPPRESSION_LIST_REASON_VALUES),
        ),
        (
            "narrow to one reason",
            json!({
                "TenantName": tenant,
                "SuppressionScope": "TENANT",
                "SuppressedReasons": ["COMPLAINT"],
            }),
            attributes("TENANT", &["COMPLAINT"]),
        ),
        (
            "back to the account-level list, keeping the reasons",
            json!({
                "TenantName": tenant,
                "SuppressionScope": "ACCOUNT",
                "SuppressedReasons": ["COMPLAINT"],
            }),
            attributes("ACCOUNT", &["COMPLAINT"]),
        ),
        (
            "the scope alone, which clears the reasons the previous step stored",
            json!({"TenantName": tenant, "SuppressionScope": "TENANT"}),
            attributes("TENANT", &[]),
        ),
        (
            "neither member, which returns the tenant to the documented default",
            json!({"TenantName": tenant}),
            attributes("ACCOUNT", &[]),
        ),
    ];

    for (case, body, expected) in steps {
        let put = put_suppression(&ses, body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected 200 from the preferences put, got {} {:?}",
            put.status,
            put.message()
        );
        let reported = reported_attributes(&ses, tenant).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tenant to report {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_put_touches_only_the_tenant_it_names() {
    let ses = Ses::new();
    for tenant in ["first", "second"] {
        let created = create_tenant(&ses, json!({"TenantName": tenant})).await;
        assert_eq!(
            created.status, 200,
            "case {tenant:?}: expected 200 from CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );
    }

    let put = put_suppression(
        &ses,
        json!({
            "TenantName": "first",
            "SuppressionScope": "TENANT",
            "SuppressedReasons": ["BOUNCE"],
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"first\": expected 200 from the preferences put, got {} {:?}",
        put.status,
        put.message()
    );

    for (case, tenant, expected) in [
        (
            "the tenant the put named",
            "first",
            attributes("TENANT", &["BOUNCE"]),
        ),
        (
            "a tenant the put did not name",
            "second",
            attributes("ACCOUNT", &[]),
        ),
    ] {
        let reported = reported_attributes(&ses, tenant).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {tenant:?} to report {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_its_modelled_set_is_refused_and_changes_nothing() {
    let tenant = "strict";
    // Each row carries a value no member's value set holds: an unknown reason, a reason
    // differing only in case, a good reason beside an unknown one, an unknown scope and
    // a scope differing only in case.
    let rows: Vec<(&str, Value)> = vec![
        ("an unknown reason", json!({"SuppressedReasons": ["SPAM"]})),
        (
            "a reason differing only in case",
            json!({"SuppressedReasons": ["bounce"]}),
        ),
        (
            "one good reason and one unknown",
            json!({"SuppressedReasons": ["BOUNCE", "UNSUBSCRIBE"]}),
        ),
        ("an unknown scope", json!({"SuppressionScope": "GLOBAL"})),
        (
            "a scope differing only in case",
            json!({"SuppressionScope": "tenant"}),
        ),
    ];

    for (case, members) in rows {
        let ses = Ses::new();
        let created = create_tenant(
            &ses,
            json!({
                "TenantName": tenant,
                "SuppressionAttributes": {
                    "SuppressionScope": "TENANT",
                    "SuppressedReasons": ["BOUNCE"],
                }
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );
        let before = reported_attributes(&ses, tenant).await;

        let mut body = json!({"TenantName": tenant});
        for (member, value) in members.as_object().expect("object body") {
            body[member] = value.clone();
        }
        let refused = put_suppression(&ses, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from the preferences put, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} {:?}",
            refused.error_code(),
            refused.message()
        );

        // The refused request stored nothing: the tenant still reports the pair it
        // held before it.
        let after = reported_attributes(&ses, tenant).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused put must leave the stored preferences at \
             {before:?}, got {after:?}"
        );

        // The other write of this pair refuses the same value on the same check, and
        // creates no tenant for it.
        let ses = Ses::new();
        let refused = create_tenant(
            &ses,
            json!({"TenantName": tenant, "SuppressionAttributes": members}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from CreateTenant, got {} {:?}",
            refused.status,
            refused.message()
        );
        let read = get_tenant(&ses, tenant).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store no tenant, got {} {:?}",
            read.status,
            read.member("Tenant")
        );
    }
}

#[tokio::test]
async fn a_tenant_the_account_cannot_reach_is_refused() {
    let ses = Ses::new();
    let created = create_tenant(&ses, json!({"TenantName": "Mixed_Case"})).await;
    assert_eq!(
        created.status, 200,
        "case \"create\": expected 200 from CreateTenant, got {} {:?}",
        created.status,
        created.message()
    );

    // A tenant whose name is well formed but which the account does not hold: the
    // modelled 404, worded as every operation that reads a tenant by name words it.
    for (case, name) in [
        ("a tenant nothing ever created", "never-created"),
        ("the same name in another case", "mixed_case"),
    ] {
        let refused = put_suppression(
            &ses,
            json!({"TenantName": name, "SuppressionScope": "TENANT"}),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404 from the preferences put, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?} {:?}",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            format!("The requested tenant <{name}> does not exist."),
            "case {case:?}: the refusal must name the tenant it did not find, got {:?}",
            refused.message()
        );
    }

    // A name outside the documented form is refused on the form, before anything is
    // looked up: one character past the documented bound, and each character class the
    // form excludes.
    for (case, name) in [
        (
            "one character past the documented bound",
            "x".repeat(MAX_TENANT_NAME_CHARS + 1),
        ),
        ("a space", "has space".to_string()),
        ("a dot", "dot.name".to_string()),
        ("a character outside the ASCII range", "ünïcode".to_string()),
    ] {
        let refused = put_suppression(
            &ses,
            json!({"TenantName": name, "SuppressionScope": "TENANT"}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from the preferences put, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} {:?}",
            refused.error_code(),
            refused.message()
        );
        assert!(
            refused.message().contains(&name),
            "case {case:?}: the refusal must name the tenant it refused, got {:?}",
            refused.message()
        );
    }
}
