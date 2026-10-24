Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5c8d6c13f19e0574ca93569a583e30c59c58b477

# ListContacts refuses the modeled ephemeris filter with 431 and never returns AVAILABLE contact windows

_Filed: 2026-10-24T00:13:24Z_
_Operation: ListContacts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5c8d6c13f19e0574ca93569a583e30c59c58b477`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListContacts is implemented, but two parts of its contract produce answers AWS does not give.

(a) The modeled optional input member `ephemeris` (an `EphemerisFilter` union with member `azEl`) is treated as an unsupported request: the handler returns the operation's `InvalidParameterException` (HTTP 431) for ANY request that carries it, before looking at state. Expected: the member is a documented, optional FILTER — AWS answers 200 with the contact list narrowed to contacts using that ephemeris (an empty `contactList` when none do). Actual: HTTP 431 `InvalidParameterException` with message "ephemeris filtering is not available: ...". A client that always sends its filter union (SDK-generated callers commonly do) cannot list contacts at all, and cannot distinguish "no contact uses this ephemeris" from "the request was rejected".

(b) `statusList: ["AVAILABLE"]` can never return anything. The handler validates the documented AVAILABLE precondition (groundStation + missionProfileArn + satelliteArn all present) and then filters the STORED contact map by `contact_status == "AVAILABLE"`. No code in the crate ever creates a contact with status AVAILABLE (`ReserveContact` admits `SCHEDULING`, `settle_due_contacts` moves it to `SCHEDULED`, `CancelContact` to `CANCELLED`), so the response is always an empty `contactList` with no `nextToken`. Expected: AWS computes candidate (unreserved) contact windows for the named satellite/ground station/mission profile inside the requested time window and returns them as `ContactData` entries with status AVAILABLE, each carrying startTime/endTime/prePassStartTime/postPassEndTime/maximumElevation — this is the discovery step of the canonical workflow (ListContacts AVAILABLE → ReserveContact). In the emulator that workflow has no first step, so a caller must invent window times instead of reserving a window the service offered.

This is separate from the already-open DescribeContact lifecycle issue (which covers SCHEDULED never advancing to PREPASS/PASS/POSTPASS/COMPLETED); here the gap is the AVAILABLE side of the status set and the ephemeris filter, neither of which that issue addresses.

## Evidence

Spec: `smithy_query(query="operation", operation="ListContacts")` lists `ephemeris` as a plain optional input member (target `EphemerisFilter`, a union whose doc reads "Filter for selecting contacts that use a specific ephemeris") — the model declares no condition under which supplying it is an error, and the operation's only client error is `InvalidParameterException` for invalid parameters. `smithy_query(query="shape", shape="ContactStatus")` includes `AVAILABLE`, and the operation doc quoted in the handler itself ("If statusList contains AVAILABLE, the request must include groundStation, missionprofileArn, and satelliteArn") exists precisely because an AVAILABLE search returns computed availability for one antenna/satellite/profile pairing.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/list_contacts.rs` — lines ~171-183 return `InvalidParameterException` whenever `input.ephemeris.is_some()` (with a `TODO` admitting it is a placeholder); lines ~266-294 build the page by filtering only `guard.contacts`, with `.filter(|c| input.status_list.contains(&c.contact_status))`, so AVAILABLE selects nothing. `src/state.rs` has no satellite, antenna or ground-station catalog from which availability could be derived.
Tests: `tests/all/scheduling_loop.rs` lines ~1405-1430 assert the CURRENT 431 answer for an `ephemeris` filter, and lines ~1507-1520 assert that a well-formed AVAILABLE search returns an empty list ("this emulator holds no AVAILABLE contact (nothing computes availability windows)") — i.e. the tests pin the divergence rather than the AWS behavior; no test asserts the AWS answer.

## Steps to Reproduce

1. Create a tracking config and a mission profile referencing it, then ReserveContact a window against it.
2. POST /contacts with body {"statusList":["SCHEDULED","SCHEDULING"],"startTime":<window start - 3600>,"endTime":<window end + 3600>,"ephemeris":{"azEl":{"id":"33333333-3333-3333-3333-333333333333"}}} — observe HTTP 431 InvalidParameterException where AWS answers 200 with a (possibly empty) contactList.
3. POST /contacts with body {"statusList":["AVAILABLE"],"startTime":<now>,"endTime":<now + 7 days>,"groundStation":"Ohio 1","missionProfileArn":<profile arn>,"satelliteArn":<satellite arn>} — observe 200 with an empty contactList and no nextToken, where AWS returns the available contact windows that can then be passed to ReserveContact.
