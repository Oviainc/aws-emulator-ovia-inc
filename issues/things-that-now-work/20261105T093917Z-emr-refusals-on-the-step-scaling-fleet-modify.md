Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.add-steps-to-terminated-cluster-rejected emr.auto-scaling-policy-master-group-rejected emr.put-auto-scaling-policy-malformed-rule-rejected emr.managed-scaling-policy-invalid-capacity-bounds-rejected emr.managed-scaling-policy-invalid-capacity-range-rejected emr.modify-cluster-step-concurrency-level-out-of-range-rejected emr.modify-instance-fleet-invalid-capacity-rejected emr.op-tag-ops-reject-unknown-resource
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.add-steps-to-terminated-cluster-rejected|run#11|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: emr.auto-scaling-policy-master-group-rejected|run#8|AssertionError|assert message == MASTER_GROUP_MESSAGE, f"unexpected rejection message: {message!r}"
Issue-Key: emr.put-auto-scaling-policy-malformed-rule-rejected|run#10|AssertionError|assert message == "Auto Scaling policy should not be specified for Master Instance Groups.", (
Issue-Key: emr.managed-scaling-policy-invalid-capacity-bounds-rejected|run#8|AssertionError|assert message == (
Issue-Key: emr.managed-scaling-policy-invalid-capacity-range-rejected|run#10|AssertionError|assert error.get("Message") == (
Issue-Key: emr.modify-cluster-step-concurrency-level-out-of-range-rejected|run#11|AssertionError|assert err["Message"] == _BARE_MESSAGE, f"level 0: message was {err['Message']!r}"
Issue-Key: emr.modify-instance-fleet-invalid-capacity-rejected|run#12|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: emr.op-tag-ops-reject-unknown-resource|_error#10|AssertionError|raise AssertionError("expected the operation to be rejected, but it succeeded")
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# EMR refusals on the step, scaling, fleet-modify and tag paths carry emulator-authored messages, the wrong error code, or are not made at all

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAutoScalingPolicy

## Description
Six refusal paths diverge from AWS: five answer the right status but an emulator-authored message (or the wrong error code), and one is not refused at all. THIS ISSUE BUNDLES 6 CAUSES. All six must be closed.

Cause 1 of 6: AddJobFlowSteps on a terminated cluster appends emulator-authored detail to AWS's sentence.
  AWS: 400 ValidationException with Message "A job flow that is shutting down, terminated, or finished may not be modified." — entirely fixed text, with NO cluster id or state interpolated. Smithy: AddJobFlowSteps.JobFlowId required (XmlStringMaxLen256); the operation's modelled error list carries only InternalServerError, so the ValidationException envelope is recorded behavior only.
  Emulator: 400 ValidationException with "A job flow that is shutting down, terminated, or finished may not be modified: 'j-<id>' is TERMINATED" — it appends ': ' plus the cluster id from the request and the state from its own store.
  Why: a client matching AWS's sentence (log scraping, retry classification) does not match; the extra values are not part of AWS's template.
  Fix: the AddJobFlowSteps handler's cluster-state gate message.
  Traces: emr.add-steps-to-terminated-cluster-rejected.

Cause 2 of 6: PutAutoScalingPolicy on a primary (MASTER) instance group rewords the refusal.
  AWS: 400 ValidationException with Message "Auto Scaling policy should not be specified for Master Instance Groups." — entirely fixed text, naming no group id. Smithy: PutAutoScalingPolicy.InstanceGroupId required (InstanceGroupId @length 0..256); the operation declares no modelled errors, so the envelope is recorded behavior only.
  Emulator: 400 with "An automatic scaling policy is supported for an instance group of type CORE or TASK, and instance group 'ig-<id>' is of type 'MASTER'." — an emulator-authored sentence that interpolates the group id and role.
  Why: a client asserting on AWS's sentence sees a different message for the same refusal class, on both the well-formed and the malformed-rule request.
  Fix: the PutAutoScalingPolicy handler's instance-group-role validation message.
  Traces: emr.auto-scaling-policy-master-group-rejected, emr.put-auto-scaling-policy-malformed-rule-rejected.

Cause 3 of 6: PutManagedScalingPolicy's min>max refusal rewords the message.
  AWS: 400 ValidationException with Message "Invalid configuration for managed scaling. The specified max capacity must be greater than min capacity." — entirely fixed text, with no capacity values interpolated. Smithy: PutManagedScalingPolicy.ManagedScalingPolicy -> ManagedScalingPolicy.ComputeLimits.
  Emulator: 400 with "ComputeLimits.MinimumCapacityUnits <min> is above ComputeLimits.MaximumCapacityUnits <max>" — an emulator-authored sentence built from the request's values (observed with 10/4 and 10/2).
  Why: the code matches but the message does not, for every min>max request.
  Fix: the PutManagedScalingPolicy handler's ComputeLimits validation message.
  Traces: emr.managed-scaling-policy-invalid-capacity-bounds-rejected, emr.managed-scaling-policy-invalid-capacity-range-rejected.

Cause 4 of 6: ModifyCluster's out-of-range StepConcurrencyLevel refusals use one emulator-authored sentence instead of AWS's two.
  AWS: 400 InvalidRequestException (Smithy: ModifyCluster models InvalidRequestException httpError 400) with two distinct fixed messages under that one code — "Invalid step concurrency level. Allowed level is from 1 to 256" for a level in 1..6400 that is above 256, and the bare "Invalid step concurrency level." for 0 or below and for anything above 6400. No value from the request appears in either.
  Emulator: 400 InvalidRequestException with "StepConcurrencyLevel <level> is outside 1..256" — one emulator-authored sentence interpolating the requested level, for every out-of-range value.
  Why: a client cannot distinguish the two refusal classes AWS distinguishes, and neither message matches.
  Fix: the ModifyCluster handler's StepConcurrencyLevel validation (two message templates selected by the requested level).
  Traces: emr.modify-cluster-step-concurrency-level-out-of-range-rejected.

Cause 5 of 6: ModifyInstanceFleet's negative-capacity refusal uses an error code the operation does not model.
  AWS: 400 InvalidRequestException with Message "Invalid instance fleet capacity '<value>' specified for instance fleet '<instanceFleetId>', must be a non-negative number." — fixed text with the rejected capacity value from the request and the fleet id from the request; the fleet's capacity is left untouched. Smithy: ModifyInstanceFleet's modelled errors are exactly InvalidRequestException (httpError 400) and InternalServerException (500) — ValidationException is NOT in its error list.
  Emulator: 400 with error code 'ValidationException'.
  Why: a client switching on the modelled error code never matches InvalidRequestException and treats a modelled client refusal as an unknown error.
  Fix: the ModifyInstanceFleet handler's capacity validation (error class and message template).
  Traces: emr.modify-instance-fleet-invalid-capacity-rejected.

Cause 6 of 6: AddTags/RemoveTags do not validate at all — an empty tag list and an unknown cluster id are both accepted.
  AWS: 400 InvalidRequestException with Message "Tags cannot be null or empty." when Tags is an empty list on an EXISTING cluster, and 400 InvalidRequestException with "Cluster id '<ResourceId>' is not valid." — fixed text with the ResourceId from the request — for an unknown cluster id, including when the tag list is also empty (the target resource is resolved BEFORE the tag list is validated). Smithy: AddTags.ResourceId and AddTags.Tags both required; modelled errors InvalidRequestException (400) and InternalServerException (500).
  Emulator: 200 success — the call is accepted with no refusal, so the check's rejection helper reports 'expected the operation to be rejected, but it succeeded'.
  Why: a client gets a success for a tag call that changed nothing, and for a resource that does not exist, hiding both a typo'd cluster id and an empty payload.
  Fix: the AddTags and RemoveTags handlers' validation, resolving ResourceId before validating the tag list.
  Traces: emr.op-tag-ops-reject-unknown-resource.

Note: open issue 20261029T212242Z-elasticmapreduce-emr-validation-refusals-carry-the-wrong-error-code.md and 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md cover other operations' refusal wording and do not carry these check ids.

## Evidence
Replay of simparity-behavioral-aws-v1, '### emr':

1. emr.add-steps-to-terminated-cluster-rejected — AssertionError: AddJobFlowSteps on a TERMINATED cluster should be refused ValidationException 'A job flow that is shutting down, terminated, or finished may not be modified.', got 'ValidationException' "A job flow that is shutting down, terminated, or finished may not be modified: 'j-0000000000001' is TERMINATED".
2. emr.auto-scaling-policy-master-group-rejected / emr.put-auto-scaling-policy-malformed-rule-rejected — expected 'Auto Scaling policy should not be specified for Master Instance Groups.', got "An automatic scaling policy is supported for an instance group of type CORE or TASK, and instance group 'ig-0000000000001' is of type 'MASTER'.".
3. emr.managed-scaling-policy-invalid-capacity-bounds-rejected / -range-rejected — expected 'Invalid configuration for managed scaling. The specified max capacity must be greater than min capacity.', got 'ComputeLimits.MinimumCapacityUnits 10 is above ComputeLimits.MaximumCapacityUnits 4' (and '… above … 2').
4. emr.modify-cluster-step-concurrency-level-out-of-range-rejected — level 0: expected 'Invalid step concurrency level.', got 'StepConcurrencyLevel 0 is outside 1..256' (the code InvalidRequestException matched).
5. emr.modify-instance-fleet-invalid-capacity-rejected — AssertionError: expected InvalidRequestException, got 'ValidationException' for InstanceFleet{TargetOnDemandCapacity: -5, TargetSpotCapacity: 0}; expected message "Invalid instance fleet capacity '-5' specified for instance fleet '<fleetId>', must be a non-negative number.".
6. emr.op-tag-ops-reject-unknown-resource — AssertionError: expected the operation to be rejected, but it succeeded (AddTags with Tags=[] on a live cluster, expected InvalidRequestException 'Tags cannot be null or empty.'; AddTags/AddTags([])/RemoveTags against ResourceId 'j-ZZZZZZZZZZZZZ', expected InvalidRequestException "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.").

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator.
2. Launch a live cluster with an instance group layout (and a second with instance fleets) using a fresh service role, instance profile and subnet.
3. Cause 1: terminate the cluster, then AddJobFlowSteps on it — compare Message with AWS's bare sentence.
4. Cause 2: PutAutoScalingPolicy with the primary group's InstanceGroupId.
5. Cause 3: PutManagedScalingPolicy with ComputeLimits MinimumCapacityUnits > MaximumCapacityUnits.
6. Cause 4: ModifyCluster with StepConcurrencyLevel 0, 257 and 6401 — expect InvalidRequestException with the bare sentence, the ranged sentence and the bare sentence respectively.
7. Cause 5: ModifyInstanceFleet (unvalidated client) with TargetOnDemandCapacity=-5 and TargetSpotCapacity=0 — expect InvalidRequestException and the quoted message.
8. Cause 6: AddTags with Tags=[] on the live cluster, then AddTags/RemoveTags against ResourceId 'j-ZZZZZZZZZZZZZ' — expect InvalidRequestException both times; observe 200.
9. Replay the checks in trace_ids to confirm.
