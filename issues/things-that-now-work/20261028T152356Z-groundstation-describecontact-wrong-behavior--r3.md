Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b45fe0b1030ed2b932d59ccf686ccc68d031b528

# A contact's `ephemeris` is reported absent even when the contact's own programTrackSettings name the ephemeris that points its antenna

_Filed: 2026-10-28T15:23:56Z_
_Operation: DescribeContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b45fe0b1030ed2b932d59ccf686ccc68d031b528`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every read of a stored contact hardcodes the response's `ephemeris` member to `None`: `DescribeContact` (src/ops/describe_contact.rs:231), `DescribeContactVersion` (src/ops/describe_contact_version.rs:283) and `ListContacts`' stored-contact items (src/ops/list_contacts.rs:330).

The model documents that member as the ephemeris that DETERMINES antenna pointing for the contact, and this crate already holds exactly that fact in a form that maps 1:1 onto the response shape: a contact version's `trackingOverrides.programTrackSettings` carries an `ephemerisId`, and `crate::state::program_track_reference` (src/state.rs:3213) hands back `(ephemeris_id, required_type)` — i.e. `EphemerisResponseData { ephemerisId, ephemerisType }`. `ReserveContact` and `UpdateContact` resolve that reference against the ephemeris store before they will store the version (`program_track_fault`, src/state.rs:4176), and `DeleteEphemeris` refuses to drop an ephemeris a contact program-tracks (`Contact::program_tracks_ephemeris`, src/state.rs:1613). So the service validates the reference, protects it from deletion, and then never reports it.

The crate also contradicts itself about what the member means. `ListContacts` fills `ephemeris` for a COMPUTED (`AVAILABLE`) window on the stated premise that the member names a determination ("the ephemeris that determines its antenna pointing is the one its geometry was read from", src/ops/list_contacts.rs:377-379, item built at src/ops/list_contacts.rs:422-425), while the two describes withhold it on the opposite premise — that it is an artefact a flown pass produced. A client that explicitly program-tracks a contact cannot read back which ephemeris its contact uses, although the same listing reports that for a window it has not even reserved yet.

## Evidence

Model (contradicts the crate's reading): `smithy_query(query="doc", shape="DescribeContactResponse", member="ephemeris")` -> "The ephemeris that determines antenna pointing directions for the contact."; `smithy_query(query="doc", shape="ContactData", member="ephemeris")` -> "The ephemeris that determines antenna pointing for the contact." The target shape is `EphemerisResponseData { ephemerisId ("Unique identifier of the ephemeris. Appears only for custom ephemerides."), ephemerisType (required) }`.

Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/describe_contact.rs:231`: `ephemeris: None,` (no comment). `src/ops/describe_contact_version.rs:275-283` states the crate's reading: "`ephemeris` is the trajectory the antenna was pointed by, which is that same unflown geometry. All of them are therefore reported absent rather than invented, exactly as `DescribeContact` reports them." and then `ephemeris: None,`.

Sibling path in the SAME crate reads the member the other way: `src/ops/list_contacts.rs:377-379` — "// The same filter, on a computed window: the ephemeris that determines // its antenna pointing is the one its geometry was read from." — and `src/ops/list_contacts.rs:422-425` builds `ephemeris: Some(EphemerisResponseData { ephemeris_id: Some(pass.ephemeris_id), ephemeris_type: pass.ephemeris_type })`.

The data is already stored and already resolved: `src/state.rs:3191-3227` `pub fn program_track_reference(overrides: &TrackingOverrides) -> Option<ProgramTrackReference>` yields `ephemeris_id` + `required_type` (the `EphemerisType` the member requires); `src/state.rs:4176` `program_track_fault` refuses a write whose `ephemerisId` names no ephemeris of the account; `src/state.rs:1613` `program_tracks_ephemeris` is what keeps that ephemeris from being deleted.

Test coverage: `tests/all/contact_version_description.rs:79-89` lists `ephemeris` in `UNFLOWN_MEMBERS` with the reason "the ephemeris it was pointed by. Nothing in this emulator flies a pass, so a description reports none of them." That stated reason is the reading the model sentence above contradicts (the member is what *determines* pointing, which is settled when the version is stored, not what a flown pass measured), and it is also the opposite of the reading `list_contacts.rs:377` applies to the same member.

## Steps to Reproduce

1. CreateEphemeris with an `azEl` ephemeris (inline segments) for an onboarded satellite; note the returned `ephemerisId`.
2. CreateConfig + CreateMissionProfile, then ReserveContact against that profile with `trackingOverrides: {programTrackSettings: {azEl: {ephemerisId: <that id>}}}` — accepted only because the handler resolved the reference.
3. GET /contact/{contactId} (DescribeContact): the response carries no `ephemeris` member, although the contact's antenna pointing is determined by the ephemeris just named (and DeleteEphemeris on that id now refuses with ResourceInUseException, proving the service knows the contact uses it).
4. GET /contact/{contactId}/versions/{versionId} (DescribeContactVersion): same absent member.
5. POST /contacts with statusList ["AVAILABLE", "SCHEDULING"], the same groundStation/satelliteArn/missionProfileArn: the computed AVAILABLE items DO carry `ephemeris`, the stored contact item does not.
