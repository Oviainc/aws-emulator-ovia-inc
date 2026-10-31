Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4cd87268b802a4a9f30129e90266bba0b164f980

# AssociateAdminAccount reads the clock twice while writing the administrator slot, the one case crate::state says must take a single reading

_Filed: 2026-10-29T05:20:37Z_
_Operation: AssociateAdminAccount | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4cd87268b802a4a9f30129e90266bba0b164f980`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateAdminAccount` is a writer of the organisation's single administrator slot: it resolves the current association, decides from it, and then writes a new `AdminAccountRecord`. `crate::state` states the rule for exactly that shape of handler in two places — the clock-derived predicate and the mutable resolver both take the request's `now` precisely so "an operation that checks the association and then writes to it must not be able to see it active and then gone" — and the crate provides `active_admin_account_mut_at(now)` as the variant "an operation that CHANGES the administrator slot works on, so the write site neither re-derives the active-association predicate nor re-reads the clock after deciding".

This handler does neither. It calls the clock-reading convenience variant `store.active_admin_account()` (which takes its own `SystemTime::now()` internally, settles the pending tear-down at that instant and filters on it), and then takes a SECOND, independent `::std::time::SystemTime::now()` when it builds the record it persists. So one request decides from one reading of a non-monotonic clock and stamps the record it writes from another.

The sibling writer of the same slot, `DisassociateAdminAccount`, does it the documented way: one `now` for the whole request, passed into `active_admin_account_mut_at(now)`, with a comment stating the reason. Two handlers that write the same field therefore follow two different clock disciplines, and the one that breaks the rule is the one the rule's own doc names ("an operation that CHANGES the administrator slot").

The crate has already corrected this same class of defect once (the fixed issue "The administrator gate re-reads the clock inside a handler that states it decides from ONE reading") and still carries one open instance of it (`member_accounts()`), so the discipline is established; `AssociateAdminAccount` was simply never brought into it. Today the only value the second reading reaches is `associated_at`, which `AdminAccountRecord`'s own doc says is "recorded here but reported by no operation", so there is no response a client can see differ — which is why this is LOW — but the handler is the write site the rule exists for, and the next member read off that record inherits the straddle.

## Evidence

The rule, stated on the predicate — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:116-118:
```
    /// It takes `now` so one request decides from ONE reading of a clock that is
    /// not monotonic: an operation that checks the association and then writes to
    /// it must not be able to see it active and then gone.
    pub fn is_active_at(&self, now: SystemTime) -> bool {
```

The rule, stated on the resolver this handler should have used — src/state.rs:1282-1289:
```
    /// [`Self::active_admin_account`] as of `now`, mutably: the association an
    /// operation that CHANGES the administrator slot works on, so the write site
    /// neither re-derives the active-association predicate nor re-reads the clock
    /// after deciding.
    pub fn active_admin_account_mut_at(
        &mut self,
        now: SystemTime,
    ) -> Option<&mut AdminAccountRecord> {
```
and the variant it did use, which reads the clock itself — src/state.rs:1276-1280:
```
    pub fn active_admin_account(&mut self) -> Option<&AdminAccountRecord> {
        let now = SystemTime::now();
        self.settle_admin_account_at(now);
        self.admin_account.as_ref().filter(|a| a.is_active_at(now))
    }
```

The handler, with no `now` of its own — src/ops/associate_admin_account.rs:224-230:
```
        let mut store = state.lock();
        // `active_admin_account` settles what the clock owes before it answers, so a
        // disassociation already completed frees the administrator slot for this
        // request rather than blocking it.
        let current = store
            .active_admin_account()
            .map(|admin| (admin.account_id.clone(), admin.removal_pending()));
```
and the SECOND reading, at the write — src/ops/associate_admin_account.rs:266-272:
```
            None => {
                store.admin_account = Some(crate::state::AdminAccountRecord {
                    account_id: input.admin_account.clone(),
                    role_status: crate::state::ROLE_STATUS_READY.to_string(),
                    associated_at: ::std::time::SystemTime::now(),
                    removal_due: None,
                });
```

The sibling writer of the same slot, which follows the rule — src/ops/disassociate_admin_account.rs:244-247 and :261:
```
        // ONE reading of the clock for the whole request: this handler checks the
        // association and then writes to it, and `SystemTime` is not monotonic, so
        // a second reading could see it active and then gone.
        let now = ::std::time::SystemTime::now();
...
        let Some(admin) = store.active_admin_account_mut_at(now) else {
```

And the gate's own doc naming the same pairing as the precedent — src/state.rs:2404-2411:
```
/// A handler that already holds a reading — because it settles what the clock
/// owes, projects a status as of an instant, or writes a due instant — calls
/// [`require_administrator_at`] with THAT reading instead, the same way
/// [`ComAmazonawsFmsAwsfms20180101State::active_admin_account_mut_at`] exists
/// beside [`ComAmazonawsFmsAwsfms20180101State::active_admin_account`]: a clock
/// that is not monotonic must not be able to authorize a request at one instant
/// and answer it from another.
```

Test coverage that must stay green: tests/all/admin_account_disassociation.rs drives AssociateAdminAccount → DisassociateAdminAccount → AssociateAdminAccount round trips (lines 99-134, 151-245) and tests/all/administrator_gate_clock_reading.rs pins the one-reading rule for the gated handlers — its `gated_calls()` table (lines 292-367) deliberately does not include `AssociateAdminAccount`, so nothing pins this handler's clock discipline either way.

## Steps to Reproduce

1. Open src/state.rs:1282-1289 and read the doc on `active_admin_account_mut_at`: it exists for "an operation that CHANGES the administrator slot", so that "the write site neither re-derives the active-association predicate nor re-reads the clock after deciding".
2. Open src/ops/associate_admin_account.rs — a writer of `store.admin_account` — and note it never takes a `now`: line 229 calls `store.active_admin_account()`, the variant that reads `SystemTime::now()` itself (src/state.rs:1277).
3. Read src/ops/associate_admin_account.rs:270: `associated_at: ::std::time::SystemTime::now()` — a second, independent reading of the same non-monotonic clock inside one request, after the decision was made from the first.
4. Contrast src/ops/disassociate_admin_account.rs:244-247 and :261, the other writer of the same slot: one `now` for the whole request, passed into `active_admin_account_mut_at(now)`, with the rule restated in the comment.
5. Confirm the class has already been fixed elsewhere in this crate (fixed issue 20261029T025456Z-fms-listadminaccountsfororganization-wrong-behavior) and is still open for `member_accounts()` (20261028T211448Z-fms-batchassociateresource-wrong-behavior--r3), so AssociateAdminAccount is the remaining write site that was never brought into the discipline.
