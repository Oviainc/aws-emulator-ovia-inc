Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels secretsmanager__ssm.parameter-store-reference-is-refused-without-the-secretsmanager-grant
Issue-Key: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels|_poll#16|AssertionError|raise AssertionError(
Issue-Key: secretsmanager__ssm.parameter-store-reference-is-refused-without-the-secretsmanager-grant|_poll#13|AssertionError|raise AssertionError(
Issue-Commit: c1cb63d17f24ad84276bf0784afa9b177e8d858c

# secretsmanager__ssm: GetParameter answers ParameterNotFound for /aws/reference/secretsmanager/<secret>, so the Parameter Store front door to Secrets Manager never resolves

> _Note: this issue was identified against the code base at commit `c1cb63d17f24ad84276bf0784afa9b177e8d858c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: SSM GetParameter never resolves the virtual /aws/reference/secretsmanager/<secret-name> path for a secret that exists, so neither the happy path nor the authorization path over that front door can be exercised. Failing checks: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels and secretsmanager__ssm.parameter-store-reference-is-refused-without-the-secretsmanager-grant.

AWS: Parameter Store serves Secrets Manager secrets through the reserved, virtual path /aws/reference/secretsmanager/<secret-name> (AWS docs: 'Referencing AWS Secrets Manager secrets from Parameter Store parameters'). GetParameter on that name, for a secret that exists and a caller holding both ssm:GetParameter and secretsmanager:GetSecretValue, returns 200 with a Parameter whose Name is the reference path exactly as asked, Type='SecureString', Value the AWSCURRENT SecretString byte-for-byte, and SourceResult set to the raw Secrets Manager response JSON — whose keys are the source service's lower-camelCase form, so the resolved version is readable as versionId (Smithy, ssm#Parameter: members Name, Type (ParameterType enum including SecureString), Value, Selector, SourceResult 'the raw result or response from the source'). A ':<STAGE>' suffix selects by staging label (':AWSPREVIOUS' serves the demoted version's bytes and echoes Selector=':AWSPREVIOUS'); a staging label that does not exist does NOT fall back to AWSCURRENT; the path is not enumerable through GetParametersByPath. When the caller lacks secretsmanager:GetSecretValue the read is refused — the recording shows SSM wrapping the failed downstream call in its own ValidationException rather than AccessDeniedException or ParameterNotFound. ParameterNotFound (Smithy, GetParameter errors: ParameterNotFound, 400 with awsQueryError httpResponseCode 404) is AWS's answer only when no such parameter or secret exists.

Emulator: GetParameter on /aws/reference/secretsmanager/<name> raises ParameterNotFound for a secret the same run just created (CreateSecret returned its ARN, and Secrets Manager itself serves GetSecretValue for AWSCURRENT and AWSPREVIOUS). Both checks poll for 90s and the last observation stays ('ParameterNotFound', None), so the reference path is treated as an ordinary, non-existent parameter name: the virtual prefix is not special-cased at all. Because the positive read never works, the authorization check cannot distinguish a correct refusal from this defect, and the dependent behaviours — Type='SecureString', the echoed Name, SourceResult/versionId, the ':<STAGE>' selector and its no-fall-back rule, the WithDecryption=False behaviour and the non-enumerability through GetParametersByPath — are all unreachable.

Why: a client that reads secrets through Parameter Store (the documented way to let SSM-only consumers, EC2/ECS agents, CFN and application code reach a secret without a Secrets Manager client) gets a hard ParameterNotFound 400/404 for a secret that exists. Code cannot tell 'no such secret' from 'this emulator does not implement the front door', and a caller that correctly lacks secretsmanager:GetSecretValue sees the wrong error class (ParameterNotFound instead of ValidationException), which turns an authorization problem into a 'missing resource' one.

Fix: the ssm GetParameter handler and its name-resolution/validation step — recognise the reserved /aws/reference/secretsmanager/ prefix, resolve the remainder (plus an optional ':<STAGE>' selector) against Secrets Manager, build the Parameter projection (Name echoed, Type SecureString, Value, Selector, SourceResult carrying the raw source JSON), relay the downstream authorization failure as ValidationException, and keep the virtual path out of GetParametersByPath results. Open issues that name this same front door for other checks and do not carry these two replay keys: 20261024T190718Z-secretsmanager-ssm-ssm-getparameter-does-not-resolve-aws-reference-secretsmanager.md, 20261025T031105Z-secretsmanager-ssm-ssm-aws-reference-secretsmanager-never-resolves-for-the.md, and (CloudFormation heading) 20261107T184737Z-cloudformation-secretsmanager-ssm-the-ssm-aws-reference-secretsmanager-secret-reference-is.md.

Traces: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels; secretsmanager__ssm.parameter-store-reference-is-refused-without-the-secretsmanager-grant.

## Evidence
Expected (AWS), for a secret created in the same run and two versions staged so v2=AWSCURRENT, v1=AWSPREVIOUS:
- GetParameter(Name='/aws/reference/secretsmanager/<secret>', WithDecryption=True) => 200, Parameter.Name='/aws/reference/secretsmanager/<secret>', Type='SecureString', Value == GetSecretValue(VersionStage='AWSCURRENT').SecretString, SourceResult parses as JSON carrying versionId == the AWSCURRENT version id.
- GetParameter(Name='<ref>:AWSPREVIOUS') => the demoted version's bytes with Selector=':AWSPREVIOUS'; GetParameter(Name='<ref>:SIM2REALNOSUCHSTAGE') must NOT resolve; GetParametersByPath(Path='/aws/reference/secretsmanager', Recursive=True) must not enumerate the reference.
- For a principal holding ssm:GetParameter but not secretsmanager:GetSecretValue: the read is refused with ValidationException (not AccessDeniedException, not ParameterNotFound), and succeeds once the grant is added.

Actual (emulator): every GetParameter on the reference path raises ParameterNotFound (HTTP 400, awsQueryError 404) with no Parameter, for 90s of polling. Replay details: 'waited 90.0s for the Parameter Store reference "/aws/reference/secretsmanager/<name>" to become resolvable ...; last observation was ("ParameterNotFound", None)' and 'waited 90.0s for the reference "/aws/reference/secretsmanager/<name>" to resolve for the harness's own credentials ...; last observation was ("ParameterNotFound", None)'. The secret names in those messages are per-run generated values; the fixed part is the reserved prefix and the ParameterNotFound outcome.

## Steps to Reproduce
1. secretsmanager:CreateSecret(Name=<name>, SecretString=<json>) and capture the ARN and version id v1.
2. PutSecretValue(SecretId=<arn>, SecretString=<different json>) => v2; confirm with DescribeSecret/GetSecretValue that v2 is AWSCURRENT and v1 is AWSPREVIOUS (the emulator does serve these).
3. ssm:GetParameter(Name='/aws/reference/secretsmanager/<name>', WithDecryption=True): AWS returns the SecureString projection described above; the emulator raises ParameterNotFound (retrying for 90s does not change it).
4. ssm:GetParameter(Name='/aws/reference/secretsmanager/<name>:AWSPREVIOUS') and ':SIM2REALNOSUCHSTAGE', and GetParametersByPath(Path='/aws/reference/secretsmanager', Recursive=True) — all unreachable while step 3 fails.
5. Assume a role with ssm:GetParameter but without secretsmanager:GetSecretValue and repeat step 3: AWS refuses with ValidationException; the emulator returns ParameterNotFound, the same answer it gives the fully-authorized caller.
