Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped|run#16|AssertionError|assert (code, message) == ("AccessDeniedException", expected), (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Secrets Manager GetSecretValue reports ResourceNotFoundException for a missing secret regardless of the caller's policy scope, instead of evaluating authorization first

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Cause 1 of 1: GetSecretValue does not evaluate authorization before existence, so a principal that is NOT authorized for the requested resource learns that the secret does not exist.
AWS: authorization is evaluated BEFORE existence. Smithy (secretsmanager, com-amazonaws-secretsmanager-secretsmanager): GetSecretValue models ResourceNotFoundException, InvalidParameterException, InvalidRequestException, DecryptionFailure and InternalServiceError - AccessDeniedException is NOT among them because the authorization refusal is produced by the IAM layer ahead of the operation handler. Recorded behavior: for a secret NAME that does not exist, a role granted secretsmanager:GetSecretValue on "*" receives ResourceNotFoundException, while a role granted it only on ONE specific secret ARN receives AccessDeniedException - the standard IAM message "User: <assumed-role arn> is not authorized to perform: secretsmanager:GetSecretValue on resource: <name> because no identity-based policy allows the secretsmanager:GetSecretValue action". The same scoped role still reads its own secret successfully, so the rule is "authorize, then look up", not "always deny".
Emulator: the scoped role also receives ResourceNotFoundException "Secrets Manager can't find the specified secret." - the emulator looks the secret up first and never consults the caller's policy for an out-of-scope resource.
Why: this is an information-disclosure divergence, not a wording one. AWS deliberately refuses out-of-scope reads with AccessDenied so an unauthorized caller cannot use GetSecretValue as an existence oracle to enumerate secret names in the account; the emulator answers "no such secret" to a principal that was never allowed to ask, which both leaks the negative answer and inverts the two error codes. Anything that asserts least-privilege behaviour - a policy test, a boundary test, a security control that expects AccessDeniedException from a narrowly scoped role - gets the wrong code and silently passes or silently fails.
Fix: the authorization gate ahead of the GetSecretValue handler in services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/get_secret_value.rs - resolve the caller's identity-based policy against the requested resource and return the IAM AccessDeniedException refusal BEFORE the secret store is consulted; only an authorized caller may receive ResourceNotFoundException.
Traces: ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped.

## Evidence
Replay output for ec2__iam__kms__secretsmanager.ecsm-missing-secret-is-not-found-with-wildcard-grant-but-denied-when-scoped:
  AssertionError: with a grant scoped to another ARN a missing name is AccessDeniedException 'User: arn:aws:sts::000000000000:assumed-role/sim2real-ecsm-narrow-0ca777e62afa/sim2real-ecsm is not authorized to perform: secretsmanager:GetSecretValue on resource: sim2real-ecsm-missing-8bf3ca555233 because no identity-based policy allows the secretsmanager:GetSecretValue action', got 'ResourceNotFoundException': "Secrets Manager can't find the specified secret."
Expected error code: AccessDeniedException. Observed error code: ResourceNotFoundException. The role name, role session name and secret name above vary per run and are examples; the requirement is the error CODE for an unauthorized caller, and the IAM message rule (principal ARN + action + resource) which is built from the request. The check also exercises the wildcard-grant role (which must get ResourceNotFoundException) and the scoped role reading its OWN secret (which must succeed), so a target cannot pass by always denying.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Create two IAM roles: role A granted secretsmanager:GetSecretValue on Resource "*", role B granted it only on one specific existing secret ARN.
3. As role A, call GetSecretValue on a secret name that does not exist -> expect ResourceNotFoundException.
4. As role B, call GetSecretValue on that same nonexistent name -> expect AccessDeniedException (authorization before existence); observe ResourceNotFoundException.
5. As role B, call GetSecretValue on its own in-scope secret -> expect success (positive control).
