Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a2d8365760a86e4759c409db3cefda62d7096263

# The administrator gate re-reads the clock inside a handler that states it decides from ONE reading, so a request is authorized at one instant and answered from another

_Filed: 2026-10-29T02:54:56Z_
_Operation: ListAdminAccountsForOrganization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a2d8365760a86e4759c409db3cefda62d7096263`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAdminAccountsForOrganization` takes a single `now` at the top of `handle` and says, in a comment directly above it, that the whole request decides from that ONE reading because `SystemTime` is not monotonic and "a second reading could straddle a settle boundary". The very next statement is `crate::state::require_administrator(&store, &region)?`, which decides the gate through `AdminAccountRecord::is_active()` — and `is_active()` is defined as `is_active_at(SystemTime::now())`, i.e. a SECOND reading of the clock inside the same request.

So this handler authorizes the call at instant T2 (the gate's own reading) and then settles and reports every administrator's status at instant T1 (`settle_admin_account_at(now)`, `settle_organization_admins_at(now)`, `admin_account_summaries(now)`). With a disassociation pending, `AdminAccountRecord::removal_due` can fall between T1 and T2: the gate then refuses the call with `AccessDeniedException` although the request's own `now` still has the association active and `admin_account_summaries(now)` would have reported it `OFFBOARDING`; and because `SystemTime` is explicitly documented here as non-monotonic, the reverse straddle admits a call whose `now`-based settle writes `ROLE_STATUS_DELETED` and reports the administrator as removed immediately after the gate said it exists. Either way the two readings disagree inside one request, which is precisely what `AdminAccountRecord::is_active_at`'s own doc says must not be possible.

The crate already has the right shape for this and uses it elsewhere: `ComAmazonawsFmsAwsfms20180101State::active_admin_account_mut_at(now)` exists so "the write site neither re-derives the active-association predicate nor re-reads the clock after deciding", and `DisassociateAdminAccount` takes it with the request's own `now`. `require_administrator` has no `_at` counterpart, so every gated handler that already holds a `now` re-reads the clock: `PutAdminAccount` (230/237), `GetAdminScope` (193/199), `PutPolicy` (496/503), `PutResourceSet` (201/205), `PutAppsList` (195/199), `PutProtocolsList` (196/200), `BatchAssociateResource` (220/230), `BatchDisassociateResource` (205/215), `ListThirdPartyFirewallFirewallPolicies` (208/211), `AssociateThirdPartyFirewall` (183/186), `DisassociateThirdPartyFirewall` (177/180), `GetThirdPartyFirewallAssociationStatus` (191/194). `PutAdminAccount` and `ListAdminAccountsForOrganization` are the two that then WRITE from the first reading.

This is a different helper from the already-filed `member_accounts()` clock re-read (20261028T211448Z-fms-batchassociateresource-wrong-behavior--r3): that one is the organisation-roster derivation, this one is the authorization gate itself, and fixing `member_accounts()` leaves `require_administrator` re-reading.

## Evidence

The rule this handler states for itself — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_admin_accounts_for_organization.rs:171-175:
```
        // ONE reading of the clock for the whole request: this handler decides
        // every administrator's status from it and then writes the transitions
        // that reading made due, and `SystemTime` is not monotonic, so a second
        // reading could straddle a settle boundary.
        let now = ::std::time::SystemTime::now();
```

The second reading, six lines later — src/ops/list_admin_accounts_for_organization.rs:178-181:
```
        // The authorization gate, BEFORE the pagination token is examined: this
        // call is organisation-scoped, so a caller that is not entitled to it
        // cannot learn anything about what the organization holds.
        crate::state::require_administrator(&store, &region)?;
```

…because the gate decides through the clock-reading predicate — src/state.rs:2339-2348:
```
pub fn require_administrator<'store, E>(
    store: &'store ComAmazonawsFmsAwsfms20180101State,
    region: &str,
) -> Result<&'store AdminAccountRecord, crate::scaffolded::Scaffolded<E>> {
    store
        .admin_account
        .as_ref()
        .filter(|admin| admin.is_active())
        .ok_or_else(|| no_administrator_refusal(region))
}
```
and src/state.rs:78-82:
```
    /// [`Self::is_active_at`] at the current instant, for a caller that makes ONE
    /// decision from it.
    pub fn is_active(&self) -> bool {
        self.is_active_at(SystemTime::now())
    }
```

The invariant both sides are measured against — src/state.rs:72-75:
```
    /// It takes `now` so one request decides from ONE reading of a clock that is
    /// not monotonic: an operation that checks the association and then writes to
    /// it must not be able to see it active and then gone.
```

The first reading is what the writes and the response use — src/ops/list_admin_accounts_for_organization.rs:204-210:
```
        store.settle_admin_account_at(now);
        store.settle_organization_admins_at(now);
...
        let summaries = store.admin_account_summaries(now);
```

The `_at` helper the crate already has for exactly this, and the sibling that uses it — src/state.rs:1237-1247:
```
    /// [`Self::active_admin_account`] as of `now`, mutably: the association an
    /// operation that CHANGES the administrator slot works on, so the write site
    /// neither re-derives the active-association predicate nor re-reads the clock
    /// after deciding.
    pub fn active_admin_account_mut_at(
        &mut self,
        now: SystemTime,
    ) -> Option<&mut AdminAccountRecord> {
        self.settle_admin_account_at(now);
        self.admin_account.as_mut().filter(|a| a.is_active_at(now))
    }
```
src/ops/disassociate_admin_account.rs:247 and 261:
```
        let now = ::std::time::SystemTime::now();
...
        let Some(admin) = store.active_admin_account_mut_at(now) else {
```

Test coverage today: tests/all/admin_accounts_for_organization.rs drives the gate (lines 196-208) and the enumeration, and tests/all/admin_account_disassociation.rs pins the `PENDING_DELETION` → `DELETED` span, but no test exercises a call whose gate reading and `now` straddle `DISASSOCIATION_SPAN`, so nothing pins the two readings being allowed to differ — adding a `require_administrator_at(store, region, now)` leaves every existing assertion true.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with `{"AdminAccount": "<the emulator account id>"}` → 200.
2. Call `PutAdminAccount` with `{"AdminAccount": "222222222222"}` so the organisation holds a delegated administrator too.
3. Call `DisassociateAdminAccount` → 200; the record now carries `role_status = PENDING_DELETION` and `removal_due = now + crate::state::DISASSOCIATION_SPAN` (500 ms, state.rs:296). (With the delegated admin onboarded this call is refused by first-in-last-out; drop step 2 to get the pending state, or reach the same state in a unit test by setting `removal_due` directly.)
4. Call `ListAdminAccountsForOrganization` timed so that `removal_due` falls between `list_admin_accounts_for_organization.rs:175` (`let now = SystemTime::now()`) and the gate's own `SystemTime::now()` inside `require_administrator` at line 181.
5. Expected, per the comment at lines 171-174: the whole request decides from the single `now`, so the association is still active and the response reports the default administrator with `Status: OFFBOARDING`.
6. Actual: the gate's later reading sees `now >= removal_due`, `is_active()` is false, and the call is refused 400 `AccessDeniedException` — authorized at one instant, answered from another.
