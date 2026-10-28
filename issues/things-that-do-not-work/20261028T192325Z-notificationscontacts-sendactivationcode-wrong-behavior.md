Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# SendActivationCode stamps the stored activation-token instant from the raw clock, skipping the monotonic instant its sibling writers use

_Filed: 2026-10-28T19:23:25Z_
_Operation: SendActivationCode | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` offers two clock accessors with an explicitly documented division of labour: `ComAmazonawsNotificationscontactsNotificationscontactsState::next_instant_millis` for an operation that RECORDS a change (it is documented as "strictly later than every instant this store has already handed out"), and the free function `now_epoch_millis` for one that only COMPARES a stored instant against the clock "WITHOUT recording a change of its own".

Every other writing handler in this crate obeys that split: `CreateEmailContact` stamps `creation_time_millis`/`update_time_millis` with `store.next_instant_millis()`, and `ActivateEmailContact` reads `now_epoch_millis()` only for the expiry COMPARISON while stamping the status change it records with `store.next_instant_millis()`.

`SendActivationCode` is the one writer that does not: it holds the same `&mut` store guard (it has already called `store.fresh_activation_code()` on it, so `next_instant_millis()` is reachable) yet stamps the instant it PERSISTS on the record — `ActivationToken::issued(code, issued_at)`, the instant the documented 12-hour lifetime runs from — with the raw `now_epoch_millis()`. The stored `issued_at_millis` is therefore not guaranteed to be later than the instants the store has already handed out: a send that lands in the same millisecond as other store writes records an issue instant that can be earlier than the contact's own `creationTime`, i.e. a token "issued" before the contact it belongs to existed. The normalisation the crate built for exactly this case (the sibling writers' accessor) is skipped on the one write path that persists a timestamp outside `EmailContact`.

## Evidence

Documented split — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs:429-434:
```
    /// Now, in milliseconds since the Unix epoch, and strictly later than every
    /// instant this store has already handed out.
    pub fn next_instant_millis(&mut self) -> i64 {
```
and state.rs:573-579:
```
/// Now, in whole milliseconds since the Unix epoch.
///
/// Read by an operation that has to compare a stored instant against the clock
/// WITHOUT recording a change of its own, which is what
/// [`ComAmazonawsNotificationscontactsNotificationscontactsState::next_instant_millis`]
/// is for.
pub fn now_epoch_millis() -> i64 {
```

The handler that breaks it — src/ops/send_activation_code.rs:162-163 and :190 (inside `let mut store = state.lock();`, line 157):
```
        let code = store.fresh_activation_code();
        let issued_at = crate::state::now_epoch_millis();
...
        contact.activation_token = Some(crate::state::ActivationToken::issued(code, issued_at));
```
Its own comment at :160-161 states the recorded nature of the value: "The issue instant is what the code's documented lifetime runs from, so it is read from the clock here rather than inferred later."

Siblings that do apply the normalisation for the instants they record — src/ops/create_email_contact.rs:202-204:
```
        // One instant for both members: the contact was created and last updated at
        // the same moment, and nothing has changed it since.
        let created_at = store.next_instant_millis();
```
and src/ops/activate_email_contact.rs:172 vs :236 (comparison vs record, from the same handler):
```
        let now = crate::state::now_epoch_millis();
...
        let activated_at = code_matches.then(|| store.next_instant_millis());
```

State doc for the stamped field — src/state.rs:276-277: "/// When this code was issued, in milliseconds since the Unix epoch." (`ActivationToken::issued_at_millis`).

Test coverage: tests/all/activation_code_bounds.rs:57-66 (`age_token`) rewrites `token.issued_at_millis` directly and the lifetime rows at :147-158 compare it against the real clock, so no existing test pins WHICH accessor stamps the field — switching `SendActivationCode` to `store.next_instant_millis()` leaves every assertion in that suite, and every wire response (the instant is not serialized), unchanged.

## Steps to Reproduce

1. Read src/state.rs:429-434 and :573-579: the crate documents `next_instant_millis()` as the accessor for an operation that records a change and `now_epoch_millis()` as the one for an operation that compares without recording one.
2. Read src/ops/create_email_contact.rs:204 and src/ops/activate_email_contact.rs:236: both stamp the instants they persist with `store.next_instant_millis()`.
3. Read src/ops/send_activation_code.rs:157-163 and :190: the same handler holds a mutable store guard but stamps the `ActivationToken` it persists with `crate::state::now_epoch_millis()`.
4. Drive several CreateEmailContact calls inside one millisecond (e.g. the loop in tests/all/email_contacts.rs that stocks contacts), so `last_instant_millis` is ahead of the raw clock, then call SendActivationCode on one of those contacts in the same millisecond.
5. Read the store: the contact's `activation_token.issued_at_millis` is earlier than the same record's `creation_time_millis`, i.e. the stored issue instant is not strictly after the instants the store already handed out.
