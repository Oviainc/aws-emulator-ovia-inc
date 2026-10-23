Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.code-signing-config-lifecycle lambda.code-signing-enforce-rejects-unsigned-package-rejected lambda.codesigning-config-delete-while-attached-rejected lambda.codesigning-config-lifecycle lambda.codesigning-config-list-roundtrip lambda.create-function-codesigning-not-found-rejected lambda.delete-codesigning-config-in-use-conflict lambda.function-codesigning-association-lifecycle lambda.update-function-code-invalid-signature-rejected lambda.update-function-configuration-signature-errors
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateCodeSigningConfig|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement lambda CreateCodeSigningConfig: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCodeSigningConfig

## Description
`CreateCodeSigningConfig` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_code_signing_config.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `CreateCodeSigningConfig` call, so each one unblocks only when `CreateCodeSigningConfig` works: `lambda.code-signing-config-lifecycle`, `lambda.code-signing-enforce-rejects-unsigned-package-rejected`, `lambda.codesigning-config-delete-while-attached-rejected`, `lambda.codesigning-config-lifecycle`, `lambda.codesigning-config-list-roundtrip`, `lambda.create-function-codesigning-not-found-rejected`, `lambda.delete-codesigning-config-in-use-conflict`, `lambda.function-codesigning-association-lifecycle`, `lambda.update-function-code-invalid-signature-rejected`, `lambda.update-function-configuration-signature-errors`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_code_signing_config.rs`. Replaying the corpus, the emulator refused the `CreateCodeSigningConfig` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateCodeSigningConfig operation: operation CreateCodeSigningConfig is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda emulator
Call CreateCodeSigningConfig and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
