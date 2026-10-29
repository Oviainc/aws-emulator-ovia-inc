Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d0484a4a393e00c66ab2f2d9da27b0764a19da5b

# The clock-owed TRN settle rule is implemented twice, once per stored-registration struct, so the two copies can drift

_Filed: 2026-10-29T00:53:42Z_
_Operation: GetTaxRegistration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d0484a4a393e00c66ab2f2d9da27b0764a19da5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a registration whose validation was already due at `now` moves to the status that validation reached, and settling is idempotent because the due instant is cleared" is written out twice in `state.rs`, character for character: once on `StoredSupplementalRegistration` and once on `StoredTaxRegistration`. The two bodies are identical (only the struct they sit on differs), and the paired `validation_outcome` / `from_entry` initialisers around them restate the same pairing of `STATUS_PENDING` + `validation_due_at = written_at + TRN_VALIDATION_DELAY`. `settle_due` already calls both copies in one loop, so the single caller cannot keep them in step: a change to the settle rule (another status that must not move, a second clock-owed transition, clearing an extra field) has to be made in two places and silently diverges if only one is edited. Folding the duplicated body into one helper (a small private fn or trait taking `&mut Option<Instant>`, `&mut String` and the outcome) leaves every response and every existing test byte-identical.

## Evidence

`services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:995-1006` (on `StoredSupplementalRegistration`):
```
    /// Move this registration to the status its validation reached, if that
    /// validation was already due at `now`. Answers whether it moved.
    pub fn settle(&mut self, now: Instant) -> bool {
        match self.validation_due_at {
            Some(due) if due <= now => {
                self.status = self.validation_outcome().to_string();
                self.validation_due_at = None;
                true
            }
            _ => false,
        }
    }
```
`services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1149-1160` (on `StoredTaxRegistration`) is the same text:
```
    /// Move this registration to the status its validation reached, if that
    /// validation was already due at `now`. Answers whether it moved.
    pub fn settle(&mut self, now: Instant) -> bool {
        match self.validation_due_at {
            Some(due) if due <= now => {
                self.status = self.validation_outcome().to_string();
                self.validation_due_at = None;
                true
            }
            _ => false,
        }
    }
```
Both are driven from the one caller, `state.rs:606-613`: `pub fn settle_due(&mut self, now: Instant) { for stored in self.registrations.values_mut() { stored.settle(now); } for stored in self.supplemental_registrations.values_mut() { stored.settle(now); } }` — reached from `src/ops/get_tax_registration.rs:156`, `src/ops/list_tax_registrations.rs:199` and `src/ops/list_supplemental_tax_registrations.rs:189`, and per-account from `state.rs:652` inside `store_registration`.

Existing tests that pin the behaviour both copies must keep: `tests/all/fidelity_rules.rs` and `tests/all/trn_validation_outcome.rs` (the main TRN's Pending -> Verified/Rejected transition once `TRN_VALIDATION_DELAY` has elapsed), `tests/all/accepted_registration_verifies.rs`, and `tests/all/supplemental_tax_registrations.rs` (the supplemental TRN's status after the same delay).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs` and compare lines 995-1006 with lines 1149-1160: the two `settle` bodies are identical
2. Note the single caller `settle_due` (state.rs:606-613) invokes both, so neither copy is specialised
3. Change the settle rule in one copy only (for example, refuse to move a record whose status is no longer `STATUS_PENDING`) and observe that the other kind of registration keeps the old rule, with no test or type error flagging the divergence
