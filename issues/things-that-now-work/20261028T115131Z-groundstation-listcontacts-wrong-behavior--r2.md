Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# ListContacts' ephemeris filter is built from the ephemeris-DELETE in-use rule, so it silently drops every terminal contact that uses the ephemeris

_Filed: 2026-10-28T11:51:31Z_
_Operation: ListContacts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `ephemeris` member of `ListContacts` is a filter over the contacts that USE an ephemeris. The handler implements it by reusing `crate::state::contacts_using_ephemeris`, which is the predicate `DeleteEphemeris` uses to decide whether an ephemeris is still IN USE. That predicate deliberately restricts itself to contacts that are not in a terminal status, because a terminal contact no longer holds a dependency on the ephemeris.

As a filter, that restriction is wrong: a contact that is CANCELLED, COMPLETED, FAILED, AWS_CANCELLED, AWS_FAILED or FAILED_TO_SCHEDULE still *used* the ephemeris, and `statusList` is free to ask for exactly those statuses. So `ListContacts` with `statusList:["CANCELLED"]` plus an `ephemeris` filter answers an EMPTY page, while the same request without the filter returns the contact. The filter does not narrow the selection — it erases it for the whole terminal half of the `ContactStatus` enum.

The handler's own comment admits the provenance of the predicate ("the same rule that keeps such an ephemeris from being deleted"), which is the rule that does not apply to a listing.

## Evidence

Smithy: `smithy_query(query="shape", shape="EphemerisFilter")` — `"doc": "<p>Filter for selecting contacts that use a specific ephemeris\".</p>"`, and `AzElEphemerisFilter` — "Filter for selecting contacts that use a specific <a>AzElEphemeris</a>." Nothing in the model restricts the filter to non-terminal contacts.

Handler, services/service-com-amazonaws-groundstation-groundstation/src/ops/list_contacts.rs:257-261:
```
        // Which contacts the `ephemeris` filter admits: the ones the ephemeris it names
        // points, by this service's one rule for that (`contacts_using_ephemeris`, the same
        // rule that keeps such an ephemeris from being deleted).
        let pointed_by_filter: Option<Vec<String>> =
            filter_ephemeris_id.map(|id| guard.contacts_using_ephemeris(id));
```
and the filter application, list_contacts.rs:304-308:
```
            .filter(|contact| {
                pointed_by_filter
                    .as_ref()
                    .is_none_or(|pointed| pointed.contains(&contact.contact_id))
            })
```

The borrowed predicate, services/service-com-amazonaws-groundstation-groundstation/src/state.rs:4600-4609:
```
        let mut holding: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| {
                contact.is_non_terminal()
                    && (contact.program_tracks_ephemeris(ephemeris_id)
                        || flown_satellite_arn
                            .as_deref()
                            .is_some_and(|arn| contact.flies_satellite(arn)))
            })
```
with its own doc stating why it is restricted that way, state.rs:4567-4570:
```
    /// contact depends on them for as long as it is not terminal
    /// ([`Contact::is_non_terminal`]) — once its pass is over or its reservation released,
    /// nothing of it is still to be flown.
```
and the terminal set, state.rs:1450-1457 (`TERMINAL_CONTACT_STATUSES` = AWS_CANCELLED, AWS_FAILED, CANCELLED, COMPLETED, FAILED, FAILED_TO_SCHEDULE) — all six of which `ListContacts` accepts in `statusList`.

Test coverage: the only test of this filter, services/service-com-amazonaws-groundstation-groundstation/tests/all/available_contacts.rs:1298 (`let stored_statuses = json!(["SCHEDULED", "SCHEDULING"]);`), exercises only non-terminal statuses, so no test pins the terminal case either way.

## Steps to Reproduce

1. Start the emulator with an on-boarded satellite that reaches a ground station (SATELLITES_ENV), and create a mission profile whose dataflow graph is valid.
2. CreateEphemeris with `satelliteId` = that satellite, `enabled: true`, az/el data for that ground station; wait for the validation to settle so the ephemeris is ENABLED.
3. ReserveContact against that mission profile, ground station and satellite ARN; note the contactId.
4. CancelContact that contactId, and wait until DescribeContact reports CANCELLED (a terminal status).
5. ListContacts with `statusList: ["CANCELLED"]` and a window covering the contact — the contact is returned.
6. ListContacts with the same `statusList` and window plus `ephemeris: {"azEl": {"id": <ephemerisId from step 2>}}` — the answer is an empty `contactList`, even though step 5 proves the very same contact is the one the ephemeris points.
