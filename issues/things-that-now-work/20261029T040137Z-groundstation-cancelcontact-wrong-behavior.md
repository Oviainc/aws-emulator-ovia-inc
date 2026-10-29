Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# A cancel withdraws only one of the two clock-owed moves it holds, so a released contact still takes a version change (and a new lastUpdated) after it was cancelled

_Filed: 2026-10-29T04:01:37Z_
_Operation: CancelContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelContact` releases a reservation and, for a contact whose pass has not begun, writes CANCELLED outright. `Contact::begin_cancellation` explicitly withdraws the scheduler's pending move at that point (`self.scheduling_settles_at = None`, documented as "the scheduler no longer owes it its `SCHEDULING` -> `SCHEDULED` move"), but it leaves the OTHER clock-owed move the contact may be carrying — a version appended by `UpdateContact` with `activates_at` still in the future — armed.

`settle_due_contacts` calls `apply_due_versions()` on every contact unconditionally, with no status test, so once that instant passes the pending version of the already-released contact is flipped to ACTIVE, the version before it to SUPERSEDED, and `contact.last_updated` is stamped with the current instant — after the cancellation. A later `DescribeContact` / `DescribeContactVersion` / `ListContactVersions` of the CANCELLED contact therefore reports a different "current active version of the contact" (and a `lastUpdated` later than the release) than the cancel left behind.

That contradicts the gate `UpdateContact` applies to the very same question: a contact whose pass is no longer ahead of it "takes no new version", so a caller cannot START a version change on a cancelled contact — while one already in flight is still applied to it. By the crate's own reading of a released reservation ("once its pass is over or its reservation released, nothing of it is still to be flown"), there is nothing left for a version to be applied to.

## Evidence

The cancel withdraws exactly one owed move — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:1807-1818`:
```
    pub fn begin_cancellation(&mut self, now: DateTime) {
        if STATUSES_CANCELLABLE_DURING_PASS.contains(&self.contact_status.as_str()) {
            self.antenna_time_spent = true;
            self.contact_status = CONTACT_CANCELLING.to_string();
            self.cancellation_settles_at = Some(SystemTime::now() + CANCELLATION_SETTLES_AFTER);
        } else {
            self.contact_status = CONTACT_CANCELLED.to_string();
            self.cancellation_settles_at = None;
        }
        self.scheduling_settles_at = None;
        self.last_updated = now;
    }
```
with its own doc stating that withdrawal as the point of the branch — `src/state.rs:1797-1799`:
```
    /// A contact whose pass has yet to start is `CANCELLED` outright: there is nothing to
    /// stop, and the scheduler no longer owes it its `SCHEDULING` -> `SCHEDULED` move.
```
Nothing clears `ContactVersionRecord::activates_at`, which `UpdateContact` armed — `src/state.rs:1992-2002` (`append_version`):
```
        self.versions.push(ContactVersionRecord {
            version_id,
            ...
            status: UPDATING_VERSION_STATUS.to_string(),
            client_token,
            activates_at: Some(SystemTime::now() + VERSION_ACTIVATES_AFTER),
        });
```
and the settle applies it to EVERY contact, whatever status it now holds — `src/state.rs:5142-5143`:
```
        for contact in self.contacts.values_mut() {
            contact.apply_due_versions();
```
which rewrites the version statuses and the contact's `lastUpdated` — `src/state.rs:2033-2046`:
```
        for (position, version) in self.versions.iter_mut().enumerate() {
            if position == activating {
                version.status = ACTIVE_VERSION_STATUS.to_string();
                version.activated = Some(stamp);
                version.activates_at = None;
            } else if position < activating && version.superseded.is_none() {
                ...
        self.last_updated = stamp;
```

The same question answered the other way one handler over — `src/ops/update_contact.rs:212-227`:
```
        // An update changes what the antenna will do during the pass, so it is only
        // answered while the pass is still ahead of the contact.
        if !crate::state::STATUSES_OPEN_TO_UPDATE.contains(&contact.contact_status.as_str()) {
            return Err(UpdateContactErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(format!(
                        "a contact in {} takes no new version; only a contact whose pass has \
                         not begun ({}) does",
```
(`STATUSES_OPEN_TO_UPDATE` = `["SCHEDULED", "SCHEDULING"]` — `src/state.rs:1501`.)

And the crate's reading of what a released reservation still holds — `src/state.rs:4963-4968`:
```
    /// The contacts it points ([`Self::contacts_pointed_by_ephemeris`]) that are still
    /// going somewhere: a contact depends on what it is flown with for as long as it is not
    /// terminal ([`Contact::is_non_terminal`]) — once its pass is over or its reservation
    /// released, nothing of it is still to be flown
```

Test coverage: `tests/all/owed_transition_spans.rs` and `tests/all/contact_versions.rs` exercise the version `UPDATING` -> `ACTIVE` span and `tests/all/contact_cancellation.rs` the release, but no suite appends a version and then cancels the contact before that span elapses, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /missionprofile, then POST /contact (ReserveContact) for a future window; wait for the contact to settle to SCHEDULED.
2. POST /contact/{contactId}/versions (UpdateContact) with a `satelliteArn` (or `trackingOverrides`) — answered 200 with versionId 2, stored UPDATING with its activation owed a few seconds out.
3. Immediately DELETE /contact/{contactId} (CancelContact) — answered 200; GET /contact/{contactId} reports contactStatus CANCELLED, version 2 still UPDATING, and a `lastUpdated` at the cancel.
4. Wait out `state::VERSION_ACTIVATES_AFTER`, then GET /contact/{contactId} again: the CANCELLED contact now reports `version.status` = ACTIVE with an `activated` stamp AFTER the cancellation, GET /contact/{contactId}/versions reports version 1 SUPERSEDED, and the contact's `lastUpdated` has moved past the release.
5. Try the write that would produce that same state directly — POST /contact/{contactId}/versions on the CANCELLED contact — and it is refused 431 "a contact in CANCELLED takes no new version".
