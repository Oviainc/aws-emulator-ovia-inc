Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# The clock-due "settle the pending transition" rule is implemented twice in state.rs, once per record type

_Filed: 2026-10-28T19:45:36Z_
_Operation: AssociateThirdPartyFirewall | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two record types in this crate carry the same pair of fields (`settles_to: Option<String>`, `settles_at: Option<SystemTime>`) and each implements the SAME rule over them in its own `settle` method: once `settles_at` has passed, `status` becomes `settles_to` and both pending fields are cleared. `OrganizationAdminRecord::settle` and `ThirdPartyFirewallRecord::settle` are byte-for-byte the same four statements, the second one only inserting an extra side effect before them.

The duplication is already acknowledged in the crate's own prose: `OrganizationAdminRecord`'s doc says its `settles_to` / `settles_at` carry the transition the clock owes "exactly as [`ThirdPartyFirewallRecord`] does". So the two copies are known to express one rule, and nothing keeps them in step: a change to how a due transition is recognised (for example `>` instead of `>=`, or clearing only one of the two fields) applied to one copy would silently leave the other behaving differently, and the two record types are read by different operations so no single test would catch the split.

The fix is a pure extraction — one shared pending-transition type (or one free function over the `(status, settles_to, settles_at)` triple) that both record types delegate to, the way the pagination-token decoders already delegate to `decode_key_token` and both list writers already delegate to `archive_superseded_list_version`. Every response and every existing assertion stays identical: the derived read side (`OrganizationAdminRecord::status_at`) and the extra Marketplace side effect in the third-party-firewall copy are unchanged behaviour that the shared helper is simply called from.

## Evidence

Copy 1 — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:205-219:
```
    /// Write the clock-due transition into the record — the ONLY write a
    /// reporting operation makes, and the persisted form of what
    /// [`Self::status_at`] already derives.
    pub fn settle(&mut self, now: SystemTime) {
        let due = self
            .settles_at
            .is_some_and(|at| now >= at)
            .then(|| self.settles_to.clone())
            .flatten();
        if let Some(status) = due {
            self.status = status;
            self.settles_to = None;
            self.settles_at = None;
        }
    }
```

Copy 2 — src/state.rs:616-636:
```
    /// Apply the clock-due transition: once `settles_at` has passed, the status
    /// IS the terminal one, because the transition was owed at that instant even
    /// if nobody had called. Materialising it is the ONLY write a reporting
    /// operation makes.
    pub fn settle(&mut self, now: SystemTime) {
        let due = self
            .settles_at
            .is_some_and(|at| now >= at)
            .then(|| self.settles_to.clone())
            .flatten();
        if let Some(status) = due {
            if status == THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE {
                // The designation completing is what makes the subscription
                // active; see the type's docs.
                self.marketplace_onboarding_status = MARKETPLACE_ONBOARDING_COMPLETE.to_string();
            }
            self.status = status;
            self.settles_to = None;
            self.settles_at = None;
        }
    }
```

The crate already states that these are one rule — src/state.rs:173-176:
```
/// `settles_to` / `settles_at` carry the transition the CLOCK owes, exactly as
/// [`ThirdPartyFirewallRecord`] does: an account being onboarded reaches
/// `ONBOARDING_COMPLETE` [`ADMIN_ONBOARDING_SPAN`] after the call that started
/// it, whether or not anybody asks. Readers materialise it with [`Self::settle`].
```

Call sites, which the extraction leaves untouched:
- `ThirdPartyFirewallRecord::settle` — src/ops/associate_third_party_firewall.rs:194, src/ops/disassociate_third_party_firewall.rs:196, src/ops/get_third_party_firewall_association_status.rs:203, src/ops/list_third_party_firewall_firewall_policies.rs:226.
- `OrganizationAdminRecord::settle` — src/state.rs:1227-1231 (`settle_organization_admins_at`), reached from src/ops/put_admin_account.rs:271, src/ops/get_admin_scope.rs:213 (via `organization_admin_at`) and src/ops/list_admin_accounts_for_organization.rs:205.

Tests that cover both copies today and must keep passing unchanged: tests/all/third_party_firewall_association.rs and tests/all/third_party_firewall_firewall_policies.rs (the vendor association settling to `ONBOARD_COMPLETE` / `OFFBOARD_COMPLETE` and the Marketplace status following it), and tests/all/admin_account_scopes.rs, tests/all/get_admin_scope_lifecycle_status.rs, tests/all/admin_accounts_for_organization.rs and tests/all/admin_disassociation_first_in_last_out.rs (the onboarding settling to `ONBOARDING_COMPLETE`, each waiting out `ADMIN_ONBOARDING_SPAN` through `admin_account_scopes::wait_out_the_onboarding`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs and read `OrganizationAdminRecord::settle` at lines 208-219.
2. Read `ThirdPartyFirewallRecord::settle` at lines 620-636 and compare: the `let due = ...` expression and the three assignments that clear the pending transition are identical.
3. Note src/state.rs:173-176, where the first record type's own doc says its pending-transition fields work "exactly as [`ThirdPartyFirewallRecord`] does" — one rule, two copies.
4. Change the due test in one copy only (for example `now > at` instead of `now >= at`) and run the test suite: the suites for the other record type still pass, so the two copies can drift apart undetected.
