Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback
Issue-Key: cloudformation__cognito-idp|GetGroup|ResourceNotFoundException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cloudformation__cognito-idp: the physical id of an AWS::Cognito::UserPoolGroup is a stack-derived name, so GetGroup cannot find the group the stack created (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGroup

## Description
Cause 1 of 1 — a CloudFormation-created Cognito user-pool group is addressed by a stack-derived physical id that the Cognito service does not know, so the group cannot be read back and the check's rollback assertion is never reached.

AWS: the physical resource id of AWS::Cognito::UserPoolGroup is the group NAME the owning service holds (GroupNameType), so Ref of the resource is a value GetGroup(UserPoolId, GroupName) resolves; Smithy (cognito-idp) GetGroup models ResourceNotFoundException (404) only for a group that genuinely does not exist. Where the template sets an explicit GroupName that the service refuses, the resource fails and the stack reaches ROLLBACK_COMPLETE (the behavior this check exists to measure).

Emulator: Ref of the group resource is '<stack>-Group-<suffix>' ('sim2real-xsvc-coggrpneg-91f3ad7e01ac-Group-4A3F95E21186'), and GetGroup with it answers ResourceNotFoundException 'Group not found: <that id>' — the stack reports a resource whose id no Cognito operation can resolve. Because the lookup raises first, the check never gets to assert the rollback behavior for the explicit-name case.

Why: nothing a stack creates in Cognito can be addressed through the Cognito API, so group-based authorization deployed by a template cannot be verified, modified or torn down through the service that owns it; and the stack reports success for a resource that effectively does not exist.

Fix: the AWS::Cognito::UserPoolGroup resource provider in the CloudFormation provisioner — create the group through cognito-idp CreateGroup and store the group name the service holds as PhysicalResourceId. This is the same family as Cause 8 of open issue 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md (which covers AWS::Cognito::UserPool and AWS::StepFunctions::StateMachine only and carries none of this check's replay keys).

Traces: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback.

## Evidence
botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetGroup operation: Group not found: sim2real-xsvc-coggrpneg-91f3ad7e01ac-Group-4A3F95E21186
Expected: Ref of the AWS::Cognito::UserPoolGroup resource is the group name, and GetGroup(UserPoolId, GroupName=<that Ref>) returns the group the stack created (and, for the explicit-name negative case, the stack reaches ROLLBACK_COMPLETE instead).
Actual: the Ref is the stack-derived id '<stack>-Group-<suffix>' and GetGroup answers 404 ResourceNotFoundException. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a template declaring AWS::Cognito::UserPool and AWS::Cognito::UserPoolGroup (no explicit GroupName), wait for CREATE_COMPLETE.
3. Take Ref of the group resource (DescribeStackResources PhysicalResourceId) and call cognito-idp GetGroup(UserPoolId=<pool>, GroupName=<that value>).
4. Expect the group; observe ResourceNotFoundException 'Group not found: <stack>-Group-<suffix>'.
5. Re-replay cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback, which then proceeds to its ROLLBACK_COMPLETE assertion for a template-supplied explicit group name.
