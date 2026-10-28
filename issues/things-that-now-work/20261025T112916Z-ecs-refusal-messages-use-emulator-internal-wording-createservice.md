Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecracc-service-on-a-deregistered-revision-rejected ecs.invalid-parameter-cluster-and-account-ops
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecracc-service-on-a-deregistered-revision-rejected|run#21|AssertionError|assert msg == want, f"all-inactive family refusal message: expected {want!r}, got {msg!r}"
Issue-Key: ecs.invalid-parameter-cluster-and-account-ops|run#14|AssertionError|assert error == expected, f"CreateCluster: expected {expected!r}, got {error!r}"
Issue-Commit: 1f7e5adccf9e411825240a72188af4d50262df19

# ECS refusal messages use emulator-internal wording: CreateService on an all-INACTIVE family and CreateCluster with an invalid clusterName (2 causes)

> _Note: this issue was identified against the code base at commit `1f7e5adccf9e411825240a72188af4d50262df19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Two ECS validation paths refuse the request with the correct modeled error type and HTTP status, but serialize an emulator-authored `message` instead of the message AWS words for that error class. A client that surfaces, logs, parses or asserts on the refusal text sees text that never appears on AWS. The two causes are independent code paths (one in CreateService's task-definition resolution, one in CreateCluster's name validation); neither is covered by any open issue. The nearest open issues — 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md and 20261022T152302Z-ecs-accepts-invalid-requests-with-200-and-misreports.md — are about MISSING refusals (200 where AWS returns 400) and about a wrong error TYPE; these two refusals do happen, with the right type, and diverge only in the message text, so they are net-new.

THIS BUNDLE CONTAINS 2 CAUSES.

Cause 1 of 2 — CreateService naming a bare family whose only revision is INACTIVE returns the wrong refusal message.
AWS: Smithy: CreateService errors include ClientException (client, HTTP 400, httpError defaulted) and InvalidParameterException (client, HTTP 400); CreateService.taskDefinition accepts `family`, `family:revision` or a full revision ARN. Recorded behavior: resolving a bare family that has no ACTIVE revision (its only revision was deregistered) is refused with code ClientException and message 'TaskDefinition not found.' — the whole message is fixed text for this error class, with no variable part. (The sibling path in the same check is correct: naming the INACTIVE revision's exact ARN is refused InvalidParameterException / 'TaskDefinition is inactive', and the emulator matches it.)
Emulator: refuses with the right code, ClientException, but message 'Unable to describe task definition.' — internal wording describing the emulator's own failed lookup rather than AWS's not-found statement.
Why: a client (or a CDK/Terraform-style tool, or a test suite) that distinguishes "the family does not resolve to an ACTIVE revision" from other ClientException causes by matching the message gets no match, and the message it shows an operator does not describe the real cause (the family's revisions are all INACTIVE) — it reads like an internal describe failure, pointing at the wrong remediation.
Fix: the task-definition resolution/refusal in the CreateService handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs) must raise ClientException with AWS's text 'TaskDefinition not found.' when a bare family resolves to no ACTIVE revision, instead of propagating the internal describe-failure string.
Traces: ecs.ecracc-service-on-a-deregistered-revision-rejected.

Cause 2 of 2 — CreateCluster's clusterName validation message does not follow AWS's pattern-refusal wording.
AWS: Smithy: CreateCluster errors include InvalidParameterException (client, HTTP 400, httpError defaulted); input member clusterName is a String. Recorded behavior: a clusterName containing characters outside the allowed set is refused with code InvalidParameterException and message of the form 'Cluster name must match ^[a-zA-Z0-9\-_]{1,255}$, but was <clusterName>'. Fixed text: 'Cluster name must match ^[a-zA-Z0-9\-_]{1,255}$, but was ' — including the literal pattern and the character-class bounds. Variable part: the rejected clusterName, echoed verbatim from the request (in the recorded run that value was 'bad cluster name !! not allowed'; that string is the filled-in slot, not part of the requirement).
Emulator: same code and status, but message 'clusterName must be 1 to 255 letters, numbers, underscores or hyphens; got "<clusterName>"' — prose restating the constraint, with a different member spelling ('clusterName' vs 'Cluster name'), no regex, and the offending value wrapped in double quotes instead of appended bare.
Why: a client asserting on, or regex-matching, the documented refusal text cannot recognize the emulator's refusal; and anything that extracts the allowed pattern out of the message (to report it to a user, or to generate a compliant name) finds no pattern at all.
Fix: the clusterName validation in the CreateCluster handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_cluster.rs) must emit AWS's message template, interpolating the request's clusterName after the fixed 'but was ' prefix. The sibling messages the same check grades (DescribeClusters 'ClusterName must match ...', ListClusters 'Max results can not be lesser than 1', the account-setting 'Invalid setting ''value''' / 'Setting ''name'' must be provided when ''value'' is present' / 'Invalid PrincipalArn: <arn>' refusals) were not reached because the check aborts on the first mismatch; re-replay the check after fixing to confirm the rest of that path.
Traces: ecs.invalid-parameter-cluster-and-account-ops.

## Evidence
Cause 1 (check ecs.ecracc-service-on-a-deregistered-revision-rejected): CreateService{cluster, serviceName, taskDefinition=<family with only an INACTIVE revision>, desiredCount=0, launchType=EC2} -> code ClientException (matches AWS) but AssertionError: "all-inactive family refusal message: expected 'TaskDefinition not found.', got 'Unable to describe task definition.'". The preceding assertions in the same check passed, so the deregistered revision does describe INACTIVE and the by-ARN refusal (InvalidParameterException / 'TaskDefinition is inactive') is already correct — only the bare-family message diverges.

Cause 2 (check ecs.invalid-parameter-cluster-and-account-ops): CreateCluster{clusterName='bad cluster name !! not allowed'} -> AssertionError: "CreateCluster: expected ('InvalidParameterException', 'Cluster name must match ^[a-zA-Z0-9\\-_]{1,255}$, but was bad cluster name !! not allowed'), got ('InvalidParameterException', 'clusterName must be 1 to 255 letters, numbers, underscores or hyphens; got \"bad cluster name !! not allowed\"')". Error code and HTTP status agree; only the message text diverges. The check's positive controls (CreateCluster/DescribeClusters/ListClusters on a valid name) passed, and the rejected CreateCluster correctly left no cluster behind.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the two failing checks are ecs.ecracc-service-on-a-deregistered-revision-rejected and ecs.invalid-parameter-cluster-and-account-ops.
2. Cause 1 by hand: CreateCluster a cluster; RegisterTaskDefinition family F (networkMode bridge, one container); DeregisterTaskDefinition that revision (DescribeTaskDefinition must show status INACTIVE); then CreateService{cluster, serviceName=<new>, taskDefinition='F', desiredCount=0, launchType='EC2'}. Observe ClientException with message 'Unable to describe task definition.' where AWS returns 'TaskDefinition not found.'
3. Cause 2 by hand: CreateCluster{clusterName='bad cluster name !! not allowed'}. Observe InvalidParameterException with message 'clusterName must be 1 to 255 letters, numbers, underscores or hyphens; got "bad cluster name !! not allowed"' where AWS returns 'Cluster name must match ^[a-zA-Z0-9\-_]{1,255}$, but was bad cluster name !! not allowed'.
