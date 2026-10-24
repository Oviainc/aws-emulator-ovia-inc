Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# CreateInput stores inputSecurityGroups, sdiSources and inputDevices without resolving them, and accepts a push input with no security group / a pull input with no sources

_Filed: 2026-10-24T21:14:26Z_
_Operation: CreateInput | Class: validation_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInput` is the one operation of this service whose only modelled client error is `BadRequestException` (400) — the real service uses it to refuse an input whose configuration cannot be realised. The emulator performs only the two `Vpc` cross-member checks and then stores every other member verbatim, so an input nothing can ever ingest through is created with 201:

* `InputSecurityGroups` entries are never resolved against the account's input security groups. An id no `CreateInputSecurityGroup` ever minted is stored and reported back in `Input$SecurityGroups`, and the group's own derived `Inputs`/`State` never mention it, so the input claims a whitelist that does not exist. Expected: 400 BadRequestException naming the unknown group.
* A PUSH-protocol input (`Type` = RTP_PUSH / RTMP_PUSH) is accepted with no `InputSecurityGroups` at all. The real service requires a security group for a push input, since the whitelist is what admits the upstream encoder.
* A PULL-protocol input (URL_PULL / RTMP_PULL / MP4_FILE / TS_FILE …) is accepted with an empty `Sources` list, so the input has nothing to pull from.
* `SdiSources` entries are never resolved against the stored SDI sources, and `InputDevices[].Id` is never resolved against the devices this account has claimed — an input can name a device that was never claimed and an SDI source that was never created.

`UpdateInput` has the same holes (it assigns `record.security_groups`, `record.sdi_sources` and `record.input_devices` straight from the request).

## Evidence

`ops/create_input.rs` `handle` (lines 177-277): only `input.vpc` triggers checks (lines 183-198); lines 249-258 assign `security_groups`, `sources`, `sdi_sources`, `input_devices` directly with no lookup against `state.input_security_groups`, `state.sdi_sources` or `state.input_devices`. `state.rs::allocate_input_ingest_addresses` (~line 2483) only tops up ingest IPs for push types and performs no validation. `smithy_query(query="operation", operation="CreateInput")` shows `BadRequestException` (400) as the only client error besides Forbidden/TooManyRequests, i.e. the error the service uses for an unusable configuration; `InputSecurityGroups`/`SdiSources`/`InputDevices` are all modelled input members. No test asserts any of these refusals.

## Steps to Reproduce

1. CreateInputSecurityGroup once so the store is non-empty, then CreateInput with Type=RTP_PUSH and InputSecurityGroups=["99999999"] (an id nothing minted) — observe 201 and the bogus id echoed in Input$SecurityGroups instead of 400
2. CreateInput with Type=RTP_PUSH and no InputSecurityGroups member — observe 201 instead of 400
3. CreateInput with Type=URL_PULL and no Sources member — observe 201 instead of 400
4. CreateInput with SdiSources=["not-a-source"] and InputDevices=[{"id":"hd-0000000000000000"}] with no prior ClaimDevice — observe 201 and both echoed back
5. Repeat the first step through UpdateInput on a live input — observe 200
