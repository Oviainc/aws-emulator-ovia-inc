Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9dfd5bd5800ceb89967be182e813f239a03c5430

# GetProfile echoes the stored Content-Type, so contentType can be a media type the model says it never is

_Filed: 2026-10-27T09:42:59Z_
_Operation: GetProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9dfd5bd5800ceb89967be182e813f239a03c5430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states what `GetProfile`'s required `contentType` output member can be: "The content type of the profile in the payload. It is either application/json or the default application/x-amzn-ion." The handler instead reports back whatever media-type string `PostAgentProfile` happened to carry in its `Content-Type` header.

`PostAgentProfile` stores the header verbatim (`content_type: input.content_type.clone()`), with no check against the two representations this service serves — `PostAgentProfile$contentType` is an unconstrained `String` in the model, so any value reaches the store. `GetProfile`'s `served_profile` then takes the `as_stored()` branch for any profile whose stored media type is not one of the two it can read (`Representation::of` returns `None`), and that branch returns `profile.content_type.clone()` as the response's `contentType`. The handler's own doc comment admits this: the profile "is served as stored, under the media type it really has, which is what the response's `contentType` then reports."

So a profile posted under e.g. `Content-Type: application/octet-stream` is handed back with `contentType: application/octet-stream` — a value the model's member doc rules out, and one a client that branches on the two documented media types cannot interpret. The negotiation code already knows the only two media types the service serves (`Representation::media_type`, get_profile.rs:192-197); the stored-bytes branch is the one path that reports something else.

Note: the open issue "PostAgentProfile accepts payloads AWS refuses" is about refusing malformed profile BODIES; this is about the response member `GetProfile` computes, which is wrong for any stored media type outside the two documented ones even for a perfectly well-formed Ion/JSON payload.

## Evidence

Smithy (smithy_query query="doc" shape="GetProfileResponse" member="contentType"):
  "The content type of the profile in the payload. It is either application/json or the default application/x-amzn-ion."
(restated in the handler's own generated contract block, src/ops/get_profile.rs:105-106: "* `contentType` (String) **required** — The content type of the profile in the payload. It is either application/json or the default application/x-amzn-ion.")

The code that reports something else — src/ops/get_profile.rs:294-317:
```
/// in it to re-encode or to bound — so it is served as stored, under the media type it
/// really has, which is what the response's `contentType` then reports.
fn served_profile(
...
    let as_stored = || (profile.bytes.clone(), profile.content_type.clone());
    let stored = Representation::of(&profile.content_type);
...
    let (Some(stored), Some(wanted)) = (stored, wanted) else {
        return as_stored();
    };
```
and the handler wiring it straight into the response — src/ops/get_profile.rs:717-728:
```
        let served =
            ::tokio::task::spawn_blocking(move || served_profile(&stored, accept.as_deref(), max_depth))
...
        Ok(GetProfileOutput { profile: bytes, content_type, ... })
```

Where the unconstrained media type enters the store — src/ops/post_agent_profile.rs:163-167:
```
            crate::state::AgentProfile {
                bytes: input.agent_profile.clone(),
                content_type: input.content_type.clone(),
```
(`AgentProfile.content_type` is a plain `String`, src/state.rs:712; nothing in src/ checks it — grep for `content_type` across src/ returns only these sites.)

Test coverage: tests/all/profile_representations.rs::a_payload_that_is_no_document_is_served_as_it_is_stored (lines 662-702) exercises exactly this `as_stored` branch, but every row posts under the `ION` or `JSON` constants, so the asserted "the media type it really has" is always one of the two documented values. No test posts a third media type, so nothing pins the behaviour reported here.

## Steps to Reproduce

1. CreateProfilingGroup with profilingGroupName=pg.
2. PostAgentProfile for pg with a valid Ion or JSON agent profile body but Content-Type: application/octet-stream (the model puts no constraint on PostAgentProfile$contentType, so the request is accepted and src/ops/post_agent_profile.rs:165 stores the header verbatim).
3. Once the posted profile's aggregation is due (or seed the store the way tests/all/profile_representations.rs::group_holding does, with content_type="application/octet-stream"), call GET /profilingGroups/pg/profile.
4. Observe the 200 reports contentType: application/octet-stream, while the model's GetProfileResponse$contentType doc says it "is either application/json or the default application/x-amzn-ion".
