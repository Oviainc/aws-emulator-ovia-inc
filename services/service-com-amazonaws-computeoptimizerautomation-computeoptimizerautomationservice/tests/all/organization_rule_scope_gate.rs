// SPDX-License-Identifier: CC0-1.0

//! The ASSOCIATION gate on the accounts an organization rule is SCOPED to, end
//! to end.
//!
//! The rule under test: a call that decides where an automation rule's actions
//! land — by writing an organization configuration, whose `accountIds` are the
//! accounts "where the organization rule should be applied", or by asking for the
//! `OrganizationRule` type, which is what puts a configuration into effect — may
//! leave the rule scoped only to accounts the caller may automate in right now,
//! i.e. the caller's own account and the member accounts that are currently
//! associated. The scope judged is the one the RESULTING rule would carry: the
//! configuration the call writes, or the stored one when it writes none.
//! An association is what "permits the management account to create Automation
//! rules that automatically apply actions to that account", and once an account
//! is disassociated the management account (or delegated administrator) "cannot
//! create Automation rules for that account", so a rule naming an unassociated
//! account is refused with the same `AccessDeniedException` (HTTP 403) the
//! operations that initiate work in another account answer with.
//!
//! The table below spans what the rule generalises over: how many accounts the
//! configuration names (none, one, several, and the fifty the model's
//! `OrganizationConfigurationAccountIds` allows), whether the caller's own
//! account is among them, where an unservable account sits in the list, and the
//! association state each named account is in (never associated, associated,
//! disassociated, re-associated).
//!
//! Every call goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, through the harness
//! `automation_rule_lifecycle` owns. Nothing reaches into the store, so the
//! association state each row is judged on is the one its own
//! `AssociateAccounts` / `DisassociateAccounts` calls put there.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};
// The access-denied assertion this suite needs is the one the initiation gate
// already states (the same error code, status and wording), so it is imported
// rather than written again.
use crate::member_account_association_gate::assert_access_denied;

/// `count` distinct member account ids in the model's `AccountId` form
/// (`^[0-9]{12}$`), numbered from `first`. Each row draws from its own range, so
/// no row's association state can disturb another's, and none of them is the
/// calling account (`first` is never 0).
fn member_ids(first: usize, count: usize) -> Vec<String> {
    (first..first + count).map(|n| format!("{n:012}")).collect()
}

/// An organization configuration naming `account_ids`, or carrying no
/// `accountIds` member at all when they are `None`. `ruleApplyOrder` is a value
/// of the model's `RuleApplyOrder`; what a case varies is the account list.
fn scope_configuration(rule_apply_order: &str, account_ids: Option<&[String]>) -> Value {
    let mut configuration = json!({"ruleApplyOrder": rule_apply_order});
    if let Some(account_ids) = account_ids {
        configuration["accountIds"] = json!(account_ids);
    }
    configuration
}

/// A create request for an `OrganizationRule` whose configuration names
/// `account_ids`, or carries no `accountIds` member at all when they are `None`.
fn organization_rule(name: &str, account_ids: Option<&[String]>) -> Value {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!("OrganizationRule");
    request["organizationConfiguration"] = scope_configuration("BeforeAccountRules", account_ids);
    request
}

/// One arrangement step: `AssociateAccounts` when `associate`, otherwise
/// `DisassociateAccounts`, over the accounts it names.
type Step = (bool, Vec<String>);

/// A row of the table: the association state its accounts are put in, the
/// account list the rule names, and whether the write is served.
struct Row {
    case: &'static str,
    arrange: Vec<Step>,
    /// The `accountIds` the configuration names; `None` means the configuration
    /// carries no `accountIds` member.
    account_ids: Option<Vec<String>>,
    served: bool,
}

/// Run an arrangement over the wire, so a case is judged on association state
/// some call actually stored.
async fn arrange(router: &MultiServiceRouter, steps: &[Step], case: &str) {
    for (associate, accounts) in steps {
        let operation = if *associate {
            "AssociateAccounts"
        } else {
            "DisassociateAccounts"
        };
        call_ok(router, operation, &json!({"accountIds": accounts}), case).await;
    }
}

/// The mode a row's arrangement leaves `account_id` in: the LAST step naming it
/// decides, and an account no step names has no stored entry at all.
fn expected_rule_mode(row: &Row, account_id: &str) -> String {
    row.arrange
        .iter()
        .rev()
        .find(|(_, accounts)| accounts.iter().any(|candidate| candidate == account_id))
        .map(|(associate, _)| {
            if *associate {
                state::ORGANIZATION_RULE_MODE_ANY_ALLOWED
            } else {
                state::ORGANIZATION_RULE_MODE_NONE_ALLOWED
            }
            .to_string()
        })
        .unwrap_or_default()
}

/// Every account's `organizationRuleMode`, keyed by account id, as `ListAccounts`
/// reports them. One page holds every account the store can hold (`maxResults`
/// is modelled at 1..=1000 and the store is capped at
/// `state::MAX_ENROLLED_ACCOUNTS`), and an account the store has no entry for is
/// simply absent.
async fn reported_rule_modes(
    router: &MultiServiceRouter,
    case: &str,
) -> BTreeMap<String, String> {
    let listed = call_ok(
        router,
        "ListAccounts",
        &json!({"maxResults": state::MAX_ENROLLED_ACCOUNTS}),
        case,
    )
    .await;
    member(&listed, "accounts")
        .as_array()
        .map(|accounts| {
            accounts
                .iter()
                .map(|account| {
                    (
                        string_member(account, "accountId"),
                        string_member(account, "organizationRuleMode"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How many rules the store holds, read over the wire. One page holds every rule
/// a case creates: `maxResults` is modelled at 1..=100, which is also
/// `state::MAX_RULES_PER_ACCOUNT`.
async fn rule_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = call_ok(
        router,
        "ListAutomationRules",
        &json!({"maxResults": state::MAX_RULES_PER_ACCOUNT}),
        case,
    )
    .await;
    member(&listed, "automationRules")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The table: every shape of account list, in every association state its
/// accounts can be in when the rule is written.
fn rows() -> Vec<Row> {
    let caller = state::caller_account_id();
    // The model allows `OrganizationConfigurationAccountIds` at length 1..=50,
    // and `AssociateAccounts` takes up to `state::MAX_ACCOUNT_IDS_PER_REQUEST`
    // ids, so the largest list the model admits is arranged in one call.
    let fifty = member_ids(700, state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let fifty_with_one_removed = member_ids(800, state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let removed = fifty_with_one_removed
        .last()
        .cloned()
        .expect("the list is not empty");

    vec![
        Row {
            // A configuration that names no account names no OTHER account, so
            // there is no association to hold it to.
            case: "no-account-ids",
            arrange: Vec::new(),
            account_ids: None,
            served: true,
        },
        Row {
            // The caller's own account is nobody's associated member account.
            case: "caller-own-account",
            arrange: Vec::new(),
            account_ids: Some(vec![caller.clone()]),
            served: true,
        },
        Row {
            case: "associated-member",
            arrange: vec![(true, member_ids(101, 1))],
            account_ids: Some(member_ids(101, 1)),
            served: true,
        },
        Row {
            case: "never-associated-member",
            arrange: Vec::new(),
            account_ids: Some(member_ids(201, 1)),
            served: false,
        },
        Row {
            case: "disassociated-member",
            arrange: vec![(true, member_ids(301, 1)), (false, member_ids(301, 1))],
            account_ids: Some(member_ids(301, 1)),
            served: false,
        },
        Row {
            case: "re-associated-member",
            arrange: vec![
                (true, member_ids(401, 1)),
                (false, member_ids(401, 1)),
                (true, member_ids(401, 1)),
            ],
            account_ids: Some(member_ids(401, 1)),
            served: true,
        },
        Row {
            case: "caller-and-several-associated-members",
            arrange: vec![(true, member_ids(501, 3))],
            account_ids: Some(
                std::iter::once(caller.clone())
                    .chain(member_ids(501, 3))
                    .collect(),
            ),
            served: true,
        },
        Row {
            // One unservable account among servable ones, and not in first
            // place: the list is judged account by account.
            case: "one-never-associated-among-associated",
            arrange: vec![(true, member_ids(601, 2))],
            account_ids: Some(
                member_ids(601, 2)
                    .into_iter()
                    .chain(member_ids(650, 1))
                    .collect(),
            ),
            served: false,
        },
        Row {
            case: "fifty-associated-members",
            arrange: vec![(true, fifty.clone())],
            account_ids: Some(fifty),
            served: true,
        },
        Row {
            case: "fifty-members-with-the-last-one-disassociated",
            arrange: vec![
                (true, fifty_with_one_removed.clone()),
                (false, vec![removed]),
            ],
            account_ids: Some(fifty_with_one_removed),
            served: false,
        },
    ]
}

/// A create scopes an organization rule only to accounts the caller may automate
/// in: a served row round-trips its configuration through `GetAutomationRule`,
/// and a refused row is answered 403 `AccessDeniedException` and stores no rule.
#[tokio::test]
async fn a_create_scopes_a_rule_only_to_accounts_the_caller_may_automate_in() {
    let router = emulator();
    let caller = state::caller_account_id();

    let mut stored = rule_count(&router, "initial").await;
    for row in rows() {
        let case = row.case;
        arrange(&router, &row.arrange, row.case).await;

        // The state the row is judged on, read back over the wire for every
        // member account the configuration names.
        let modes = reported_rule_modes(&router, case).await;
        for account_id in row.account_ids.iter().flatten() {
            if *account_id == caller {
                continue;
            }
            assert_eq!(
                modes.get(account_id).cloned().unwrap_or_default(),
                expected_rule_mode(&row, account_id),
                "case {case:?}: account {account_id} must be in the state the row arranges \
                 before the rule is written"
            );
        }

        let request = organization_rule(case, row.account_ids.as_deref());
        let answer = call(&router, "CreateAutomationRule", &request).await;

        if row.served {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: a rule scoped to accounts the caller may automate in is \
                 created, got {status} and {body}"
            );
            let rule_arn = string_member(&body, "ruleArn");
            let fetched = call_ok(
                &router,
                "GetAutomationRule",
                &json!({"ruleArn": rule_arn}),
                case,
            )
            .await;
            assert_eq!(
                member(&fetched, "organizationConfiguration"),
                member(&request, "organizationConfiguration"),
                "case {case:?}: the stored scope must read back as it was written, got {fetched}"
            );
            stored += 1;
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

/// A refused create consumes nothing: the same client token creates the rule
/// once the account it names is associated, and the create that follows the
/// association is a fresh one rather than a replay of the refusal.
#[tokio::test]
async fn a_refused_create_consumes_neither_the_client_token_nor_a_rule_id() {
    let router = emulator();
    let member_account = member_ids(901, 1);
    let mut request = organization_rule("token-after-a-refusal", Some(&member_account));
    request["clientToken"] = json!("scope-gate-token-1");

    assert_access_denied(
        call(&router, "CreateAutomationRule", &request).await,
        "before-the-association",
    );
    assert_eq!(
        rule_count(&router, "before-the-association").await,
        0,
        "case \"before-the-association\": the refused create stored nothing"
    );

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": member_account}),
        "associate",
    )
    .await;

    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &request,
        "after-the-association",
    )
    .await;
    assert_eq!(
        member(&created, "organizationConfiguration"),
        member(&request, "organizationConfiguration"),
        "case \"after-the-association\": the same token creates the rule it was refused for, \
         got {created}"
    );
    assert_eq!(
        rule_count(&router, "after-the-association").await,
        1,
        "case \"after-the-association\": exactly one rule was created"
    );
}

/// An update is held to the same rule as a create, but only for what it writes:
/// an update that carries no organization configuration re-scopes nothing and is
/// served while a scoped account's association is gone, and one that names an
/// account the caller may not automate in is refused 403 and leaves the rule —
/// configuration, status and revision — exactly as it was.
#[tokio::test]
async fn an_update_re_scopes_a_rule_only_to_accounts_the_caller_may_automate_in() {
    let router = emulator();
    let kept = member_ids(1001, 1);
    let dropped = member_ids(1101, 1);

    for accounts in [&kept, &dropped] {
        call_ok(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": accounts}),
            "associate",
        )
        .await;
    }

    let scope: Vec<String> = kept.clone();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &organization_rule("update-scope-gate", Some(&scope)),
        "create",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");

    // The account the re-scoping update would name loses its association.
    call_ok(
        &router,
        "DisassociateAccounts",
        &json!({"accountIds": dropped}),
        "disassociate",
    )
    .await;

    // An update that writes no configuration re-scopes nothing, so it is served
    // and bumps the revision.
    let untouched = call_ok(
        &router,
        "UpdateAutomationRule",
        &json!({"ruleArn": rule_arn, "ruleRevision": 1, "status": "Inactive"}),
        "update-without-a-configuration",
    )
    .await;
    assert_eq!(
        member(&untouched, "ruleRevision").as_i64(),
        Some(2),
        "case \"update-without-a-configuration\": an update that re-scopes nothing is served, \
         got {untouched}"
    );

    // (case, the accounts the update would scope the rule to, whether it is served)
    let cases: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "re-scope-to-a-disassociated-member",
            dropped.clone(),
            false,
        ),
        (
            "re-scope-to-a-disassociated-member-among-associated",
            kept.iter().cloned().chain(dropped.clone()).collect(),
            false,
        ),
        (
            "re-scope-to-the-caller-and-an-associated-member",
            std::iter::once(state::caller_account_id())
                .chain(kept.clone())
                .collect(),
            true,
        ),
    ];

    let mut revision = 2;
    for (case, accounts, served) in cases {
        let request = json!({
            "ruleArn": rule_arn,
            "ruleRevision": revision,
            "organizationConfiguration": {
                "ruleApplyOrder": "AfterAccountRules",
                "accountIds": accounts
            }
        });
        let answer = call(&router, "UpdateAutomationRule", &request).await;
        if served {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: a re-scoping an association permits is served, got {status} \
                 and {body}"
            );
            revision += 1;
        } else {
            assert_access_denied(answer, case);
        }

        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, "ruleRevision").as_i64(),
            Some(revision),
            "case {case:?}: only a served update bumps the revision, got {fetched}"
        );
        let expected_scope = if served {
            member(&request, "organizationConfiguration").clone()
        } else {
            json!({"ruleApplyOrder": "BeforeAccountRules", "accountIds": scope})
        };
        assert_eq!(
            member(&fetched, "organizationConfiguration"),
            &expected_scope,
            "case {case:?}: a refused update leaves the stored scope as it was, got {fetched}"
        );
        assert_eq!(
            string_member(&fetched, "status"),
            "Inactive",
            "case {case:?}: no other member of the rule moved, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------
// The scope an update PUTS INTO EFFECT without writing one
// ---------------------------------------------------------------------------

/// What an update is answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// HTTP 200, and the merged rule is what a later reader sees.
    Served,
    /// 403 `AccessDeniedException`: the merged rule would apply its actions in an
    /// account the caller may not automate in.
    AccessDenied,
    /// 400 `InvalidParameterValueException`: the merged rule is one no create
    /// could have made — an organization rule with no configuration at all.
    NoConfiguration,
}

/// A row of the retype table: the rule the update starts from, what happens to
/// the association of the accounts that rule's stored configuration names, the
/// members the update writes, and the answer it must get.
struct RetypeRow {
    case: &'static str,
    /// The `ruleType` the create stores.
    stored_type: &'static str,
    /// The configuration the create stores, or `Value::Null` for a rule stored
    /// with none.
    stored_configuration: Value,
    /// Associated before the create, so the create itself is always served and
    /// the update is judged on a scope that was legal when it was written.
    associate: Vec<String>,
    /// What happens to those accounts AFTER the rule is stored.
    then: Vec<Step>,
    /// The members the update writes, beside `ruleArn` and `ruleRevision`.
    update: Value,
    answer: Answer,
}

/// A create request for a rule of `rule_type` carrying `configuration`, or no
/// configuration member at all when it is `Value::Null`.
fn stored_rule(name: &str, rule_type: &str, configuration: &Value) -> Value {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!(rule_type);
    if !configuration.is_null() {
        request["organizationConfiguration"] = configuration.clone();
    }
    request
}

/// The table: every way an update can put a STORED scope into effect, or leave
/// it where it is, with the association state that scope's accounts are in by
/// the time the update lands. The rows differ along what the rule generalises
/// over: the stored rule type, how many accounts the stored configuration names
/// (none, one, three, the fifty the model's
/// `OrganizationConfigurationAccountIds` allows), where an unservable account
/// sits in that list, the association state it is in (associated,
/// disassociated, re-associated), and which members the update writes.
fn retype_rows() -> Vec<RetypeRow> {
    let caller = state::caller_account_id();
    let lost = member_ids(2001, 1);
    let held = member_ids(2101, 1);
    let three = member_ids(2201, 3);
    // Not in first place: the stored list is judged account by account.
    let middle = vec![three[1].clone()];
    // The model allows `OrganizationConfigurationAccountIds` at length 1..=50,
    // and `AssociateAccounts` takes up to `state::MAX_ACCOUNT_IDS_PER_REQUEST`
    // ids, so the largest list the model admits is arranged in one call.
    let fifty = member_ids(2300, state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let re_associated = member_ids(2401, 1);
    let corrected = member_ids(2501, 1);
    let de_escalated = member_ids(2601, 1);
    let untouched = member_ids(2701, 1);

    vec![
        RetypeRow {
            // An update that asks for the organization type puts the STORED
            // configuration into effect, so it is judged on that scope — here an
            // account whose association is gone.
            case: "retype-to-organization-with-a-disassociated-stored-scope",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&lost)),
            associate: lost.clone(),
            then: vec![(false, lost)],
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::AccessDenied,
        },
        RetypeRow {
            case: "retype-to-organization-with-an-associated-stored-scope",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&held)),
            associate: held.clone(),
            then: Vec::new(),
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // A stored configuration naming no account names no OTHER account,
            // so there is no association to hold the retype to.
            case: "retype-to-organization-with-a-stored-scope-naming-no-account",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("AfterAccountRules", None),
            associate: Vec::new(),
            then: Vec::new(),
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // The caller's own account is nobody's associated member account.
            case: "retype-to-organization-with-only-the-callers-own-account-stored",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration(
                "BeforeAccountRules",
                Some(std::slice::from_ref(&caller)),
            ),
            associate: Vec::new(),
            then: Vec::new(),
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // Re-asserting the type the rule already has still asks for the
            // stored scope to be applied.
            case: "re-assert-the-organization-type-after-a-disassociation",
            stored_type: "OrganizationRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&three)),
            associate: three.clone(),
            then: vec![(false, middle)],
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::AccessDenied,
        },
        RetypeRow {
            case: "retype-to-organization-with-fifty-associated-stored-accounts",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("AfterAccountRules", Some(&fifty)),
            associate: fifty.clone(),
            then: Vec::new(),
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // The association is judged as it stands when the update lands, not
            // as it stood in between.
            case: "retype-to-organization-after-a-re-association",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&re_associated)),
            associate: re_associated.clone(),
            then: vec![
                (false, re_associated.clone()),
                (true, re_associated.clone()),
            ],
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // The same retype, correcting the scope in the same call: the
            // configuration this request writes is the one the merged rule
            // carries, so the stale stored scope is not what is judged.
            case: "retype-to-organization-correcting-the-scope-in-the-same-call",
            stored_type: "AccountRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&corrected)),
            associate: corrected.clone(),
            then: vec![(false, corrected)],
            update: json!({
                "ruleType": "OrganizationRule",
                "organizationConfiguration": {
                    "ruleApplyOrder": "AfterAccountRules",
                    "accountIds": [caller.clone()]
                }
            }),
            answer: Answer::Served,
        },
        RetypeRow {
            // Turning an organization rule into an account rule stops it
            // applying anywhere else, so it is not held to the stale scope.
            case: "retype-to-an-account-rule-after-a-disassociation",
            stored_type: "OrganizationRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&de_escalated)),
            associate: de_escalated.clone(),
            then: vec![(false, de_escalated)],
            update: json!({"ruleType": "AccountRule"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // An update that writes neither member re-points nothing: a
            // disassociation stops an organization rule applying to the member
            // account, it does not make the stored rule unmaintainable.
            case: "update-another-member-after-a-disassociation",
            stored_type: "OrganizationRule",
            stored_configuration: scope_configuration("BeforeAccountRules", Some(&untouched)),
            associate: untouched.clone(),
            then: vec![(false, untouched)],
            update: json!({"status": "Inactive"}),
            answer: Answer::Served,
        },
        RetypeRow {
            // The merged rule must satisfy the OTHER rule a create is held to as
            // well: an organization rule needs a configuration at all.
            case: "retype-to-organization-with-no-stored-configuration",
            stored_type: "AccountRule",
            stored_configuration: Value::Null,
            associate: Vec::new(),
            then: Vec::new(),
            update: json!({"ruleType": "OrganizationRule"}),
            answer: Answer::NoConfiguration,
        },
    ]
}

/// An update that puts a scope into effect is held to it even when it writes no
/// configuration: asking for the `OrganizationRule` type applies the STORED
/// configuration, so the accounts it names must be ones the caller may automate
/// in when the update lands. A refused update leaves the rule — type,
/// configuration and revision — exactly as it was.
#[tokio::test]
async fn an_update_that_turns_a_rule_into_an_organization_rule_is_held_to_the_scope_it_activates() {
    let router = emulator();

    for row in retype_rows() {
        let case = row.case;
        // An association request names at least one account, so a row that
        // associates nothing arranges nothing.
        if !row.associate.is_empty() {
            arrange(&router, &[(true, row.associate.clone())], case).await;
        }

        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &stored_rule(case, row.stored_type, &row.stored_configuration),
            case,
        )
        .await;
        let rule_arn = string_member(&created, "ruleArn");
        let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();

        arrange(&router, &row.then, case).await;

        let mut request = row.update.clone();
        request["ruleArn"] = json!(rule_arn);
        request["ruleRevision"] = json!(revision);
        let answer = call(&router, "UpdateAutomationRule", &request).await;

        match row.answer {
            Answer::Served => {
                let (status, body) = &answer;
                assert_eq!(
                    *status, 200,
                    "case {case:?}: an update that activates no unservable scope is served, got \
                     {status} and {body}"
                );
            }
            Answer::AccessDenied => assert_access_denied(answer, case),
            Answer::NoConfiguration => {
                let (status, body) = answer;
                assert_refused(
                    (status, body.clone()),
                    400,
                    "InvalidParameterValueException",
                    case,
                );
                assert_eq!(
                    string_member(&body, "message"),
                    state::constraint_report(
                        "organizationConfiguration",
                        "Member is required for the OrganizationRule rule type"
                    ),
                    "case {case:?}: the refusal must report the member the merged rule lacks, \
                     got {body}"
                );
            }
        }

        // What a later reader sees: the merged rule on a served update, the
        // stored one untouched on a refused one.
        let served = row.answer == Answer::Served;
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        let expected_revision = if served { revision + 1 } else { revision };
        let expected_type = if served {
            member(&request, "ruleType").clone()
        } else {
            Value::Null
        };
        let expected_type = if expected_type.is_null() {
            json!(row.stored_type)
        } else {
            expected_type
        };
        let written_configuration = member(&request, "organizationConfiguration").clone();
        let expected_configuration = if served && !written_configuration.is_null() {
            written_configuration
        } else {
            row.stored_configuration.clone()
        };

        assert_eq!(
            member(&fetched, "ruleRevision").as_i64(),
            Some(expected_revision),
            "case {case:?}: only a served update bumps the revision, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "ruleType"),
            &expected_type,
            "case {case:?}: a refused update leaves the stored rule type as it was, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "organizationConfiguration"),
            &expected_configuration,
            "case {case:?}: a refused update leaves the stored scope as it was, got {fetched}"
        );
    }
}
