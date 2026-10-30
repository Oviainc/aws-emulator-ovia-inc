Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# CaptureConfiguration is stored verbatim, so the capture limits the model documents a default for are never settled

_Filed: 2026-10-28T04:30:24Z_
_Operation: CreateInstrumentationConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInstrumentationConfiguration` stores the request's `CaptureConfiguration` structure byte for byte and answers with it unchanged, so every member of `CodeCaptureConfiguration`/`CaptureLimitsConfig` that the model documents a server default for is left ABSENT in the stored record. A create that sends the minimum the model admits (`{"CodeCapture": {"CaptureLimits": {}}}` — `CaptureLimits` is the only `@required` member and all of its own members are optional) is stored and later reported by `GetInstrumentationConfiguration`/`ListInstrumentationConfigurations` with no `MaxHits`, no `MaxStringLength`, no `MaxCollectionWidth`, no `MaxCollectionDepth`, no `MaxStackFrames`, no `MaxStackTraceSize`, no `MaxObjectDepth`, no `MaxFieldsPerObject`, no `CaptureReturn` and no `CaptureStackTrace` — although each of those member docs states exactly which value the service uses when the member is omitted.

This is the one write path of this crate that does NOT settle a prose-documented default. The SAME handler, five lines earlier, settles the documented `ExpiresAt` default for a BREAKPOINT configuration through `crate::state::requested_expiry`; the SLO create settles the documented `Goal` defaults through `crate::state::goal_with_defaults` ("with each member the model documents a default for filled in when the request omitted it"); and `crate::state::stored_exclusion_window` settles `ExclusionWindow.StartTime` for the same reason ("the stored window — and so every later read of it — reports when the exclusion begins"). So a caller who omits a capture limit is told the configuration carries no limit at all, while a caller who omits an SLO goal member or a BREAKPOINT expiry is told the value the service actually applied.

The gap is in how the record is built, so it is one defect of the create; the reads merely project the record the create wrote.

## Evidence

MODEL (`smithy_query(query="shape", shape="CaptureLimitsConfig")`), one documented default per member:
- `MaxHits`: "The maximum number of times the instrumentation point can be hit before it is automatically disabled. Defaults to 100."
- `MaxStringLength`: "... Strings longer than this are truncated. Defaults to 128."
- `MaxCollectionWidth`: "... to prevent large payloads. Defaults to 10."
- `MaxCollectionDepth`: "The maximum nesting depth to traverse inside collections. Defaults to 3."
- `MaxStackFrames`: "The maximum number of stack frames to capture in stack traces. Defaults to 2."
- `MaxStackTraceSize`: "The maximum total size, in bytes, of a captured stack trace. Defaults to 1000."
- `MaxObjectDepth`: "The maximum depth for nested object traversal when capturing structured data. Defaults to 3."
- `MaxFieldsPerObject`: "The maximum number of fields to capture for any object. Defaults to 10."
and `smithy_query(query="shape", shape="CodeCaptureConfiguration")`: `CaptureReturn` — "Whether to capture the return value. Defaults to false."; `CaptureStackTrace` — "Whether to capture a stack trace when the instrumentation point is hit. Defaults to true."

HANDLER — the structure is cloned straight out of the request, with no defaulting step:
`services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/create_instrumentation_configuration.rs:289`
    `capture_configuration: input.capture_configuration.clone(),`
(and `:317` answers with that same value: `capture_configuration: configuration.capture_configuration,`)

SAME HANDLER, the documented default it DOES settle — `create_instrumentation_configuration.rs:217-226`:
    `// The model cannot express "this member is only supported for one value of that`
    `// one", nor a window measured from the time of the request, so both parts of the`
    `// documented ExpiresAt rule are enforced here and refused with 400 rather than`
    `// silently coerced.`
    `let expires_at = crate::state::requested_expiry(...)`
and `src/state.rs:4190-4192`: `let Some(requested) = requested else { return Ok(Some(instant_plus(now, BREAKPOINT_DEFAULT_EXPIRY))); };`

SIBLING WRITE PATH in the same crate — `src/state.rs:853-873`:
    `/// The `Goal` stored for a request, with each member the model documents a default for`
    `/// filled in when the request omitted it.`
    `pub fn goal_with_defaults(requested: Option<&Goal>) -> Goal {`
and `src/state.rs:1919-1922` for `stored_exclusion_window`: `/// `window` in the form this service STORES it: every member as the addition stated it, and a /// `StartTime` the addition omitted settled to the instant the call is served.`

TEST COVERAGE that currently pins the echo (and would need the defaulted values instead) — `tests/all/instrumentation_configurations.rs:390-394`:
    `assert_eq!(`
    `    answer["CaptureConfiguration"], body["CaptureConfiguration"],`
    `    "case {:?}: expected the stored capture configuration, got {:?}",`
with rows that deliberately leave members out (`tests/all/instrumentation_configurations.rs:193`: "how much of `CodeCaptureConfiguration` the capture settings fill in") and `tests/all/get_instrumentation_configuration.rs:123-125`:
    `fn minimal_capture() -> Value { json!({ "CodeCapture": { "CaptureLimits": {} } }) }`
`tests/all/get_instrumentation_configuration.rs:309-313` compares the Get against the create's own answer, so it keeps passing once the create settles the defaults.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. `POST /create-instrumentation-configuration` with `{"InstrumentationType":"BREAKPOINT","Service":"checkout","Environment":"prod","SignalType":"SNAPSHOT","Location":{"CodeLocation":{"Language":"Java","ClassName":"CartService","FilePath":"src/main/java/CartService.java"}},"CaptureConfiguration":{"CodeCapture":{"CaptureLimits":{}}}}`.
3. Read the 200 response's `CaptureConfiguration`: expected the documented server defaults to be settled (`CaptureLimits.MaxHits` 100, `MaxStringLength` 128, `MaxCollectionWidth` 10, `MaxCollectionDepth` 3, `MaxStackFrames` 2, `MaxStackTraceSize` 1000, `MaxObjectDepth` 3, `MaxFieldsPerObject` 10, plus `CaptureReturn` false and `CaptureStackTrace` true); actual: `{"CodeCapture":{"CaptureLimits":{}}}` — the request echoed, every documented default absent.
4. `POST /get-instrumentation-configuration` for the same identity: `Configuration.CaptureConfiguration` is the same empty-limits structure, so no caller can learn which limits the service applied.
5. Compare with the same handler's `ExpiresAt`: omit `ExpiresAt` on a BREAKPOINT create and the response DOES carry the settled 24-hour default — the two documented defaults of one operation are treated differently.
