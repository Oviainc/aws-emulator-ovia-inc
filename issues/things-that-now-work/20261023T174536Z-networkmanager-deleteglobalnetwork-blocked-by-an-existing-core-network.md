Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.delete-global-network-blocked-until-core-network-removed
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.delete-global-network-blocked-until-core-network-removed|run#7|AssertionError|assert code == "ValidationException", f"actual code: {code!r}"
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# networkmanager DeleteGlobalNetwork blocked by an existing core network is refused with ConflictException (409) instead of ValidationException (400)

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGlobalNetwork

## Description
The networkmanager emulator classifies the "global network still has a core network" dependency refusal as the wrong modeled error. This issue carries ONE cause.

Cause 1 of 1: DeleteGlobalNetwork on a global network that still owns a core network is refused with the wrong error shape/status.
AWS: Smithy (services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json): DeleteGlobalNetwork -> DELETE /global-networks/{GlobalNetworkId}, success http code 200, output DeleteGlobalNetworkResponse{GlobalNetwork: GlobalNetwork}; modeled errors include both ValidationException (@httpError 400) and ConflictException (@httpError 409), so the model alone does not pick one. Recorded behavior: real AWS refuses this specific dependency case with HTTP 400 and error code ValidationException (example message: 'Global Network cannot be deleted due to an existing core network associated with it.'); after the core network is deleted the identical DeleteGlobalNetwork call succeeds.
Emulator: the same call is refused with error code ConflictException (409). The emulator does detect the dependency (it refuses, and the global network stays AVAILABLE) — only the error classification diverges.
Why: clients branch on the error code, not the message. Code that treats ValidationException as a terminal "you must remove the core network first" condition instead sees ConflictException, which SDK retry/waiter logic and customer code commonly treat as a transient conflict to be retried; against the emulator such a client retries the delete until its own timeout instead of unwinding its core network, and conversely code that catches ConflictException to retry will loop forever against AWS.
Fix: the error classification in the DeleteGlobalNetwork handler's dependency check (services/service-com-amazonaws-networkmanager-networkmanager/src/ops/delete_global_network.rs and the dependency predicate in the crate's state module) must raise the ValidationException shape (400) for the existing-core-network case rather than ConflictException. Note: the open issue 'networkmanager resource state machine: mutations and deletes not applied to stored state' (20261023T132108Z-networkmanager-resource-state-machine-mutations-and-deletes-not.md, Cause 2) covers the opposite situation — DeleteGlobalNetwork refusing with 409 when the dependents are already GONE and it should succeed with 200. This cause is the remaining one: when a dependent core network really does exist, the refusal itself is correct but is emitted as the wrong error type/status.
Traces: behavioral check networkmanager.delete-global-network-blocked-until-core-network-removed.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (networkmanager 6/24 checks passed; bootstrap_signal=false, so the emulator was serving).

[mismatch] networkmanager.delete-global-network-blocked-until-core-network-removed
  check_networkmanager_delete_global_network_blocked_until_core_network_removed.py, line 31:
  assert code == "ValidationException", f"actual code: {code!r}"
  AssertionError: actual code: 'ConflictException'

Sequence exercised by the check: CreateGlobalNetwork -> poll until GlobalNetwork.State == AVAILABLE -> CreateCoreNetwork(GlobalNetworkId) -> poll until CoreNetwork.State == AVAILABLE -> DeleteGlobalNetwork(GlobalNetworkId).
Expected: HTTP 400, Error.Code = 'ValidationException' (example message: 'Global Network cannot be deleted due to an existing core network associated with it.').
Actual: Error.Code = 'ConflictException' (409).

Model basis: DeleteGlobalNetwork errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; success http code 200.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and observe check networkmanager.delete-global-network-blocked-until-core-network-removed.
3. Or by hand against the emulator's networkmanager endpoint:
   a. nm.create_global_network(Description='x') and poll describe_global_networks until State == 'AVAILABLE'.
   b. nm.create_core_network(GlobalNetworkId=<gn>) and poll get_core_network until State == 'AVAILABLE'.
   c. nm.delete_global_network(GlobalNetworkId=<gn>) -> observe Error.Code 'ConflictException' / HTTP 409; AWS answers HTTP 400 with Error.Code 'ValidationException'.
