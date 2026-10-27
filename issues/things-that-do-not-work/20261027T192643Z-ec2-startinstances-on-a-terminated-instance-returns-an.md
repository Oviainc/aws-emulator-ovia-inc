Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.instance-run-stop-start-terminate-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.instance-run-stop-start-terminate-lifecycle|run#32|AssertionError|assert got == expected, f"StartInstances on the terminated instance: expected {expected}, got {got!r}"
Issue-Commit: 498a3c4496c661f7096ad33513094351185b7aba

# EC2 StartInstances on a terminated instance returns an IncorrectInstanceState message that diverges from AWS's template

> _Note: this issue was identified against the code base at commit `498a3c4496c661f7096ad33513094351185b7aba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstances

## Description
One net-new root cause in the EC2 emulator, grounded in one failing behavioral check. The HTTP status and the error code are correct; the MESSAGE text the emulator builds for the refusal diverges from AWS's fixed, request-derived template.

Cause 1 of 1: StartInstances on a terminated instance emits a non-AWS IncorrectInstanceState message.

AWS: recorded behavior only — EC2 uses the ec2Query protocol and models no error shapes for this operation (Smithy: ec2 StartInstances, input InstanceIds (InstanceIdStringList, required), output StartingInstances, errors: [] — no modeled error shapes, so the error contract is only observable from recorded responses). The recording shows StartInstances naming an instance whose state is `terminated` refused with HTTP 400 and error code `IncorrectInstanceState`, message: "The instance '<instance-id>' is not in a state from which it can be started." The FIXED text of that template is `The instance '` + `' is not in a state from which it can be started.` (single trailing full stop); the only VARIABLE part is the instance id, taken verbatim from the request's InstanceIds member. AWS deliberately does not name the offending state in this message.

Emulator: same HTTP 400 and the same code `IncorrectInstanceState`, but the message is "The instance '<instance-id>' is in state '<state>', which is not a state from which it can be started" — it substitutes a different sentence, interpolates the instance's current state from emulator state (a value AWS's template does not contain), and drops the trailing full stop. Observed in the replay as: expected ('IncorrectInstanceState', "The instance 'i-00000000000000001' is not in a state from which it can be started."), got ('IncorrectInstanceState', "The instance 'i-00000000000000001' is in state 'terminated', which is not a state from which it can be started"). The instance id and the state token above are this run's values filling the template, not requirements.

Why: a client that relies on AWS's behavior gets the wrong text for an error class it must often match on. Harnesses, retry wrappers and operator runbooks that compare the IncorrectInstanceState message against AWS's documented sentence (or regex it for the trailing period / exact phrasing) see an unrecognised message and either mis-handle a terminated-instance start or surface a message AWS never produces. The emulator additionally discloses the instance's internal state in a message AWS keeps state-free, so a consumer written against the emulator will come to depend on a field real AWS never supplies.

Fix: the StartInstances handler's state-check error construction in the EC2 service (services/service-com-amazonaws-ec2-amazonec2/src/ops/start_instances.rs) — build the message from AWS's template with the request's instance id as the only interpolated value, and stop appending the current-state clause. This is the same class of defect as the message-template causes already filed in 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md (DeleteVpc, DeleteSecurityGroup, CreateSubnet, CreatePlacementGroup, DetachVolume) and 20261025T030047Z-ec2-rejection-message-templates-diverge-from-aws-on.md (ModifyVolume, DescribeInstances); neither of those carries this check's replay key and neither names StartInstances, so this cause is filed separately and those issues are named here so a fixer can address them together if the message builder is shared.

Traces: ec2.instance-run-stop-start-terminate-lifecycle.

## Evidence
Check ec2.instance-run-stop-start-terminate-lifecycle (verdict: mismatch).

AssertionError at check_ec2_instance_run_stop_start_terminate_lifecycle.py line 174:
  assert got == expected, f"StartInstances on the terminated instance: expected {expected}, got {got!r}"
  AssertionError: StartInstances on the terminated instance: expected ('IncorrectInstanceState', "The instance 'i-00000000000000001' is not in a state from which it can be started."), got ('IncorrectInstanceState', "The instance 'i-00000000000000001' is in state 'terminated', which is not a state from which it can be started")

Expected (AWS): HTTP 400, Error.Code = IncorrectInstanceState, Error.Message = "The instance '<instance-id>' is not in a state from which it can be started."
Actual (emulator): HTTP 400, Error.Code = IncorrectInstanceState, Error.Message = "The instance '<instance-id>' is in state '<current-state>', which is not a state from which it can be started"

Message anatomy — fixed text: "The instance '" ... "' is not in a state from which it can be started." ; variable: the instance id, copied from the request's InstanceIds. The emulator's extra "is in state '<state>', which" clause and its missing trailing period are the divergence. The literal id i-00000000000000001 and the state token 'terminated' are this run's values and are not part of the requirement.

The status (400) and the code (IncorrectInstanceState) matched; the preceding RunInstances -> running -> StopInstances -> stopped -> StartInstances -> running -> TerminateInstances -> terminated beats, the live-instance tag filter, and the 4xx status assertion all passed, so the only divergence is the message template.

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): StartInstances errors = [] — no modeled error shapes, hence "recorded behavior only" for the code and message.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Replay check ec2.instance-run-stop-start-terminate-lifecycle.
3. Or by hand against the emulator: CreateVpc 10.211.0.0/16, CreateSubnet 10.211.1.0/24, RunInstances (t3.micro, the catalog AMI, that subnet), wait for state `running`.
4. StopInstances, wait for `stopped`; StartInstances, wait for `running`; TerminateInstances, wait for `terminated`.
5. Call StartInstances again on the terminated instance id.
6. Observe HTTP 400 / IncorrectInstanceState with message "The instance '<id>' is in state 'terminated', which is not a state from which it can be started"; AWS answers HTTP 400 / IncorrectInstanceState with "The instance '<id>' is not in a state from which it can be started." (instance id from the request, no state clause, trailing full stop).
