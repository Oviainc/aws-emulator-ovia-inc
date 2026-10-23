Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.concurrent-extension-version-optimistic-lock appconfig.concurrent-update-extension-optimistic-lock appconfig.concurrent-update-extension-version-last-writer-wins appconfig.create-extension-association-missing-required-parameter-rejected appconfig.create-extension-missing-required-parameter-rejected appconfig.create-extension-version-conflict-rejected appconfig.delete-extension-with-active-association-rejected appconfig.e2e-extension-association-flow appconfig.extension-action-point-invocation-enforced-server-side appconfig.extension-association-drives-list-visibility appconfig.extension-association-full-lifecycle-with-list appconfig.extension-association-list-reflects-lifecycle appconfig.extension-association-update-and-list-reflects-latest-parameters appconfig.extension-association-update-fields-and-not-found appconfig.extension-create-and-association-lifecycle appconfig.extension-duplicate-name-conflict-rejected appconfig.extension-full-lifecycle-with-list appconfig.extension-required-parameter-enforced-at-association appconfig.extension-update-fields-persist appconfig.extension-update-server-side-version-increment appconfig.list-extension-associations-filtered-by-resource appconfig.list-extension-associations-reflects-created-associations appconfig.list-extensions-pagination-consistent-order appconfig.op-contract-deleteExtensionAssociation appconfig.update-extension-association-change-extension-identifier-rejected appconfig.update-extension-association-nonexistent-rejected appconfig.update-extension-association-nonexistent-resource-rejected appconfig.update-extension-association-wrong-resource-type-rejected appconfig.update-extension-description-length-exceeded-rejected appconfig.update-extension-invalid-parameter-schema-rejected appconfig.update-extension-parameter-schema-enforced-server-side appconfig.update-extension-stale-version-conflict-rejected appconfig.update-extension-version-increments-and-is-gettable appconfig.update-extension-version-increments-on-change appconfig__lambda.at-deployment-tick-directive-rollback-reverts-the-deployment appconfig__lambda.extension-action-point-invokes-lambda-during-deployment appconfig__lambda.pre-create-hosted-configuration-version-extension-transforms-stored-content
Issue-Key: appconfig|CreateExtension|NotImplemented
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# Implement appconfig CreateExtension: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExtension

## Description
`CreateExtension` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/create_extension.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

37 recorded unit(s) fail on their `CreateExtension` call, so each one unblocks only when `CreateExtension` works: `appconfig.concurrent-extension-version-optimistic-lock`, `appconfig.concurrent-update-extension-optimistic-lock`, `appconfig.concurrent-update-extension-version-last-writer-wins`, `appconfig.create-extension-association-missing-required-parameter-rejected`, `appconfig.create-extension-missing-required-parameter-rejected`, `appconfig.create-extension-version-conflict-rejected`, `appconfig.delete-extension-with-active-association-rejected`, `appconfig.e2e-extension-association-flow`, `appconfig.extension-action-point-invocation-enforced-server-side`, `appconfig.extension-association-drives-list-visibility`, `appconfig.extension-association-full-lifecycle-with-list`, `appconfig.extension-association-list-reflects-lifecycle`, `appconfig.extension-association-update-and-list-reflects-latest-parameters`, `appconfig.extension-association-update-fields-and-not-found`, `appconfig.extension-create-and-association-lifecycle`, `appconfig.extension-duplicate-name-conflict-rejected`, `appconfig.extension-full-lifecycle-with-list`, `appconfig.extension-required-parameter-enforced-at-association`, `appconfig.extension-update-fields-persist`, `appconfig.extension-update-server-side-version-increment` (+17 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/create_extension.rs`. Replaying the corpus, the emulator refused the `CreateExtension` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateExtension operation: operation CreateExtension is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appconfig emulator
Call CreateExtension and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
