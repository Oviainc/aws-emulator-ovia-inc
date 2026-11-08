Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative|_poll_until#12|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative|run#25|AssertionError|assert marker["message"] == expected_message, (
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive|run#20|AssertionError|assert final_version != initial_version, (
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# dynamodb__lambda__s3__secretsmanager: 3 net-new causes — a stack-created secret is unreadable (ResourceNotFoundException) after the grant, the AccessDenied message names the resolved ARN instead of the requested SecretId, and RotateSecret never advances AWSCURRENT

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Three independent root causes in this scenario group; each is listed with its own failing check ids and all three must be closed before this issue is submitted.

Cause 1 of 3: a secret created by a CloudFormation stack cannot be read by the stack's own function even once the IAM grant is in place — GetSecretValue answers ResourceNotFoundException.
AWS: Smithy: secretsmanager GetSecretValue takes SecretId (a secret name or ARN) and models ResourceNotFoundException (400) only for a secret that does not exist. An AWS::SecretsManager::Secret created by a stack exists, and the ARN the stack passes to the function (through an environment variable) resolves, so after the conditional policy statement granting secretsmanager:GetSecretValue is applied the handler's read SUCCEEDS and returns the SecretString.
Emulator: with the grant in place the in-function read keeps failing for the whole 90 s window with error_code `ResourceNotFoundException` — the stack-provisioned secret is not addressable by the identifier the stack handed out.
Why: the permission-boundary story is inverted: a client that fixes the missing policy still sees a failure, and 'not found' points the operator at the wrong cause (a mis-wired ARN) when the resource was created by the stack moments earlier.
Fix: the CloudFormation provisioner for AWS::SecretsManager::Secret together with secretsmanager's secret lookup — the created secret must exist in Secrets Manager state under the ARN/name the stack's Ref/GetAtt publishes, so GetSecretValue on that identifier resolves.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative. Related open issue that does not carry this check's replay key: 20261103T171935Z-secretsmanager-secrets-manager-describesecret-refuses-a-secret-created-by.md.

Cause 2 of 3: the GetSecretValue AccessDenied message rewrites the resource as the resolved secret ARN instead of echoing the SecretId the caller supplied.
AWS: the authorization refusal is AccessDeniedException with the message `User: <caller role-session ARN> is not authorized to perform: secretsmanager:GetSecretValue on resource: <the SecretId exactly as the request supplied it> because no identity-based policy allows the secretsmanager:GetSecretValue action` — the fixed text is `User: … is not authorized to perform: … on resource: … because no identity-based policy allows the … action`, and the variable parts are the caller's assumed-role ARN, the action, and the resource string TAKEN FROM THE REQUEST (here the plain secret name passed in the handler's SECRET_ID environment variable).
Emulator: the same denial is worded with the full resolved ARN including the six-character Secrets Manager suffix: `… on resource: arn:aws:secretsmanager:<region>:<account>:secret:<name>-<suffix> because no identity-based policy allows …`.
Why: the denial message is the only machine-readable statement of WHICH resource was refused; clients and policy-debugging tooling match the resource field against what they asked for. Substituting a server-side ARN (with a suffix the caller never saw) breaks that correlation and makes the message non-reproducible.
Fix: the secretsmanager authorization-denial message builder — the resource clause must echo the request's SecretId verbatim rather than the resolved ARN.
Traces: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-missing-secretsmanager-permission-negative. Related open issue that does not carry this check's replay key: 20261106T151148Z-dynamodb-lambda-s3-secretsmanager-a-missing-secretsmanager-policy.md.

Cause 3 of 3: RotateSecret completes without moving AWSCURRENT to a new version.
AWS: Smithy: secretsmanager RotateSecret returns ARN, Name and VersionId — the version id of the new secret version created by the rotation. AWS drives the configured rotation function through createSecret/setSecret/testSecret/finishSecret and finishSecret moves the AWSCURRENT staging label onto the new version, so a DescribeSecret/GetSecretValue after rotation reports a DIFFERENT AWSCURRENT version id than before.
Emulator: after RotateSecret the AWSCURRENT version id is unchanged from the pre-rotation value, so no rotation actually happened (the rotation function's side effects, e.g. the S3 backup object the handler writes, are therefore also absent).
Why: rotation appearing to succeed while the credential never changes is the worst possible outcome for a secret-rotation workflow — compliance reports a rotated secret, consumers keep the old value, and nothing signals the failure.
Fix: the secretsmanager rotation driver — RotateSecret must invoke the rotation Lambda for each step and promote AWSPENDING to AWSCURRENT at finishSecret, minting and returning the new VersionId.
Traces: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive. Related open issues that do not carry this check's current replay key: 20261107T173512Z-dynamodb-lambda-s3-secretsmanager-3-net-new-causes.md (its Cause 3 covers the same check failing earlier, at the missing backup object) and 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md.

## Evidence
Cause 1 — expected: once the stack applies the conditional secretsmanager:GetSecretValue statement, the handler's read returns the secret. Actual: `AssertionError: lambda invoke retrieves the secret once the policy statement is granted not met within 90.0s. Last result: {'ok': False, 'error_code': 'ResourceNotFoundException'}`.

Cause 2 — expected message: `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret-name-as-requested> because no identity-based policy allows the secretsmanager:GetSecretValue action`. Actual: the same sentence with `on resource: arn:aws:secretsmanager:<region>:<account>:secret:<secret-name>-<6-char suffix>`. (Account, role, function and secret names are per-run; the divergence is the resource form: request-supplied SecretId vs resolved ARN.)

Cause 3 — expected: the AWSCURRENT version id after RotateSecret differs from the pre-rotation one. Actual: `AssertionError: rotation did not advance AWSCURRENT: still '<version-uuid>' after RotateSecret`.

## Steps to Reproduce
Cause 1: deploy a stack with an AWS::SecretsManager::Secret, a function whose role gets secretsmanager:GetSecretValue only under a Condition, and the secret ARN in the function's environment; flip the condition to true and invoke — observe ResourceNotFoundException instead of the value. Cause 2: invoke a handler whose role has no secretsmanager permission calling GetSecretValue(SecretId=<plain secret name>) and compare the denial message's resource clause with the SecretId sent. Cause 3: attach a rotation Lambda to a secret, call RotateSecret, then compare the AWSCURRENT version id before and after. Re-replay the three checks in trace_ids.
