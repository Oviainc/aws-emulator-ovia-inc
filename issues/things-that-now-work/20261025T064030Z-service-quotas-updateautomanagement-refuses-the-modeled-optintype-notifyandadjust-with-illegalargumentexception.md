Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.auto-management-start-update-stop-lifecycle
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|UpdateAutoManagement|IllegalArgumentException
Issue-Commit: 0b6208938f6837cba6ec149beb646465c55a3ac6

# UpdateAutoManagement refuses the modeled OptInType NotifyAndAdjust with IllegalArgumentException, breaking the Automatic Management start/update/stop round trip

> _Note: this issue was identified against the code base at commit `0b6208938f6837cba6ec149beb646465c55a3ac6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoManagement

## Description
REOPENS 20261024T183922Z-service-quotas-updateautomanagement-validation.md: its fix left the call refused.

Cause 1 of 1: `UpdateAutoManagement` rejects `OptInType=NotifyAndAdjust` instead of storing it, so the account-level Automatic Management state machine cannot be driven past its initial opt-in type. Failing check: service-quotas.auto-management-start-update-stop-lifecycle.

AWS: `UpdateAutoManagement` takes an optional `OptInType` member targeting the `OptInType` enum. Smithy: `com.amazonaws.servicequotas#OptInType` is an enum whose values are exactly `NotifyOnly` and `NotifyAndAdjust`, and `UpdateAutoManagementRequest$OptInType` is a plain optional enum member with no `@pattern`, `@length` or other constraint narrowing it to a subset. The operation models `IllegalArgumentException` (HTTP 400, "Invalid input was provided") only for invalid input; a value the enum declares is not invalid input. Recorded behavior: the replayed check calls `StartAutoManagement(OptInLevel=ACCOUNT, OptInType=NotifyOnly)`, then `UpdateAutoManagement(OptInType=NotifyAndAdjust)` and that call SUCCEEDS; `GetAutoManagementConfiguration` then reports `OptInType=NotifyAndAdjust` with `OptInStatus` still `ENABLED` (an update of the opt-in type must not change the opt-in status), and a subsequent `StopAutoManagement` returns the configuration to `OptInStatus=DISABLED`.

Emulator: the call is refused with HTTP 400 `IllegalArgumentException`, message (example) "OptInType failed to satisfy constraint: Member must be an opt-in type Automatic Management currently offers: NotifyOnly". Nothing is written, so the configuration still reports `OptInType=NotifyOnly` and the lifecycle check aborts at the update step before it ever reaches `StopAutoManagement`. The refusal comes from an availability allow-list in the service state that holds only `NotifyOnly`, consulted by the handler before any state is read or written; the same allow-list is applied on the `StartAutoManagement` write path, so the second enum member is unreachable on both writers.

The member's prose documentation ("There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available.") is what motivated the earlier, now-fixed issue to add this allow-list. The replayed behavior shows that prose is stale relative to the service: the enum member is accepted and stored, so the allow-list is an emulator-only restriction and must be removed rather than extended. The documented availability note is not a modeled constraint and must not be enforced as one.

Why: a client that drives Automatic Management — opting in with notify-only and later switching to notify-and-auto-adjust, which is the only way to change the mode without tearing the configuration down — gets a 400 `IllegalArgumentException` where AWS returns success. Because `IllegalArgumentException` is a modeled client error, SDKs and retry policies treat it as a permanent caller fault, so the client surfaces a bogus validation failure to its own user and the configuration is stuck on whichever opt-in type the initial `StartAutoManagement` named. The error also masks the downstream contract the check verifies: that updating the opt-in type leaves `OptInStatus` at `ENABLED`, and that the configuration can then be stopped back to `DISABLED`.

Fix: the pre-write availability guard in the `UpdateAutoManagement` handler and the identical guard on the `StartAutoManagement` write path, plus the allow-list constant and its helpers in the service state that both consult. Every `OptInType` member the enum declares must be accepted and stored; validation of this member belongs to the schema/boundary enum check alone. Pinned tests asserting the refusal must move with the fix.

Traces: service-quotas.auto-management-start-update-stop-lifecycle. Code citations: services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/update_auto_management.rs lines 125-140 (the guard) and its module doc lines 21-25; services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs lines 143-170 (`AVAILABLE_OPT_IN_TYPES`, `opt_in_type_is_available`, `unavailable_opt_in_type_message`) and lines 499-501.

## Evidence
Expected (replayed AWS behavior):
  1. StartAutoManagement{OptInLevel:"ACCOUNT",OptInType:"NotifyOnly"} -> HTTP 200
  2. GetAutoManagementConfiguration{} -> OptInStatus="ENABLED", OptInType="NotifyOnly"
  3. UpdateAutoManagement{OptInType:"NotifyAndAdjust"} -> HTTP 200 (empty body)
  4. GetAutoManagementConfiguration{} -> OptInType="NotifyAndAdjust", OptInStatus="ENABLED" (unchanged by the update)
  5. StopAutoManagement{} -> HTTP 200; GetAutoManagementConfiguration{} -> OptInStatus="DISABLED"

Actual (emulator): step 3 is refused.
  HTTP 400, error code IllegalArgumentException
  botocore.errorfactory.IllegalArgumentException: An error occurred (IllegalArgumentException) when calling the UpdateAutoManagement operation: OptInType failed to satisfy constraint: Member must be an opt-in type Automatic Management currently offers: NotifyOnly
  (message text quoted as an example; the diverging requirements are the 400 status and the IllegalArgumentException code where the recording succeeded with 200)
Steps 4 and 5 are never reached, so the update-preserves-OptInStatus and stop-returns-DISABLED assertions are also unverified.

Model evidence:
  smithy_query(query="shape", shape="OptInType") -> {"type":"enum","values":["NotifyOnly","NotifyAndAdjust"]}
  smithy_query(query="operation", operation="UpdateAutoManagement") -> input member OptInType targets OptInType, optional, no additional constraints; errors AccessDeniedException(403), IllegalArgumentException(400), NoSuchResourceException(404), ServiceException(500), TooManyRequestsException(429)
  smithy_query(query="doc", shape="UpdateAutoManagementRequest", member="OptInType") -> "...There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available." (prose availability note, not a modeled constraint; the replay shows the service accepts NotifyAndAdjust)

Provenance: the refusal was introduced by the fix for 20261024T183922Z-service-quotas-updateautomanagement-validation.md, which read that prose note as a validation rule and added the AVAILABLE_OPT_IN_TYPES allow-list to both the Update and Start write paths.

## Steps to Reproduce
1. Boot the emulator and point a botocore/AWS SDK service-quotas client at it.
2. Call StartAutoManagement with OptInLevel="ACCOUNT", OptInType="NotifyOnly" -> HTTP 200.
3. Call GetAutoManagementConfiguration -> OptInStatus="ENABLED", OptInType="NotifyOnly" (passes today).
4. Call UpdateAutoManagement with OptInType="NotifyAndAdjust".
   Observed: HTTP 400 IllegalArgumentException "OptInType failed to satisfy constraint: Member must be an opt-in type Automatic Management currently offers: NotifyOnly".
   Expected: HTTP 200 with an empty body.
5. Call GetAutoManagementConfiguration -> expected OptInType="NotifyAndAdjust" and OptInStatus="ENABLED"; observed (if step 4 is tolerated) OptInType still "NotifyOnly".
6. Call StopAutoManagement, then GetAutoManagementConfiguration -> expected OptInStatus="DISABLED".
7. Equivalently, replay the behavioral check service-quotas.auto-management-start-update-stop-lifecycle.
