Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.escc-assign-public-ip-value-is-case-sensitive
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.escc-assign-public-ip-value-is-case-sensitive|run#9|AssertionError|assert msg == want, f"lower-case assignPublicIp refusal message: expected {want!r}, got {msg!r}"
Issue-Commit: fb1de11771ea07ab11d207d382a4d90e01236a0b

# ECS CreateService refuses a lower-case assignPublicIp with a generic constraint-violation message instead of AWS's 'assign public ip should be one of [DISABLED,ENABLED]'

> _Note: this issue was identified against the code base at commit `fb1de11771ea07ab11d207d382a4d90e01236a0b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
THIS BUNDLE CONTAINS 1 CAUSE.

Cause 1 of 1 — CreateService with networkConfiguration.awsvpcConfiguration.assignPublicIp given in the wrong case is refused with the right error type but an emulator-framework message, not the message AWS words for this refusal.

AWS: Smithy: com.amazonaws.ecs#AssignPublicIp is an enum with exactly the values [ENABLED, DISABLED] (case-sensitive, matched exactly); CreateService.networkConfiguration.awsvpcConfiguration.assignPublicIp targets it; CreateService's modeled errors include InvalidParameterException (errorKind client, httpError 400, defaulted) whose single member is `message`, documented as "Message that describes the cause of the exception." The model does not state the refusal text, so for the text this is recorded behavior only: a value outside the enum set (here the lower-case spelling of a valid value) is refused HTTP 400 / InvalidParameterException with message 'assign public ip should be one of [DISABLED,ENABLED]'. That whole string is FIXED text for this error class — ECS names the member in spaced lower-case prose ('assign public ip') and lists the permitted values in the order [DISABLED,ENABLED] with no spaces; there is NO variable part, in particular the rejected value is not echoed and no member path is included.

Emulator: returns the correct code InvalidParameterException (the check's code assertion passed) but the message "1 validation error detected: Value 'enabled' at 'networkConfiguration.awsvpcConfiguration.assignPublicIp' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]". That is the generic Smithy/constraint-framework rendering: a leading violation count, the echoed request value, the full member path, and the enum set in model order. Hypothesis: enum membership is being enforced by a generic input-validation layer that formats its own message, so ECS's own refusal text for this parameter is never produced.

Why: a client that surfaces, logs, or matches the refusal text sees text that never appears on AWS. Anything keying on the documented sentence (a deployment tool mapping the refusal to "assignPublicIp must be ENABLED or DISABLED", a test suite asserting the message, an operator-facing error pass-through) gets no match, and callers that parse the value list out of the message read it in a different order and a different shape than AWS emits. Because only the text differs, the divergence is invisible to code that checks the error code alone and shows up exactly where a user reads the message.

Fix: the assignPublicIp (AwsVpcConfiguration) validation path reached from the CreateService handler in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs — it must raise InvalidParameterException with ECS's own fixed message for an out-of-set assignPublicIp instead of letting the generic enum-constraint validator author the message. The same parameter is validated by UpdateService and RunTask, so the refusal text belongs with the shared awsvpcConfiguration validation rather than being special-cased in one handler.

Note on existing issues: the open issue 20261025T112916Z-ecs-refusal-messages-use-emulator-internal-wording-createservice.md covers two OTHER ECS message-wording causes (CreateService on an all-INACTIVE task-definition family, and CreateCluster clusterName pattern validation) and does not carry this check's replay key; 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md is about MISSING refusals (200 where AWS returns 400) — here the refusal does happen with the right type. So this cause is net-new.

Traces: ecs.escc-assign-public-ip-value-is-case-sensitive. The check aborts on this message assertion, so its remaining assertions were never exercised: that the rejected request leaves no service behind (DescribeServices returns services=[] and one failure with reason MISSING) and that the upper-case control 'ENABLED' is accepted and read back from DescribeServices. Re-replay the whole check after fixing the message to confirm those.

## Evidence
Check ecs.escc-assign-public-ip-value-is-case-sensitive (verdict: mismatch).

Request: CreateService{cluster=<fresh cluster>, serviceName=<fresh name>, taskDefinition=<Fargate awsvpc task definition>, desiredCount=0, launchType='FARGATE', networkConfiguration={awsvpcConfiguration:{subnets:[<subnet>], assignPublicIp:'enabled'}}}.

Expected (real AWS): HTTP 400, Error.Code 'InvalidParameterException', Error.Message 'assign public ip should be one of [DISABLED,ENABLED]'.

Actual (emulator): HTTP 400, Error.Code 'InvalidParameterException' (agrees), Error.Message "1 validation error detected: Value 'enabled' at 'networkConfiguration.awsvpcConfiguration.assignPublicIp' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]".

Replay output:
  File "check_ecs_escc_assign_public_ip_value_is_case_sensitive.py", line 40, in run
    assert msg == want, f"lower-case assignPublicIp refusal message: expected {want!r}, got {msg!r}"
  AssertionError: lower-case assignPublicIp refusal message: expected 'assign public ip should be one of [DISABLED,ENABLED]', got "1 validation error detected: Value 'enabled' at 'networkConfiguration.awsvpcConfiguration.assignPublicIp' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]"

Message-part breakdown: AWS's string is entirely fixed text for this error class ('assign public ip should be one of [DISABLED,ENABLED]'); no part of it is taken from the request or from emulator state. In the emulator's string, 'enabled' is the value echoed from the request and 'networkConfiguration.awsvpcConfiguration.assignPublicIp' is the member path — neither belongs in AWS's message at all.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the failing check is ecs.escc-assign-public-ip-value-is-case-sensitive.
2. By hand: CreateVpc + CreateSubnet to get one subnet; CreateCluster{clusterName=<name>}; RegisterTaskDefinition{family=<name>, networkMode='awsvpc', requiresCompatibilities=['FARGATE'], cpu='256', memory='512', containerDefinitions=[{name:'web', image:'public.ecr.aws/docker/library/busybox:latest', essential:true, command:['sleep','3600']}]}.
3. Call CreateService{cluster=<cluster>, serviceName=<new name>, taskDefinition=<arn from step 2>, desiredCount=0, launchType='FARGATE', networkConfiguration={awsvpcConfiguration:{subnets:[<subnet>], assignPublicIp:'enabled'}}} — note the lower-case value.
4. Observe InvalidParameterException with message "1 validation error detected: Value 'enabled' at 'networkConfiguration.awsvpcConfiguration.assignPublicIp' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]" where AWS returns 'assign public ip should be one of [DISABLED,ENABLED]'.
5. Then confirm the rest of the check's path: DescribeServices{cluster, services:[<rejected name>]} must return services=[] with one failure of reason 'MISSING', and the same CreateService with assignPublicIp='ENABLED' must succeed and read back assignPublicIp='ENABLED'.
