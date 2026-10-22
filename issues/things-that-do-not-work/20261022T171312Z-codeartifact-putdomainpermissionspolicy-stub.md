Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.cross-domain-unauthorized-access-denied codeartifact.delete-domain-permissions-policy-revision-mismatch-conflict codeartifact.domain-permissions-policy-document-json-preserved codeartifact.domain-permissions-policy-server-enforced-on-repository-actions codeartifact.put-domain-permissions-policy-stale-revision-conflict
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|PutDomainPermissionsPolicy|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codeartifact PutDomainPermissionsPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDomainPermissionsPolicy

## Description
`PutDomainPermissionsPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/put_domain_permissions_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `PutDomainPermissionsPolicy` call, so each one unblocks only when `PutDomainPermissionsPolicy` works: `codeartifact.cross-domain-unauthorized-access-denied`, `codeartifact.delete-domain-permissions-policy-revision-mismatch-conflict`, `codeartifact.domain-permissions-policy-document-json-preserved`, `codeartifact.domain-permissions-policy-server-enforced-on-repository-actions`, `codeartifact.put-domain-permissions-policy-stale-revision-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/put_domain_permissions_policy.rs`. Replaying the corpus, the emulator refused the `PutDomainPermissionsPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutDomainPermissionsPolicy operation: operation PutDomainPermissionsPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeartifact emulator
Call PutDomainPermissionsPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
