Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam.ekscc-e2e-create-eks-cluster-lifecycle
Issue-Key: eks|ListFargateProfiles|NotImplemented
Issue-Commit: 6e4fa4ae0561cad796d7088ff8894510cc385391

# Implement eks ListFargateProfiles: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e4fa4ae0561cad796d7088ff8894510cc385391`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFargateProfiles

## Description
`ListFargateProfiles` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_fargate_profiles.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListFargateProfiles` call, so each one unblocks only when `ListFargateProfiles` works: `eks__iam.ekscc-e2e-create-eks-cluster-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_fargate_profiles.rs`. Replaying the corpus, the emulator refused the `ListFargateProfiles` call: File "contextlib.py", line 158, in __exit__
    self.gen.throw(value)
  File "check_eks__iam_ekscc_e2e_create_eks_cluster_lifecycle.py", line 365, in phase
    raise _PhaseError(
_simparity_dyn_check_eks__iam_ekscc_e2e_create_eks_cluster_lifecycle_8e26564a._PhaseError: [phase 4/6 post_invoke] ClientError: An error occurred (NotImplemented) when calling the ListFargateProfiles operation: operation ListFargateProfiles is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the eks emulator
Call ListFargateProfiles and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
