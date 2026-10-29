Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: e47c691b8b899576dd7de95cf7f79b31f1a62071

# A repeated DisableOrganizationAdminAccount is accepted, writes nothing, and still consumes an instant of the state's monotonic clock

_Filed: 2026-10-29T17:22:08Z_
_Operation: DisableOrganizationAdminAccount | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e47c691b8b899576dd7de95cf7f79b31f1a62071`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisableOrganizationAdminAccount` goes through `ComAmazonawsGuarddutyGuarddutyapiserviceState::disable_organization_admin_account`, which reads the mutation clock (`tick_millis()`) and only THEN calls `OrganizationAdminAccountRecord::begin_disable`. `begin_disable` is documented — and implemented — as IDEMPOTENT: it writes the due instant only while none is pending, so a second disable of the same administrator writes nothing at all. The call is still answered `Some(true)` → HTTP 200 by the handler, and the instant `tick_millis()` handed out is spent: the store's "this resource changed now" counter is advanced by at least one millisecond for a call that left the store exactly as it found it.

That is the one thing the crate's own no-op rule forbids, and the sibling helper in the SAME `impl` block does it the other way round: `disassociate_detector_from_administrator` tests the no-write condition (`detector.administrator.is_none()`) BEFORE it ticks, and its doc states the discipline explicitly. The observable consequence is the one the crate's own `no_op_mutation_clock` suite describes: a burst of repeated disables inside one wall-clock millisecond pushes `last_millis` past the wall clock, after which the next ACCEPTED create or update stamps a `CreatedAt`/`UpdatedAt` in the future relative to real time — a value no caller's activity produced.

## Evidence

THE NO-WRITE PATH TICKS — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:1063-1078 (doc then code; the tick at :1074 precedes the write at :1076):
```
    /// `Some(true)`
    /// when the removal was accepted. The mutation clock is read only on the path
    /// that writes, so neither refusal advances it — the discipline
    /// [`Self::disassociate_detector_from_administrator`] keeps too.
    pub fn disable_organization_admin_account(
        &mut self,
        region: &str,
        admin_account_id: &str,
    ) -> Option<bool> {
        if self.organization_admin_account_in(region)?.admin_account_id != admin_account_id {
            return Some(false);
        }
        let now = self.tick_millis();
        if let Some(record) = self.organization_admin_accounts.get_mut(region) {
            record.begin_disable(now);
        }
        Some(true)
    }
```

THE WRITE IT TICKED FOR IS A NO-OP ON A REPEAT — src/state.rs:8395-8405:
```
    /// Accept a removal of this delegated administrator, due
    /// [`ADMIN_DISABLE_SPAN_MILLIS`] after `now`.
    ///
    /// Idempotent: a second removal of the same administrator leaves the instant
    /// the first one made due where it was, so repeating the call cannot postpone
    /// the removal for ever.
    pub fn begin_disable(&mut self, now: i64) {
        if self.disable_due_millis.is_none() {
            self.disable_due_millis = Some(now.saturating_add(ADMIN_DISABLE_SPAN_MILLIS));
        }
    }
```

AND IT IS ANSWERED AS A SUCCESS — src/ops/disable_organization_admin_account.rs:168-171:
```
            // The response models no members (`smithy_query(operation="DisableOrganizationAdminAccount")`):
            // `ListOrganizationAdminAccounts` is what reports the removal taking
            // effect.
            Some(true) => Ok(DisableOrganizationAdminAccountOutput {}),
```

THE SIBLING IN THE SAME IMPL BLOCK, WHICH CHECKS BEFORE IT TICKS — src/state.rs:275-278 and :290-293:
```
    /// The mutation clock ([`Self::tick_millis`]) is read ONLY on the path that
    /// writes, so no refusal advances it — which is what lets this be one walk in
    /// the handler instead of a resolve-then-tick-then-resolve-again one. The write
    /// itself is [`Detector::disassociate_from_administrator`].
...
        if detector.administrator.is_none() {
            return AdministratorDisassociation::NoAdministrator;
        }
        let now = self.tick_millis();
```

THE RULE THIS BREAKS, stated by the crate's own suite — tests/all/no_op_mutation_clock.rs:11-18:
```
//! The rule under test is the one the `refused_create_clock` and
//! `refused_mutation_clock` suites assert for the calls that are REFUSED, here over
//! the SET of calls that are ACCEPTED and still write nothing: a mutating operation
//! reads the service's monotonic "this resource changed now" clock only on the path
//! that actually writes, so a call which leaves the store exactly as it found it
//! consumes no instant either — and the `CreatedAt`/`UpdatedAt` a LATER accepted call
//! stamps therefore still reports the wall-clock instant of that change rather than
//! one pushed into the future by the no-ops before it.
```

TEST COVERAGE: tests/all/organization_admin_accounts.rs and tests/all/organization_admin_removal.rs exercise the designation, the removal and its clock-owed settling; the `no_op_mutation_clock` suite's rows are detector sub-resources and the detector's scan settings only (no row addresses `/admin/disable`), so nothing pins the current ordering and the fix — testing `disable_due_millis.is_none()` before ticking, exactly as `disassociate_detector_from_administrator` tests `administrator.is_none()` — changes no response.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector`) and designate a delegated administrator with `POST /admin/enable` for account 111122223333.
2. Call `POST /admin/disable` with `adminAccountId=111122223333` once: accepted (200), and `begin_disable` records the due instant (state.rs:8401-8404).
3. Send a burst of several hundred further identical `POST /admin/disable` calls within the removal span. Each is answered `Some(true)` → 200 (disable_organization_admin_account.rs:171) and each one has already reached `state.tick_millis()` (state.rs:1074), while `begin_disable` writes nothing because `disable_due_millis` is already `Some` (state.rs:8402).
4. Immediately create a sub-resource on the detector (e.g. `POST /detector/{detectorId}/ipset`) and read it back with `GetIPSet`, or read the detector with `GetDetector`.
5. Observe `createdAt`/`updatedAt` sitting ahead of real wall-clock time by roughly the number of no-op disables sent in the preceding millisecond — instants the crate's own no-op rule says those calls must not have consumed.
6. Contrast with a burst of repeated `POST /detector/{detectorId}/administrator/disassociate` calls, which run through `disassociate_detector_from_administrator` (state.rs:290-293) and consume no instant once the relationship is gone.
