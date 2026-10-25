Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.associate-dissociate-packages-concurrent-race-conflict opensearch.associate-package-nonexistent-domain-rejected opensearch.associate-packages-batch-duplicate-name-collapsed opensearch.associate-packages-duplicate-in-batch-conflict-rejected opensearch.concurrent-associate-dissociate-packages-race-consistency opensearch.conflict-precedence-package-association opensearch.delete-package-while-associated-rejected opensearch.e2e-package-association-enables-index-analyzer opensearch.e2e-package-association-index-effect-teardown opensearch.e2e-package-association-search-flow opensearch.e2e-package-version-update-domain-propagation opensearch.get-package-version-history-access-denied-cross-account opensearch.get-package-version-history-cross-account-denied opensearch.ordering-package-version-history-sequence opensearch.package-association-batch-lifecycle opensearch.package-association-lifecycle opensearch.package-dissociation-blocks-dependent-index-creation-rejected opensearch.package-lifecycle-create-delete opensearch.package-scope-invalid-account-rejected opensearch.package-scope-update-semantics opensearch.package-version-history-chronological-ordering opensearch.package-version-history-monotonic-ordering opensearch.package-version-history-retention-window opensearch.representation-package-version-numeric-size-precision-preserved opensearch.update-index-dependency-failure-on-unavailable-package opensearch.update-package-concurrent-version-bump-serialized opensearch.update-package-invalid-version-format-base-exception
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|CreatePackage|NotImplemented
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement opensearch CreatePackage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackage

## Description
`CreatePackage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_package.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

27 recorded unit(s) fail on their `CreatePackage` call, so each one unblocks only when `CreatePackage` works: `opensearch.associate-dissociate-packages-concurrent-race-conflict`, `opensearch.associate-package-nonexistent-domain-rejected`, `opensearch.associate-packages-batch-duplicate-name-collapsed`, `opensearch.associate-packages-duplicate-in-batch-conflict-rejected`, `opensearch.concurrent-associate-dissociate-packages-race-consistency`, `opensearch.conflict-precedence-package-association`, `opensearch.delete-package-while-associated-rejected`, `opensearch.e2e-package-association-enables-index-analyzer`, `opensearch.e2e-package-association-index-effect-teardown`, `opensearch.e2e-package-association-search-flow`, `opensearch.e2e-package-version-update-domain-propagation`, `opensearch.get-package-version-history-access-denied-cross-account`, `opensearch.get-package-version-history-cross-account-denied`, `opensearch.ordering-package-version-history-sequence`, `opensearch.package-association-batch-lifecycle`, `opensearch.package-association-lifecycle`, `opensearch.package-dissociation-blocks-dependent-index-creation-rejected`, `opensearch.package-lifecycle-create-delete`, `opensearch.package-scope-invalid-account-rejected`, `opensearch.package-scope-update-semantics` (+7 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_package.rs`. Replaying the corpus, the emulator refused the `CreatePackage` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePackage operation: operation CreatePackage is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearch emulator
Call CreatePackage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
