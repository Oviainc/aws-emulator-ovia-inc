// SPDX-License-Identifier: CC0-1.0

//! The clause that binds an automation rule's `organizationConfiguration` to
//! its `ruleType`, end to end.
//!
//! The rule under test: the configuration is the organization-wide half of a
//! rule — the order organization rules apply in and the accounts they apply to
//! — so a rule the service holds carries one exactly when its rule type is
//! `OrganizationRule`. The type requires the member ("Required for
//! OrganizationRule type", from
//! `smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="organizationConfiguration")`),
//! and any other rule type is refused it with `InvalidParameterValueException`
//! (HTTP 400) rather than stored with a member that puts nothing into effect.
//! A create is judged on what it sends and an update on the rule it MERGES
//! into, so neither can leave the store holding a pair the other would refuse,
//! and no reader — `GetAutomationRule`, `ListAutomationRules`, or the
//! `OrganizationConfigurationRuleApplyOrder` filter that selects on the
//! member — ever reports a configuration on a rule scoped to one account.
//!
//! The tables below span what the rule generalises over: both values of the
//! model's `RuleType` crossed with the forms a configuration arrives in
//! (absent, each value of `RuleApplyOrder`, an account list, both members, and
//! a configuration carrying neither member), and, for the update half, which of
//! the two members the request writes and which it leaves to the stored rule.
//! Every call goes over the wire — an awsJson1_0 `POST /` through the same
//! router the emulator binary mounts — through the harness
//! `automation_rule_lifecycle` owns, so nothing reaches into the store.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    RULE_APPLY_ORDER_VALUES, RULE_TYPE_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

/// The rule type a configuration belongs to, and the one it does not — the two
/// values of the model's `RuleType` (the generated `RULE_TYPE_VALUES`).
const ORGANIZATION_RULE: &str = "OrganizationRule";
const ACCOUNT_RULE: &str = "AccountRule";

/// What a rule of the `OrganizationRule` type carrying no configuration is
/// refused with.
const CONFIGURATION_REQUIRED: &str = "Member is required for the OrganizationRule rule type";

/// What a rule carrying a configuration outside the `OrganizationRule` type is
/// refused with.
///
/// `pub(crate)` so the suite about what that configuration's accounts are — and
/// are not — judged for states the refusal from here rather than spelling it
/// again.
pub(crate) const CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES: &str =
    "Member is valid only for the OrganizationRule rule type";

/// Every form an `organizationConfiguration` arrives in, as `(case, member)`.
///
/// The forms are built from the two members the model gives
/// `OrganizationConfiguration`
/// (`smithy_query(query="shape",shape="OrganizationConfiguration")`:
/// `ruleApplyOrder` and `accountIds`) — each on its own, both together, and
/// neither — with the order taking every value of `RuleApplyOrder` (the
/// generated `RULE_APPLY_ORDER_VALUES`, which is where the set comes from
/// rather than being spelled here). `Value::Null` is the request that carries
/// no configuration member at all.
///
/// Every account list names the CALLER's own account, which every caller may
/// automate in, so what these cases are judged on is the rule-type clause and
/// not the association gate `organization_rule_scope_gate` covers.
fn configuration_forms() -> Vec<(String, Value)> {
    let caller = state::caller_account_id();
    let mut forms = vec![
        ("no configuration member at all".to_string(), Value::Null),
        (
            "a configuration carrying neither member".to_string(),
            json!({}),
        ),
        (
            "an account list alone".to_string(),
            json!({"accountIds": [caller.clone()]}),
        ),
    ];
    for order in RULE_APPLY_ORDER_VALUES {
        forms.push((
            format!("the {order} apply order alone"),
            json!({"ruleApplyOrder": order}),
        ));
        forms.push((
            format!("the {order} apply order with an account list"),
            json!({"ruleApplyOrder": order, "accountIds": [caller.clone()]}),
        ));
    }
    forms
}

/// The requirement a rule of `rule_type` carrying `configuration` must be
/// refused with, or `None` when the pair is one the service holds.
fn expected_requirement(rule_type: &str, configuration: &Value) -> Option<&'static str> {
    match (rule_type == ORGANIZATION_RULE, configuration.is_null()) {
        (true, true) => Some(CONFIGURATION_REQUIRED),
        (false, false) => Some(CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES),
        _ => None,
    }
}

/// A create request for a rule of `rule_type` carrying `configuration`, or no
/// configuration member at all when it is `Value::Null`.
fn create_request(name: &str, rule_type: &str, configuration: &Value) -> Value {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!(rule_type);
    if !configuration.is_null() {
        request["organizationConfiguration"] = configuration.clone();
    }
    request
}

/// Every rule the store holds, read over the wire. One page holds every rule a
/// case creates: `maxResults` is modelled at 1..=100, which is also
/// `state::MAX_RULES_PER_ACCOUNT`.
async fn listed_rules(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let listed = call_ok(
        router,
        "ListAutomationRules",
        &json!({"maxResults": state::MAX_RULES_PER_ACCOUNT}),
        case,
    )
    .await;
    member(&listed, "automationRules")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert a call was refused with the report that names `requirement` on
/// `organizationConfiguration`.
pub(crate) fn assert_pairing_refusal((status, body): (u16, Value), requirement: &str, case: &str) {
    assert_refused(
        (status, body.clone()),
        400,
        "InvalidParameterValueException",
        case,
    );
    let expected = state::constraint_report("organizationConfiguration", requirement);
    assert_eq!(
        string_member(&body, "message"),
        expected,
        "case {case:?}: expected the refusal to report {expected:?}, got {body}"
    );
}

/// A create stores a configuration exactly on an organization rule: every other
/// pairing is refused, reports the member and leaves the store as it was, and
/// every pairing it serves reads back through `GetAutomationRule` as it was
/// written.
#[tokio::test]
async fn a_create_pairs_an_organization_configuration_only_with_the_organization_rule_type() {
    let router = emulator();
    let mut stored = 0usize;

    for rule_type in RULE_TYPE_VALUES {
        for (index, (form, configuration)) in configuration_forms().into_iter().enumerate() {
            let case = format!("a {rule_type} created with {form}");
            let name = format!("create-{}-{index}", rule_type.to_ascii_lowercase());
            let request = create_request(&name, rule_type, &configuration);
            let answer = call(&router, "CreateAutomationRule", &request).await;

            match expected_requirement(rule_type, &configuration) {
                Some(requirement) => assert_pairing_refusal(answer, requirement, &case),
                None => {
                    let (status, body) = answer;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the pair the service holds must be created, got {status} \
                         and {body}"
                    );
                    stored += 1;
                    let rule_arn = string_member(&body, "ruleArn");
                    let fetched = call_ok(
                        &router,
                        "GetAutomationRule",
                        &json!({"ruleArn": rule_arn}),
                        &case,
                    )
                    .await;
                    assert_eq!(
                        string_member(&fetched, "ruleType"),
                        *rule_type,
                        "case {case:?}: the stored rule type must read back as written, got \
                         {fetched}"
                    );
                    assert_eq!(
                        member(&fetched, "organizationConfiguration"),
                        &configuration,
                        "case {case:?}: the stored configuration must read back as written, got \
                         {fetched}"
                    );
                }
            }

            assert_eq!(
                listed_rules(&router, &case).await.len(),
                stored,
                "case {case:?}: only a served create adds a rule to the store"
            );
        }
    }

    // The consequence a reader sees: the filter that selects on the member can
    // only ever select organization rules, because no other rule carries one.
    for order in RULE_APPLY_ORDER_VALUES {
        let case = format!("rules filtered on the {order} apply order");
        let selected = call_ok(
            &router,
            "ListAutomationRules",
            &json!({
                "maxResults": state::MAX_RULES_PER_ACCOUNT,
                "filters": [{
                    "name": "OrganizationConfigurationRuleApplyOrder",
                    "values": [order]
                }]
            }),
            &case,
        )
        .await;
        let rules = member(&selected, "automationRules")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !rules.is_empty(),
            "case {case:?}: the organization rules created with that order must be selected, got \
             {selected}"
        );
        for rule in &rules {
            assert_eq!(
                string_member(rule, "ruleType"),
                ORGANIZATION_RULE,
                "case {case:?}: a rule selected on the member must be one the member applies to, \
                 got {rule}"
            );
        }
    }
}

/// A row of the update table: the rule the update starts from, the members it
/// writes, and the requirement it must be refused with, if any.
struct UpdateRow {
    case: String,
    /// The `ruleType` the create stores, with the configuration that type takes.
    stored_type: &'static str,
    stored_configuration: Value,
    /// The members the update writes, beside `ruleArn` and `ruleRevision`.
    update: Value,
    /// The requirement the refusal reports, or `None` when the update is served.
    refusal: Option<&'static str>,
}

/// The table: every way an update can pair the two members, whichever of them
/// it writes and whichever it leaves to the stored rule.
fn update_rows() -> Vec<UpdateRow> {
    let caller = state::caller_account_id();
    let configuration = json!({
        "ruleApplyOrder": "BeforeAccountRules",
        "accountIds": [caller.clone()]
    });
    let replacement = json!({
        "ruleApplyOrder": "AfterAccountRules",
        "accountIds": [caller]
    });
    let mut rows = vec![
        UpdateRow {
            // Writing the type alone, on a rule that has no configuration to
            // put into effect.
            case: "ask for the organization type with nothing to configure".to_string(),
            stored_type: ACCOUNT_RULE,
            stored_configuration: Value::Null,
            update: json!({"ruleType": ORGANIZATION_RULE}),
            refusal: Some(CONFIGURATION_REQUIRED),
        },
        UpdateRow {
            // Both members in one call: the pair the service holds.
            case: "ask for the organization type and configure it in one call".to_string(),
            stored_type: ACCOUNT_RULE,
            stored_configuration: Value::Null,
            update: json!({
                "ruleType": ORGANIZATION_RULE,
                "organizationConfiguration": configuration.clone()
            }),
            refusal: None,
        },
        UpdateRow {
            // The type the stored rule already has, so the merged rule keeps
            // its configuration.
            case: "replace the configuration of an organization rule".to_string(),
            stored_type: ORGANIZATION_RULE,
            stored_configuration: configuration.clone(),
            update: json!({"organizationConfiguration": replacement}),
            refusal: None,
        },
        UpdateRow {
            // An update that writes neither member leaves the pair alone.
            case: "write another member of an organization rule".to_string(),
            stored_type: ORGANIZATION_RULE,
            stored_configuration: configuration.clone(),
            update: json!({"status": "Inactive"}),
            refusal: None,
        },
        UpdateRow {
            case: "write another member of an account rule".to_string(),
            stored_type: ACCOUNT_RULE,
            stored_configuration: Value::Null,
            update: json!({"status": "Inactive"}),
            refusal: None,
        },
        UpdateRow {
            // The stored configuration is what the merged account rule would
            // carry, and the account type puts nothing into effect with it.
            case: "ask for the account type on a configured rule".to_string(),
            stored_type: ORGANIZATION_RULE,
            stored_configuration: configuration.clone(),
            update: json!({"ruleType": ACCOUNT_RULE}),
            refusal: Some(CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES),
        },
        UpdateRow {
            case: "ask for the account type and write a configuration with it".to_string(),
            stored_type: ORGANIZATION_RULE,
            stored_configuration: configuration.clone(),
            update: json!({
                "ruleType": ACCOUNT_RULE,
                "organizationConfiguration": configuration.clone()
            }),
            refusal: Some(CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES),
        },
    ];
    // Writing a configuration onto a rule that stays an account rule, in every
    // form a configuration arrives in: the stored type is what the merged rule
    // keeps, so each form is refused.
    for (form, configuration) in configuration_forms() {
        if configuration.is_null() {
            continue;
        }
        rows.push(UpdateRow {
            case: format!("write {form} onto an account rule"),
            stored_type: ACCOUNT_RULE,
            stored_configuration: Value::Null,
            update: json!({"organizationConfiguration": configuration}),
            refusal: Some(CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES),
        });
    }
    rows
}

/// An update is held to the same clause as a create, on the rule it MERGES
/// into: a refused update reports the member and leaves the stored rule — type,
/// configuration and revision — exactly as it was, and a served one is the
/// merged rule a later reader sees.
#[tokio::test]
async fn an_update_may_not_merge_a_configuration_onto_a_rule_outside_the_organization_type() {
    let router = emulator();

    for (index, row) in update_rows().into_iter().enumerate() {
        let case = row.case.as_str();
        let name = format!("update-{index}");
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &create_request(&name, row.stored_type, &row.stored_configuration),
            case,
        )
        .await;
        let rule_arn = string_member(&created, "ruleArn");
        let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();

        let mut request = row.update.clone();
        request["ruleArn"] = json!(rule_arn);
        request["ruleRevision"] = json!(revision);
        let answer = call(&router, "UpdateAutomationRule", &request).await;

        let served = match row.refusal {
            Some(requirement) => {
                assert_pairing_refusal(answer, requirement, case);
                false
            }
            None => {
                let (status, body) = &answer;
                assert_eq!(
                    *status, 200,
                    "case {case:?}: a merged rule the service holds must be updated, got {status} \
                     and {body}"
                );
                true
            }
        };

        // What a later reader sees: the merged rule on a served update, the
        // stored one untouched on a refused one.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        let written_type = string_member(&request, "ruleType");
        let expected_type = if served && !written_type.is_empty() {
            written_type
        } else {
            row.stored_type.to_string()
        };
        let written_configuration = member(&request, "organizationConfiguration").clone();
        let expected_configuration = if served && !written_configuration.is_null() {
            written_configuration
        } else {
            row.stored_configuration.clone()
        };

        assert_eq!(
            member(&fetched, "ruleRevision").as_i64(),
            Some(if served { revision + 1 } else { revision }),
            "case {case:?}: only a served update bumps the revision, got {fetched}"
        );
        assert_eq!(
            string_member(&fetched, "ruleType"),
            expected_type,
            "case {case:?}: a refused update leaves the stored rule type as it was, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "organizationConfiguration"),
            &expected_configuration,
            "case {case:?}: a refused update leaves the stored configuration as it was, got \
             {fetched}"
        );
    }
}
