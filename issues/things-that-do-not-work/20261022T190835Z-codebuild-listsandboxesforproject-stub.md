Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.list-sandboxes-for-project-reflects-start-order
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|ListSandboxesForProject|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement codebuild ListSandboxesForProject: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSandboxesForProject

## Description
`ListSandboxesForProject` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/list_sandboxes_for_project.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListSandboxesForProject` call, so each one unblocks only when `ListSandboxesForProject` works: `codebuild.list-sandboxes-for-project-reflects-start-order`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/list_sandboxes_for_project.rs`. Replaying the corpus, the emulator refused the `ListSandboxesForProject` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSandboxesForProject operation: operation ListSandboxesForProject is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codebuild emulator
Call ListSandboxesForProject and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
