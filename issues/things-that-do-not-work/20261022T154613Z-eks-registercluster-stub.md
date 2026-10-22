Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.cluster-register-deregister-lifecycle eks.cluster-tag-round-trip-and-untag eks.cluster-tags-empty-vs-absent-preserved eks.cluster-tags-unicode-round-trip eks.connected-cluster-refuses-eks-managed-children eks.create-certificate-authority-invalid-parameter-rejected eks.deregister-cluster-blocked-in-use eks.list-clusters-include-projection-under-pagination eks.list-clusters-pagination-invariants eks.list-tags-for-resource-not-found eks.op-register-cluster-validation eks.register-cluster-access-denied eks.register-cluster-client-request-token-is-idempotent eks.tag-resource-contract-limits-rejected eks.tag-resource-malformed-arn-rejected eks.untag-resource-not-found
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|RegisterCluster|NotImplemented
Issue-Key: eks.register-cluster-access-denied|run#6|AssertionError|assert code == "AccessDeniedException", f"{op} without permission -> {code}: {msg}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement eks RegisterCluster: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCluster

## Description
`RegisterCluster` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/register_cluster.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `RegisterCluster` call, so each one unblocks only when `RegisterCluster` works: `eks.cluster-register-deregister-lifecycle`, `eks.cluster-tag-round-trip-and-untag`, `eks.cluster-tags-empty-vs-absent-preserved`, `eks.cluster-tags-unicode-round-trip`, `eks.connected-cluster-refuses-eks-managed-children`, `eks.create-certificate-authority-invalid-parameter-rejected`, `eks.deregister-cluster-blocked-in-use`, `eks.list-clusters-include-projection-under-pagination`, `eks.list-clusters-pagination-invariants`, `eks.list-tags-for-resource-not-found`, `eks.op-register-cluster-validation`, `eks.register-cluster-access-denied`, `eks.register-cluster-client-request-token-is-idempotent`, `eks.tag-resource-contract-limits-rejected`, `eks.tag-resource-malformed-arn-rejected`, `eks.untag-resource-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/register_cluster.rs`. Replaying the corpus, the emulator refused the `RegisterCluster` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterCluster operation: operation RegisterCluster is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the eks emulator
Call RegisterCluster and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
