Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.create-outpost-resolver-invalid-instance-count-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.create-outpost-resolver-invalid-instance-count-rejected|run#9|AssertionError|assert msg_invalid == '[RSLVR-03002] The instance count must be equal to or greater than "4".', (
Issue-Commit: 8e40a1008c33b06d6b244a457efc673d78e6bf69

# CreateOutpostResolver: the out-of-range InstanceCount refusal omits Resolver's RSLVR-03002 service error code

> _Note: this issue was identified against the code base at commit `8e40a1008c33b06d6b244a457efc673d78e6bf69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOutpostResolver

## Description
ONE root cause in the route53resolver service.

Cause 1 of 1: CreateOutpostResolver refuses an InstanceCount below the minimum with the right status and error code but an invented message that carries no RSLVR-nnnnn service error code, so the only discriminator AWS exposes for this fault is missing.
AWS: Smithy: CreateOutpostResolver.InstanceCount is a plain optional integer with NO @range trait (its doc says "The default and minimal value is 4"), and the operation models ValidationException (HTTP 400). Recorded behavior: InstanceCount=1 is refused with HTTP 400 / ValidationException whose message carries Resolver's own service error code, example text: '[RSLVR-03002] The instance count must be equal to or greater than "4".' The requirement is the RSLVR-nnnnn identifier inside the message, not the exact prose: because every modelled Resolver error is HTTP 400 and several faults of this operation share the ValidationException shape, the RSLVR code is the only per-fault discriminator — this emulator's own src/state.rs documents exactly that ("Route 53 Resolver carries its own `RSLVR-nnnnn` code inside every exception message ... the code is what operators and tooling key on") and already provides the rslvr_message() helper used by the endpoint-not-found path.
Emulator: status and error code are correct (HTTP 400, ValidationException) but the message is locally composed with no RSLVR code: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_outpost_resolver.rs:185-188 returns 'InstanceCount must be at least 4, got 1'.
Why: a caller cannot tell an instance-count rejection from the other ValidationException faults of this operation (notably the non-existent-Outpost rejection, which AWS codes RSLVR-03007), because the status, the error code and the shape name are identical and the code that would separate them is absent. That is precisely the differential this check exercises.
Fix: emit this refusal through crate::state::rslvr_message with code RSLVR-03002 in the CreateOutpostResolver handler's InstanceCount validation (and use RSLVR-03007 for the Outpost-lookup refusal when that validation lands).
NOTE for the fixer: this check's second half asserts the companion behavior — CreateOutpostResolver with a valid InstanceCount but an OutpostArn naming no racked Outpost must be refused with ValidationException '[RSLVR-03007] Cannot find the Outpost with Arn "<arn>".'. The emulator's handler performs no Outpost existence lookup at all (create_outpost_resolver.rs:194-271 goes straight to minting the record), so that assertion will fail next. That missing-validation root cause is ALREADY FILED as cause 4 of open issue 20261025T012620Z-route53resolver-route-53-resolver-rule-firewall-write-path-disassociateresolverrule.md and is deliberately NOT re-filed here; this check passes only once that issue's cause 4 and this message cause are both closed.
Traces: route53resolver.create-outpost-resolver-invalid-instance-count-rejected.

## Evidence
route53resolver.create-outpost-resolver-invalid-instance-count-rejected: CreateOutpostResolver(CreatorRequestId=<unique>, Name=<name>, PreferredInstanceType='r5.large', OutpostArn='arn:aws:outposts:<region>:<account>:outpost/op-<17 hex>', InstanceCount=1) -> AssertionError "expected the out-of-range InstanceCount to be called out in the error message, got 'InstanceCount must be at least 4, got 1'" (check line 50). The preceding assertion on the error code PASSED: expected and actual are both HTTP 400 / ValidationException. Expected message carries the RSLVR code, recorded example: '[RSLVR-03002] The instance count must be equal to or greater than "4".'. Source: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_outpost_resolver.rs:182-192; the RSLVR helper the fix should use is crate::state::rslvr_message (src/state.rs:163-182, 276-284).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Call CreateOutpostResolver(CreatorRequestId=<unique>, Name=<unique>, PreferredInstanceType='r5.large', OutpostArn=<pattern-valid ARN for op-<17 hex>>, InstanceCount=1).
3. Observe HTTP 400 ValidationException with message 'InstanceCount must be at least 4, got 1' — no RSLVR-nnnnn code, where AWS answers an RSLVR-03002-coded message.
4. Repeat with InstanceCount=4 and the same nonexistent OutpostArn: the call SUCCEEDS (200, an OutpostResolver is created) where AWS answers ValidationException RSLVR-03007 — that half is covered by open issue 20261025T012620Z-route53resolver-route-53-resolver-rule-firewall-write-path-disassociateresolverrule.md cause 4.
5. Replay route53resolver.create-outpost-resolver-invalid-instance-count-rejected.
