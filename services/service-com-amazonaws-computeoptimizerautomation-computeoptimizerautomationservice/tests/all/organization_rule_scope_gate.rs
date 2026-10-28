// SPDX-License-Identifier: CC0-1.0

//! The ASSOCIATION gate on the accounts an organization rule is SCOPED to, end
//! to end.
//!
//! The rule under test: a call that writes an organization configuration's
//! `accountIds` — the accounts "where the organization rule should be applied" —
//! may name only accounts the caller may automate in right now, i.e. the
//! caller's own account and the member accounts that are currently associated.
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

use crate::automation_rule_lifecycle::{call, call_ok, emulator, member, minimal_rule, string_member};
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

/// A create request for an `OrganizationRule` whose configuration names
/// `account_ids`, or carries no `accountIds` member at all when they are `None`.
fn organization_rule(name: &str, account_ids: Option<&[String]>) -> Value {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!("OrganizationRule");
    // `ruleApplyOrder` is a value of the model's `RuleApplyOrder`; what each row
    // varies is the account list.
    let mut configuration = json!({"ruleApplyOrder": "BeforeAccountRules"});
    if let Some(account_ids) = account_ids {
        configuration["accountIds"] = json!(account_ids);
    }
    request["organizationConfiguration"] = configuration;
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

/// Run a row's arrangement over the wire and read the resulting association
/// state back, so a row is judged on state some call actually stored.
async fn arrange(router: &MultiServiceRouter, row: &Row) {
    for (associate, accounts) in &row.arrange {
        let operation = if *associate {
            "AssociateAccounts"
        } else {
            "DisassociateAccounts"
        };
        call_ok(
            router,
            operation,
            &json!({"accountIds": accounts}),
            row.case,
        )
        .await;
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
        arrange(&router, &row).await;

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
