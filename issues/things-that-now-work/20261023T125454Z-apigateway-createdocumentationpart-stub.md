Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.concurrent-create-documentation-version-race apigateway.create-documentation-part-duplicate-location-fails apigateway.create-documentation-part-duplicate-location-rejected apigateway.create-documentation-version-duplicate-conflict apigateway.delete-nonexistent-documentation-part-returns-notfound apigateway.documentation-part-duplicate-location-rejected apigateway.documentation-part-lifecycle apigateway.documentation-part-nested-properties-round-trip apigateway.documentation-part-properties-json-preserved apigateway.documentation-part-properties-json-round-trip apigateway.documentation-part-properties-unicode-preserved apigateway.documentation-version-duplicate-creation-rejected apigateway.documentation-version-lifecycle apigateway.documentation-version-ordering apigateway.duplicate-documentation-part-location-rejected apigateway.get-documentation-part-not-found apigateway.get-documentation-version-error-contract apigateway.get-documentation-version-unauthorized apigateway.get-documentation-version-unauthorized-vs-notfound apigateway.get-documentation-versions-lists-created apigateway.import-documentation-parts-overwrite-mode
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|CreateDocumentationPart|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigateway CreateDocumentationPart: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDocumentationPart

## Description
`CreateDocumentationPart` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_documentation_part.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

21 recorded unit(s) fail on their `CreateDocumentationPart` call, so each one unblocks only when `CreateDocumentationPart` works: `apigateway.concurrent-create-documentation-version-race`, `apigateway.create-documentation-part-duplicate-location-fails`, `apigateway.create-documentation-part-duplicate-location-rejected`, `apigateway.create-documentation-version-duplicate-conflict`, `apigateway.delete-nonexistent-documentation-part-returns-notfound`, `apigateway.documentation-part-duplicate-location-rejected`, `apigateway.documentation-part-lifecycle`, `apigateway.documentation-part-nested-properties-round-trip`, `apigateway.documentation-part-properties-json-preserved`, `apigateway.documentation-part-properties-json-round-trip`, `apigateway.documentation-part-properties-unicode-preserved`, `apigateway.documentation-version-duplicate-creation-rejected`, `apigateway.documentation-version-lifecycle`, `apigateway.documentation-version-ordering`, `apigateway.duplicate-documentation-part-location-rejected`, `apigateway.get-documentation-part-not-found`, `apigateway.get-documentation-version-error-contract`, `apigateway.get-documentation-version-unauthorized`, `apigateway.get-documentation-version-unauthorized-vs-notfound`, `apigateway.get-documentation-versions-lists-created` (+1 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_documentation_part.rs`. Replaying the corpus, the emulator refused the `CreateDocumentationPart` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDocumentationPart operation: operation CreateDocumentationPart is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call CreateDocumentationPart and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
