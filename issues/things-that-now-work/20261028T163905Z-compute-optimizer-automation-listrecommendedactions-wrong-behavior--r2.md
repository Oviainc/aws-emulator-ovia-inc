Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc

# A disassociated member account's recommended actions stay in the retrieval listing, although the crate reads the association per call everywhere else it touches another account's actions

_Filed: 2026-10-28T16:39:05Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendedActions` reports every entry the recommended-action store holds, with no regard for whether the account the action belongs to is currently associated with the organization. The only standing check the handler makes is the CALLER's own enrollment (`opt_in_required`); `is_associated` / `may_initiate_in` are never consulted.

An account's inventory enters the store exactly because it was associated (the `AssociateAccounts` handler records it, quoting this operation's own doc sentence as the reason), but `DisassociateAccounts` deliberately leaves the inventory in place. Nothing then removes those actions from the listing, so after a disassociation the management account keeps retrieving a non-associated account's recommended actions.

The result is a listing the crate's own gates contradict: the very action ids `ListRecommendedActions` reports for the disassociated member are answered 403 `AccessDeniedException` by `StartAutomationEvent` (`may_initiate_in`), and a rule scoped to that account is refused by `may_scope_rule_to`. The crate's stated rule for this is that the association is judged per call, never inferred from the stored resource — the listing is the one path over another account's actions that skips it.

The defect sits in the shared selection paths in `state.rs`, so it shows identically on `ListRecommendedActionSummaries` (where the disassociated member's estimates are also summed into the group totals, producing a wrong `estimatedMonthlySavings`) and on both preview lists (`previewed_actions` filters by `organizationScope` only, so an `organizationScope` naming a disassociated account previews its actions even though a rule carrying that same scope is refused 403 at create). `ListRecommendedActions` is named here because it is the operation whose own documentation states the retrieval scope.

## Evidence

The documented retrieval scope — `smithy_query(query="doc",shape="ListRecommendedActions")`:
```
<p> Lists the recommended actions based that match specified filters. </p> <note> <p>Management accounts and delegated administrators can retrieve recommended actions that include associated member accounts. You can associate a member account using <code>AssociateAccounts</code>.</p> </note>
```
(the identical sentence is on `ListRecommendedActionSummaries`.)

The handler's only standing check, and the unfiltered walk — `src/ops/list_recommended_actions.rs:226-249`:
```
        let guard = state.lock();
        // The service-wide enrollment gate, AHEAD of filter evaluation: an
        // account that is not opted in is refused the whole call rather than
        // told there are no recommended actions to take. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(ListRecommendedActionsErrorModeled::OptInRequiredException(refusal).into());
        }
        // The matching actions are what gets paged, and the page walk is the one
        // every paginated listing of this service performs:
        // `crate::state::serve_page`.
        let (page, next_token): (Vec<RecommendedAction>, Option<String>) =
            crate::state::serve_page(
                guard
                    .recommended_actions
                    .iter_in_order()
                    .filter(|action| {
                        crate::state::recommended_action_matches_filters(action, &filters)
                    })
                    .cloned(),
```

The crate itself cites THIS operation's doc sentence as the ground for putting an associated member's actions into the listing — `src/ops/associate_accounts.rs:248-260`:
```
                for account_id in &requested {
                    guard.associate_account(account_id, associated_at);
                    // An association "automatically enables the Automation
                    // feature" for a member account that had not enabled it, and
                    // an enrolled account's resources are analysed — which is
                    // what gives the management account "recommended actions
                    // that include associated member accounts"
                    // (`ListRecommendedActions`) to list, preview and automate.
```
while the removal keeps them — `src/state.rs:3203-3208`:
```
    /// The account's OWN enrollment is left alone: an association and an
    /// enrollment are two different facts about an account, the status is "The
    /// enrollment status of the account" and only that account's
    /// `UpdateEnrollmentConfiguration` sets it, so a member account that enabled
    /// the Automation feature keeps it enabled (and keeps the inventory that
    /// enrollment produced) once the organization stops reaching it.
```

The sibling rule this listing skips — `src/state.rs:3094-3107`:
```
    /// Judged per call, never taken from the stored resource: a disassociation
    /// removes the association and leaves the member's events and analysed
    /// actions exactly where they were ([`Self::disassociate_account`]), so a
    /// resource of that account stays reachable by id after the organization
    /// has stopped reaching into it.
...
    pub fn may_initiate_in(&self, account_id: &str) -> bool {
        account_id == caller_account_id() || self.is_associated(account_id)
    }
```
applied to the same actions by `src/ops/start_automation_event.rs:267-275`:
```
        // The caller's STANDING in the account the automation would run in: an
        // automation is initiated only for a recommended action of an
        // associated member account. The association is read NOW rather than
        // inferred from the action being stored, because a disassociation
        // leaves the member's analysed actions exactly where they were — see
        // `crate::state::…::may_initiate_in`.
        if !guard.may_initiate_in(&account_id) {
            return Err(access_denied());
        }
```
and the preview selection, which filters by scope and criteria only — `src/state.rs:1436-1450`:
```
    actions
        .iter_in_order()
        .filter(move |action| {
            action
                .account_id
                .as_deref()
                .is_some_and(|account_id| account_in_scope(account_id, scope))
        })
```

Test coverage: the suite pins the INITIATION side of the gate for a disassociated member (`tests/all/member_account_association_gate.rs:185-197`: `disassociate(&router, MEMBER_A, "disassociated").await;` then `assert_access_denied(start(&router, action_id, None).await, &case);`), and pins that an association ADDS the member's inventory to the listing (`tests/all/recommended_action_inventory.rs:684-706`, "the member account's analysis output joins the listing"). No test anywhere calls `ListRecommendedActions`, `ListRecommendedActionSummaries` or either preview after a `DisassociateAccounts`, so the behaviour reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call ListRecommendedActions with {} and note the action count (the caller's own inventory only).
3. Call AssociateAccounts with {"accountIds": ["111111111111"]} — 200. Call ListRecommendedActions with {} again: the member's six analysed actions have joined the listing (tests/all/recommended_action_inventory.rs:684-706 pins exactly this).
4. Call DisassociateAccounts with {"accountIds": ["111111111111"]} — 200, and GetEnrollmentConfiguration/ListAccounts report the member as NoneAllowed.
5. Call ListRecommendedActions with {} once more: the member's six actions are STILL reported, although the account is no longer associated and the operation documents retrieval as covering "associated member accounts".
6. Call StartAutomationEvent with {"recommendedActionId": <one of those still-listed ids>} — 403 AccessDeniedException from src/ops/start_automation_event.rs:273-275, because may_initiate_in reads the association now. The listing advertises actions the service refuses to act on.
7. Call ListRecommendedActionSummaries with {}: the disassociated member's estimates are still summed into the per-type totals (src/ops/list_recommended_action_summaries.rs:255-277).
8. Call ListAutomationRulePreview with {"ruleType": "OrganizationRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "organizationScope": {"accountIds": ["111111111111"]}} — 200 with the disassociated account's actions, while CreateAutomationRule with the same accountIds in organizationConfiguration is refused 403 by may_scope_rule_to.
