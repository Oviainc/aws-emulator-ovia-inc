Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-inventory-and-status
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.health-check-inventory-and-status|run#13|AssertionError|assert status_err["Message"] == f"A health check with id {unknown} does not exist.", status_err
Issue-Commit: 35b313377b9e6f690d49d7738eac41604761469d

# Route 53 NoSuchHealthCheck from GetHealthCheckStatus/GetHealthCheckLastFailureReason carries the shape doc instead of AWS's not-found sentence

> _Note: this issue was identified against the code base at commit `35b313377b9e6f690d49d7738eac41604761469d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHealthCheckStatus

## Description
ONE CAUSE is filed here; it must be closed.

Cause 1 of 1: the NoSuchHealthCheck refusal returned by GetHealthCheckStatus and GetHealthCheckLastFailureReason is rendered from the error shape's own Smithy documentation instead of the request-derived sentence AWS returns. The id quoted under Evidence is an example that varies per run; the message RULE is the requirement.

AWS: Smithy com.amazonaws.route53#NoSuchHealthCheck (errorKind client, httpError 404, member `message`; shape doc '<p>No health check exists with the specified ID.</p>'), bound to GetHealthCheckStatus (GET /2013-04-01/healthcheck/{HealthCheckId}/status) and GetHealthCheckLastFailureReason (GET /2013-04-01/healthcheck/{HealthCheckId}/lastfailurereason) — both of which also take HealthCheckId as an @httpLabel with @length max 64. The model does not state the message text, so this is recorded behavior: the refusal's message is request-derived and has the fixed form 'A health check with id <health check id> does not exist.' — fixed text: 'A health check with id ', ' does not exist.'; the single variable part is the health check id taken verbatim from the request's HealthCheckId label. The check builds that same expectation independently for BOTH operations from its own request (a freshly minted UUID that never existed), so the form is the rule, not one recording.

Emulator: the HTTP status class and the error Code are correct — Code 'NoSuchHealthCheck', Type 'Sender' — but Error.Message is 'no health check exists with the ID <id>', i.e. a lower-cased restatement of the error shape's model documentation with the id appended. It differs from AWS in capitalisation, in word order, and in lacking the trailing period-terminated 'does not exist.' form.

Why: a client that tells a not-found health check apart from other 404s by the documented message prefix, or that parses the health check id out of the refusal text, misclassifies or fails to parse the refusal. Because the two operations are the only readers of health-check status, polling loops that narrowly tolerate the documented NoSuchHealthCheck window right after CreateHealthCheck (the documented replication race) cannot match it by text.

Why this is not covered elsewhere: open issue 20261025T115222Z-route53-route-53-health-check-refusals-use-the-error.md covers the SAME systematic pattern but only for UpdateHealthCheck's HealthCheckVersionMismatch and CreateHealthCheck's HealthCheckAlreadyExists, and its Issue-Trace-Ids do not carry route53.health-check-inventory-and-status. Open issue 20261025T054608Z-route53-route-53-not-found-and-last-vpc-refusals.md covers NoSuchHostedZone and LastVPCAssociation, not NoSuchHealthCheck. Open issue 20261025T035714Z-route53-route-53-health-check-references-are-unusable-changeresourcerecordsets.md concerns ChangeResourceRecordSets refusing VALID HealthCheckIds — a behavioral defect, not this message rule. None of them covers the health-check status not-found text.

Fix: the health-check not-found branch used by the GetHealthCheckStatus and GetHealthCheckLastFailureReason handlers must mint the message from the request's HealthCheckId in the AWS form rather than echoing the error shape's description. Likely handlers: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_health_check_status.rs and .../get_health_check_last_failure_reason.rs, plus whatever shared helper mints NoSuchHealthCheck.

Traces: route53.health-check-inventory-and-status — every earlier assertion in that check passed (GetHealthCheckCount increased by exactly 1, ListHealthChecks paged to the new id, GetHealthCheck resolved the id), and the Code assertion for the refusal passed; only the Message assertion at check line 113 diverged.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 62/114 checks passing, so the service is serving; bootstrap_signal false).

[mismatch] route53.health-check-inventory-and-status
  File "check_route53_health_check_inventory_and_status.py", line 113, in run
    assert status_err["Message"] == f"A health check with id {unknown} does not exist.", status_err
  AssertionError: {'Message': 'no health check exists with the ID 3b98a031-9661-41e3-ab53-1cb1f52c1d08', 'Code': 'NoSuchHealthCheck', 'Type': 'Sender'}

Expected (built by the check from its own request, with <id> = the HealthCheckId it sent): "A health check with id <id> does not exist."
Got: "no health check exists with the ID <id>"

The preceding assertion `status_err["Code"] == "NoSuchHealthCheck"` passed, so the status and code are right and only the message rule diverges. The check asserts the identical message form a second time for GetHealthCheckLastFailureReason (line 121) with the same id, which was not reached because the status assertion failed first — the same shared not-found branch serves both operations.

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): NoSuchHealthCheck — errorKind client, httpError 404, member `message`, doc '<p>No health check exists with the specified ID.</p>'; listed as an error of both GetHealthCheckStatus and GetHealthCheckLastFailureReason. The emulator's text tracks that doc rather than the recorded refusal sentence — the same systematic pattern already filed for other Route 53 error shapes.

## Steps to Reproduce
1. Boot the route53 emulator.
2. Pick an id that never existed, e.g. a fresh UUID: U = str(uuid.uuid4()).
3. Call GetHealthCheckStatus(HealthCheckId=U). The call is correctly refused with Code 'NoSuchHealthCheck' / HTTP 404; read Error.Message — it is 'no health check exists with the ID <U>' instead of 'A health check with id <U> does not exist.'
4. Call GetHealthCheckLastFailureReason(HealthCheckId=U). Same correct code, same wrong message form.
5. Positive control (all of which already pass): CreateHealthCheck(CallerReference=<unique>, HealthCheckConfig={FullyQualifiedDomainName:<name>, Port:80, Type:'HTTP', ResourcePath:'/', RequestInterval:30, FailureThreshold:3}); GetHealthCheckCount increases by exactly 1, ListHealthChecks lists the new id, and GetHealthCheck(HealthCheckId=<id>) returns it.
6. Replay: route53.health-check-inventory-and-status
