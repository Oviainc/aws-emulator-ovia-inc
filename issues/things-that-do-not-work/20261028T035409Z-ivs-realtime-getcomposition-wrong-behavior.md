Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# Every @timestampFormat("date-time") member except ParticipantToken$expirationTime is rendered as an epoch number instead of an ISO-8601 string

_Filed: 2026-10-28T03:54:09Z_
_Operation: GetComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This model marks every timestamp member with `@timestampFormat("date-time")` and each member's own doc says the value is "returned as a string". The crate knows this and fixes it for exactly ONE member: `ParticipantToken$expirationTime`, whose schema is hand-written solely to emit `with_timestamp_format(TimestampFormat::DateTime)`. Its own comment states the rule and the consequence of not applying it — a macro-built member schema reports no format, so the restJson1 codec falls back to epoch seconds, "a NUMBER where the model, and every AWS SDK's generated deserializer for a `date-time` member, requires an ISO-8601 string."

Every OTHER date-time member of this service is left to `#[smithy_macros::smithy(shape = ...)]`, so each one goes on the wire as an epoch-seconds number. `with_timestamp_format` occurs exactly once in the whole crate (`src/ops/types/shared_types.rs:311`). The members still rendered as numbers are:

- `Composition$startTime` / `$endTime` and `Destination$startTime` / `$endTime` — `GetComposition`, `StartComposition` (src/ops/types/shared_types.rs:784-795 and :1060-1069)
- `CompositionSummary$startTime` / `$endTime` and the nested `DestinationSummary` times — `ListCompositions` (src/ops/types/list_compositions_types.rs:38-39, :47-48)
- `StageSession$startTime` / `$endTime` — `GetStageSession` (src/ops/types/get_stage_session_types.rs:31-32)
- `StageSessionSummary$startTime` / `$endTime` — `ListStageSessions` (src/ops/types/list_stage_sessions_types.rs:33-34)
- `Participant$firstJoinTime` — `GetParticipant` (src/ops/types/get_participant_types.rs:34)
- `ParticipantSummary$firstJoinTime` — `ListParticipants` (src/ops/types/list_participants_types.rs:40)
- `Event$eventTime` and `ExchangedParticipantToken$expirationTime` — `ListParticipantEvents` (src/ops/types/list_participant_events_types.rs:36, :52)

So one operation of this service (`CreateParticipantToken` / `CreateStage`, through `ParticipantToken`) answers an ISO-8601 string while its siblings answer a JSON number for members carrying the identical modelled trait. An SDK client whose generated deserializer expects a `date-time` string for `Composition$startTime` cannot read these responses.

The crate's own test suites record the gap as open rather than as a decision: both `timestamp_secs` helpers say the rendering "is a separate question from the one this suite asks" and accept either form, while `tests/all/stages.rs` asserts the ISO-8601 string for `expirationTime` only.

The fix is the same one chain call the hand-written `ParticipantToken` schema already carries, applied to the remaining timestamp members (or, as that comment itself says, in the vendored codegen — which is outside what a fix may change here).

## Evidence

Smithy, `smithy_query(query="shape", shape="StageSession")` — the member carries the trait AND the doc states the rendering:
```
"startTime": {"target": "Time", "timestampFormat": "date-time", "type": "timestamp",
  "doc": "<p> ISO 8601 timestamp (returned as a string) when this stage session began.</p>"}
```
Same trait + same sentence on `Composition$startTime`/`$endTime` ("This is an ISO 8601 timestamp; <i>note that this is returned as a string</i>"), `Destination$startTime`/`$endTime`, `Participant$firstJoinTime` ("ISO 8601 timestamp (returned as a string) when the participant first joined the stage session"), `Event$eventTime` ("ISO 8601 timestamp (returned as a string) for when the event occurred") — `smithy_query(query="shape", shape="Composition,Destination,Participant,Event")`.

The crate states the rule and the failure mode itself — src/ops/types/shared_types.rs:154-167:
```
// `ParticipantToken` is the ONE shape in this module whose schema is written out by
// hand rather than produced by `#[smithy_macros::smithy(shape = ...)]`, and the reason
// is a single trait the macro does not read: `@timestampFormat`.
...
// The vendored codegen never emits
// `with_timestamp_format`, so a macro-built member schema reports no format and the
// restJson1 JSON codec falls back to its protocol default, epoch seconds — a NUMBER
// where the model, and every AWS SDK's generated deserializer for a `date-time` member,
// requires an ISO-8601 string.
```

The one place it is applied — src/ops/types/shared_types.rs:298-311:
```
    /// `@timestampFormat("date-time")` from `ParticipantTokenExpirationTime` — the
    /// chain the macro cannot emit, and the whole reason this schema is hand-written.
    pub const M_EXPIRATION_TIME: ::aws_smithy_schema::Schema = ...
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime);
```
`grep -rn with_timestamp_format src/` finds that line and no other in the crate.

The sibling shapes left to the macro — src/ops/types/shared_types.rs:784-795:
```
#[smithy_macros::smithy(shape = "com.amazonaws.ivsrealtime#Composition")]
#[derive(Debug, Clone, Default)]
pub struct Composition {
    ...
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
}
```
and src/ops/types/get_stage_session_types.rs:27-33:
```
#[smithy_macros::smithy(shape = "com.amazonaws.ivsrealtime#StageSession")]
#[derive(Debug, Clone, Default)]
pub struct StageSession {
    pub session_id: Option<String>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
}
```

Test coverage — the ISO-8601 rendering is pinned ONLY for the hand-written member, tests/all/stages.rs:839-842:
```
        let rendered = &token["expirationTime"];
        assert!(
            rendered.is_string(),
            "case {case:?}: expected expirationTime as an ISO-8601 string, got {rendered}"
```
and the suites that read the other timestamps explicitly leave the rendering unpinned rather than justify it — tests/all/stage_sessions.rs:300-306:
```
/// `StageSessionSummary$startTime` targets `Time`, which carries
/// `@timestampFormat("date-time")`; whether this emulator's restJson1 serializer
/// renders that as the ISO-8601 string or as the protocol's default epoch-seconds
/// number is a separate question from the one this suite asks, so both renderings are
/// read here as the instant they denote.
fn timestamp_secs(case: &str, value: &Value) -> i64 {
```
(the same paragraph for `Event$eventTime` at tests/all/participant_events.rs:256-262).

## Steps to Reproduce

1. CreateStage, then CreateEncoderConfiguration, and note both ARNs.
2. StartComposition with that stageArn and one `channel` destination carrying that encoderConfigurationArn.
3. GetComposition on the returned ARN and read `composition.startTime` in the JSON body.
4. Expected (the model's `@timestampFormat("date-time")` plus the member doc "This is an ISO 8601 timestamp; note that this is returned as a string", and the rendering the crate already produces for `ParticipantToken$expirationTime`): a JSON string such as "2026-01-01T00:00:00Z".
5. Actual: a JSON number (epoch seconds), because `Composition` is macro-built and carries no `with_timestamp_format` (src/ops/types/shared_types.rs:784-795; the chain exists only at src/ops/types/shared_types.rs:311).
6. Contrast in the same emulator: CreateParticipantToken for that stage and read `participantToken.expirationTime` — an ISO-8601 string, as tests/all/stages.rs:839-842 asserts.
7. Repeat for GetStageSession (`stageSession.startTime`), GetParticipant (`participant.firstJoinTime`), ListParticipantEvents (`events[].eventTime`), ListStageSessions, ListParticipants and ListCompositions: every one answers a number.
