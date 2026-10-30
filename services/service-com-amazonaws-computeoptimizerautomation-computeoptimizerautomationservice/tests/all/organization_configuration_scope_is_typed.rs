// SPDX-License-Identifier: CC0-1.0

//! WHOSE accounts an `organizationConfiguration` scopes, end to end.
//!
//! The rule under test: a member that is only in force for one rule type is
//! judged for the rule type the call RESULTS in, so a call that leaves the rule
//! outside that type is refused the member as inapplicable
//! (`InvalidParameterValueException`, HTTP 400) rather than held to what the
//! member would have put into effect. The configuration's `accountIds` are the
//! accounts "where the organization rule should be applied" and the
//! `OrganizationRule` type is what puts a configuration into effect, so a rule
//! of any other type applies its actions in the account it lives in and those
//! account ids scope nothing: the association standing of the accounts they name
//! cannot decide such a call. Where the member IS in force — a merged rule of
//! the `OrganizationRule` type — the association gate
//! `organization_rule_scope_gate` covers still answers 403
//! `AccessDeniedException` for an account the caller may not automate in.
//!
//! The table below spans what the rule generalises over: the forms an account
//! list arrives in (no `accountIds` member, the caller's own account, one member
//! account, one unservable account among servable ones and not in first place,
//! the fifty the model's `OrganizationConfigurationAccountIds` allows with one
//! of them withdrawn) crossed with the association state each named account is
//! in when the call lands (never associated, associated, disassociated,
//! re-associated), and both values of `RuleApplyOrder`; the update half spans
//! which of the two members the request writes and which it leaves to the
//! stored rule.
//!
//! Every call goes over the wire — an awsJson1_0 `POST /` through the same
//! router the emulator binary mounts — through the harness
//! `automation_rule_lifecycle` owns, so the association state a case is judged
//! on is the one its own `AssociateAccounts` / `DisassociateAccounts` calls put
//! there.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RULE_APPLY_ORDER_VALUES;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, member, minimal_rule, string_member,
};
use crate::member_account_association_gate::assert_access_denied;
// The refusal this suite expects is the one the rule-type pairing clause
// already states, and the association arrangement is the one the scope gate
// already drives, so both are imported rather than written again.
use crate::organization_configuration_rule_type::{
    assert_pairing_refusal, CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES,
};
use crate::organization_rule_scope_gate::{
    arrange, member_ids, rule_count, scope_configuration, Step,
};

/// The two values of the model's `RuleType`
/// (`smithy_query(query="shape",shape="RuleType")`, the generated
/// `RULE_TYPE_VALUES`).
const ORGANIZATION_RULE: &str = "OrganizationRule";
const ACCOUNT_RULE: &str = "AccountRule";

/// One row: the association state its accounts are put in, the account list the
/// configuration names, and whether an ORGANIZATION rule may be scoped to them
/// once that arrangement has run.
struct Scope {
    case: &'static str,
    arrange: Vec<Step>,
    /// The `accountIds` the configuration names; `None` means the configuration
    /// carries no `accountIds` member.
    account_ids: Option<Vec<String>>,
    /// Whether the accounts are ones the caller may automate in right now, i.e.
    /// whether the configuration is one an organization rule may carry.
    servable: bool,
}

/// A `RuleApplyOrder` for the row, taken from the generated
/// `RULE_APPLY_ORDER_VALUES` rather than spelled here, so the rows cover every
/// value the model declares.
fn apply_order(index: usize) -> &'static str {
    RULE_APPLY_ORDER_VALUES[index % RULE_APPLY_ORDER_VALUES.len()]
}

/// The table: every form of account list, in every association state its
/// accounts can be in when the call lands.
fn scopes() -> Vec<Scope> {
    let caller = state::caller_account_id();
    // The model allows `OrganizationConfigurationAccountIds` at length 1..=50,
    // and `AssociateAccounts` takes up to `state::MAX_ACCOUNT_IDS_PER_REQUEST`
    // ids, so the largest list the model admits is arranged in one call.
    let fifty = member_ids(3300, state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let withdrawn = fifty.last().cloned().expect("the list is not empty");
    let associated = member_ids(3101, 1);
    let never = member_ids(3201, 1);
    let dropped = member_ids(3401, 1);
    let re_associated = member_ids(3501, 1);
    let among = member_ids(3601, 2);
    let outsider = member_ids(3650, 1);

    vec![
        Scope {
            // A configuration that names no account names no OTHER account, so
            // there is no association to hold an organization rule to.
            case: "no-account-ids",
            arrange: Vec::new(),
            account_ids: None,
            servable: true,
        },
        Scope {
            // The caller's own account is nobody's associated member account.
            case: "the-callers-own-account",
            arrange: Vec::new(),
            account_ids: Some(vec![caller]),
            servable: true,
        },
        Scope {
            case: "an-associated-member",
            arrange: vec![(true, associated.clone())],
            account_ids: Some(associated),
            servable: true,
        },
        Scope {
            case: "a-re-associated-member",
            arrange: vec![
                (true, re_associated.clone()),
                (false, re_associated.clone()),
                (true, re_associated.clone()),
            ],
            account_ids: Some(re_associated),
            servable: true,
        },
        Scope {
            case: "a-never-associated-member",
            arrange: Vec::new(),
            account_ids: Some(never),
            servable: false,
        },
        Scope {
            case: "a-disassociated-member",
            arrange: vec![(true, dropped.clone()), (false, dropped.clone())],
            account_ids: Some(dropped),
            servable: false,
        },
        Scope {
            // One unservable account among servable ones, and not in first
            // place: a list in force is judged account by account.
            case: "a-never-associated-member-among-associated-ones",
            arrange: vec![(true, among.clone())],
            account_ids: Some(among.into_iter().chain(outsider).collect()),
            servable: false,
        },
        Scope {
            case: "fifty-members-with-the-last-one-disassociated",
            arrange: vec![(true, fifty.clone()), (false, vec![withdrawn])],
            account_ids: Some(fifty),
            servable: false,
        },
    ]
}

/// Create a rule of `rule_type`, carrying `configuration` unless it is
/// `Value::Null`, and hand back its ARN and revision.
async fn stored_rule(
    router: &MultiServiceRouter,
    name: &str,
    rule_type: &str,
    configuration: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!(rule_type);
    if !configuration.is_null() {
        request["organizationConfiguration"] = configuration.clone();
    }
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision").as_i64().unwrap_or_default(),
    )
}

/// What a later reader must see after a REFUSED call: the stored rule exactly as
/// it was, read back through `GetAutomationRule`.
async fn assert_rule_unchanged(
    router: &MultiServiceRouter,
    rule_arn: &str,
    rule_type: &str,
    configuration: &Value,
    revision: i64,
    case: &str,
) {
    let fetched = call_ok(
        router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        case,
    )
    .await;
    assert_eq!(
        member(&fetched, "ruleRevision").as_i64(),
        Some(revision),
        "case {case:?}: a refused update does not bump the revision, got {fetched}"
    );
    assert_eq!(
        string_member(&fetched, "ruleType"),
        rule_type,
        "case {case:?}: a refused update leaves the stored rule type as it was, got {fetched}"
    );
    assert_eq!(
        member(&fetched, "organizationConfiguration"),
        configuration,
        "case {case:?}: a refused update leaves the stored configuration as it was, got {fetched}"
    );
}

/// A create outside the organization type is answered on the member being
/// inapplicable, never on the standing of the accounts it names: the same
/// configuration that is refused 400 for an account rule is served or refused
/// 403 for an organization rule, where it really is the rule's scope.
#[tokio::test]
async fn a_create_outside_the_organization_type_is_refused_the_configuration_not_held_to_it() {
    let router = emulator();
    let mut stored = rule_count(&router, "initial").await;

    for (index, scope) in scopes().into_iter().enumerate() {
        let case = scope.case;
        arrange(&router, &scope.arrange, case).await;
        let configuration = scope_configuration(apply_order(index), scope.account_ids.as_deref());

        let mut request = minimal_rule(&format!("account-rule-{index}"));
        assert_eq!(
            string_member(&request, "ruleType"),
            ACCOUNT_RULE,
            "case {case:?}: the create under test must be one of the account type, got {request}"
        );
        request["organizationConfiguration"] = configuration.clone();
        assert_pairing_refusal(
            call(&router, "CreateAutomationRule", &request).await,
            CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES,
            case,
        );
        assert_eq!(
            rule_count(&router, case).await,
            stored,
            "case {case:?}: a refused create adds no rule to the store"
        );

        // The same configuration where the type DOES put it into effect: there
        // the accounts it names are the rule's scope, so the association gate
        // decides.
        let mut organization = minimal_rule(&format!("organization-rule-{index}"));
        organization["ruleType"] = json!(ORGANIZATION_RULE);
        organization["organizationConfiguration"] = configuration.clone();
        let answer = call(&router, "CreateAutomationRule", &organization).await;
        if scope.servable {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: an organization rule scoped to accounts the caller may automate \
                 in is created, got {status} and {body}"
            );
            stored += 1;
            let fetched = call_ok(
                &router,
                "GetAutomationRule",
                &json!({"ruleArn": string_member(&body, "ruleArn")}),
                case,
            )
            .await;
            assert_eq!(
                member(&fetched, "organizationConfiguration"),
                &configuration,
                "case {case:?}: the stored scope must read back as it was written, got {fetched}"
            );
        } else {
            assert_access_denied(answer, case);
        }
        assert_eq!(
            rule_count(&router, case).await,
            stored,
            "case {case:?}: only a served create adds a rule to the store"
        );
    }
}

/// An update is judged on the rule it MERGES into: one whose merged rule stays
/// outside the organization type is refused the configuration as inapplicable,
/// whatever accounts it names and whatever state their associations are in,
/// while one whose merged rule is an organization rule is held to the scope it
/// writes. Every refusal leaves the stored rule — type, configuration and
/// revision — exactly as it was.
#[tokio::test]
async fn an_update_outside_the_organization_type_is_refused_the_configuration_not_held_to_it() {
    let router = emulator();
    // A scope every caller may automate in, so a rule stored with it is one the
    // organization type holds and the row is judged on the member it writes.
    let callers_own =
        scope_configuration("BeforeAccountRules", Some(&[state::caller_account_id()]));

    for (index, scope) in scopes().into_iter().enumerate() {
        arrange(&router, &scope.arrange, scope.case).await;
        let configuration = scope_configuration(apply_order(index), scope.account_ids.as_deref());

        // (sub-case, the stored rule's type, the configuration it is stored
        // with, the `ruleType` the update writes) — every way an update can
        // leave the merged rule outside the organization type while writing a
        // configuration.
        let outside: Vec<(&str, &str, Value, Option<&str>)> = vec![
            (
                "write-the-scope-onto-a-stored-account-rule",
                ACCOUNT_RULE,
                Value::Null,
                None,
            ),
            (
                "write-the-scope-and-re-assert-the-account-type",
                ACCOUNT_RULE,
                Value::Null,
                Some(ACCOUNT_RULE),
            ),
            (
                "write-the-scope-while-retyping-an-organization-rule-to-the-account-type",
                ORGANIZATION_RULE,
                callers_own.clone(),
                Some(ACCOUNT_RULE),
            ),
        ];

        for (sub_index, (sub_case, stored_type, stored_configuration, written_type)) in
            outside.into_iter().enumerate()
        {
            let case = format!("{}/{sub_case}", scope.case);
            let (rule_arn, revision) = stored_rule(
                &router,
                &format!("outside-{index}-{sub_index}"),
                stored_type,
                &stored_configuration,
                &case,
            )
            .await;

            let mut request = json!({
                "ruleArn": rule_arn,
                "ruleRevision": revision,
                "organizationConfiguration": configuration.clone()
            });
            if let Some(rule_type) = written_type {
                request["ruleType"] = json!(rule_type);
            }
            assert_pairing_refusal(
                call(&router, "UpdateAutomationRule", &request).await,
                CONFIGURATION_ONLY_FOR_ORGANIZATION_RULES,
                &case,
            );
            assert_rule_unchanged(
                &router,
                &rule_arn,
                stored_type,
                &stored_configuration,
                revision,
                &case,
            )
            .await;
        }

        // The control: a merged rule that stays an organization rule puts the
        // written scope into effect, so it is the association gate that decides.
        let case = format!("{}/re-scope-a-stored-organization-rule", scope.case);
        let (rule_arn, revision) = stored_rule(
            &router,
            &format!("organization-{index}"),
            ORGANIZATION_RULE,
            &callers_own,
            &case,
        )
        .await;
        let request = json!({
            "ruleArn": rule_arn,
            "ruleRevision": revision,
            "organizationConfiguration": configuration.clone()
        });
        let answer = call(&router, "UpdateAutomationRule", &request).await;
        if scope.servable {
            let (status, body) = &answer;
            assert_eq!(
                *status, 200,
                "case {case:?}: a re-scoping the associations permit is served, got {status} and \
                 {body}"
            );
            let fetched = call_ok(
                &router,
                "GetAutomationRule",
                &json!({"ruleArn": rule_arn}),
                &case,
            )
            .await;
            assert_eq!(
                member(&fetched, "organizationConfiguration"),
                &configuration,
                "case {case:?}: the written scope is what a later reader sees, got {fetched}"
            );
        } else {
            assert_access_denied(answer, &case);
            assert_rule_unchanged(
                &router,
                &rule_arn,
                ORGANIZATION_RULE,
                &callers_own,
                revision,
                &case,
            )
            .await;
        }
    }
}
