Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores
Issue-Key: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# The Lambda environment-variable KMS cycle (disable the CMK blocks a cold invoke, enable restores it) never converges and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the disable/enable cycle around a function's KMSKeyArn-encrypted environment variables produces no observable change in invocation outcome.
AWS: Lambda decrypts a function's encrypted environment variables while building the execution environment. With the function's KMSKeyArn CMK DISABLED, a cold invoke fails with a KMS-attributed function error rather than running the handler; after EnableKey, the same cold invoke succeeds and the handler echoes the decrypted variable. The check polls for each transition because the state change is not instantaneous.
Emulator: neither transition is ever observed — the check ends as 'Check timed out after 60.0s'. The two sibling checks of this same service are already attributed to the open issue '20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md' (Invoke answers NotImplemented because there is no in-process runtime for the deployment package), which is the likely shared root cause; that issue does not carry this check's replay key, and this check additionally requires that the CMK's state gate the environment decryption, not merely that Invoke run code.
Why: a client cannot verify that a revoked/disabled CMK stops a function from starting — the production failure where disabling a key bricks a function is invisible, and the recovery path after EnableKey is equally untestable.
Fix: the Invoke path's execution-environment bootstrap must decrypt the function's environment through KMS and surface the KMS key-state failure as an invocation failure (and recover after EnableKey).

## Evidence
kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores: 'Error: Check timed out after 60.0s'. The check creates a CMK, a role, and a function whose handler echoes os.environ['SECRET'] with KMSKeyArn set, then polls (interval 3 s) for the cold-invoke outcome to change across DisableKey and EnableKey; no transition is ever observed. Expected: a KMS-attributed invocation failure while the key is Disabled, and a successful invoke returning the decrypted value after EnableKey.

## Steps to Reproduce
1. kms:CreateKey. 2. iam:CreateRole for Lambda. 3. lambda:CreateFunction(python3.12) with Environment={'Variables': {'SECRET': ...}} and KMSKeyArn=<key>. 4. kms:DisableKey. 5. lambda:Invoke (cold) — must fail with a KMS-attributed function error. 6. kms:EnableKey. 7. lambda:Invoke (cold) — must return {'secret': <value>}. In the emulator neither step 5 nor step 7 produces the expected outcome and the check runs to its cap.
