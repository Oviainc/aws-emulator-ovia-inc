Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# StreamNameOutputMode's documented INDEX default is materialised onto every endpoint, including endpoints of channels the model says may not carry the member at all

_Filed: 2026-10-29T01:46:15Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateOriginEndpoint` materialises `StreamNameOutputMode` for every endpoint it mints, from the submitted value or the service default, without ever consulting the parent channel's `InputType`. The model restricts that member to channels whose `InputType` is `HLS`, so an endpoint created under a `CMAF` or `MULTIVIEW` channel is stored with — and then reports on every read — a member the same service says is not valid for it.

This crate already has the one rule for exactly this shape of member, where the model documents BOTH a service-applied default and a restriction on which input types may carry it: `materialized_output_locking_mode` materialises the documented default only on a channel whose input type may carry the member, and leaves it absent otherwise. `materialized_stream_name_output_mode` takes no input type at all, so the rule is applied to one member and skipped for the other, even though the create handler has the parent channel in hand when it mints the endpoint.

The consequence compounds in `UpdateOriginEndpoint`: it treats the stored mode as immutable, so an update to a CMAF-channel endpoint that names a mode is compared against the `INDEX` the create invented and refused with `STREAM_NAME_OUTPUT_MODE_IMMUTABLE` — a refusal derived from a value the endpoint should not hold.

(The separate half of this member's rule — refusing a SUBMITTED mode on a non-HLS channel, which `ValidationExceptionType` names `ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE` — is already covered by the open CreateOriginEndpoint/validation issue. This issue is the materialisation half: the value the store invents and reports when the request names nothing.)

## Evidence

The model states both halves on the member itself — `smithy_query(query="doc", shape="CreateOriginEndpointRequest", member="StreamNameOutputMode")`:
> "The output mode for stream names in egress manifests. This setting is valid only when the associated channel's `InputType` is `HLS`. You can't change the stream name output mode after you create the endpoint. ... If you don't specify a value, the default is `INDEX`."

The crate applies the default unconditionally — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1454-1458:
```
/// The stream-name output mode an origin endpoint is stored with — the submitted one,
/// else [`DEFAULT_STREAM_NAME_OUTPUT_MODE`].
pub fn materialized_stream_name_output_mode(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_STREAM_NAME_OUTPUT_MODE.to_string())
}
```
called with no input type at src/ops/create_origin_endpoint.rs:498-503:
```
            // The URI separator and the stream-name output mode are server-applied too:
            // every endpoint has one, whether or not the request named it.
            uri_separator: crate::state::materialized_uri_separator(input.uri_separator),
            stream_name_output_mode: crate::state::materialized_stream_name_output_mode(
                input.stream_name_output_mode,
            ),
```
while the parent channel is already resolved and held in the same scope (src/ops/create_origin_endpoint.rs:346-351) and its `input_type` is never read.

The crate's own rule for a member documented this way — src/state.rs:1374-1398:
```
/// The output locking mode a channel is stored with.
///
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `EPOCH_LOCKED`") AND restricts which input types may carry it at
/// all, so the default is materialised only on a channel whose input type may carry it; a
/// channel that may not has no mode and reports none.
pub fn materialized_output_locking_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
```

That the crate reads this very member's doc is proven by the half it DID implement — src/ops/update_origin_endpoint.rs:237-247:
```
/// `StreamNameOutputMode`: the model's member doc says "If you provide a value, it must
/// match the current value. You can't change the stream name output mode after you create
/// the endpoint", and `ValidationExceptionType` names
/// `STREAM_NAME_OUTPUT_MODE_IMMUTABLE` for it.
const STREAM_NAME_OUTPUT_MODE: ImmutableMember = ImmutableMember {
```

And the invented value is reported on every projection — src/ops/get_origin_endpoint.rs:254:
```
            stream_name_output_mode: Some(endpoint.stream_name_output_mode.clone()),
```
src/ops/list_origin_endpoints.rs:185:
```
        stream_name_output_mode: Some(endpoint.stream_name_output_mode.clone()),
```
`ValidationExceptionType` names the restriction the crate ignores: `ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE` (model enum, mirrored in src/ops/types/shared_types.rs:586). No test pins today's value for a non-HLS channel: tests/all/server_materialised_members.rs and tests/all/update_origin_endpoint.rs assert the materialised members without crossing them with the channel's input type.

## Steps to Reproduce

1. Boot the emulator and CreateChannelGroup `g`, then CreateChannel `g/c` with InputType `CMAF`.
2. CreateOriginEndpoint `g/c/e` with ContainerType `CMAF`, one HlsManifests entry, and NO StreamNameOutputMode member.
3. Observe the 200 response (and a later GetOriginEndpoint / ListOriginEndpoints) reporting `StreamNameOutputMode: "INDEX"` for an endpoint whose channel InputType is CMAF, although the member's own documentation says the setting "is valid only when the associated channel's InputType is HLS".
4. Compare with the sibling rule for a member documented the same way: create a channel with InputType `HLS` and observe `OutputLockingMode` reported as absent, because `materialized_output_locking_mode` withholds the documented default on an input type that may not carry the member.
5. PUT /channelGroup/g/channel/c/originEndpoint/e with ContainerType CMAF and StreamNameOutputMode `PASSTHROUGH_NAME`: the update is refused 400 STREAM_NAME_OUTPUT_MODE_IMMUTABLE against the `INDEX` the create invented.
