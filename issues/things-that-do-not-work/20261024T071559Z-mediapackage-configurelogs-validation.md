Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# ConfigureLogs accepts log group names CloudWatch Logs cannot hold (empty, over 512 characters, illegal characters)

_Filed: 2026-10-24T07:15:59Z_
_Operation: ConfigureLogs | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `PUT /channels/{Id}/configure_logs` subscribes a channel's ingress and egress access logs to CloudWatch Logs log groups named by `IngressAccessLogs$LogGroupName` / `EgressAccessLogs$LogGroupName`. MediaPackage has to be able to write to the group it is given, so the name must be one CloudWatch Logs can actually hold — 1 to 512 characters drawn from alphanumerics and `.`, `-`, `_`, `/`, `#`. A name outside that domain (the empty string, a 600-character name, `not a valid group!$`) is refused with the modelled `UnprocessableEntityException` (HTTP 422) naming the member, and the channel keeps the log subscription it already had.

Actual: the handler bounds only the LENGTH, and at the wrong bound — `crate::state::bound_chars(member, value, crate::state::MAX_STRING_CHARS)` with `MAX_STRING_CHARS = 1024`, which is twice CloudWatch's 512-character ceiling — and there is no charset check and no lower bound at all. So `{"ingressAccessLogs": {"logGroupName": ""}}` and `{"egressAccessLogs": {"logGroupName": "<600 chars>"}}` and `{"logGroupName": "not a valid group!$"}` all answer HTTP 200, the unusable name is written to the record (`record.ingress_log_group_name = ...`), and every later `DescribeChannel`/`ListChannels` reports it as a live log subscription.

Note the empty-string case is the sharpest: an empty `logGroupName` is indistinguishable in the response from "this direction is unsubscribed" only by shape — the member is present and empty — so a client cannot tell whether the subscription took effect.

## Evidence

Spec grounding: `smithy_query(query="operation",operation="ConfigureLogs")` binds `IngressAccessLogs` / `EgressAccessLogs`, each a structure whose single member `LogGroupName` is documented as the log group to customise, and declares `UnprocessableEntityException` (errorKind client, httpError 422) as the operation's invalid-parameter channel. The accepted domain is CloudWatch Logs' own documented log-group-name rule (1-512 characters, `[\.\-_/#A-Za-z0-9]+`), which MediaPackage must honour because it writes into the group it is handed; the model publishes no `@length`/`@pattern`, so the framework's constraint walk cannot refuse it and the handler has to.

Handler: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/configure_logs.rs` lines 169-196 — the only check is the `bound_chars(..., MAX_STRING_CHARS)` loop over the two names (1024, not 512), after which both are assigned to the record unconditionally. `src/state.rs` line ~125 defines `MAX_STRING_CHARS: usize = 1024` as a generic memory bound, not this member's domain.

In-repo corroboration: the open sibling issue `20261024T051129Z-mediapackage-vod-configurelogs-validation.md` files the same gap against the VOD `ConfigureLogs` and cites THIS crate's length bound as the partial precedent — it notes the v1 handler bounds the string but does not claim v1 validates its charset or refuses an empty name, which it does not.

Test coverage: `tests/all/mutation_persistence.rs` (lines ~231-341) and `tests/all/rotate_*_credentials.rs` exercise only well-formed names (`/aws/MediaPackage/probe-ingress`, `my-own-group`, `a/deeply/nested/group-name_1`) and the absent-member case; `tests/all/error_envelope.rs` covers only the over-1024-character refusal. No test asserts acceptance or refusal of an empty, 600-character, or illegal-character name.

## Steps to Reproduce

1. Boot the emulator and point boto3 at it; mp.create_channel(Id="c1").
2. mp.configure_logs(Id="c1", IngressAccessLogs={"LogGroupName": "/aws/MediaPackage/good"}) -> 200 (baseline subscription stored).
3. mp.configure_logs(Id="c1", IngressAccessLogs={"LogGroupName": ""}) -> observe HTTP 200 and an echoed empty name; AWS refuses with UnprocessableEntityException / 422 and keeps the previous subscription.
4. mp.configure_logs(Id="c1", EgressAccessLogs={"LogGroupName": "g"*600}) -> observe HTTP 200 (the handler's bound is 1024, CloudWatch's ceiling is 512).
5. mp.configure_logs(Id="c1", EgressAccessLogs={"LogGroupName": "not a valid group!$"}) -> observe HTTP 200.
6. mp.describe_channel(Id="c1") -> the unusable names are reported as live ingress/egress access log subscriptions.
