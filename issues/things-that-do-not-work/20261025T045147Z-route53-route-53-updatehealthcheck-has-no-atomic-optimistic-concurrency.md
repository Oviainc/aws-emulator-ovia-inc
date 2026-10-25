Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-concurrent-update-race
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.health-check-concurrent-update-race|run#7|AssertionError|assert successes == 1, f"expected exactly one winner, got {successes}"
Issue-Commit: 19ea02f55f324c76104eb2ef88e91452a421f03a

# Route 53 UpdateHealthCheck has no atomic optimistic-concurrency: concurrent updates at the same HealthCheckVersion produce zero winners instead of exactly one applied update plus 409 HealthCheckVersionMismatch for the rest

> _Note: this issue was identified against the code base at commit `19ea02f55f324c76104eb2ef88e91452a421f03a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHealthCheck

## Description
Cause 1 of 1: UpdateHealthCheck does not implement the version-gated (optimistic-concurrency) update contract, so concurrent updates that all carry the same base HealthCheckVersion produce no applied update at all. Failing check: route53.health-check-concurrent-update-race.

RELATED (not a duplicate of its replay list): the open issue 20261025T035714Z-route53-updatehealthcheck-stub.md asks for UpdateHealthCheck to be implemented and carries only route53.health-check-lifecycle, route53.health-check-lifecycle-version-increment and route53.health-check-version-mismatch-rejected in its Issue-Trace-Ids. It does NOT carry route53.health-check-concurrent-update-race, so the contention behavior described here is never replayed when that issue is closed: an implementation that reads the stored version, then writes outside the same critical section, satisfies that issue's three checks and still fails this one. Close the stub issue first if you prefer; this issue is closed only when the check named below passes.

AWS: Smithy model services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json — UpdateHealthCheck, POST /2013-04-01/healthcheck/{HealthCheckId}, success 200; input member HealthCheckVersion (long, @range min 1); modeled errors HealthCheckVersionMismatch (HTTP 409), InvalidInput (HTTP 400), NoSuchHealthCheck (HTTP 404). The model's documentation for HealthCheckVersion states the gate explicitly: "This prevents Route 53 from overwriting an intervening update: If the value in the UpdateHealthCheck request matches the value of HealthCheckVersion in the health check, Route 53 updates the health check with the new settings. If the value of HealthCheckVersion in the health check is greater, the health check was changed [after you got the value]", and HealthCheckVersionMismatch is documented as "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check." So when N requests are issued concurrently against one health check all carrying the same base version V: exactly one is applied and returns 200 with the health check at version V+1, and every other one is refused with HTTP 409 and error code HealthCheckVersionMismatch. The stored version advances by exactly 1 under that contention, never by N and never not at all.

Emulator: the handler is an unmodified scaffold (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_health_check.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateHealthCheck"))`), so every one of the five concurrent requests is refused with HTTP 501 `NotImplemented` and nothing is persisted. The check observes zero winners ("expected exactly one winner, got 0"), so neither the single-applied-update behavior nor the 409 HealthCheckVersionMismatch refusal of the losers exists at all.

Why: a client that uses HealthCheckVersion as AWS documents it — read the version with GetHealthCheck, send it back with the update, retry on HealthCheckVersionMismatch — gets no update applied and no 409 to retry on, so a read-modify-write loop against the emulator either spins forever or wrongly concludes the health check cannot be updated. Once the operation is implemented without a compare-and-set, the opposite failure appears: all concurrent writers succeed, last-write-wins silently discards intervening updates and the version counter no longer reflects the number of applied updates, which is exactly the lost-update hazard the version member exists to prevent.

Fix: the operation's handler, services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_health_check.rs, together with the health-check record in the service state it writes. The version comparison and the mutation plus version increment must happen inside one critical section on the state guard (crate::state::service_state's lock), so that two requests carrying the same base version cannot both observe it as current; the loser returns UpdateHealthCheckErrorModeled::HealthCheckVersionMismatch (409). When HealthCheckVersion is omitted from the request the update is unconditional, per the model's "we recommend that you use GetHealthCheck ... and that you include that value" wording (the member is optional).

Traces: route53.health-check-concurrent-update-race — creates one HTTP health check, reads HealthCheckVersion v0, fires five UpdateHealthCheck calls with HealthCheckVersion=v0 and distinct ResourcePath values from five threads, then asserts one success, four HealthCheckVersionMismatch, and a final version of v0+1.

## Evidence
Failing check route53.health-check-concurrent-update-race (verdict: mismatch):

  File "check_route53_health_check_concurrent_update_race.py", line 109, in run
    assert successes == 1, f"expected exactly one winner, got {successes}"
  AssertionError: expected exactly one winner, got 0

Expected (AWS, per the Smithy model cited in the description): of five concurrent UpdateHealthCheck calls carrying the same HealthCheckVersion v0, exactly 1 returns HTTP 200 and 4 return HTTP 409 with error code HealthCheckVersionMismatch; a following GetHealthCheck reports HealthCheckVersion == v0 + 1.

Actual (emulator): 0 of the 5 calls succeeded. The handler file services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_health_check.rs lines 4 and 260 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateHealthCheck"))`, i.e. every call is answered HTTP 501 `NotImplemented` ("operation UpdateHealthCheck is scaffolded but not implemented in this emulator") and no state is written, so neither the single applied update nor the four 409 refusals occur. The surrounding CreateHealthCheck, ChangeTagsForResource and GetHealthCheck calls in the check succeeded, so the divergence is isolated to UpdateHealthCheck.

## Steps to Reproduce
1. Boot the route53 emulator.
2. CreateHealthCheck with HealthCheckConfig {Type: HTTP, FullyQualifiedDomainName: <any>, Port: 80, ResourcePath: "/", RequestInterval: 30, FailureThreshold: 3} and note the returned Id.
3. GetHealthCheck on that Id and note HealthCheckVersion (v0).
4. From five concurrent threads call UpdateHealthCheck(HealthCheckId=<id>, HealthCheckVersion=v0, ResourcePath="/race-<i>") for i in 0..4.
5. Observe all five calls refused with HTTP 501 NotImplemented: zero successes, no HealthCheckVersionMismatch (409) responses. AWS applies exactly one of them (HTTP 200) and refuses the other four with HTTP 409 HealthCheckVersionMismatch.
6. GetHealthCheck again: AWS reports HealthCheckVersion == v0 + 1; the emulator's health check is unchanged.
7. Replay: route53.health-check-concurrent-update-race.
