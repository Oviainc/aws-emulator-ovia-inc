Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s
Issue-Key: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s|_poll#12|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# The SSM /aws/reference/secretsmanager/<secret> reference is never resolvable: GetParameter answers ParameterNotFound for a secret that exists

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter (/aws/reference/secretsmanager/<secret-name>)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: Parameter Store does not serve the Secrets Manager reference path — GetParameter on /aws/reference/secretsmanager/<secret-name> is refused with ParameterNotFound even while the secret exists and is readable through Secrets Manager.
Failing check ids for this cause: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s.

AWS: AWS documents Parameter Store's Secrets Manager integration: GetParameter with Name=/aws/reference/secretsmanager/<secret-name> (optionally WithDecryption=true) resolves the referenced secret and returns a Parameter whose Value is the secret string and whose Type is SecureString, for as long as the secret exists; once the secret is gone the same call is refused with ParameterNotFound. The recording shows exactly that ordering: the reference resolves to the stack-created secret's value before the stack is deleted, and only after DeleteStack removes the secret does GetParameter on the reference answer ParameterNotFound.
Emulator: the reference never resolves at all. The check waits 90 s for the pre-teardown observation and reports: waited 90.0s for the Parameter Store reference '/aws/reference/secretsmanager/<stack-name>-secret' to become resolvable before teardown; last observation was ('ParameterNotFound', ''). The error code is right for the post-deletion state but is returned for the live secret too, so the whole "resolvable, then 404 after the stack is deleted" transition is collapsed into a permanent refusal. The secret name in the path is derived from the stack name by the template's !Sub; the requirement is that the reference for any existing secret resolves.
Why: the documented way to read a secret through Parameter Store — used by anything that consumes secrets via an SSM parameter path, including CloudFormation dynamic references and SSM-backed configuration — never works. A consumer cannot tell "this secret does not exist" from "this emulator does not implement the reference path", so a deleted secret and a live one look identical.
Fix: SSM GetParameter's name resolution — recognise the /aws/reference/secretsmanager/<secret-name> prefix, resolve the named secret through Secrets Manager under the caller's identity, and return it as a SecureString Parameter (honouring WithDecryption), falling back to ParameterNotFound only when the secret genuinely does not exist.
Traces: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s.

## Evidence
Expected (AWS, documented + recorded): while the stack-created AWS::SecretsManager::Secret exists, GetParameter(Name="/aws/reference/secretsmanager/<stack>-secret") returns a Parameter with the secret's value (Type SecureString); after DeleteStack removes the secret, the same call is refused with ParameterNotFound.

Actual (emulator): AssertionError: waited 90.0s for the Parameter Store reference '/aws/reference/secretsmanager/<stack>-secret' to become resolvable before teardown; last observation was ('ParameterNotFound', '') — the reference is refused with ParameterNotFound for the whole wait, while the secret exists.

## Steps to Reproduce
1. CreateStack with an AWS::SecretsManager::Secret whose Name is !Sub "${AWS::StackName}-secret" and a GenerateSecretString block.
2. Wait for CREATE_COMPLETE.
3. Call SSM GetParameter with Name=/aws/reference/secretsmanager/<stack-name>-secret (WithDecryption=true).
4. Observe ParameterNotFound instead of the secret's value.
Or replay the check id in trace_ids.
