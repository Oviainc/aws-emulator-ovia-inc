Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-lifecycle route53.health-check-version-mismatch-rejected route53.health-check-lifecycle-version-increment
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.health-check-lifecycle|run#13|AssertionError|assert err["Message"] == (
Issue-Key: route53.health-check-version-mismatch-rejected|run#9|AssertionError|assert err["Message"] == (
Issue-Key: route53.health-check-lifecycle-version-increment|run#20|AssertionError|assert dup_err["Message"] == (
Issue-Commit: e6eec4415380d0861b3cd74514fe77af8743d21c

# Route 53 health-check refusals use the error shape's documentation as the message instead of AWS's sentence (2 causes)

> _Note: this issue was identified against the code base at commit `e6eec4415380d0861b3cd74514fe77af8743d21c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateHealthCheck

## Description
REOPENS 20261025T035714Z-route53-updatehealthcheck-stub.md: its fix left the call refused. That fixed issue carries exactly the three checks replayed here (route53.health-check-lifecycle, route53.health-check-lifecycle-version-increment, route53.health-check-version-mismatch-rejected) in its Issue-Trace-Ids. UpdateHealthCheck and CreateHealthCheck now answer, the HTTP status and error CODE are correct, and the effect-absent / version-increment controls in all three checks pass — but each check still ends on an error-code response whose MESSAGE is a restatement of the error shape's own Smithy documentation rather than the sentence AWS returns, so all three remain failing. TWO DISTINCT CAUSES are bundled; both must be closed. The ids and versions quoted below are examples that vary per run; the message RULE is the requirement.

Cause 1 of 2: UpdateHealthCheck's HealthCheckVersionMismatch message is the shape doc, not the request/state-derived sentence.
AWS: Smithy com.amazonaws.route53#HealthCheckVersionMismatch (errorKind client, httpError 409, member `message`), bound to UpdateHealthCheck (POST /2013-04-01/healthcheck/{HealthCheckId}); its shape documentation is the prose "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check." The message AWS actually returns is built from the request and the stored state, in the fixed form 'Health check with id <health check id> has version <the version currently stored>, but expected to have version <the HealthCheckVersion sent in the request>. Could not proceed with update.' — fixed text: 'Health check with id ', ' has version ', ', but expected to have version ', '. Could not proceed with update.'; variable parts: the health check id from the request's HealthCheckId label, the current version from the stored health check, and the version from the request's HealthCheckVersion member. Two independently written checks build that same expectation from their own request (one from a version that went stale after a successful update, one from current+999), so the form is the rule, not one recording.
Emulator: code HealthCheckVersionMismatch and the 409 classification are correct, and the rejected call correctly leaves version and config untouched, but Error.Message is "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check." — the shape's documentation verbatim, carrying neither the id nor either version.
Why: a client doing the documented read-modify-write loop (GetHealthCheck, resend HealthCheckVersion, retry on mismatch) cannot learn from the refusal which version is current, so it cannot resolve the conflict without an extra read; tooling that parses the id or the two version numbers out of the text gets nothing, and a client matching the documented sentence prefix misclassifies the refusal.
Why this is not covered elsewhere: open issue 20261025T045147Z-route53-route-53-updatehealthcheck-has-no-atomic-optimistic-concurrency.md concerns the atomicity of the version gate under contention and replays only route53.health-check-concurrent-update-race; it says nothing about the refusal's message text and does not carry these checks.
Fix: the version-mismatch branch of the UpdateHealthCheck handler must mint the message from the request's HealthCheckId, the stored HealthCheckVersion and the requested HealthCheckVersion in the AWS form instead of echoing the error shape's description.
Traces: route53.health-check-lifecycle (assertion at check line 85, after the Code assertion passed and after the version bump, config update and effect-absent reads all passed), route53.health-check-version-mismatch-rejected (assertion at check line 46, Code assertion passed). Handler: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_health_check.rs.

Cause 2 of 2: CreateHealthCheck's HealthCheckAlreadyExists message is a paraphrase of the shape doc, not AWS's constant caller-reference sentence.
AWS: Smithy com.amazonaws.route53#HealthCheckAlreadyExists (errorKind client, httpError 409, member `message`), bound to CreateHealthCheck; the shape documentation describes the condition ("The same value for CallerReference as an existing health check, and one or more values that differ from the existing health check that has the same caller reference"). The message AWS returns for that condition is a constant sentence with no request values in it: 'A different health check has already been created with the specified caller reference. If that health check has been deleted, Route 53 will still maintain the caller reference for a number of days.'
Emulator: code HealthCheckAlreadyExists and the 409 classification are correct, and the surrounding behavior is correct (the rejected duplicate leaves the existing health check's config and version untouched, and the identical ref+config replay returns the same id), but Error.Message is 'A health check has already been created with the specified caller reference and a different configuration.' — a paraphrase of the shape's condition text, missing the whole second sentence about the retained caller reference and differing in the first.
Why: this refusal is how a client tells a caller-reference collision apart from an input-validation 4xx on CreateHealthCheck; matching or logging the documented constant fails, and the operator-facing hint that a deleted health check's caller reference is still held for days never reaches the caller, so a client retries a create that cannot succeed.
Fix: the caller-reference-collision branch of the CreateHealthCheck handler must emit the constant AWS sentence rather than a description of the condition.
Traces: route53.health-check-lifecycle-version-increment (assertion at check line 103; the Code assertion, the version-increment assertions, the effect-absent reads and the idempotent-replay assertion all passed). Handler: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_health_check.rs.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 34/114 checks passing, so the service is serving; bootstrap_signal false). In all three checks the error Code and the HTTP class matched and only Error.Message diverged.

Cause 1 — route53.health-check-version-mismatch-rejected:
  File "check_route53_health_check_version_mismatch_rejected.py", line 46, in run
    assert err["Message"] == (...)
  AssertionError: {'Message': "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check.", 'Code': 'HealthCheckVersionMismatch', 'Type': 'Sender'}
  Expected (built by the check from its own request): "Health check with id <hc_id> has version <current_version>, but expected to have version <current_version+999>. Could not proceed with update."

Cause 1 — route53.health-check-lifecycle:
  File "check_route53_health_check_lifecycle.py", line 85, in run
    assert err["Message"] == (...)
  AssertionError: {'Message': "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check.", 'Code': 'HealthCheckVersionMismatch', 'Type': 'Sender'}
  Expected: "Health check with id <hc_id> has version <current_version>, but expected to have version <initial_version>. Could not proceed with update."

Cause 2 — route53.health-check-lifecycle-version-increment:
  File "check_route53_health_check_lifecycle_version_increment.py", line 103, in run
    assert dup_err["Message"] == (...)
  AssertionError: {'Message': 'A health check has already been created with the specified caller reference and a different configuration.', 'Code': 'HealthCheckAlreadyExists', 'Type': 'Sender'}
  Expected: "A different health check has already been created with the specified caller reference. If that health check has been deleted, Route 53 will still maintain the caller reference for a number of days."

Model evidence (smithy_query shape on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): HealthCheckVersionMismatch — client, httpError 409, member `message`, doc "The value of HealthCheckVersion in the request doesn't match the value of HealthCheckVersion in the health check."; HealthCheckAlreadyExists — client, httpError 409, member `message`, doc describing the same-CallerReference-different-settings condition. In both causes the emulator's message tracks the shape doc rather than the sentence the recordings show, the same systematic pattern already filed for other Route 53 error shapes in 20261025T054608Z-route53-route-53-not-found-and-last-vpc-refusals.md and 20261025T023940Z-route53-route-53-vpc-authorization-pagination-and-traffic-policy.md (neither of which covers the two health-check shapes here).

## Steps to Reproduce
1. Boot the route53 emulator.
2. Cause 1: CreateHealthCheck(CallerReference=<unique>, HealthCheckConfig={IPAddress:'8.8.8.8', Port:80, Type:'TCP', RequestInterval:30, FailureThreshold:3}); note Id and HealthCheckVersion v0.
3. Call UpdateHealthCheck(HealthCheckId=<id>, HealthCheckVersion=v0+999, FailureThreshold=7). The call is correctly refused with Code HealthCheckVersionMismatch / HTTP 409 and the config is correctly unchanged; read Error.Message — it is the shape's documentation sentence instead of 'Health check with id <id> has version <v0>, but expected to have version <v0+999>. Could not proceed with update.'
4. Same cause via a genuinely stale version: CreateHealthCheck, UpdateHealthCheck with HealthCheckVersion=v0 and ResourcePath='/healthz' (succeeds, version v0+1), then UpdateHealthCheck again with HealthCheckVersion=v0 — same wrong message.
5. Cause 2: CreateHealthCheck(CallerReference=R, HealthCheckConfig={FullyQualifiedDomainName:<name>, Port:80, Type:'HTTP', RequestInterval:30, FailureThreshold:3}); then CreateHealthCheck again with the SAME CallerReference=R but Port:443/Type:'HTTPS'. The call is correctly refused with Code HealthCheckAlreadyExists / HTTP 409; read Error.Message — it is 'A health check has already been created with the specified caller reference and a different configuration.' instead of AWS's two-sentence constant 'A different health check has already been created with the specified caller reference. If that health check has been deleted, Route 53 will still maintain the caller reference for a number of days.'
6. Replay: route53.health-check-lifecycle, route53.health-check-version-mismatch-rejected, route53.health-check-lifecycle-version-increment.
