Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__eks-auth.pod-identity-absent-cluster-rejected-on-token-not-existence
Issue-Key: eks__eks-auth.pod-identity-absent-cluster-rejected-on-token-not-existence|run#14|AssertionError|assert list_assoc == ("ResourceNotFoundException", not_found), (
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement eks ListPodIdentityAssociations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPodIdentityAssociations

## Description
`ListPodIdentityAssociations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_pod_identity_associations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListPodIdentityAssociations` call, so each one unblocks only when `ListPodIdentityAssociations` works: `eks__eks-auth.pod-identity-absent-cluster-rejected-on-token-not-existence`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_pod_identity_associations.rs`. Replaying the corpus, the emulator refused the `ListPodIdentityAssociations` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_eks__eks_auth_pod_identity_absent_cluster_rejected_on_token_not_existence.py", line 103, in run
    assert list_assoc == ("ResourceNotFoundException", not_found), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: EKS ListPodIdentityAssociations on 'sim2realbehavabsentcluster' returned ('NotImplemented', 'operation ListPodIdentityAssociations is scaffolded but not implemented in this emulator'), expected ('ResourceNotFoundException', 'No cluster found for name: sim2realbehavabsentcluster.')

## Steps to Reproduce
Boot the eks emulator
Call ListPodIdentityAssociations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
