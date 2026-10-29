Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-missing-invoke-permission-negative dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-wrong-function-arn-negative dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-invocation-positive dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-writes-dynamodb-audit dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-decrypts-object-with-secret-writes-dynamodb dynamodb__lambda__s3__secretsmanager.cli-s3-upload-lambda-reads-config-secret-writes-dynamodb dynamodb__lambda__s3__secretsmanager.deploy-cfn-lambda-role-trusts-lambda-service dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative dynamodb__lambda__s3__secretsmanager.deploy-sam-lambda-secret-s3-dynamodb-full-stack
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-missing-invoke-permission-negative|run#9|AssertionError|assert rejected, "RotateSecret succeeded despite the Lambda lacking invoke permission"
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-wrong-function-arn-negative|run#9|AssertionError|assert rejected, "RotateSecret succeeded despite RotationLambdaARN pointing to a nonexistent function"
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-invocation-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-writes-dynamodb-audit|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.s3-notification-lambda-decrypts-object-with-secret-writes-dynamodb|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.cli-s3-upload-lambda-reads-config-secret-writes-dynamodb|run#29|Timeout|time.sleep(3)
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-lambda-role-trusts-lambda-service|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-sam-lambda-secret-s3-dynamodb-full-stack|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# Secrets Manager RotateSecret is accepted for an un-invokable or non-existent rotation Lambda and never rotates; SAM stack rolls back (4 causes, 10 checks)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Four net-new root causes in the dynamodb__lambda__s3__secretsmanager group. Each needs its own fix.

Cause 1 of 4: RotateSecret is accepted when the rotation Lambda cannot be invoked by Secrets Manager.
AWS: RotateSecret validates the rotation function first — without a `lambda:InvokeFunction` resource-policy statement allowing `secretsmanager.amazonaws.com`, it is refused with a client error (HTTP 400) stating that Secrets Manager cannot invoke the specified Lambda.
Emulator: the call SUCCEEDS.
Why: the caller believes rotation is configured when nothing can ever rotate the secret; the negative path is untested.
Fix: RotateSecret validation (authorize the rotation function's resource policy before accepting).
Traces: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-missing-invoke-permission-negative

Cause 2 of 4: RotateSecret is accepted when RotationLambdaARN names no function.
AWS: a RotationLambdaARN that does not resolve is refused with a client error (HTTP 400) naming the function that could not be found.
Emulator: the call SUCCEEDS.
Why: a secret is marked as rotating against a function that does not exist, so rotation silently never happens.
Fix: RotateSecret validation (resolve RotationLambdaARN in lambda state).
Traces: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-wrong-function-arn-negative

Cause 3 of 4: nothing downstream of a rotation or an S3 notification ever runs, so the checks never finish.
AWS: RotateSecret invokes the rotation function through the createSecret/setSecret/testSecret/finishSecret steps (the audit write, the S3 backup and the AWSPENDING->AWSCURRENT stage move all become observable), and an S3 object event invokes the configured function.
Emulator: no invocation and no state change; each check polls for the effect and ends with `Check timed out after 60.0s`.
Why: rotation and notification-driven work are inert — a caller cannot verify a rotation happened, nor that a role/permission negative blocked it.
Fix: the secretsmanager rotation driver (invoke the rotation function and advance the version stages) and the s3 notification dispatcher it shares with Lambda. (The open issue 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md carries one sibling check; it does not carry these replay keys.)
Traces: dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-invocation-positive, .secretsmanager-rotation-lambda-s3-backup-positive, .secretsmanager-rotation-lambda-writes-dynamodb-audit, .s3-notification-lambda-decrypts-object-with-secret-writes-dynamodb, .cli-s3-upload-lambda-reads-config-secret-writes-dynamodb, .deploy-cfn-lambda-role-trusts-lambda-service, .deploy-cfn-missing-secretsmanager-policy-negative

Cause 4 of 4: the SAM stack combining Lambda, a secret, S3 and DynamoDB rolls back.
AWS: the stack reaches `CREATE_COMPLETE` with all four resources created.
Emulator: the stack ends in `ROLLBACK_COMPLETE`.
Why: a valid SAM template cannot be deployed at all.
Fix: the cloudformation/SAM provisioner for the resource that fails (read the stack events).
Traces: dynamodb__lambda__s3__secretsmanager.deploy-sam-lambda-secret-s3-dynamodb-full-stack

## Evidence
- Cause 1: AssertionError: RotateSecret succeeded despite the Lambda lacking invoke permission.
- Cause 2: AssertionError: RotateSecret succeeded despite RotationLambdaARN pointing to a nonexistent function.
- Cause 3 (seven checks): Error: Check timed out after 60.0s.
- Cause 4: AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE.

## Steps to Reproduce
Boot the emulator and replay the 10 checks listed in trace_ids.
