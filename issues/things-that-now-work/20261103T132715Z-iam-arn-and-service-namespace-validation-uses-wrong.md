Issue-Source: trace-sweep-divergence
Issue-Trace: iam-bap-simulate-with-bucket-policy
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-bap-simulate-with-bucket-policy iam-ebecs-events-role-run-task-pass-role-simulation iam-ecsm-simulate-instance-role-secret-read-chain iam-execute-api-simulation-request-mistakes iam-bap-identity-side-grants-via-group-and-managed-policy iam-ekscc-traj-attach-cluster-policy-mistakes iam-gtr-detach-lf-data-admin-from-glue-role
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|SimulatePrincipalPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListEntitiesForPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|ListEntitiesForPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|AttachRolePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|AttachRolePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DetachRolePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|DetachRolePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|SimulatePrincipalPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|SimulatePrincipalPolicy|4xx->2xx|status
Issue-Key: iam|ListPoliciesGrantingServiceAccess|4xx->2xx|$.ErrorResponse
Issue-Key: iam|ListPoliciesGrantingServiceAccess|4xx->2xx|status
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM ARN and service-namespace validation uses wrong codes, wrong wording, or is skipped entirely

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
IAM operations that take an ARN or a service namespace reject bad input with the wrong error code and an invented explanatory message, and two such checks are missing altogether (the emulator answers 200 where AWS refuses). 6 causes.

Cause 1 of 6: SimulatePrincipalPolicy's message for a PolicySourceArn that is not an ARN is reworded.
AWS: Smithy SimulatePrincipalPolicy.PolicySourceArn is @required, target arnType, @length 20..2048; the modeled refusal is InvalidInputException (awsQueryError code InvalidInput, HTTP 400). Recording: 400/InvalidInput with message "Invalid ARN provided in the request: <value>" — fixed text "Invalid ARN provided in the request: " followed by the caller-supplied value.
Emulator: 400/InvalidInput with "ARN <value> is not valid: PolicySourceArn must be the ARN of an IAM user, group or role" — a different sentence with an invented trailing clause.
Why: AWS's message is the contract a caller logs or matches; the emulator's form fails any such match and implies a narrower rule than AWS states.
Fix: the SimulatePrincipalPolicy input validation message.
Traces: iam-bap-simulate-with-bucket-policy#task#7.

Cause 2 of 6: ListEntitiesForPolicy refuses an unparseable PolicyArn with the wrong code and message.
AWS: Smithy ListEntitiesForPolicy.PolicyArn is @required, target arnType. Recording: 400 with Error.Code ValidationError and the fixed message "Invalid ARN:  Could not be parsed!" (note the double space; no part of it is variable — the offending value is not echoed).
Emulator: 400 with Error.Code InvalidInput and "ARN AmazonS3ReadOnlyAccess is not valid: PolicyArn must be an IAM managed policy ARN".
Why: the code is what a client branches on, and the emulator both changes it and echoes the request value into a sentence AWS never emits.
Fix: the shared ARN-parse validation used by the policy-ARN inputs (code classification + message).
Traces: iam-bap-identity-side-grants-via-group-and-managed-policy#task#15.

Cause 3 of 6: AttachRolePolicy refuses a bare policy name with the wrong code and an extra clause.
AWS: Smithy AttachRolePolicy.PolicyArn is @required, target arnType; modeled errors include InvalidInputException/InvalidInput and PolicyNotAttachableException/PolicyNotAttachable. Recording: 400 with Error.Code ValidationError and message "ARN <value> is not valid." — fixed text "ARN " + the supplied value + " is not valid." and nothing else.
Emulator: 400 with Error.Code InvalidInput and "ARN <value> is not valid: PolicyArn must be an IAM managed policy ARN".
Why: as Cause 2; the trailing clause also mis-states the rule (a customer-managed policy ARN is equally acceptable to AWS).
Fix: same component.
Traces: iam-ekscc-traj-attach-cluster-policy-mistakes#task#3.

Cause 4 of 6: DetachRolePolicy has the same defect as Cause 2.
AWS: recording shows 400 with Error.Code ValidationError and "Invalid ARN:  Could not be parsed!".
Emulator: 400/InvalidInput with "ARN AWSLakeFormationDataAdmin is not valid: PolicyArn must be an IAM managed policy ARN".
Why: as Cause 2.
Fix: same component.
Traces: iam-gtr-detach-lf-data-admin-from-glue-role#task#8.

Cause 5 of 6: a PolicySourceArn of "*" is accepted and simulated instead of being refused.
AWS: recording shows 400 with Error.Type Sender, Error.Code InvalidInput and the message "Invalid Entity Arn: * does not clearly define entity type and name." — fixed text, with the offending entity ARN as its variable part. (Modeled error: InvalidInputException, awsQueryError InvalidInput, HTTP 400.)
Emulator: 200 with a full simulation result.
Why: a caller that passes a wildcard where an entity ARN is required gets a fabricated allow/deny verdict instead of being told the request is invalid — the worst case for a tool that uses the simulator to pre-flight permissions.
Fix: SimulatePrincipalPolicy input validation (entity-ARN shape check before evaluation).
Traces: iam-bap-simulate-with-bucket-policy#task#8.

Cause 6 of 6: ListPoliciesGrantingServiceAccess accepts a service namespace that is not a real AWS service namespace.
AWS: Smithy ListPoliciesGrantingServiceAccess.ServiceNamespaces is @required (serviceNamespaceListType, length 1..200) and serviceNamespaceType allows @pattern ^[\w-]*$, so "amazon-s3" passes the syntactic constraint — AWS additionally checks it against the known service namespaces. Recording: 400 with Error.Type Sender, Error.Code InvalidInput and message "Invalid service namespace: [<namespace>]" — fixed text with the rejected namespace(s) in square brackets.
Emulator: 200 with an empty/synthesised result.
Why: a caller that mistypes a namespace (amazon-s3 instead of s3) is told the principal has no policies granting access to it rather than that the namespace is wrong, and draws the opposite conclusion about its permissions.
Fix: ListPoliciesGrantingServiceAccess validation against the service-namespace catalog.
Traces: iam-bap-simulate-with-bucket-policy#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1.

Cause 1 — iam-bap-simulate-with-bucket-policy#task#7, SimulatePrincipalPolicy, 400 vs 400 (cluster of 7, also #task#9, iam-ebecs-events-role-run-task-pass-role-simulation#task#5/#task#14, iam-ecsm-simulate-instance-role-secret-read-chain#task#5/#task#6, iam-execute-api-simulation-request-mistakes#task#4): $.ErrorResponse.Error.Message expected 'Invalid ARN provided in the request: sim2real-probe-bap-dev-7f9d1fb2', got 'ARN sim2real-probe-bap-dev-7f9d1fb2 is not valid: PolicySourceArn must be the ARN of an IAM user, group or role'.

Cause 2 — iam-bap-identity-side-grants-via-group-and-managed-policy#task#15, ListEntitiesForPolicy, 400 vs 400: Error.Code expected 'ValidationError', got 'InvalidInput'; Error.Message expected 'Invalid ARN:  Could not be parsed!', got 'ARN AmazonS3ReadOnlyAccess is not valid: PolicyArn must be an IAM managed policy ARN'.

Cause 3 — iam-ekscc-traj-attach-cluster-policy-mistakes#task#3, AttachRolePolicy, 400 vs 400: Error.Code expected 'ValidationError', got 'InvalidInput'; Error.Message expected 'ARN AmazonEKSClusterPolicy is not valid.', got 'ARN AmazonEKSClusterPolicy is not valid: PolicyArn must be an IAM managed policy ARN'.

Cause 4 — iam-gtr-detach-lf-data-admin-from-glue-role#task#8, DetachRolePolicy, 400 vs 400: Error.Code expected 'ValidationError', got 'InvalidInput'; Error.Message expected 'Invalid ARN:  Could not be parsed!', got 'ARN AWSLakeFormationDataAdmin is not valid: PolicyArn must be an IAM managed policy ARN'.

Cause 5 — iam-bap-simulate-with-bucket-policy#task#8, SimulatePrincipalPolicy (also iam-execute-api-simulation-request-mistakes#task#5): status expected 400, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'InvalidInput', 'Message': 'Invalid Entity Arn: * does not clearly define entity type and name.'}, ...}, got None.

Cause 6 — iam-bap-simulate-with-bucket-policy#task#11, ListPoliciesGrantingServiceAccess: status expected 400, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'InvalidInput', 'Message': 'Invalid service namespace: [amazon-s3]'}, ...}, got None.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay iam-bap-simulate-with-bucket-policy and compare steps task#7, task#8 and task#11 against the recording.
3. Replay iam-bap-identity-side-grants-via-group-and-managed-policy#task#15, iam-ekscc-traj-attach-cluster-policy-mistakes#task#3 and iam-gtr-detach-lf-data-admin-from-glue-role#task#8.
4. Diff the HTTP status and $.ErrorResponse.Error.{Type,Code,Message} for each step.
