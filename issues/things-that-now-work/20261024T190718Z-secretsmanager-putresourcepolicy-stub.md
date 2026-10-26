Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.block-public-policy-condition-key-refuses-a-put-resource-policy-that-disables-the-check secretsmanager.block-public-policy-false-attaches-the-policy-that-true-refuses secretsmanager.concurrent-put-resource-policy-no-corruption secretsmanager.deploy-cfn-resource-policy-attached-enforced secretsmanager.get-resource-policy-absent-no-error secretsmanager.get-secret-value-access-denied-by-resource-policy secretsmanager.identity-policy-deny-overrides-a-resource-policy-allow-for-the-same-role secretsmanager.promoted-replica-retains-tags-and-resource-policy-and-both-become-independently-mutable secretsmanager.put-resource-policy-roundtrip secretsmanager.replica-refuses-tag-and-resource-policy-mutations-while-reads-propagate secretsmanager.resource-policy-deny-blocks-get-secret-value secretsmanager.resource-policy-deny-unless-principal-arn-locks-reads-to-one-role secretsmanager.resource-policy-large-json-byte-preserved secretsmanager.server-side-resource-policy-blocks-get-secret-value secretsmanager.server-side-resource-policy-enforced-on-access
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|PutResourcePolicy|NotImplemented
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement secretsmanager PutResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
`PutResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/put_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `PutResourcePolicy` call, so each one unblocks only when `PutResourcePolicy` works: `secretsmanager.block-public-policy-condition-key-refuses-a-put-resource-policy-that-disables-the-check`, `secretsmanager.block-public-policy-false-attaches-the-policy-that-true-refuses`, `secretsmanager.concurrent-put-resource-policy-no-corruption`, `secretsmanager.deploy-cfn-resource-policy-attached-enforced`, `secretsmanager.get-resource-policy-absent-no-error`, `secretsmanager.get-secret-value-access-denied-by-resource-policy`, `secretsmanager.identity-policy-deny-overrides-a-resource-policy-allow-for-the-same-role`, `secretsmanager.promoted-replica-retains-tags-and-resource-policy-and-both-become-independently-mutable`, `secretsmanager.put-resource-policy-roundtrip`, `secretsmanager.replica-refuses-tag-and-resource-policy-mutations-while-reads-propagate`, `secretsmanager.resource-policy-deny-blocks-get-secret-value`, `secretsmanager.resource-policy-deny-unless-principal-arn-locks-reads-to-one-role`, `secretsmanager.resource-policy-large-json-byte-preserved`, `secretsmanager.server-side-resource-policy-blocks-get-secret-value`, `secretsmanager.server-side-resource-policy-enforced-on-access`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/put_resource_policy.rs`. Replaying the corpus, the emulator refused the `PutResourcePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutResourcePolicy operation: operation PutResourcePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the secretsmanager emulator
Call PutResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
