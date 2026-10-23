Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.assessment-grades-app-against-policy-targets resiliencehub.assessment-produces-policy-compliance-verdict resiliencehub.assessment-verdict-propagates-to-app-record resiliencehub.draft-app-version-assessment-rejected resiliencehub.draft-version-assessment-rejected resiliencehub.published-app-version-freezes-resource-mapping resiliencehub.published-version-freezes-resource-mapping
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateAppVersionResource|NotImplemented
Issue-Commit: 8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6

# Implement resiliencehub CreateAppVersionResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppVersionResource

## Description
`CreateAppVersionResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/create_app_version_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateAppVersionResource` call, so each one unblocks only when `CreateAppVersionResource` works: `resiliencehub.assessment-grades-app-against-policy-targets`, `resiliencehub.assessment-produces-policy-compliance-verdict`, `resiliencehub.assessment-verdict-propagates-to-app-record`, `resiliencehub.draft-app-version-assessment-rejected`, `resiliencehub.draft-version-assessment-rejected`, `resiliencehub.published-app-version-freezes-resource-mapping`, `resiliencehub.published-version-freezes-resource-mapping`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/create_app_version_resource.rs`. Replaying the corpus, the emulator refused the `CreateAppVersionResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAppVersionResource operation: operation CreateAppVersionResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the resiliencehub emulator
Call CreateAppVersionResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
