// SPDX-License-Identifier: CC0-1.0

//! The Clean Rooms privacy budget template: create, read back, and the one-per-
//! collaboration rule.
//!
//! The rules under test:
//!
//! * a created privacy budget template is PERSISTED, with its collaboration-derived
//!   members (`membershipId`/`membershipArn`, `collaborationId`/`collaborationArn`)
//!   copied from the stored membership and the required `autoRefresh` materialized at
//!   the cadence in force, and `GetPrivacyBudgetTemplate` reports exactly that record;
//! * "each collaboration can have only one privacy budget template"
//!   (`CreatePrivacyBudgetTemplate`'s own doc), so a second create in the same
//!   collaboration is refused with `ConflictException` and changes nothing, while
//!   another collaboration still accepts its own;
//! * a request that names a resource the store does not hold, or a `privacyBudgetType`
//!   that does not match the `parameters` union it carries, is refused with the
//!   modelled error for that failure rather than accepted;
//! * the template store carries an emulator-enforced maximum entry count and the
//!   create refuses past it with the modelled `ServiceQuotaExceededException`, and the
//!   template does not outlive the collaboration it belongs to.
//!
//! The tables vary what those rules generalise over: both `PrivacyBudgetType` values,
//! each `PrivacyBudgetTemplateAutoRefresh` value plus absent, the published bounds of
//! the differential-privacy parameters, the `AccessBudgetType` catalog, the number of
//! budget parameters, and tags present or absent. Every request goes over the wire
//! through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, is_model_uuid, minimal_collaboration, Emulator, ALL_LOG_STATUSES,
    REGION,
};

/// Every `PrivacyBudgetType` the model declares (shape `PrivacyBudgetType`).
const ALL_PRIVACY_BUDGET_TYPES: [&str; 2] = ["DIFFERENTIAL_PRIVACY", "ACCESS_BUDGET"];

/// Every `PrivacyBudgetTemplateAutoRefresh` the model declares (shape
/// `PrivacyBudgetTemplateAutoRefresh`).
const ALL_AUTO_REFRESH: [&str; 2] = ["CALENDAR_MONTH", "NONE"];

/// Every `AccessBudgetType` the model declares (shape `AccessBudgetType`).
const ALL_ACCESS_BUDGET_TYPES: [&str; 4] =
    ["CALENDAR_DAY", "CALENDAR_MONTH", "CALENDAR_WEEK", "LIFETIME"];

/// Every `AutoRefreshMode` the model declares (shape `AutoRefreshMode`).
const ALL_AUTO_REFRESH_MODES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The published bounds of `DifferentialPrivacyTemplateParametersInput$epsilon`
/// (model shape `Epsilon`, range 1..20).
const EPSILON_MIN: i64 = 1;
const EPSILON_MAX: i64 = 20;

/// The published bounds of `DifferentialPrivacyTemplateParametersInput$usersNoisePerQuery`
/// (model shape `UsersNoisePerQuery`, range 10..100).
const USERS_NOISE_MIN: i64 = 10;
const USERS_NOISE_MAX: i64 = 100;

/// The published bounds of `BudgetParameter$budget` (model shape `Budget`, range
/// 0..1000000).
const BUDGET_MIN: i64 = 0;
const BUDGET_MAX: i64 = 1_000_000;

/// The cadence a template created without `autoRefresh` reports: the request member is
/// optional while `PrivacyBudgetTemplate$autoRefresh` is required, and CALENDAR_MONTH is
/// documented as an opt-in, so a template nobody asked to refresh does not refresh.
const DEFAULT_AUTO_REFRESH: &str = "NONE";

/// A membership id in the form the model's `MembershipIdentifier` pattern declares that
/// no create in these tests mints.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// A collaboration plus the caller's membership in it: `(collaboration id, membership)`.
async fn collaboration_with_membership(emulator: &Emulator, name: &str) -> (String, Value) {
    let (status, created) = emulator.create_collaboration(minimal_collaboration(name)).await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no collaboration.id in {created}"))
        .to_string();

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    (collaboration_id, joined["membership"].clone())
}

async fn create_template(emulator: &Emulator, membership_id: &str, body: Value) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/privacybudgettemplates"),
            Some(body),
        )
        .await
}

async fn get_template(emulator: &Emulator, membership_id: &str, template_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership_id}/privacybudgettemplates/{template_id}"),
            None,
        )
        .await
}

/// Differential-privacy parameters.
fn differential_privacy(epsilon: i64, users_noise_per_query: i64) -> Value {
    json!({
        "differentialPrivacy": {
            "epsilon": epsilon,
            "usersNoisePerQuery": users_noise_per_query,
        }
    })
}

/// Access-budget parameters for a configured table association of `membership_arn`,
/// in the form the model's `BudgetedResourceArn` pattern declares.
fn access_budget(membership_arn: &str, budget_parameters: Value) -> Value {
    json!({
        "accessBudget": {
            "budgetParameters": budget_parameters,
            "resourceArn": format!("{membership_arn}/configuredtableassociation/0a1b2c3d"),
        }
    })
}

/// One row of the create/read round-trip table.
struct TemplateCase {
    label: &'static str,
    privacy_budget_type: &'static str,
    /// The request's `autoRefresh`, if any.
    auto_refresh: Option<&'static str>,
    /// Builds the request's `parameters` from the membership ARN (the access-budget
    /// rows name a resource of that membership).
    parameters: fn(&str) -> Value,
    /// The request's `tags`, if any.
    tags: Option<fn() -> Value>,
}

fn template_cases() -> Vec<TemplateCase> {
    vec![
        TemplateCase {
            label: "differential privacy at the lower published bounds, no autoRefresh",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[0],
            auto_refresh: None,
            parameters: |_| differential_privacy(EPSILON_MIN, USERS_NOISE_MIN),
            tags: None,
        },
        TemplateCase {
            label: "differential privacy at the upper published bounds, CALENDAR_MONTH",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[0],
            auto_refresh: Some(ALL_AUTO_REFRESH[0]),
            parameters: |_| differential_privacy(EPSILON_MAX, USERS_NOISE_MAX),
            tags: None,
        },
        TemplateCase {
            label: "differential privacy mid-range, NONE, with tags",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[0],
            auto_refresh: Some(ALL_AUTO_REFRESH[1]),
            parameters: |_| differential_privacy(7, 42),
            tags: Some(|| json!({ "env": "test", "owner": "fixer" })),
        },
        TemplateCase {
            label: "access budget: one LIFETIME parameter at the budget minimum",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[1],
            auto_refresh: None,
            parameters: |membership_arn| {
                access_budget(
                    membership_arn,
                    json!([{ "type": ALL_ACCESS_BUDGET_TYPES[3], "budget": BUDGET_MIN }]),
                )
            },
            tags: None,
        },
        TemplateCase {
            label: "access budget: two parameters, both AutoRefreshMode values, budget maximum",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[1],
            auto_refresh: Some(ALL_AUTO_REFRESH[0]),
            parameters: |membership_arn| {
                access_budget(
                    membership_arn,
                    json!([
                        {
                            "type": ALL_ACCESS_BUDGET_TYPES[0],
                            "budget": BUDGET_MAX,
                            "autoRefresh": ALL_AUTO_REFRESH_MODES[0],
                        },
                        {
                            "type": ALL_ACCESS_BUDGET_TYPES[2],
                            "budget": 7,
                            "autoRefresh": ALL_AUTO_REFRESH_MODES[1],
                        },
                    ]),
                )
            },
            tags: Some(|| json!({ "team": "privacy" })),
        },
        TemplateCase {
            label: "access budget: one CALENDAR_MONTH parameter, NONE at the template level",
            privacy_budget_type: ALL_PRIVACY_BUDGET_TYPES[1],
            auto_refresh: Some(ALL_AUTO_REFRESH[1]),
            parameters: |membership_arn| {
                access_budget(
                    membership_arn,
                    json!([{
                        "type": ALL_ACCESS_BUDGET_TYPES[1],
                        "budget": 500,
                        "autoRefresh": ALL_AUTO_REFRESH_MODES[0],
                    }]),
                )
            },
            tags: None,
        },
    ]
}

/// The request body for one row.
fn request_body(case: &TemplateCase, membership_arn: &str) -> Value {
    let mut body = json!({
        "privacyBudgetType": case.privacy_budget_type,
        "parameters": (case.parameters)(membership_arn),
    });
    if let Some(auto_refresh) = case.auto_refresh {
        body["autoRefresh"] = json!(auto_refresh);
    }
    if let Some(tags) = case.tags {
        body["tags"] = tags();
    }
    body
}

#[tokio::test]
async fn create_privacy_budget_template_persists_and_get_returns_it_unchanged() {
    let emulator = Emulator::new();

    for (row, case) in template_cases().into_iter().enumerate() {
        let label = case.label;
        // Each row needs its own collaboration: a collaboration holds one template.
        let (collaboration_id, membership) =
            collaboration_with_membership(&emulator, &format!("pbt-{row}")).await;
        let membership_id = membership["id"].as_str().expect("membership.id").to_string();
        let membership_arn = membership["arn"].as_str().expect("membership.arn").to_string();
        let collaboration_arn = membership["collaborationArn"]
            .as_str()
            .expect("membership.collaborationArn")
            .to_string();

        let body = request_body(&case, &membership_arn);
        let (status, created) = create_template(&emulator, &membership_id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreatePrivacyBudgetTemplate 200, got {status} with {created}"
        );
        let template = created["privacyBudgetTemplate"].clone();
        let id = template["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no privacyBudgetTemplate.id in {created}"))
            .to_string();

        assert!(
            is_model_uuid(&id),
            "case {label:?}: expected a 36-char lowercase-hex UUID id, got {id:?}"
        );
        assert_eq!(
            template["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:cleanrooms:{REGION}:{}:privacybudgettemplate/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {label:?}: unexpected arn, got {:?}",
            template["arn"]
        );

        // The collaboration-derived members come from the stored membership.
        for (member, expected) in [
            ("membershipId", membership_id.as_str()),
            ("membershipArn", membership_arn.as_str()),
            ("collaborationId", collaboration_id.as_str()),
            ("collaborationArn", collaboration_arn.as_str()),
        ] {
            assert_eq!(
                template[member].as_str(),
                Some(expected),
                "case {label:?}: expected {member} {expected:?}, got {:?}",
                template[member]
            );
        }

        // What the caller chose is reported back; what it left out is materialized at
        // the cadence in force.
        assert_eq!(
            template["privacyBudgetType"].as_str(),
            Some(case.privacy_budget_type),
            "case {label:?}: expected privacyBudgetType {:?}, got {:?}",
            case.privacy_budget_type,
            template["privacyBudgetType"]
        );
        let expected_auto_refresh = case.auto_refresh.unwrap_or(DEFAULT_AUTO_REFRESH);
        assert_eq!(
            template["autoRefresh"].as_str(),
            Some(expected_auto_refresh),
            "case {label:?}: expected autoRefresh {expected_auto_refresh:?}, got {:?}",
            template["autoRefresh"]
        );
        // The output union declares the same members as the input union, so the stored
        // parameters are the ones the request carried.
        assert_eq!(
            template["parameters"], body["parameters"],
            "case {label:?}: expected parameters {}, got {}",
            body["parameters"], template["parameters"]
        );
        assert_eq!(
            template["createTime"], template["updateTime"],
            "case {label:?}: a freshly created template has createTime == updateTime, got {:?} and {:?}",
            template["createTime"], template["updateTime"]
        );

        // Persisted: the reader reports the identical record, twice over (a read
        // changes nothing a later caller sees).
        for attempt in 1..=2 {
            let (status, fetched) = get_template(&emulator, &membership_id, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: read {attempt}: expected GetPrivacyBudgetTemplate 200, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["privacyBudgetTemplate"], template,
                "case {label:?}: read {attempt}: the reader must report what was stored, got {:?}",
                fetched["privacyBudgetTemplate"]
            );
        }

        // `tags` is not a member of the `PrivacyBudgetTemplate` shape, so a row that
        // carried tags must not grow one on the resource.
        assert!(
            template.get("tags").is_none(),
            "case {label:?}: PrivacyBudgetTemplate models no tags member, got {:?}",
            template.get("tags")
        );
    }
}

/// One row of the one-per-collaboration table: what the SECOND create asks for,
/// built from the membership ARN the access-budget row needs.
struct SecondCreate {
    label: &'static str,
    body: fn(&str) -> Value,
}

#[tokio::test]
async fn a_collaboration_holds_one_privacy_budget_template() {
    // Rows vary what the SECOND create asks for: the same type and parameters, the
    // same type with different parameters, and the other type entirely. The rule is
    // about the collaboration already having a template, not about what the second
    // request says.
    let first = || {
        json!({
            "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
            "parameters": differential_privacy(5, 20),
        })
    };
    let second_creates = vec![
        SecondCreate {
            label: "the identical request",
            body: |_| {
                json!({
                    "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                    "parameters": differential_privacy(5, 20),
                })
            },
        },
        SecondCreate {
            label: "the same type with other parameters",
            body: |_| {
                json!({
                    "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                    "autoRefresh": ALL_AUTO_REFRESH[0],
                    "parameters": differential_privacy(EPSILON_MAX, USERS_NOISE_MAX),
                })
            },
        },
        SecondCreate {
            label: "the other privacy budget type",
            body: |membership_arn| {
                json!({
                    "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[1],
                    "parameters": access_budget(
                        membership_arn,
                        json!([{ "type": ALL_ACCESS_BUDGET_TYPES[3], "budget": 10 }]),
                    ),
                })
            },
        },
    ];

    for (row, case) in second_creates.into_iter().enumerate() {
        let label = case.label;
        let second_body = case.body;
        let emulator = Emulator::new();
        let (_, membership) =
            collaboration_with_membership(&emulator, &format!("conflict-{row}")).await;
        let membership_id = membership["id"].as_str().expect("membership.id").to_string();
        let membership_arn = membership["arn"].as_str().expect("membership.arn").to_string();

        let (status, created) = create_template(&emulator, &membership_id, first()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first create to answer 200, got {status} with {created}"
        );
        let template = created["privacyBudgetTemplate"].clone();
        let id = template["id"].as_str().expect("template.id").to_string();

        let (status, refused) =
            create_template(&emulator, &membership_id, second_body(&membership_arn)).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected the second create to answer 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["reason"].as_str(),
            Some("ALREADY_EXISTS"),
            "case {label:?}: expected reason ALREADY_EXISTS, got {:?}",
            refused["reason"]
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the conflict must name the existing template {id}, got {:?}",
            refused["resourceId"]
        );

        // The refusal changed nothing: the stored template is still the first one.
        let (status, fetched) = get_template(&emulator, &membership_id, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetPrivacyBudgetTemplate 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["privacyBudgetTemplate"], template,
            "case {label:?}: the refused create must not change the stored template, got {:?}",
            fetched["privacyBudgetTemplate"]
        );

        // The rule is per collaboration: another collaboration accepts its own.
        let (other_collaboration_id, other_membership) =
            collaboration_with_membership(&emulator, &format!("conflict-other-{row}")).await;
        let other_membership_id = other_membership["id"]
            .as_str()
            .expect("membership.id")
            .to_string();
        let (status, other_created) =
            create_template(&emulator, &other_membership_id, first()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: another collaboration must accept its own template, got {status} with {other_created}"
        );
        let other_template = other_created["privacyBudgetTemplate"].clone();
        assert_eq!(
            other_template["collaborationId"].as_str(),
            Some(other_collaboration_id.as_str()),
            "case {label:?}: expected collaborationId {other_collaboration_id:?}, got {:?}",
            other_template["collaborationId"]
        );
        assert_ne!(
            other_template["id"], template["id"],
            "case {label:?}: each collaboration's template has its own id, got {:?} twice",
            template["id"]
        );

        // And neither collaboration can see the other's template through its own
        // membership: the template belongs to the collaboration.
        let (status, not_found) = get_template(&emulator, &membership_id, other_template["id"].as_str().unwrap()).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a template of another collaboration is not found here, got {status} with {not_found}"
        );
        assert_eq!(
            error_code(&not_found),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {not_found}"
        );
    }
}

#[tokio::test]
async fn a_request_the_state_or_the_model_cannot_satisfy_is_refused() {
    let emulator = Emulator::new();
    let (_, membership) = collaboration_with_membership(&emulator, "refusals").await;
    let membership_id = membership["id"].as_str().expect("membership.id").to_string();
    let membership_arn = membership["arn"].as_str().expect("membership.arn").to_string();

    // Rows: a membership the store does not hold, and each direction of a
    // privacyBudgetType that does not match the parameters union the request carries.
    struct Refusal {
        label: &'static str,
        membership: String,
        body: Value,
        status: u16,
        code: &'static str,
    }

    let refusals = vec![
        Refusal {
            label: "a membership the store does not hold",
            membership: ABSENT_UUID.to_string(),
            body: json!({
                "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                "parameters": differential_privacy(5, 20),
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            label: "DIFFERENTIAL_PRIVACY with access-budget parameters",
            membership: membership_id.clone(),
            body: json!({
                "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                "parameters": access_budget(
                    &membership_arn,
                    json!([{ "type": ALL_ACCESS_BUDGET_TYPES[3], "budget": 10 }]),
                ),
            }),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            label: "ACCESS_BUDGET with differential-privacy parameters",
            membership: membership_id.clone(),
            body: json!({
                "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[1],
                "parameters": differential_privacy(5, 20),
            }),
            status: 400,
            code: "ValidationException",
        },
    ];

    for case in refusals {
        let (status, body) = create_template(&emulator, &case.membership, case.body.clone()).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.label, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {:?}: expected {}, got {body}",
            case.label,
            case.code
        );
    }

    // Nothing was persisted by any refusal: the collaboration still has no template,
    // so a well-formed create now succeeds.
    let (status, created) = create_template(
        &emulator,
        &membership_id,
        json!({
            "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
            "parameters": differential_privacy(5, 20),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "after the refusals a well-formed create must succeed, got {status} with {created}"
    );
    let id = created["privacyBudgetTemplate"]["id"]
        .as_str()
        .expect("template.id")
        .to_string();

    // Reads of a resource the store does not hold are refused the same way.
    let read_refusals = [
        (
            "a template id the store does not hold",
            membership_id.clone(),
            ABSENT_UUID.to_string(),
        ),
        (
            "a membership the store does not hold",
            ABSENT_UUID.to_string(),
            id,
        ),
    ];
    for (label, membership, template) in read_refusals {
        let (status, body) = get_template(&emulator, &membership, &template).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetPrivacyBudgetTemplate 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_template_does_not_outlive_the_collaboration_it_belongs_to() {
    // Rows vary what the collaboration held when it was deleted, since the template is
    // removed with it either way: with and without tags on the template, and for both
    // privacy budget types.
    for (label, body) in [
        (
            "differential privacy, no tags",
            json!({
                "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                "parameters": differential_privacy(EPSILON_MIN, USERS_NOISE_MIN),
            }),
        ),
        (
            "differential privacy with tags",
            json!({
                "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
                "autoRefresh": ALL_AUTO_REFRESH[0],
                "parameters": differential_privacy(EPSILON_MAX, USERS_NOISE_MAX),
                "tags": { "env": "test" },
            }),
        ),
    ] {
        let emulator = Emulator::new();
        let (collaboration_id, membership) =
            collaboration_with_membership(&emulator, "retired").await;
        let membership_id = membership["id"].as_str().expect("membership.id").to_string();

        let (status, created) = create_template(&emulator, &membership_id, body).await;
        assert_eq!(status, 200, "case {label:?}: CreatePrivacyBudgetTemplate: {created}");
        let id = created["privacyBudgetTemplate"]["id"]
            .as_str()
            .expect("template.id")
            .to_string();
        let arn = created["privacyBudgetTemplate"]["arn"]
            .as_str()
            .expect("template.arn")
            .to_string();

        let (status, deleted) = emulator
            .call("DELETE", &format!("/collaborations/{collaboration_id}"), None)
            .await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected DeleteCollaboration 204, got {status} with {deleted}"
        );

        // The template is gone from every store that held it, so the collaboration's
        // disappearance frees the entry rather than leaving it behind forever.
        {
            let store = emulator.store();
            let store = store.lock();
            assert!(
                store.privacy_budget_templates.is_empty(),
                "case {label:?}: the template must not outlive its collaboration, {} left",
                store.privacy_budget_templates.len()
            );
            assert!(
                store.privacy_budget_template_order.is_empty(),
                "case {label:?}: the order index must not keep the template, got {:?}",
                store.privacy_budget_template_order
            );
            assert!(
                !store
                    .privacy_budget_template_by_collaboration
                    .contains_key(&collaboration_id),
                "case {label:?}: the collaboration index must not keep the template"
            );
            assert!(
                !store.tags.contains_key(&arn),
                "case {label:?}: the template's tags must go with it, got {:?}",
                store.tags.get(&arn)
            );
        }

        // And the read through the surviving membership reports it gone.
        let (status, body) = get_template(&emulator, &membership_id, &id).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetPrivacyBudgetTemplate 404 after the delete, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_template_store_refuses_to_grow_past_its_cap() {
    // Every template the store accepts lives for the life of the process, so the store
    // carries an emulator-enforced maximum entry count and the create refuses past it
    // with the `ServiceQuotaExceededException` (402) the operation models. The cap is
    // read from the crate, not retyped, so the test cannot drift from it.
    let cap = cleanrooms_state::MAX_PRIVACY_BUDGET_TEMPLATES;

    let emulator = Emulator::new();
    let (_, membership) = collaboration_with_membership(&emulator, "cap-seed").await;
    let membership_id = membership["id"].as_str().expect("membership.id").to_string();
    let (status, created) = create_template(
        &emulator,
        &membership_id,
        json!({
            "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
            "parameters": differential_privacy(5, 20),
        }),
    )
    .await;
    assert_eq!(status, 200, "case seed template: {created}");
    let seed_id = created["privacyBudgetTemplate"]["id"]
        .as_str()
        .expect("template.id")
        .to_string();

    {
        // Fill the store to the cap with copies of the stored record, each under its
        // own id, so the next create meets a full store.
        let store = emulator.store();
        let mut store = store.lock();
        let seed = store.privacy_budget_templates.get(&seed_id).cloned().unwrap();
        while store.privacy_budget_templates.len() < cap {
            let id = format!(
                "{:08x}-0000-4000-8000-{:012x}",
                store.privacy_budget_templates.len(),
                3
            );
            let mut template = seed.clone();
            template.id = id.clone();
            store.privacy_budget_templates.insert(id.clone(), template);
            store.privacy_budget_template_order.push(id);
        }
    }

    // A DIFFERENT collaboration, so the refusal is the store cap and not the
    // one-per-collaboration conflict.
    let (_, other_membership) = collaboration_with_membership(&emulator, "cap-other").await;
    let other_membership_id = other_membership["id"]
        .as_str()
        .expect("membership.id")
        .to_string();
    let (status, body) = create_template(
        &emulator,
        &other_membership_id,
        json!({
            "privacyBudgetType": ALL_PRIVACY_BUDGET_TYPES[0],
            "parameters": differential_privacy(5, 20),
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case templates at the cap of {cap}: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case templates at the cap: expected ServiceQuotaExceededException, got {body}"
    );

    // A full store still serves reads: the cap refuses growth, not access.
    let (status, body) = get_template(&emulator, &membership_id, &seed_id).await;
    assert_eq!(
        status, 200,
        "case read at the cap: a full store must still answer, got {status} with {body}"
    );
}
