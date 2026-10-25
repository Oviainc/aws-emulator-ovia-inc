Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected lambda-microvms.create-shell-auth-token-without-shell-ingress-rejected lambda-microvms.delete-image-with-running-microvm-rejected lambda-microvms.delete-last-image-version-rejected lambda-microvms.delete-microvm-image-lifecycle lambda-microvms.delete-microvm-image-version-with-multiple-versions lambda-microvms.image-delete-rejected-while-a-microvm-is-running lambda-microvms.image-version-status-toggle-active-inactive lambda-microvms.maximum-duration-terminates-the-microvm-with-no-terminate-call lambda-microvms.microvm-auth-token-grants-endpoint-access lambda-microvms.microvm-proxy-endpoint-serves-guest-http lambda-microvms.microvm-run-suspend-resume-terminate-transitions lambda-microvms.resume-running-microvm-is-an-idempotent-no-op lambda-microvms.run-microvm-duration-out-of-range-rejected lambda-microvms.suspend-already-suspended-is-an-idempotent-no-op lambda-microvms.suspend-resume-microvm-transitions lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity lambda-microvms.tag-resource-lifecycle-on-microvm-image lambda-microvms.terminate-microvm-reaches-terminated lambda-microvms.terminated-microvm-rejects-suspend-and-resume lambda-microvms.update-image-version-status-on-failed-build-accepted lambda-microvms.update-microvm-image-creates-new-version
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|GetMicrovmImageVersion|NotImplemented
Issue-Commit: 5b5a943942747caef372cc4ffaa7be31fd45b661

# Implement lambda-microvms GetMicrovmImageVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b5a943942747caef372cc4ffaa7be31fd45b661`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImageVersion

## Description
`GetMicrovmImageVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `GetMicrovmImageVersion` call, so each one unblocks only when `GetMicrovmImageVersion` works: `lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected`, `lambda-microvms.create-shell-auth-token-without-shell-ingress-rejected`, `lambda-microvms.delete-image-with-running-microvm-rejected`, `lambda-microvms.delete-last-image-version-rejected`, `lambda-microvms.delete-microvm-image-lifecycle`, `lambda-microvms.delete-microvm-image-version-with-multiple-versions`, `lambda-microvms.image-delete-rejected-while-a-microvm-is-running`, `lambda-microvms.image-version-status-toggle-active-inactive`, `lambda-microvms.maximum-duration-terminates-the-microvm-with-no-terminate-call`, `lambda-microvms.microvm-auth-token-grants-endpoint-access`, `lambda-microvms.microvm-proxy-endpoint-serves-guest-http`, `lambda-microvms.microvm-run-suspend-resume-terminate-transitions`, `lambda-microvms.resume-running-microvm-is-an-idempotent-no-op`, `lambda-microvms.run-microvm-duration-out-of-range-rejected`, `lambda-microvms.suspend-already-suspended-is-an-idempotent-no-op`, `lambda-microvms.suspend-resume-microvm-transitions`, `lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity`, `lambda-microvms.tag-resource-lifecycle-on-microvm-image`, `lambda-microvms.terminate-microvm-reaches-terminated`, `lambda-microvms.terminated-microvm-rejects-suspend-and-resume` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_version.rs`. Replaying the corpus, the emulator refused the `GetMicrovmImageVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetMicrovmImageVersion operation: operation GetMicrovmImageVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda-microvms emulator
Call GetMicrovmImageVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
