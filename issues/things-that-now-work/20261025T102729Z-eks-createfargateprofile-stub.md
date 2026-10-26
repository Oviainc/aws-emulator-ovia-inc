Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.connected-cluster-refuses-eks-managed-children
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.connected-cluster-refuses-eks-managed-children|run#12|AssertionError|assert fp_code == "InvalidParameterException", (
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# Implement eks CreateFargateProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFargateProfile

## Description
`CreateFargateProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_fargate_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateFargateProfile` call, so each one unblocks only when `CreateFargateProfile` works: `eks.connected-cluster-refuses-eks-managed-children`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_fargate_profile.rs`. Replaying the corpus, the emulator refused the `CreateFargateProfile` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_eks_connected_cluster_refuses_eks_managed_children.py", line 275, in run
    assert fp_code == "InvalidParameterException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: CreateFargateProfile with no selectors on connected cluster sim2real-behav-connected-020ed6cb9acd -> NotImplemented: operation CreateFargateProfile is scaffolded but not implemented in this emulator. Real AWS validates the payload BEFORE applying the connected-cluster gate, so the refusal must be InvalidParameterException, not ClientException

## Steps to Reproduce
Boot the eks emulator
Call CreateFargateProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
