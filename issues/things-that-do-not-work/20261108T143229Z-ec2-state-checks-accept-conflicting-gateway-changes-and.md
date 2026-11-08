Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-internet-gateway-attachment-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-internet-gateway-attachment-errors ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AttachInternetGateway|4xx->2xx|$.Response
Issue-Key: ec2|AttachInternetGateway|4xx->2xx|status
Issue-Key: ec2|DetachInternetGateway|4xx->2xx|$.Response
Issue-Key: ec2|DetachInternetGateway|4xx->2xx|status
Issue-Key: ec2|DeleteLaunchTemplate|2xx->4xx|status
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# EC2 state checks accept conflicting gateway changes and refuse a valid delete

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachInternetGateway

## Description
This issue bundles 3 causes. They share one fault: these mutating operations do not test the emulator's stored resource state the way AWS does before answering, so two requests AWS refuses are accepted with 200 and one request AWS accepts is refused with 400.

Cause 1 of 3: AttachInternetGateway against a VPC that already has an internet gateway attached (fixture ec2-internet-gateway-attachment-errors, step task#4).
AWS: recorded behavior only - Smithy (services/service-com-amazonaws-ec2-amazonec2) AttachInternetGateway has output_shape Unit and errors: [], so the refusal contract is the recording's. A VPC may have at most one attached internet gateway; AWS answers HTTP 400 with Code 'InvalidParameterValue' and Message 'Network <vpc-id> already has an internet gateway attached'. Fixed text: 'Network ' ... ' already has an internet gateway attached'; the variable part is the VpcId from the request.
Emulator: answers HTTP 200 with an empty success body and no Response/Errors element at all.
Why: the one-IGW-per-VPC invariant is silently broken. A client that attaches a second gateway believes it succeeded, and the emulator's VPC now holds state AWS cannot produce, so every later route/egress assertion built on 'the VPC's internet gateway' becomes ambiguous.
Fix: the AttachInternetGateway handler's state precondition (reject when the target VPC already has an attachment) and the error path that serialises the refusal.
Traces: ec2-internet-gateway-attachment-errors#task#4.

Cause 2 of 3: DetachInternetGateway for a gateway that is not attached to the given VPC (fixture ec2-internet-gateway-attachment-errors, step task#5).
AWS: recorded behavior only (same op shape: output Unit, errors: []). AWS answers HTTP 400 with Code 'Gateway.NotAttached' and Message 'resource <internet-gateway-id> is not attached to network <vpc-id>'. Fixed text: 'resource ' ... ' is not attached to network '; the variable parts are the InternetGatewayId and VpcId from the request.
Emulator: answers HTTP 200 with an empty success body and no error element.
Why: detach is reported as idempotent when AWS makes it conditional. Teardown logic that relies on Gateway.NotAttached to learn a gateway is already free cannot distinguish 'I detached it' from 'it was never attached', and a caller that detached the wrong (vpc, gateway) pair gets no signal.
Fix: the DetachInternetGateway handler's state precondition (verify the attachment exists for that VPC) and its error path.
Traces: ec2-internet-gateway-attachment-errors#task#5.

Cause 3 of 3: DeleteLaunchTemplate for a launch template that exists (fixture ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override, step teardown#20).
AWS: Smithy: DeleteLaunchTemplate takes LaunchTemplateId or LaunchTemplateName (@length 3..128, pattern ^[a-zA-Z0-9\(\)\.\-/_]+$), output_shape DeleteLaunchTemplateResult carrying launchTemplate, errors: []. Deleting an existing template succeeds: HTTP 200 with the deleted template echoed in launchTemplate.
Emulator: answers HTTP 400 - the delete of a template created earlier in the same fixture is refused.
Why: a valid teardown fails, so the resource is left behind and the caller cannot clean up; any suite that deletes and recreates a template by the same name is blocked on the second pass.
Hypothesis: the fixture's earlier CreateLaunchTemplateVersion steps hit the unimplemented scaffold (501) in this emulator, so the template may be in a state the delete path does not recognise; the replay reports this cluster as a standalone unowned divergence, not as blocked, so it reproduces independently.
Fix: the DeleteLaunchTemplate handler's lookup/state check (resolve the template by id or name against stored state and return DeleteLaunchTemplateResult).
Traces: ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override#teardown#20.

## Evidence
Cause 1 - fixture ec2-internet-gateway-attachment-errors step task#4, AttachInternetGateway: expected status 400, actual 200; $.Response missing - expected {'Errors': {'Error': {'Code': 'InvalidParameterValue', 'Message': 'Network vpc-00000000000000034 already has an internet gateway attached'}}}, got None.
Cause 2 - fixture ec2-internet-gateway-attachment-errors step task#5, DetachInternetGateway: expected status 400, actual 200; $.Response missing - expected {'Errors': {'Error': {'Code': 'Gateway.NotAttached', 'Message': 'resource igw-00000000000000043 is not attached to network vpc-00000000000000034'}}}, got None.
Cause 3 - fixture ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override step teardown#20, DeleteLaunchTemplate: expected status 200, actual 400 (divergence_key service=ec2|DeleteLaunchTemplate|status=200->400|keys=status).

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: replay ec2-internet-gateway-attachment-errors#task#4 - create a VPC, attach an internet gateway, then AttachInternetGateway a second gateway to the same VPC. Expect 400 InvalidParameterValue 'Network <vpc-id> already has an internet gateway attached'; the emulator answers 200.
3. Cause 2: replay ec2-internet-gateway-attachment-errors#task#5 - DetachInternetGateway for an (igw, vpc) pair with no attachment. Expect 400 Gateway.NotAttached; the emulator answers 200.
4. Cause 3: replay ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override#teardown#20 - DeleteLaunchTemplate for the template the fixture created. Expect 200 with the deleted launchTemplate; the emulator answers 400.
