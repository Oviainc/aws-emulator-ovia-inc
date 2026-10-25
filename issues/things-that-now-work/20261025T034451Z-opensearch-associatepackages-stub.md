Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.associate-dissociate-packages-concurrent-race-conflict opensearch.concurrent-associate-dissociate-packages-race-consistency opensearch.delete-package-while-associated-rejected opensearch.e2e-package-association-enables-index-analyzer opensearch.e2e-package-association-index-effect-teardown opensearch.e2e-package-version-update-domain-propagation opensearch.package-association-batch-lifecycle opensearch.package-dissociation-blocks-dependent-index-creation-rejected opensearch.update-index-dependency-failure-on-unavailable-package
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|AssociatePackages|NotImplemented
Issue-Commit: 7904814320df70b9478f23cd4eaae300bbcca259

# Implement opensearch AssociatePackages: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7904814320df70b9478f23cd4eaae300bbcca259`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociatePackages

## Description
`AssociatePackages` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/associate_packages.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `AssociatePackages` call, so each one unblocks only when `AssociatePackages` works: `opensearch.associate-dissociate-packages-concurrent-race-conflict`, `opensearch.concurrent-associate-dissociate-packages-race-consistency`, `opensearch.delete-package-while-associated-rejected`, `opensearch.e2e-package-association-enables-index-analyzer`, `opensearch.e2e-package-association-index-effect-teardown`, `opensearch.e2e-package-version-update-domain-propagation`, `opensearch.package-association-batch-lifecycle`, `opensearch.package-dissociation-blocks-dependent-index-creation-rejected`, `opensearch.update-index-dependency-failure-on-unavailable-package`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/associate_packages.rs`. Replaying the corpus, the emulator refused the `AssociatePackages` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociatePackages operation: operation AssociatePackages is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearch emulator
Call AssociatePackages and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
