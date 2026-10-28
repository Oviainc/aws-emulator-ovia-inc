Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# The "an accepted primary email update's completion is due" rule is written out four times in PrimaryEmailUpdate

_Filed: 2026-10-28T14:51:52Z_
_Operation: GetPrimaryEmailUpdateStatus | Class: maintainability_

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PrimaryEmailUpdate` stores one clock fact — `completes_at`, "The instant this update's `COMPLETED` status is DUE" (services/service-com-amazonaws-account-account/src/state.rs:685-694) — and the single rule derived from it ("the completion is owed when `completes_at` is set and `now` is at or past it") is spelled out separately in four methods of the same `impl` block: `settled_status`, `settled_address`, `settled_updated_at` and `settle`.

The four copies are what the status read, the address read and the two mutating primary-email operations each decide by, so they must agree exactly: `GetPrimaryEmailUpdateStatus` answers `Status` from `settled_status` and `UpdatedAt` from `settled_updated_at`, `GetPrimaryEmail` answers from `settled_address` (through `Account::settled_primary_email`), and `StartPrimaryEmailUpdate`/`AcceptPrimaryEmailUpdate` write through `settle`. Change the comparison in one copy and a reader would report `COMPLETED` while the address read still reports the old address, or the store would write the completion at an instant the readers do not agree is due — a drift between two answers about one stored instant.

The crate already holds the opposite standard for every other rule it cannot read off the model: `region_opt_target` is documented as "The ONE statement of that rule. `EnableRegion` and `DisableRegion` both decide through it ... so which Regions count as opt targets ... changes here alone and cannot drift between the two operations" (state.rs:574-577), and `validated_contact_information` as "The ONE statement of the rules the model cannot express about a `ContactInformation`" (state.rs:277-278). The due-completion rule is the one clock rule of this service that is not stated once.

The fix is internal: one private accessor (e.g. `fn due_completion(&self, now) -> Option<SystemTime>` returning `self.completes_at.filter(|due| now >= *due)`) that the four methods call. Every response and every existing test stays byte-for-byte unchanged.

## Evidence

Four copies of the same comparison over the same field, all in `impl PrimaryEmailUpdate` in services/service-com-amazonaws-account-account/src/state.rs:

1. state.rs:746-752 — `pub fn settled_status(&self, now: SystemTime) -> &str {` / `if self.completes_at.is_some_and(|due| now >= due) {` / `COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS`
2. state.rs:760-764 — `pub fn settled_address(&self, now: SystemTime) -> Option<&str> {` / `self.completes_at` / `.is_some_and(|due| now >= due)` / `.then_some(self.requested_email.as_str())`
3. state.rs:769-774 — `pub fn settled_updated_at(&self, now: SystemTime) -> SystemTime {` / `match self.completes_at {` / `Some(due) if now >= due => due,` / `_ => self.updated_at,`
4. state.rs:784-793 — `pub fn settle(&mut self, now: SystemTime) -> Option<String> {` / `let due = self.completes_at?;` / `if now < due {` / `return None;` / `}` (then writes `COMPLETED`)

The one stored fact they all re-derive, state.rs:685-688: "/// The instant this update's `COMPLETED` status is DUE, set when /// `AcceptPrimaryEmailUpdate` moves it to `ACCEPTED`, and `None` for a /// status the clock does not move on".

The crate's own standard for a rule it decides itself, state.rs:574-577: "/// The ONE statement of that rule. `EnableRegion` and `DisableRegion` both /// decide through it, before either resolves an account or touches the store, /// so which Regions count as opt targets — and which refusal every other case /// earns — changes here alone and cannot drift between the two operations."

Callers that would observe a drift between the copies:
- services/service-com-amazonaws-account-account/src/ops/get_primary_email_update_status.rs:220-225 — `status: update.settled_status(now).to_string(), updated_at: Some(::aws_smithy_types::DateTime::from(update.settled_updated_at(now)))`
- services/service-com-amazonaws-account-account/src/ops/get_primary_email.rs:209-211 — `primary_email: Some(account.settled_primary_email(now).to_string())` (which calls `settled_address`, state.rs:1032-1037)
- services/service-com-amazonaws-account-account/src/ops/start_primary_email_update.rs:226 and src/ops/accept_primary_email_update.rs:223 — `store.settle_primary_email_updates(now);` (which calls `settle`, state.rs:1015-1023)

Existing test coverage that pins the behaviour the refactor must not change: tests/all/primary_email_update_acceptance.rs:383-395 ("an accepted update must settle to {COMPLETED}" and "a completed update must stay {COMPLETED}"), tests/all/primary_email_read.rs:14 ("An update changes what the read reports only once it has COMPLETED") and :425-426, tests/all/primary_email_update_in_flight.rs:391-393, tests/all/primary_email_update_address_in_use.rs:374-376, tests/all/primary_email_update_self_reaffirmation.rs:456-463, tests/all/one_time_passcode_confirmation.rs (ACCEPTED→COMPLETED settling rows).

## Steps to Reproduce

1. Open services/service-com-amazonaws-account-account/src/state.rs and read `impl PrimaryEmailUpdate`.
2. Observe `settled_status` (line 747), `settled_address` (lines 761-762), `settled_updated_at` (lines 770-771) and `settle` (lines 785-787) each re-derive "`completes_at` is set and `now >= due`" in their own form.
3. Change one copy only — e.g. make `settled_status` use `now > due` instead of `now >= due` — and the whole crate still compiles.
4. At the exact due instant the service then answers `ACCEPTED` from `GetPrimaryEmailUpdateStatus` while `GetPrimaryEmail` already answers the new address from `settled_address`: two answers about one stored instant disagree, which is the drift a single statement of the rule prevents.
5. Compare with state.rs:574-577, where the crate states the same kind of rule once so it "cannot drift between the two operations".
