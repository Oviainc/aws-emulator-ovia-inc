Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-invocation-positive dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-writes-dynamodb-audit dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-missing-invoke-permission-negative dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-wrong-function-arn-negative lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol lambda__logs__secretsmanager.rotation-lambda-actually-replaces-the-current-secret-value-it-generated lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves lambda__rds__secretsmanager.cli-rds-secret-rotation-enable lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied
Issue-Key: dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages|_run_shell#17|AssertionError|raise AssertionError(
Issue-Key: secrets-manager|RotateSecret|NotImplemented
Issue-Key: lambda__rds__secretsmanager.cli-rds-secret-rotation-enable|_run_shell#26|AssertionError|raise AssertionError(
Issue-Key: lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied|run#11|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}: {message!r}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement secrets-manager RotateSecret: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
`RotateSecret` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `RotateSecret` call, so each one unblocks only when `RotateSecret` works: `dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages`, `dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-invocation-positive`, `dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-s3-backup-positive`, `dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-lambda-writes-dynamodb-audit`, `dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-missing-invoke-permission-negative`, `dynamodb__lambda__s3__secretsmanager.secretsmanager-rotation-wrong-function-arn-negative`, `lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential`, `lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves`, `lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol`, `lambda__logs__secretsmanager.rotation-lambda-actually-replaces-the-current-secret-value-it-generated`, `lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves`, `lambda__rds__secretsmanager.cli-rds-secret-rotation-enable`, `lambda__rds__secretsmanager.rds-rotation-lambda-missing-invoke-permission-denied`, `lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive`, `lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs`. Replaying the corpus, the emulator refused the `RotateSecret` call: File "check_dynamodb__lambda__s3__secretsmanager_cli_rotate_secret_and_verify_version_stages.py", line 194, in _run_aws
    proc = _run_shell(
        ctx,
    ...<2 lines>...
        check=check,
    )
  File "check_dynamodb__lambda__s3__secretsmanager_cli_rotate_secret_and_verify_version_stages.py", line 186, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'secretsmanager', 'rotate-secret', '--secret-id', 'arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-rot-secret-dce8053b3168-OZfz2Y', '--rotation-lambda-arn', 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-fn-aa936b6ccd94', '--rotation-rules', 'AutomaticallyAfterDays=30']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the RotateSecret operation: operation RotateSecret is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the secrets-manager emulator
Call RotateSecret and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
