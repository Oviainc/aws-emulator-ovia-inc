Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-get-transit-gateway-metering-policy-entries ec2.op-contract-modify-reserved-instances-rejected ec2.op-contract-purchase-reserved-instances-offering-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-get-transit-gateway-metering-policy-entries|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.op-contract-modify-reserved-instances-rejected|run#6|AssertionError|assert code == "MissingParameter", f"expected MissingParameter for a scope-less target configuration, got {code}"
Issue-Key: ec2.op-contract-purchase-reserved-instances-offering-rejected|run#5|AssertionError|assert control_err.get("Code") == "DryRunOperation", (
Issue-Commit: 1089a3896fb85320e89317bd5e056d5ee3057dc5

# ec2: 3 net-new refusal-fidelity root causes — TGW metering-policy not-found sentence drops "Id", ModifyReservedInstances skips the scope-axis precondition, PurchaseReservedInstancesOffering answers 501

> _Note: this issue was identified against the code base at commit `1089a3896fb85320e89317bd5e056d5ee3057dc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PurchaseReservedInstancesOffering

## Description
Three net-new root causes in the EC2 crate, all on refusal paths, none of them marked ALREADY FILED or OWNED by the replay report. They are bundled because they belong to one service; each must be closed on its own. The open issue 20261029T175114Z-ec2-refusal-paths-answer-notimplemented-instead-of-the.md describes the same FAMILY of defect as Cause 3 but carries neither this check nor PurchaseReservedInstancesOffering in its trace list, so it does not cover it; 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md owns the sibling check ec2.op-contract-modify-reserved-instances-required-params-rejected but not the scope-axis leg of Cause 2.

Cause 1 of 3: GetTransitGatewayMeteringPolicyEntries' not-found sentence omits the word "Id".
AWS: recorded behavior only — the Smithy model declares no errors for GetTransitGatewayMeteringPolicyEntries (errors: []), and ec2Query refusals are not modelled; the recording shows a well-formed but nonexistent TransitGatewayMeteringPolicyId refused with Code `InvalidTransitGatewayMeteringPolicyId.NotFound` and Message `Transit Gateway Metering Policy Id <TransitGatewayMeteringPolicyId> was deleted or does not exist.` The fixed text is `Transit Gateway Metering Policy Id ` + ` was deleted or does not exist.`; the only variable part is the TransitGatewayMeteringPolicyId copied from the request (the `tgw-mp-...` value in the replay is one run's filler, not part of the template).
Emulator: the Code matches, but the message is `Transit Gateway Metering Policy <id> was deleted or does not exist.` — the fixed text drops the word `Id` after `Policy`.
Why: a client (and any log/diagnostic matcher) comparing the refusal sentence against AWS's wording sees a different message for the same error class, so text-driven handling and golden-output comparisons break even though the code is right.
Fix: the refusal's message template — `crate::state::invalid_transit_gateway_metering_policy_id_not_found`, the helper called from the not-found branch of services/service-com-amazonaws-ec2-amazonec2/src/ops/get_transit_gateway_metering_policy_entries.rs; every caller of that helper inherits the sentence.
Traces: ec2.op-contract-get-transit-gateway-metering-policy-entries.

Cause 2 of 3: ModifyReservedInstances never applies the target-configuration scope-axis precondition, so a scope-less configuration is answered with the id lookup's refusal.
AWS: recorded behavior only (the model declares no errors for ModifyReservedInstances; TargetConfigurations is @required and ReservedInstancesConfiguration carries AvailabilityZone, AvailabilityZoneId, InstanceCount, InstanceType, Platform and Scope). The recording shows AWS validating the target configuration BEFORE resolving ReservedInstancesIds and distinguishing three refusals on the same nonexistent id: (a) a configuration naming neither AvailabilityZone nor Platform -> `MissingParameter`, message `The request must contain the parameter  AvailabilityZone` (fixed text, including the double space AWS emits, with the member name `AvailabilityZone` as its variable part); (b) a syntactically bad AvailabilityZone -> `InvalidParameterValue`, message `Invalid value '<AvailabilityZone>' for AvailabilityZone.`; (c) a well-formed zone plus an unknown id -> `InvalidInput`, message ` Invalid value for 'reservedInstancesIds'. Reserved Instance <ReservedInstancesId> is not found.`
Emulator: for case (a) it answers `InvalidInput` — the refusal of case (c) — because the handler's per-configuration loop only checks InstanceCount, Scope/zone contradictions and whether a named zone exists in the Region, and then falls straight through to the ReservedInstancesIds lookup. The missing-scope-axis precondition (at least one of AvailabilityZone / AvailabilityZoneId / Platform) is never evaluated, so the three refusal classes collapse to one.
Why: a client cannot tell "you forgot to say which scope axis you are modifying" from "that Reserved Instance does not exist"; it retries or reports the wrong fault, and the ordering guarantee (configuration validated before inventory lookup) is lost.
Fix: this operation's validation — the target-configuration loop in services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_reserved_instances.rs (around the existing instance_count / scope checks, before `require_service_state` is reached), minting `missing_parameter` for the scope-less case.
Traces: ec2.op-contract-modify-reserved-instances-rejected.

Cause 3 of 3: PurchaseReservedInstancesOffering answers 501 NotImplemented on both of its refusal paths.
AWS: the model marks DryRun on PurchaseReservedInstancesOffering with the standard doc "Checks whether you have the required permissions for the action, without actually making the request, and provides an error response. If you have the required permissions, the error response is DryRunOperation. Otherwise, it is UnauthorizedOperation." The recording shows DryRun=true with a well-formed (random-UUID) ReservedInstancesOfferingId refused `DryRunOperation` with the fixed message `Request would have succeeded, but DryRun flag is set.`, and the same call without DryRun against a malformed offering id refused `InvalidParameterValue` with message `Invalid value 'ReservedInstancesOfferingId' for <ReservedInstancesOfferingId>.` (fixed text with the submitted id as its variable part). Neither call purchases anything.
Emulator: both calls are refused with Code `NotImplemented` and the message `operation PurchaseReservedInstancesOffering is scaffolded but not implemented in this emulator` — the handler is still the scaffold stub (services/service-com-amazonaws-ec2-amazonec2/src/ops/purchase_reserved_instances_offering.rs carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented`).
Why: a client's permission preflight (DryRun) and its input validation both come back as an opaque, non-retriable 501 that no SDK error handler recognises, so the documented DryRunOperation gate and the malformed-id rejection are unreachable; code that uses DryRun to probe entitlement before committing real spend cannot run at all.
Fix: this operation's handler — answer the DryRun gate and the ReservedInstancesOfferingId shape validation in services/service-com-amazonaws-ec2-amazonec2/src/ops/purchase_reserved_instances_offering.rs before any scaffold short-circuit, and drop the SCAFFOLDED marker when it is implemented.
Traces: ec2.op-contract-purchase-reserved-instances-offering-rejected.

## Evidence
Verbatim replay diagnosis lines (from the behavioral suite, simparity-behavioral-aws-v1).

Cause 1 — [mismatch] ec2.op-contract-get-transit-gateway-metering-policy-entries:
  File "check_ec2_op_contract_get_transit_gateway_metering_policy_entries.py", line 65, in run
  assert message == expected_message
  AssertionError: expected message 'Transit Gateway Metering Policy Id tgw-mp-fffffffffffffffff was deleted or does not exist.', got 'Transit Gateway Metering Policy tgw-mp-fffffffffffffffff was deleted or does not exist.'
  (the error Code assertion immediately above it, `InvalidTransitGatewayMeteringPolicyId.NotFound`, PASSED — only the sentence diverges)

Cause 2 — [mismatch] ec2.op-contract-modify-reserved-instances-rejected:
  File "check_ec2_op_contract_modify_reserved_instances_rejected.py", line 43, in run
  assert code == "MissingParameter", f"expected MissingParameter for a scope-less target configuration, got {code}"
  AssertionError: expected MissingParameter for a scope-less target configuration, got InvalidInput
  (request under test: ReservedInstancesIds=['1a2b3c4d-1234-5678-9abc-1234567890ab'], TargetConfigurations=[{'InstanceCount': 1, 'InstanceType': 't3.micro'}] — no AvailabilityZone, no Platform; expected message 'The request must contain the parameter  AvailabilityZone')

Cause 3 — [mismatch] ec2.op-contract-purchase-reserved-instances-offering-rejected:
  File "check_ec2_op_contract_purchase_reserved_instances_offering_rejected.py", line 41, in run
  assert control_err.get("Code") == "DryRunOperation"
  AssertionError: a well-formed offering id should clear shape validation and reach the dry-run gate, got 'NotImplemented'
  (handler source confirms the cause: services/service-com-amazonaws-ec2-amazonec2/src/ops/purchase_reserved_instances_offering.rs lines 4 and 164 — `SCAFFOLDED, NOT IMPLEMENTED` and `Err(crate::scaffolded::Scaffolded::not_implemented("PurchaseReservedInstancesOffering"))`)

## Steps to Reproduce
1. Boot the emulator and point an EC2 client (botocore) at it.
2. Cause 1: CreateTransitGateway -> wait available -> CreateTransitGatewayMeteringPolicy -> CreateTransitGatewayMeteringPolicyEntry. Then call GetTransitGatewayMeteringPolicyEntries with a well-formed but nonexistent TransitGatewayMeteringPolicyId (e.g. the real id with its hex suffix replaced by 'f's). Observe Code InvalidTransitGatewayMeteringPolicyId.NotFound (correct) with the message missing the word 'Id' after 'Transit Gateway Metering Policy'.
3. Cause 2: call ModifyReservedInstances(ReservedInstancesIds=['1a2b3c4d-1234-5678-9abc-1234567890ab'], TargetConfigurations=[{'InstanceCount': 1, 'InstanceType': 't3.micro'}]). Observe InvalidInput (the id-not-found refusal) instead of MissingParameter / 'The request must contain the parameter  AvailabilityZone'. For contrast, repeat with AvailabilityZone='not-a-real-az' (expect InvalidParameterValue) and with a real zone (expect InvalidInput) — the emulator cannot distinguish the first case from the third.
4. Cause 3: call PurchaseReservedInstancesOffering(ReservedInstancesOfferingId=<random uuid4>, InstanceCount=1, DryRun=True). Observe 501 NotImplemented instead of DryRunOperation / 'Request would have succeeded, but DryRun flag is set.'. Repeat without DryRun and with ReservedInstancesOfferingId='not-a-real-offering-id' — observe NotImplemented instead of InvalidParameterValue.
5. Re-replay the three check ids in trace_ids to confirm a fix.
