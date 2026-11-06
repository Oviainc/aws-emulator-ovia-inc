Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fb07764462a6e3726541f141027927b0567b370d

# An ephemeris with no expirationTime is reported by EVERY ListEphemerides window, although both window members document the listing as returning ephemerides whose expiration falls inside it

_Filed: 2026-10-29T11:59:31Z_
_Operation: ListEphemerides | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fb07764462a6e3726541f141027927b0567b370d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEphemerides` binds its required `startTime`/`endTime` window to the stored ephemeris' expiration time, but the filter is written with `Option::is_none_or`, so an ephemeris whose stored `expiration_time` is `None` passes EVERY window — including a window of one instant decades away from anything the account holds.

That is not a rare shape. `CreateEphemeris` stores `expiration_time` as `input.expirationTime.or_else(|| ephemeris_validity_end(&data))`, and `ephemeris_validity_end` yields nothing for several of the forms the API accepts: the crate's own expiration table enumerates "no expirationTime, an oem body, which declares no validity here", "no expirationTime, a tle supplied from an Amazon S3 object" and "no expirationTime, azEl segments supplied from an Amazon S3 object". So an OEM ephemeris created without an explicit `expirationTime` — an ordinary create — is returned by a listing whose window selects nothing it could belong to.

The model states the selection rule itself, on both members, and the handler does the opposite for these records: the window narrows nothing at all for them. A caller paging a narrow window to find "what expires this week" is handed every non-expiring ephemeris in the account mixed into the answer, with no member in the response (`EphemerisItem` carries no expiration time) that would let it tell which records the filter was not applied to.

This is a different defect from the already-fixed ListEphemerides issue about answering an empty `ephemerides` list: that one was about how an EMPTY page is serialized, this one is about which records the window selects.

## Evidence

Model (states the selection behaviour, on BOTH window members):

  smithy_query(query="doc", shape="ListEphemeridesRequest", member="startTime") -> "<p>The start time for the list operation in UTC. Returns ephemerides with expiration times within your specified time range.</p>"
  smithy_query(query="doc", shape="ListEphemeridesRequest", member="endTime")   -> "<p>The end time for the list operation in UTC. Returns ephemerides with expiration times within your specified time range.</p>"

Both `startTime` and `endTime` are `@required` (smithy_query(query="operation", operation="ListEphemerides")), so every request names a window.

Handler does the opposite for a record with no expiration — services/service-com-amazonaws-groundstation-groundstation/src/ops/list_ephemerides.rs:269-277:

            // The window is tested against the EXPIRATION time, as both members
            // document ("returns ephemerides with expiration times within your specified
            // time range"). An ephemeris with no expiration time never expires, so no
            // window excludes it.
            .filter(|ephemeris| {
                ephemeris.expiration_time.is_none_or(|expiration| {
                    (window_start..=window_end).contains(&expiration.as_secs_f64())
                })
            })

`is_none_or` returns `true` for `None`, so the window is simply not applied to those items. The comment quotes the very sentence it then departs from.

The `None` case is reachable from an ordinary create — src/ops/create_ephemeris.rs:251-256:

            // The expiration the request named, or the end of the supplied data's own
            // validity when it named none: the data stops being usable when its validity
            // ends.
            expiration_time: input
                .expiration_time
                .or_else(|| crate::state::ephemeris_validity_end(&data)),

and the crate's own table names the forms that yield nothing — tests/all/ephemeris_expiration.rs:140-162:

            case: "no expirationTime, an oem body, which declares no validity here",
...
            case: "no expirationTime, a tle supplied from an Amazon S3 object",
...
            case: "no expirationTime, azEl segments supplied from an Amazon S3 object",

Test coverage: tests/all/ephemeris_catalog.rs:643-654 pins the current behaviour, on the handler's reason rather than on the model sentence:

            None => {
                for window in [(1_i64, 2_i64), (PAST, PAST + 60), WHOLE_WINDOW] {
                    let listed = list_ids(&router, &filters(window, json!({}))).await;
                    assert_eq!(
                        listed,
                        vec![id.clone()],
                        "case {} window {window:?}: an ephemeris with no expiration is excluded \
                         by no window, got {listed:?}",

That assertion's stated reason ("excluded by no window") is exactly what the quoted model sentence contradicts: an ephemeris with no expiration time has no expiration time inside the caller's range. tests/all/non_empty_response_members.rs:37-40 and tests/all/ephemeris_catalog.rs:52 both work around the behaviour by defining a WHOLE_WINDOW "that excludes no expiration this suite stores", so no test exercises a narrow window against a no-expiration record expecting it to be excluded.

## Steps to Reproduce

1. Onboard a satellite so CreateEphemeris accepts a satelliteId.
2. POST /ephemeris with an `oem` ephemeris supplied inline (`{"oem": {"oemData": "<CCSDS body>"}}`), a name, and NO `expirationTime` — the create succeeds; the stored ephemeris has `expiration_time == None` because `ephemeris_validity_end` derives nothing from an OEM body (tests/all/ephemeris_expiration.rs:140 'no expirationTime, an oem body, which declares no validity here').
3. Wait out the validation span so the ephemeris leaves VALIDATING.
4. POST /ephemerides with `{"startTime": 1, "endTime": 2}` — a two-second window at the start of the epoch, which no expiration this account holds can fall inside.
5. Expected per the modelled member docs ('Returns ephemerides with expiration times within your specified time range'): the ephemeris is not selected, so the page reports no `ephemerides` member. Actual: HTTP 200 with the ephemeris listed, because `is_none_or` admits it for every window (src/ops/list_ephemerides.rs:273-277).
6. Repeat with `{"startTime": 4102444800, "endTime": 4102444801}` (a window a century away): the same ephemeris is returned again, showing the window narrows nothing for it.
