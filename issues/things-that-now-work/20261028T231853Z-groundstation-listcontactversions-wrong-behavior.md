Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# A version that is neither superseded nor newest reports the CONTACT's lastUpdated, so appending a version moves the lastUpdated of the version before it

_Filed: 2026-10-28T23:18:53Z_
_Operation: ListContactVersions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Contact::version_record` is the one projection every reader of a contact version goes through (`ListContactVersions` via `version_history`, `DescribeContactVersion` for the named version, `DescribeContact`/`ListContacts` for the newest one). It derives `ContactVersion$lastUpdated` as `record.superseded.unwrap_or(self.last_updated)` under a comment that states a two-case rule: a superseded version reports when it was superseded, and "the newest version … lastUpdated is the contact's own".

There is a third, routinely reachable case the code silently folds into the second: a version that is NOT superseded and NOT the newest. That is exactly the state of the ACTIVE version while an `UpdateContact` is still being applied — the model says a version is superseded only "when a newer version becomes active" (`ContactVersion$superseded`), and this crate deliberately holds the appended version in `UPDATING` for `VERSION_ACTIVATES_AFTER` so that the transient state is observable. During that span the older, still-ACTIVE version reports `lastUpdated` = `contact.last_updated`, which `append_version` has just set to the instant the NEW version was created (src/state.rs:1963). Nothing about the older version changed — its `status`, `activated` and `superseded` are untouched — yet its reported `lastUpdated` jumps forward, and `ListContactVersions` reports the two records carrying the identical `lastUpdated`.

The same mis-attribution applies to a pending version that a newer pending version overtook — a state `apply_due_versions`' own comment says exists ("including one still pending, which a newer version overtook before it could take force", src/state.rs:1999-2000) — and it is reachable because `UpdateContact` gates on the CONTACT's status (`STATUSES_OPEN_TO_UPDATE`), not on the previous version having taken force.

The crate already holds the per-version instants it needs for this case (`record.created`, `record.activated`), so the fault is that the projection has no branch for a version the contact has moved past but not yet superseded.

## Evidence

The projection and the rule it states, which the code then breaks — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:1898-1915`:
```
    /// `record` as a `ContactVersion` response member.
    pub fn version_record(&self, record: &ContactVersionRecord) -> ContactVersion {
        ContactVersion {
            version_id: Some(record.version_id),
            created: Some(record.created),
            activated: record.activated,
            superseded: record.superseded,
            // A version that has been superseded was last touched when it was
            // superseded; the newest version is the one every later change to the
            // contact lands on, so its lastUpdated is the contact's own.
            last_updated: Some(record.superseded.unwrap_or(self.last_updated)),
```
(the `unwrap_or` gives the "newest version" treatment to EVERY version whose `superseded` is `None`, including ones that are not the newest).

The write that moves the contact's stamp when a DIFFERENT version is appended — `src/state.rs:1952-1964`:
```
        self.versions.push(ContactVersionRecord {
            version_id,
            ...
            status: UPDATING_VERSION_STATUS.to_string(),
            client_token,
            activates_at: Some(SystemTime::now() + VERSION_ACTIVATES_AFTER),
        });
        self.last_updated = now;
```

The crate's own statement that a non-newest version can still be un-superseded — `src/state.rs:1998-2003` (`apply_due_versions`):
```
            } else if position < activating && version.superseded.is_none() {
                // Every older version is replaced — including one still pending, which
                // a newer version overtook before it could take force.
                version.status = SUPERSEDED_VERSION_STATUS.to_string();
                version.superseded = Some(stamp);
```
so `superseded` is set only once a newer version ACTIVATES — matching the model: `smithy_query(query="shape", shape="ContactVersion")` -> `superseded`: "Time the contact version was superseded in UTC. A version is superseded when a newer version becomes active.", `lastUpdated`: "Time the contact version was last updated in UTC.", and `smithy_query(query="shape", shape="VersionStatus")` -> `UPDATING`: "The version is being applied to the contact."

The readers that surface it: `src/ops/list_contact_versions.rs:206` (`let versions = contact.version_history();`, which is `src/state.rs:1924-1929` mapping EVERY stored record through `version_record`) and `src/ops/describe_contact_version.rs:268` (`version: Some(contact.version_record(version)),`).

Test coverage: nothing pins a version's `lastUpdated` anywhere in `tests/` (`grep -n 'lastUpdated' tests/` matches only `agent_lifecycle.rs`, about an AGENT's `last_updated`). The pending-update case is exercised without looking at the member — `tests/all/contact_version_listing.rs:549-567` asserts only the `versionId`s and that the appended record is `UPDATING` or `ACTIVE` — while the suite does assert the neighbouring invariant for `created`, `tests/all/contact_version_listing.rs:592-597`: "case settled update: a version's created instant must not move". No test or comment justifies a version's `lastUpdated` moving because another version was created.

## Steps to Reproduce

1. Seed one on-boarded satellite, `CreateConfig` + `CreateMissionProfile`, then `POST /contact` (ReserveContact) and wait for the `SCHEDULING` -> `SCHEDULED` settle.
2. `GET /contact/{contactId}/versions` (ListContactVersions): note version 1's `lastUpdated` (call it L1), `status` ACTIVE, `superseded` absent.
3. `POST /contact/{contactId}/versions` (UpdateContact) naming a different `satelliteArn`, which appends version 2 in `UPDATING`.
4. `GET /contact/{contactId}/versions` again, WITHIN the activation span: version 1 still reports `status` ACTIVE with no `superseded`, but its `lastUpdated` is no longer L1 — it is the instant version 2 was created, identical to version 2's own `created`/`lastUpdated`, although nothing about version 1 changed.
5. Wait past the span and read once more: version 1 now reports `SUPERSEDED` and its `lastUpdated` moves again, to the activation instant — so the only value it never reports is the instant version 1 itself was last touched.
