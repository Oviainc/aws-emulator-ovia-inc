Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6e8c3e9c5f69577c8704496d0849780bb1468b0d

# An entitlement's DataTransferSubscriberFeePercent is stored outside the documented 0-100 range

_Filed: 2026-10-28T21:02:57Z_
_Operation: GrantFlowEntitlements | Class: validation_

> _Note: this issue was identified against the code base at commit `6e8c3e9c5f69577c8704496d0849780bb1468b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GrantEntitlementRequest$DataTransferSubscriberFeePercent` is documented as "Percentage from 0-100 of the data transfer cost to be billed to the subscriber", and the model gives the member no `@range`, so the framework's constraint walk cannot refuse a value outside it. The grant path checks the entitlement's tags and its subscribers and nothing else, and `mint_entitlement` copies the percentage onto the stored record verbatim — so an entitlement can be granted with `dataTransferSubscriberFeePercent: 500` or `-20`, and DescribeFlow, ListEntitlements and the grant's own response then report a percentage that is not a percentage. CreateFlow's `Entitlements` mints through the same function, so it stores the same value.

The crate refuses other out-of-documented-range values on the same records (a subscriber that cannot be an account ID, a tag key/value past its documented character maximum, a router bitrate at or below zero), and its own test calls 0 and 100 "the subscriber fee percent at both published bounds" — the bounds nothing enforces.

## Evidence

Model (smithy_query query="shape" shape="GrantEntitlementRequest"), member DataTransferSubscriberFeePercent: "Percentage from 0-100 of the data transfer cost to be billed to the subscriber." — target `Integer`, with no range trait, so nothing is enforced before the handler runs.

The value is stored verbatim — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:6478 (inside `mint_entitlement`, the crate's "ONE encoding of how an entitlement is granted", state.rs:6446-6448):
```
            data_transfer_subscriber_fee_percent: request.data_transfer_subscriber_fee_percent,
```
`grep -rn 'data_transfer_subscriber_fee_percent' src/` finds only this write and the read-back projection at state.rs:1857-1859 — no bound anywhere.

The grant handler's content checks stop at tags and subscribers — src/ops/grant_flow_entitlements.rs:260-272:
```
        for (index, entitlement) in input.entitlements.iter().enumerate() {
            let member = format!("Entitlements[{index}]");
            if let Some(rejection) =
                crate::state::tag_set_rejection(&member, entitlement.entitlement_tags.as_ref())
            { return Err(bad_request(rejection)); }
            if let Some(rejection) =
                crate::state::subscribers_rejection(&member, &entitlement.subscribers)
            { return Err(bad_request(rejection)); }
        }
```
(the same pair of checks, and no third one, in src/ops/create_flow.rs:350-366).

The crate's own test names the bounds it does not enforce — tests/all/flow_entitlements.rs:203-212:
```
        GrantCase {
            case: "the subscriber fee percent at both published bounds",
            granted: vec![
                json!({ "name": "fee-min", "dataTransferSubscriberFeePercent": 0, ... }),
                json!({ "name": "fee-max", "dataTransferSubscriberFeePercent": 100, ... }),
```
No test grants a value outside 0-100, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /v1/flows with one source — 201; keep FlowArn.
2. POST /v1/flows/{FlowArn}/entitlements with {"entitlements":[{"name":"over","dataTransferSubscriberFeePercent":500,"subscribers":["111122223333"]}]}.
3. Observe 201 and the response reporting dataTransferSubscriberFeePercent 500, although the member is documented as a "Percentage from 0-100".
4. GET /v1/flows/{FlowArn} — observe the stored entitlement still reporting 500 (and the same with -20).
