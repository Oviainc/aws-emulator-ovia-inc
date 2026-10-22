Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.op-pod-identity-identifier-validation
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.op-pod-identity-identifier-validation|run#6|AssertionError|assert code == "InvalidParameterException", f"describe malformed -> {code}: {msg}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement eks DescribePodIdentityAssociation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePodIdentityAssociation

## Description
`DescribePodIdentityAssociation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_pod_identity_association.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribePodIdentityAssociation` call, so each one unblocks only when `DescribePodIdentityAssociation` works: `eks.op-pod-identity-identifier-validation`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_pod_identity_association.rs`. Replaying the corpus, the emulator refused the `DescribePodIdentityAssociation` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_eks_op_pod_identity_identifier_validation.py", line 43, in run
    assert code == "InvalidParameterException", f"describe malformed -> {code}: {msg}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: describe malformed -> NotImplemented: operation DescribePodIdentityAssociation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the eks emulator
Call DescribePodIdentityAssociation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

`DescribePodIdentityAssociation` is implemented and computes its whole response from
the stored `PodIdentityAssociationRecord`; the SCAFFOLDED marker is gone. Because a
describe can only answer from state somebody wrote, the write side of the same
resource landed with it: `CreatePodIdentityAssociation` (which mints the id, ARN and
external id, and is the round-trip's writer), `UpdatePodIdentityAssociation` and
`DeletePodIdentityAssociation`. `src/state.rs` carries
`pod_identity_associations` keyed `(region, clusterName, associationId)`, and
`DeleteCluster` now takes its cluster's associations with it.

Every operation that takes an `associationId` or a `clusterName` checks the
identifier's documented FORM before resolving anything, so a value that can name no
resource is `InvalidParameterException` (400) and a well-formed one that names
nothing is `ResourceNotFoundException` (404). Covered by the suites in
`services/service-com-amazonaws-eks-awswesleyfrontend/tests/all/pod_identity_associations.rs`.

REMAINING, TRACKED ELSEWHERE: `ListPodIdentityAssociations` is still a 501 with its
own issue (`20261022T154613Z-eks-listpodidentityassociations-stub.md`); it reads the
same map this change added.
