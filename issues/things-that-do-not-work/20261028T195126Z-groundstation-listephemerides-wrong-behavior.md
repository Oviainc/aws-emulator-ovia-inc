Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# ListEphemerides answers an empty `ephemerides` list, although the member's modelled list shape is `@length(min: 1)` and the crate's own rule for a `len 1..` member is to omit it

_Filed: 2026-10-28T19:51:26Z_
_Operation: ListEphemerides | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEphemerides` always serializes its `ephemerides` member, including when the filtered page is empty, on the stated premise that it answers "as every other listing of this service answers". It is the ONE listing of this service whose output list carries a non-zero minimum length: `EphemeridesList` is `@length(min: 1, max: 500)`, while every other listing's list member is unbounded or `0..`. So the one member the model forbids from being empty is the one the uniform "present and empty" rule is applied to, and a request that selects nothing (no ephemeris stored, a `statusList`/`ephemerisType`/expiration-window filter that matches none, or a last page that lands exactly on the boundary) answers `"ephemerides": []` — a value outside the member's own modelled constraint.

The crate already states the opposite rule for the other two `len 1..` members it reports, in the sibling reader of the same resource: `DescribeEphemeris` reports `ephemerisData` absent rather than empty "so \"no data\" is an absent member rather than an empty string" because the shape is `len 1..`, and reports `errorReasons` (`EphemerisErrorReasonList`, `@length(min: 1, max: 50)`) as `None` rather than an empty list. The same reading applied here makes the member absent on an empty page, which is also what the handler already does for `nextToken`.

## Evidence

Model — `smithy_query(query="shape", shape="EphemeridesList")`:
```
{"shape_id": "com.amazonaws.groundstation#EphemeridesList", "type": "list", "length": {"min": 1, "max": 500}, "member": "EphemerisItem"}
```
and `smithy_query(query="shape", shape="ListEphemeridesResponse")`:
```
"ephemerides": {"target": "EphemeridesList", "length": {"min": 1, "max": 500}, "type": "list", "doc": "<p>List of ephemerides.</p>"}
```

Handler — `services/service-com-amazonaws-groundstation-groundstation/src/ops/list_ephemerides.rs:306-312`:
```
        Ok(ListEphemeridesOutput {
            next_token,
            // Present and empty when nothing matched, as every other listing of this
            // service answers: a client's pagination loop then reads the same member on
            // every page.
            ephemerides: Some(page),
        })
```

The premise of that comment does not hold for this member: the other listings' list shapes carry no minimum — `smithy_query(query="shape", shape="ConfigList,MissionProfileList,SatelliteList,GroundStationList,DataflowEndpointGroupList,ContactVersionsList,ContactList,AntennaList")` returns no `length` at all for `ConfigList`, `MissionProfileList`, `SatelliteList`, `GroundStationList`, `DataflowEndpointGroupList`, `ContactVersionsList`, `ContactList`, and `{"length": {"min": 0, "max": 100}}` for `AntennaList`. `EphemeridesList` is the only one with `min: 1`.

The crate's own rule for a `len 1..` member, in the sibling read of the same resource — `src/ops/describe_ephemeris.rs:260-262`:
```
/// Amazon S3 object describe one body of data. `None` for a set with no entry at all:
/// `EphemerisDescription$ephemerisData` is `len 1..`, so "no data" is an absent member
/// rather than an empty string.
```
(applied at `src/ops/describe_ephemeris.rs:263-266`: `fn tle_text(entries: &[TLEData]) -> Option<String> { if entries.is_empty() { return None;`), and again for the other `len 1..` list of this same response — `src/ops/describe_ephemeris.rs:210-216`:
```
            // `errorReasons` is the per-error detail of a validation failure, each entry an
            // `EphemerisErrorCode` plus a human-readable message
            ...
            // is no error list to report and none is invented here.
            error_reasons: None,
```

Test coverage (to be updated with the fix): `tests/all/ephemeris_catalog.rs:405-410` pins the empty array —
```
    assert_eq!(
        empty.field("ephemerides"),
        &json!([]),
        "case no-ephemerides: an account holding none must list none, got {}",
        empty.body
    );
```
Its stated reason ("an account holding none must list none") is about selecting nothing, which an absent member reports as faithfully as an empty one; the `@length(min: 1)` above is what rules out the empty array it currently asserts. `tests/all/ephemeris_catalog.rs:165-196` (the paging helper that reads `.field("ephemerides")` on every page) and `tests/all/ephemeris_catalog.rs:1023-1110` cover the non-empty pages.

## Steps to Reproduce

1. Start the emulator with no ephemeris created for the account.
2. POST /ephemerides with a `startTime`/`endTime` window covering everything and no other filter (ListEphemerides).
3. The response is 200 with `{"ephemerides": []}` — a list of length 0, where the member's shape `EphemeridesList` is modelled `@length(min: 1, max: 500)`.
4. Create one ephemeris, then POST /ephemerides with `statusList: ["EXPIRED"]` (a status nothing holds): the same empty list comes back.
5. Compare GET /ephemeris/{ephemerisId} (DescribeEphemeris) for an ephemeris with no validation error: `errorReasons` (also `len 1..`) is OMITTED rather than returned as `[]`, which is the crate's own rule for a `len 1..` member.
