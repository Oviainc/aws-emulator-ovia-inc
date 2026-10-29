Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4b4bad621ec65a03c6601647be32d2c9eaf1bb12

# ListEnrollmentStatuses refuses 403 for a request its own scoping confines to the caller's record (accountId=caller with includeOrganizationInfo=true)

_Filed: 2026-10-29T00:21:45Z_
_Operation: ListEnrollmentStatuses | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4b4bad621ec65a03c6601647be32d2c9eaf1bb12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler decides "does this read reach past the caller's own account?" with a hand-rolled expression instead of deriving it from `addressed_accounts`, the helper whose doc says that set is exactly what the permission gates judge. The two then disagree for one input combination.

In `addressed_accounts`, a request that NAMES accounts wins over `whole_organization`: `if !named.is_empty() { named.to_vec() } else if whole_organization { ... }`. So for `{accountId: <caller>, includeOrganizationInfo: true}` the addressed set is `[caller]` — the read reaches no further than the caller's own record, which is exactly what `{includeOrganizationInfo: false}` is served (200, one item).

The gate, however, ORs the two members: `input.include_organization_info || named.iter().any(|wanted| wanted != caller)`, so the same request is refused with the modelled `AccessDeniedException` (403) before any scoping runs. The caller is denied an organization view it does not in fact get: the scoping would have confined the answer to its own record. The gate and the scoping contradict each other for that request, which the helper's doc says cannot happen.

## Evidence

The hand-rolled gate, `src/ops/list_enrollment_statuses.rs:185`:

            // Both of these ask for accounts other than the caller's — the
            // organization's enrollment, and one named member account — which
            // is a view only the organization's management account has.
            let named: &[String] = input.account_id.as_slice();
            let asks_for_other_accounts = input.include_organization_info
                || named.iter().any(|wanted| wanted != caller);
            if asks_for_other_accounts && !store.is_management_account(caller) {
                return Err(not_an_organization_view());
            }

The scoping the same handler then applies, `src/ops/list_enrollment_statuses.rs:205`:

                .addressed_accounts(caller, named, input.include_organization_info)

The helper, and the rule it claims to own — `src/state.rs:1035`:

    /// ... Written once here
    /// because it is the same set the permission gates judge — whether the read
    /// reaches past the caller's own account — so the rows a reader then scopes
    /// to cannot disagree with what its gate admitted.

and its body, `src/state.rs:1048` (a named account beats `whole_organization`):

        let mut accounts: Vec<String> = if !named.is_empty() {
            named.to_vec()
        } else if whole_organization {
            self.enrollments.keys().cloned().collect()
        } else {
            vec![caller.to_string()]
        };

That named-wins scoping is pinned for the admitted caller by `tests/all/account_scoped_reads.rs:168`: case "a member named, with the organization asked for too" sends `{"includeOrganizationInfo": true, "accountId": MEMBER_ACTIVE}` and expects ONLY the named member's record. The gating test (`tests/all/management_account_gating.rs:148`) covers `{includeOrganizationInfo:false, accountId: caller}` (200) and `{includeOrganizationInfo:true}` (403), but no case sends the caller's own id together with `includeOrganizationInfo: true`, so no test pins the 403 this combination gets.

## Steps to Reproduce

1. Boot the default (non-management) router, as `tests/all/management_account_gating.rs` does with `router()`.
2. Call `ListEnrollmentStatuses` with `{"includeOrganizationInfo": false, "accountId": "<the emulator account id>"}` — 200 with exactly the caller's record (pinned today).
3. Call `ListEnrollmentStatuses` with `{"includeOrganizationInfo": true, "accountId": "<the emulator account id>"}`.
4. Observed: HTTP 403 `AccessDeniedException` ("Cannot get enrollment statuses for member accounts of the organization…").
5. Expected per `addressed_accounts` and its doc: the named account wins, the addressed set is `[caller]`, the read reaches past no account, so the gate should admit it and the answer should be the caller's own record.
