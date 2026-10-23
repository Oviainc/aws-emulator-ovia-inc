Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag
Issue-Key: cloudformation__cognito-identity__sts|CreateStack|ValidationError
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# cloudformation: AWS::Cognito::IdentityPoolPrincipalTag is rejected as an unrecognized resource type, so CreateStack fails before any principal-tag scenario runs

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause (Cause 1 of 1).

AWS: AWS::Cognito::IdentityPoolPrincipalTag is a supported CloudFormation resource type. CreateStack accepts a template declaring it and provisions the identity pool's principal-tag attribute mapping — the same state the cognito-identity model exposes through SetPrincipalTagAttributeMap (smithy_query: SetPrincipalTagAttributeMap(IdentityPoolId, IdentityProviderName, PrincipalTags @length{0,50}, UseDefaults)). The deployed mapping is what makes Cognito-vended session tags usable in an IAM policy condition (aws:PrincipalTag/...) to gate S3 reads.
Emulator: CreateStack is refused outright with HTTP 400, error code ValidationError, message of the form "Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]".
Why: a template that deploys in production is rejected at submission here, so no cross-service principal-tag authorization flow (Cognito -> STS session tags -> S3 policy condition) can be deployed or tested; the failure also masks everything downstream of CreateStack.
Fix: register AWS::Cognito::IdentityPoolPrincipalTag in the CloudFormation resource-type registry and give it a provider that writes the principal-tag attribute map into cognito-identity state (and resolves Ref/GetAtt from it). The matching cognito-identity-side causes are filed separately under service cognito-identity.
Traces: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag.

Note: the check id carries the composite service token 'cloudformation__cognito-identity__sts'; the defect itself is in the CloudFormation resource-type registry.

## Evidence
Replay output for cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]
Expected: CreateStack accepted, stack reaching CREATE_COMPLETE, and the principal-tag mapping readable through cognito-identity GetPrincipalTagAttributeMap.
The same refusal is observed from the cognito-identity-token checks (filed separately), confirming it is the resource type, not the surrounding template.

## Steps to Reproduce
Call cloudformation CreateStack with a template whose Resources include an AWS::Cognito::IdentityPoolPrincipalTag (IdentityPoolId, IdentityProviderName, PrincipalTags) alongside an AWS::Cognito::IdentityPool. Expect the stack to be created; observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]" at CreateStack.
