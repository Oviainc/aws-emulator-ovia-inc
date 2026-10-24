Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels
Issue-Key: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# SSM GetParameter does not resolve /aws/reference/secretsmanager or its :<STAGE> staging-label selector

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the /aws/reference/secretsmanager read path is not served ===
AWS: ssm GetParameter on /aws/reference/secretsmanager/<name> returns the secret's AWSCURRENT SecretString as Parameter.Value with Type=SecureString and Name echoed as the reference path; appending ':<STAGE>' selects by Secrets Manager staging label (so ':AWSPREVIOUS' serves the demoted version's bytes, not a parameter version); and the reserved path is not enumerable through GetParametersByPath. Recorded/documented behaviour of this cross-service path - the check is marked DRAFT (unblessed, no prior replay), so its oracle is the documented reference path rather than a recorded envelope; the Secrets Manager side (two versions, AWSCURRENT on v2 and AWSPREVIOUS on v1) is confirmed in-run from DescribeSecret/GetSecretValue.
Emulator: GetParameter on the reference never resolves. The check polls it for up to 90 s after establishing the two staging labels and runs out at the suite's 60 s cap, so no stage selection or enumeration assertion is ever reached.
Why: applications read secrets through Parameter Store precisely to avoid the Secrets Manager API; the whole path is unavailable, and the tempting wrong implementation (treat the reference as an ordinary parameter, so ':AWSPREVIOUS' silently serves AWSCURRENT) cannot be distinguished from the correct one by anything that currently works.
Fix: the SSM GetParameter handler's reserved-path resolution and selector parsing (name[:stage] against the secret's staging labels, Type=SecureString, excluded from GetParametersByPath).
Traces: secretsmanager__ssm.parameter-store-reference-serves-the-secret-and-honours-its-staging-labels.

## Evidence
'Error: Check timed out after 60.0s'. The check creates a secret, PutSecretValue mints v2, polls DescribeSecret until VersionIdsToStages shows v1 no longer AWSCURRENT, then polls _get_parameter(ssm, '/aws/reference/secretsmanager/<name>', decrypt=True) until it answers 'OK' (timeout_s=90.0, interval_s=3.0); the cap is hit in that poll.

## Steps to Reproduce
secretsmanager CreateSecret(SecretString=<json>); PutSecretValue with a new value so v2 is AWSCURRENT and v1 AWSPREVIOUS; ssm GetParameter(Name='/aws/reference/secretsmanager/<name>', WithDecryption=True) - never resolves; then GetParameter on '<path>:AWSPREVIOUS' (must serve v1's bytes) and GetParametersByPath on '/aws/reference/secretsmanager' (must not enumerate it).
