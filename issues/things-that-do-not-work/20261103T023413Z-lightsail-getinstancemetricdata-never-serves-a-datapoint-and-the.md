Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.instance-metric-data-availability lightsail.alarm-configuration-roundtrip-and-delete lightsail.domain-entry-crud-lifecycle
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.instance-metric-data-availability|run#11|Timeout|time.sleep(2)
Issue-Key: lightsail|DeleteAlarm|NotFoundException
Issue-Key: lightsail|DeleteDomainEntry|OperationFailureException
Issue-Commit: 3336dbcca0c02fdce5caa9ace17beecd71a9f5f1

# Lightsail: GetInstanceMetricData never serves a datapoint, and the DeleteAlarm / DeleteDomainEntry not-found refusals carry non-AWS message text

> _Note: this issue was identified against the code base at commit `3336dbcca0c02fdce5caa9ace17beecd71a9f5f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstanceMetricData

## Description
Three net-new root causes in the lightsail crate, surfaced by the behavioral replay. Each must be closed separately before this issue is submitted.

Cause 1 of 3: GetInstanceMetricData answers an empty metricData for every instance, metric and window, so a running instance's CPUUtilization series never becomes available.
AWS: Smithy (com.amazonaws.lightsail#GetInstanceMetricData) — input instanceName (ResourceName, required, pattern ^\w[\w\-]*\w$), metricName (InstanceMetricName enum, required), period (MetricPeriod, required, range 60..86400), startTime/endTime (timestamp, required), unit (MetricUnit enum, required), statistics (MetricStatisticList, required); output metricName (InstanceMetricName) and metricData (MetricDatapointList). AWS backs these metrics with CloudWatch: once an instance has reached `running`, a request over a window covering the elapsed minutes returns one or more MetricDatapoint entries, each carrying the requested statistic member (`average` for statistics=["Average"]) and the metric's published unit. Datapoints may take minutes to land — which is why the check polls — but they do land.
Emulator: the handler resolves the instance and then returns `metric_data: Some(Vec::new())` unconditionally; the only non-empty outcomes it has are the input refusals and the not-found refusal. No code path in the crate ever produces a MetricDatapoint, so no window can ever carry one and the polling caller cannot terminate at any timeout (the suite cut this check off at its 60s ceiling).
Why: every metric consumer sees a permanently empty series behind an authoritative 200. Dashboards, scaling loops and any caller that waits for a metric to appear block forever instead of making progress, and alarms over these metrics can never leave INSUFFICIENT_DATA, so alarm evaluation cannot be exercised at all. A client cannot distinguish "no telemetry is served here" from "this instance has been idle since creation".
Fix: the GetInstanceMetricData handler together with the state module that owns instance telemetry — synthesize a per-period series for a stored instance over the requested window (one datapoint per `period` bucket from instance creation to now, carrying the statistic members the request asked for, in the metric's published unit) rather than an always-empty list. The same owner feeds the sibling metric readers and the alarm evaluator, so it belongs in state and not in this one handler. Related open issue, which names this same check in its Issue-Trace-Ids but whose replay key no longer matches the current check, so the replay did not report it as owned: 20261030T034918Z-lightsail-serves-no-instance-metric-datapoints-and-no.md (its Cause 1 is this cause; its Cause 2, the relational-database SQL data plane, is NOT part of this issue).
Traces: lightsail.instance-metric-data-availability.

Cause 2 of 3: the crate's shared resource-not-found message template is not AWS's, so DeleteAlarm on an already-deleted alarm refuses with the wrong message text.
AWS: Smithy (com.amazonaws.lightsail#DeleteAlarm) — input alarmName (ResourceName, required, pattern ^\w[\w\-]*\w$); modeled errors include NotFoundException, documented "Lightsail throws this exception when it cannot find a resource" and carrying members code/docs/message/tip. Recorded behavior: the refusal is served as wire error NotFoundException whose `message` is "The specified resource <alarmName> does not exist within types [Alarm]. Please provide a valid resource name." Fixed text: "The specified resource " … " does not exist within types [" … "]. Please provide a valid resource name."; the variable parts are the resource name copied from the request and the bracketed resource-type list for the operation (here the single type Alarm). Nothing in that message is per-run.
Emulator: the wire error is NotFoundException as expected, but the message is "The Alarm does not exist: <alarmName>" — a different template entirely. The template is minted once, crate-wide, in Scaffolded::resource_does_not_exist (services/service-com-amazonaws-lightsail-lightsail-20161128/src/scaffolded.rs, `format!("The {resource_type} does not exist: {name}")`), which that file's own doc comment says EVERY not-found path in the crate goes through — DeleteAlarm reaches it via `not_found()` in src/ops/delete_alarm.rs, as do GetInstanceMetricData and DeleteDomainEntry's zone lookup.
Why: idempotent teardown (delete, tolerate not-found) is the normal use of DeleteAlarm, and operators and log/alert pipelines match AWS's message template to recognise the condition they are meant to ignore. Because the wrong template is minted in ONE shared helper, every resource-not-found refusal the crate serves — alarms, instances, disks, buckets, domains — diverges with it, so this is one fix with crate-wide reach.
Fix: the shared not-found constructor in src/scaffolded.rs (Scaffolded::resource_does_not_exist) — build AWS's "The specified resource <name> does not exist within types [<types>]. Please provide a valid resource name." template there, taking the bracketed type list from the operation's monitorable/addressable types, so every call site inherits it instead of each handler re-wording it. Related open issue (wire corpus, same message family, different corpus and not reported as owning this check): 20261029T234852Z-lightsail-client-error-bodies-carry-the-wrong-code.md — its Cause 2 covers DeleteAlarm's body; the `code`/`__type` part of it now matches, the message template does not.
Traces: lightsail.alarm-configuration-roundtrip-and-delete.

Cause 3 of 3: DeleteDomainEntry's "no such domain record" refusal carries hand-written message text instead of AWS's.
AWS: Smithy (com.amazonaws.lightsail#DeleteDomainEntry) — input domainName (DomainName, required) and domainEntry (DomainEntry, required); modeled errors include OperationFailureException (HTTP 400), documented "Lightsail throws this exception when an operation fails to execute". Recorded behavior: re-deleting an entry the zone no longer holds is refused as OperationFailureException with the message "Input error: No such domain record found in this domain for this name/type/target combination." — that whole string is fixed text; it interpolates no request value and no state value at all.
Emulator: the correct exception (OperationFailureException) but the message "No such domain record: the domain entry <entry-id>, nor <name> of type <type> with target <target> does not exist in the Domain <domainName>", assembled in Address::operation_failure (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_domain_entry.rs) from the id and the name/type/target triple the request carried. It also sets `code` = DoesNotExist, which AWS does not use for this operation-failure path.
Why: a caller cleaning up a DNS zone cannot tell this refusal from any other 400 by matching AWS's documented wording, and the emulator additionally leaks internal handles (the minted entry id) into text AWS keeps constant, so message-based detection and golden-output comparisons both fail.
Fix: the DeleteDomainEntry handler's address-miss path (Address::operation_failure) — emit the fixed AWS sentence for the no-such-record refusal, and stop populating a DoesNotExist `code` on an operation-failure body.
Traces: lightsail.domain-entry-crud-lifecycle.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: lightsail 13/24 (54%), 11 failed. The three causes below are the failures that were NOT marked ALREADY FILED (CreateKeyPair stub) or OWNED by an existing issue.

Cause 1 — lightsail.instance-metric-data-availability (mismatch): "Check timed out after 60.0s". The check creates an instance, waits for state `running`, then polls get_instance_metric_data(instanceName=I, metricName="CPUUtilization", period=300, startTime=now-20min, endTime=now, unit="Percent", statistics=["Average"]) until metricData is non-empty and then asserts a datapoint carrying `average`. Every poll returns HTTP 200 with metricData = []. Source: src/ops/get_instance_metric_data.rs sets `metric_data: Some(Vec::new())` (around line 303) with no path that can ever yield a datapoint, so the poll cannot succeed at any timeout.

Cause 2 — lightsail.alarm-configuration-roundtrip-and-delete (mismatch): the PutAlarm/GetAlarms/DeleteAlarm round-trip passes; the final assertion fails on the refusal message of a second DeleteAlarm for the same alarm:
  expected message: 'The specified resource <alarmName> does not exist within types [Alarm]. Please provide a valid resource name.'
  got:              'The Alarm does not exist: <alarmName>'  (raised as NotFoundException by DeleteAlarm)
The alarm name is a request-supplied value; the template around it is the requirement. Source: src/scaffolded.rs Scaffolded::resource_does_not_exist -> format!("The {resource_type} does not exist: {name}"), reached from src/ops/delete_alarm.rs fn not_found.

Cause 3 — lightsail.domain-entry-crud-lifecycle (mismatch): create/get/update of the domain entry all pass; re-deleting the already-deleted entry raises the right exception with the wrong message:
  expected message: 'Input error: No such domain record found in this domain for this name/type/target combination.'
  got:              'No such domain record: the domain entry <entry-id>, nor www.<domain>.com of type A with target 192.0.2.2 does not exist in the Domain <domain>.com'  (OperationFailureException)
The entry id, domain name, type and target in the observed text are request/state values; AWS's text interpolates none of them. Source: src/ops/delete_domain_entry.rs, Address::operation_failure.

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener in the emulator's region.

Cause 1: create_instances(instanceNames=[I], availabilityZone=<zone from get_regions(includeAvailabilityZones=True)>, blueprintId=<active amazon_linux blueprint>, bundleId=<cheapest active bundle>); poll get_instance_state until `running`; then call get_instance_metric_data(instanceName=I, metricName="CPUUtilization", period=300, startTime=now-20min, endTime=now, unit="Percent", statistics=["Average"]) repeatedly. Expect at least one MetricDatapoint carrying an `average` member once the instance has been running. Observed metricData = [] on every call, indefinitely.

Cause 2: with instance I running, put_alarm(alarmName=A, metricName="CPUUtilization", monitoredResourceName=I, comparisonOperator="GreaterThanThreshold", threshold=90.0, evaluationPeriods=1, notificationEnabled=False); delete_alarm(alarmName=A); then delete_alarm(alarmName=A) again. Expect NotFoundException whose message is "The specified resource A does not exist within types [Alarm]. Please provide a valid resource name.". Observed message "The Alarm does not exist: A".

Cause 3: with a us-east-1-pinned client, create_domain(domainName=D); create_domain_entry(domainName=D, domainEntry={name: "www."+D, type: "A", target: "192.0.2.1", isAlias: False}); read the minted id from get_domain; update_domain_entry with target "192.0.2.2"; delete_domain_entry with that entry; then delete_domain_entry with the same entry again. Expect OperationFailureException with message exactly "Input error: No such domain record found in this domain for this name/type/target combination.". Observed a different message naming the entry id, name, type, target and domain, with code=DoesNotExist.

Re-replay: lightsail.instance-metric-data-availability, lightsail.alarm-configuration-roundtrip-and-delete, lightsail.domain-entry-crud-lifecycle.
