Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 4a9463f06c8da54961be1caf9572b353149b691e

# dynamodb__lambda__s3__secretsmanager: RotateSecret stays refused for a CloudFormation-provisioned AWS::SecretsManager::RotationSchedule, so rotation never runs

> _Note: this issue was identified against the code base at commit `4a9463f06c8da54961be1caf9572b353149b691e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1 — RotateSecret is refused on a secret whose rotation schedule and rotation Lambda were created by a CloudFormation stack, so AWSCURRENT never moves.
AWS: Smithy: SecretsManager RotateSecret, input SecretId (required, length 1..2048), ClientRequestToken, RotationLambdaARN, RotationRules, RotateImmediately; output ARN, Name, VersionId; modelled errors InvalidParameterException 400, InvalidRequestException 400, ResourceNotFoundException 400, InternalServiceError 500. A secret that already carries a rotation configuration — here declared as AWS::SecretsManager::RotationSchedule with RotationLambdaARN pointing at the stack's AWS::Lambda::Function and RotationRules {AutomaticallyAfterDays: 30}, with an AWS::Lambda::Permission granting lambda:InvokeFunction to principal secretsmanager.amazonaws.com and DependsOn ordering that permission before the schedule — accepts RotateSecret with HTTP 200 and a body carrying ARN, Name and the new VersionId. Secrets Manager then drives the rotation function through its createSecret/setSecret/testSecret/finishSecret steps, so the AWSCURRENT stage moves to a new version id and the prior AWSCURRENT version is demoted to AWSPREVIOUS.
Emulator: every RotateSecret call against that secret is refused with a client error for the entire 60 s retry window the check allows, so the call is never accepted, AWSCURRENT never moves off the create-time version, and the rotation function is never invoked. The check's probe only records accepted/refused, so the replay output carries the refusal's status/code/message as 'Last result: False' rather than the code itself; what is established is that a call AWS answers 200 is refused indefinitely.
Hypothesis: the CloudFormation AWS::SecretsManager::RotationSchedule provider does not attach the rotation configuration (RotationEnabled / RotationLambdaARN / RotationRules) to the secret, so RotateSecret finds no rotation Lambda for the secret and refuses; this is not stated by the model or the replay output.
Why: a client that provisions secret rotation through CloudFormation — the documented way to do it — can never trigger or observe a rotation. Nothing ever reaches the rotation handler, GetSecretValue keeps returning the un-rotated string, and version staging (AWSCURRENT/AWSPREVIOUS/AWSPENDING) never advances, so any credential-refresh workflow built on it silently serves stale credentials forever.
Fix: the RotateSecret handler and the rotation state it reads in the secretsmanager service, together with the CloudFormation AWS::SecretsManager::RotationSchedule resource provider that must record the rotation configuration on the secret it names.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack.

## Evidence
check dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack [mismatch]:
  Expected: after the stack reaches CREATE_COMPLETE, secretsmanager.rotate_secret(SecretId=<secret arn>) is accepted (HTTP 200, body with ARN/Name/VersionId); then DescribeSecret shows AWSCURRENT on a NEW version id, the previous version carries AWSPREVIOUS, and GetSecretValue returns the rotation function's value (prefix 'rotated-').
  Actual (replay): AssertionError: rotate_secret call accepted (rotation lambda permission propagated) not met within 60.0s. Last result: False
  i.e. every RotateSecret attempt over 60 s raised a ClientError; the check never got past accepting the call, so AWSCURRENT stayed on the create-time version.

## Steps to Reproduce
1. CreateStack with: AWS::SecretsManager::Secret, an IAM role for rotation, an AWS::Lambda::Function (python3.12, inline ZipFile) implementing the four rotation steps, an AWS::Lambda::Permission {FunctionName: Ref RotationLambda, Action: lambda:InvokeFunction, Principal: secretsmanager.amazonaws.com}, and an AWS::SecretsManager::RotationSchedule {SecretId: Ref Secret, RotationLambdaARN: GetAtt RotationLambda.Arn, RotationRules: {AutomaticallyAfterDays: 30}} with DependsOn the permission. Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM. Wait for CREATE_COMPLETE and read the SecretArn output.
2. DescribeSecret and note the version id currently holding AWSCURRENT.
3. Call RotateSecret(SecretId=<secret arn>) repeatedly for up to 60 s.
   AWS: the call is accepted (200, ARN/Name/VersionId).
   Emulator: every attempt is refused with a ClientError.
4. (Blocked) DescribeSecret should show AWSCURRENT on a new version and the old version demoted to AWSPREVIOUS; GetSecretValue should return a value starting 'rotated-'.
