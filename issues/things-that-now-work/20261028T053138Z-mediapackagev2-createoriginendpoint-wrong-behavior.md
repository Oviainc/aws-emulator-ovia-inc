Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# Idempotency fingerprints flatten an absent optional member to its type default, so a CHANGED retry under the same ClientToken is replayed instead of conflicting

_Filed: 2026-10-28T05:31:38Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create in this crate builds its `ClientToken` fingerprint with `crate::state::request_fingerprint`, and `CreateOriginEndpoint`, `CreateChannel` and `CreateChannelGroup` feed it optional members through `.unwrap_or_default()`. That collapses ABSENT and present-but-default into one fingerprint, so `match_client_token` reports `TokenMatch::Replay` for a second request that asks for a DIFFERENT configuration: the handler returns 200 with the first endpoint instead of the `ConflictException` / `IDEMPOTENT_PARAMETER_MISMATCH` the crate already builds for exactly this case (`crate::state::idempotent_parameter_mismatch`).

Two concrete collapses, both on input the model admits:

* `CreateOriginEndpoint`: `input.startover_window_seconds.unwrap_or_default().to_string()` renders an absent member as `"0"`. `StartoverWindowSeconds` is `@range 0..1209600`, so `0` is a legal submitted value — and the two store DIFFERENT endpoints, because an absent window is materialised to `DEFAULT_STARTOVER_WINDOW_SECONDS` (900) while a submitted `0` is stored as `0`. A retry under the same token that now sends `StartoverWindowSeconds: 0` is answered with the 900-second endpoint.
* `CreateOriginEndpoint` / `CreateChannel` / `CreateChannelGroup`: `input.description.clone().unwrap_or_default()` renders an absent `Description` as `""`. `ResourceDescription` is `@length 0..1024`, so `""` is a legal value, and the two store different records (`None` vs `Some("")`), which every read and list projection reports differently.

`CreateHarvestJob` — the fourth create in the same crate — already states and implements the correct rule, rendering each optional member with `format!("{:?}", …)` so absent and present-but-empty stay apart. The three other creates skip it.

The fix is the one the sibling already uses: render each optional member in the fingerprint so absence is distinguishable, in all three create handlers.

## Evidence

The shared helper's own contract — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:2216-2226:
  "/// The fingerprint of a create request: a digest of every member it carried EXCEPT its
   /// `ClientToken` …
   /// Two requests share a fingerprint exactly when they ask for the same resource with the
   /// same configuration, which is what distinguishes an SDK's retry of one create from a
   /// second, different create sent under the same token.
   pub fn request_fingerprint(fields: &[String]) -> String {"

The sibling that states the rule correctly — src/ops/create_harvest_job.rs:335-352:
  "// The request's own fingerprint: every member it carried EXCEPT the token that
   // identifies the attempt. … An optional member is
   // rendered so that ABSENT and present-but-empty stay apart (`Description` admits
   // the empty string), which is what lets a retry that omitted `HarvestJobName`
   // match the first attempt instead of being read as a second, differently-named
   // create.
   let fingerprint = crate::state::request_fingerprint(&[
       …
       format!("{:?}", input.harvest_job_name),
       format!("{:?}", input.description),"

The three that break it — src/ops/create_origin_endpoint.rs:373-389:
  "let fingerprint = crate::state::request_fingerprint(&[
       input.channel_group_name.clone(),
       input.channel_name.clone(),
       input.origin_endpoint_name.clone(),
       input.container_type.clone(),
       input.description.clone().unwrap_or_default(),
       input.startover_window_seconds.unwrap_or_default().to_string(),"
src/ops/create_channel.rs:409-414:
  "let fingerprint = crate::state::request_fingerprint(&[
       input.channel_group_name.clone(),
       input.channel_name.clone(),
       input.description.clone().unwrap_or_default(),
       input.input_type.clone().unwrap_or_default(),"
src/ops/create_channel_group.rs:237-241:
  "let fingerprint = crate::state::request_fingerprint(&[
       input.channel_group_name.clone(),
       input.description.clone().unwrap_or_default(),
       crate::state::tag_fingerprint(&tags),
   ]);"

Why equal fingerprints mean a silent replay — src/state.rs:2204-2213:
  "if stored_token == Some(token) {
       return if stored_fingerprint == fingerprint {
           TokenMatch::Replay(name.to_string())
       } else {
           TokenMatch::Mismatch
       };"
and the stored values that differ — src/state.rs:1369-1374:
  "/// The startover window an origin endpoint is stored with: the one the request named, or
   /// the service's own ([`DEFAULT_STARTOVER_WINDOW_SECONDS`]) …
   pub fn materialized_startover_window_seconds(submitted: Option<i32>) -> i32 {
       submitted.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS)"
with src/state.rs:115: "pub const DEFAULT_STARTOVER_WINDOW_SECONDS: i32 = 900;"

Model: `CreateOriginEndpointRequest$StartoverWindowSeconds` is `{"target": "Integer", "range": {"min": 0, "max": 1209600}}` and `ResourceDescription` is `@length {min: 0, max: 1024}`, so `0` and `""` are both values the request boundary admits.

Test coverage: tests/all/control_plane_validation.rs:176-223 is the idempotency matrix for all three levels, and it only ever varies a PRESENT description (`create_at(&emulator, level, "res-a", Some("first"), Some(token))` vs `Some("second")`); no test sends absent-vs-empty or absent-vs-zero, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateChannelGroup `g`, then CreateChannel `g`/`c`.
2. CreateOriginEndpoint `g`/`c`/`e` with header `x-amzn-client-token: tok-1`, body `{"ContainerType":"TS","HlsManifests":[{"ManifestName":"index"}]}` (no `StartoverWindowSeconds`). Observe 200 and `StartoverWindowSeconds: 900` in the response.
3. Send CreateOriginEndpoint `g`/`c`/`e` again with the SAME `x-amzn-client-token: tok-1` but body `{"ContainerType":"TS","HlsManifests":[{"ManifestName":"index"}],"StartoverWindowSeconds":0}`. Observe 200 replaying the first endpoint with `StartoverWindowSeconds: 900`, where the crate's own rule makes this a changed request and `crate::state::idempotent_parameter_mismatch()` (409 / IDEMPOTENT_PARAMETER_MISMATCH) is the answer.
4. Repeat with `CreateChannelGroup`: first create under `tok-2` with no `Description`, then the same token with `{"Description":""}`. Observe a 200 replay whose `Description` is absent rather than the empty string the second request asked for.
5. Repeat with `CreateChannel` under `tok-3`, no `Description` then `{"Description":""}` — same silent replay.
