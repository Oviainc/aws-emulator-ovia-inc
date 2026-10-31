Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-volume-attachment
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-volume-attachment ec2-instance-monitoring-toggle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteNetworkInterface|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteNetworkInterface|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|MonitorInstances|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|MonitorInstances|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: 9737b41b9bb889e100c3bf9348295e20ff42401a

# EC2 refusals answer an emulator-authored error code and reworded message instead of AWS's InvalidParameterValue (2 causes)

> _Note: this issue was identified against the code base at commit `9737b41b9bb889e100c3bf9348295e20ff42401a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNetworkInterface

## Description
Two net-new root causes share one fault: the refusal is produced with the right HTTP status but the wrong error code and emulator-authored prose, so neither the code a client matches on nor the message AWS words is reproduced. Both causes must be closed before this issue is submitted.

Cause 1 of 2: DeleteNetworkInterface on an in-use ENI answers InvalidNetworkInterface.InUse instead of InvalidParameterValue.
AWS: recorded behavior only for the error classification — Smithy models no errors for this operation (DeleteNetworkInterface errors: [], the ec2Query error shapes are not in the model), and the request shape is DryRun + the required NetworkInterfaceId. The recording shows HTTP 400 with Response.Errors.Error.Code 'InvalidParameterValue' and Response.Errors.Error.Message "Network interface '<networkInterfaceId>' is currently in use." — fixed text "Network interface '" ... "' is currently in use." with the only variable part being the network-interface id taken from the request.
Emulator: HTTP 400 with Code 'InvalidNetworkInterface.InUse' and Message "The network interface '<networkInterfaceId>' is currently in use and must be detached first" — an invented code and reworded prose (leading "The", trailing " and must be detached first", no final period).
Why: a client that branches on the error code (retry/ignore on InvalidParameterValue versus treating an unknown code as fatal) takes the wrong branch, and any assertion on AWS's message text fails.
Fix: the DeleteNetworkInterface refusal path — its in-use validation and the error-code/message mapping it emits.
Traces: ec2-conflict-volume-attachment#task#14.

Cause 2 of 2: MonitorInstances with a malformed instance id answers InvalidInstanceID.Malformed instead of AWS's parameter-value refusal.
AWS: recorded behavior only — Smithy models no errors for MonitorInstances (errors: []); its input is the required InstanceIds list (xmlName InstanceId). The recording shows HTTP 400 with Code 'InvalidParameterValue' and Message "Value (<supplied value>) for parameter instanceId is invalid. Expected: 'i-...'." — fixed text "Value (" ... ") for parameter instanceId is invalid. Expected: 'i-...'." with the supplied parameter value as the only variable part.
Emulator: HTTP 400 with Code 'InvalidInstanceID.Malformed' and Message 'Invalid id: "<supplied value>"' — a different code and a generic message that names neither the parameter nor the expected id form.
Why: AWS classifies a syntactically invalid instanceId on MonitorInstances as a parameter-value problem; a client distinguishing "bad argument" from "unknown/ malformed instance" mis-handles the failure, and the message loses the parameter name and expected prefix it is supposed to report.
Fix: MonitorInstances input validation — the id-syntax check and the error class and message template it raises.
Traces: ec2-instance-monitoring-toggle#task#6.

Related open issues that do NOT carry these replay keys (the replay reports both clusters unowned): 20261029T200817Z-ec2-error-paths-choose-the-wrong-error-class.md, 20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md, 20261029T215142Z-ec2-refusal-messages-reword-aws-text-across-eight.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, both clusters verdict=mismatch, expected status 400 and 400 returned, stub_path empty, owned_by empty.

Cause 1 — ec2-conflict-volume-attachment#task#14, DeleteNetworkInterface:
  $.Response.Errors.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'InvalidNetworkInterface.InUse')
  $.Response.Errors.Error.Message: value_mismatch (expected "Network interface 'eni-0000000000000003b' is currently in use.", got "The network interface 'eni-0000000000000003b' is currently in use and must be detached first")

Cause 2 — ec2-instance-monitoring-toggle#task#6, MonitorInstances:
  $.Response.Errors.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'InvalidInstanceID.Malformed')
  $.Response.Errors.Error.Message: value_mismatch (expected "Value (not-an-id) for parameter instanceId is invalid. Expected: 'i-...'.", got 'Invalid id: "not-an-id"')

The eni id and the literal 'not-an-id' above come from those recordings' requests; the requirement is the fixed text plus the request's own value, not these specific strings.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture ec2-conflict-volume-attachment up to step task#14 — DeleteNetworkInterface on a network interface that is still attached. Expect 400 / InvalidParameterValue / "Network interface '<eni-id>' is currently in use."; observe 400 / InvalidNetworkInterface.InUse with the reworded message.
3. Cause 2: replay fixture ec2-instance-monitoring-toggle step task#6 — MonitorInstances with InstanceId.1 set to a non-id string. Expect 400 / InvalidParameterValue / "Value (<value>) for parameter instanceId is invalid. Expected: 'i-...'."; observe 400 / InvalidInstanceID.Malformed / 'Invalid id: "<value>"'.
