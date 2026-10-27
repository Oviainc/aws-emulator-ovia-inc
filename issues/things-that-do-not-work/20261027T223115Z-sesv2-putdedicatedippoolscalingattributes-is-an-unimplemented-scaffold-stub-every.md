Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.dedicated-ip-pool-scaling-concurrent-modification-converges-to-managed
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.dedicated-ip-pool-scaling-concurrent-modification-converges-to-managed|run#12|AssertionError|assert code == "ConcurrentModificationException", (
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# sesv2 PutDedicatedIpPoolScalingAttributes is an unimplemented scaffold stub: every call is refused 501 NotImplemented, so a STANDARD pool can never be converted to MANAGED and a racing loser never gets ConcurrentModificationException

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDedicatedIpPoolScalingAttributes

## Description
Cause 1 of 1: PutDedicatedIpPoolScalingAttributes answers 501 NotImplemented for every request, so the dedicated-IP-pool scaling plane has no behavior at all.

AWS: Smithy (com.amazonaws.sesv2#PutDedicatedIpPoolScalingAttributes): PUT /v2/email/dedicated-ip-pools/{PoolName}/scaling, success 200 with an empty body, @idempotent. Input: PoolName (httpLabel, required) and ScalingMode (enum MANAGED | STANDARD, required). Modeled errors: BadRequestException (HTTP 400, "The input you provided is invalid."), ConcurrentModificationException (HTTP 500, errorKind server, "The resource is being modified by another operation or thread."), NotFoundException (HTTP 404, "The resource you attempted to access doesn't exist."), TooManyRequestsException (HTTP 429). The operation doc states: "Used to convert a dedicated IP pool to a different scaling mode. MANAGED pools cannot be converted to STANDARD scaling mode." So a pool created with ScalingMode STANDARD that is asked for MANAGED becomes MANAGED and GetDedicatedIpPool subsequently reports DedicatedIpPool.ScalingMode = MANAGED; when two identical STANDARD->MANAGED writes race, AWS either serializes both (both 200) or refuses the loser with ConcurrentModificationException — those are the only two outcomes the behavior admits, and the refusal's message is fixed text with no request-derived parts. A MANAGED->STANDARD request is the BadRequestException case; an unknown PoolName is NotFoundException.

Emulator: every call is refused with HTTP 501 and error code NotImplemented, message "operation PutDedicatedIpPoolScalingAttributes is scaffolded but not implemented in this emulator". In the replayed race both concurrent calls came back NotImplemented (replay output: results=[('error', 'NotImplemented'), ('error', 'NotImplemented')]), there was no winner, and the pool therefore stayed at ScalingMode STANDARD after the race. The handler still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and returns `crate::scaffolded::Scaffolded::not_implemented("PutDedicatedIpPoolScalingAttributes")`.

Note on novelty: the replay did NOT mark this check ALREADY FILED or OWNED — the failure surfaced as an assertion on the returned error code rather than as a raw stub ClientError, so the automatic stub attribution did not catch it — and no open issue names this operation (the neighbouring open stub issues name PutDedicatedIpWarmupAttributes, PutDedicatedIpInPool and GetDedicatedIp, not PutDedicatedIpPoolScalingAttributes).

Why: a client cannot convert a pool's scaling mode at all, so no managed-scaling workflow can be exercised. Worse, the refusal is outside the modeled error set: retry logic that keys on ConcurrentModificationException (an HTTP 500 server-kind error AWS expects callers to retry) sees a 501 NotImplemented instead, and validation logic that expects BadRequestException for the unsupported MANAGED->STANDARD direction never sees it. GetDedicatedIpPool keeps reporting the stale STANDARD mode, so state never converges.

Fix: the operation's handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_pool_scaling_attributes.rs — replace the not_implemented return with a write of ScalingMode onto the pool record held in crate::state (so GetDedicatedIpPool reports it), refuse an unknown PoolName with NotFoundException, refuse the MANAGED->STANDARD direction with BadRequestException, serialize concurrent writes so at most one loser is refused ConcurrentModificationException, and delete the SCAFFOLDED marker line.

Traces: sesv2.dedicated-ip-pool-scaling-concurrent-modification-converges-to-managed (two concurrent STANDARD->MANAGED calls; the assertion that any loser carries ConcurrentModificationException failed because both calls carried NotImplemented).

## Evidence
Expected (AWS): PutDedicatedIpPoolScalingAttributes on a STANDARD pool with ScalingMode=MANAGED returns HTTP 200 with an empty body; a concurrent loser, if any, is refused HTTP 500 ConcurrentModificationException with the fixed message "The resource is being modified by another operation or thread."; GetDedicatedIpPool then reports DedicatedIpPool.ScalingMode = MANAGED.

Actual (emulator): HTTP 501, error code NotImplemented, message "operation PutDedicatedIpPoolScalingAttributes is scaffolded but not implemented in this emulator". Replay assertion: "AssertionError: loser raised NotImplemented, not ConcurrentModificationException (results=[('error', 'NotImplemented'), ('error', 'NotImplemented')])" — i.e. neither racing call won and the pool remained STANDARD.

Handler evidence: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_pool_scaling_attributes.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED") and line 157 (Err(crate::scaffolded::Scaffolded::not_implemented(...))).

## Steps to Reproduce
1. sesv2 CreateDedicatedIpPool(PoolName=<name>, ScalingMode="STANDARD").
2. sesv2 GetDedicatedIpPool(PoolName=<name>) — confirm DedicatedIpPool.ScalingMode == "STANDARD".
3. Issue two concurrent sesv2 PutDedicatedIpPoolScalingAttributes(PoolName=<name>, ScalingMode="MANAGED") calls (released from a barrier).
4. Observe both calls fail with HTTP 501 / NotImplemented instead of at least one 200 and, at most, one HTTP 500 ConcurrentModificationException.
5. sesv2 GetDedicatedIpPool(PoolName=<name>) — ScalingMode is still "STANDARD" instead of "MANAGED".

Replay: sesv2.dedicated-ip-pool-scaling-concurrent-modification-converges-to-managed
