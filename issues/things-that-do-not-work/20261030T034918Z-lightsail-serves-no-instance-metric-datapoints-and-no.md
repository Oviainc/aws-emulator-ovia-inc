Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.instance-metric-data-availability lightsail.relational-database-native-sql-roundtrip
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.instance-metric-data-availability|run#11|Timeout|time.sleep(30)
Issue-Key: lightsail.relational-database-native-sql-roundtrip|_connect_mysql#14|Timeout|time.sleep(5)
Issue-Commit: f4d2c47849283505bef5d66ed0dfe45248bd135a

# Lightsail serves no instance metric datapoints and no SQL on a managed database endpoint, so both data planes never answer

> _Note: this issue was identified against the code base at commit `f4d2c47849283505bef5d66ed0dfe45248bd135a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstanceMetricData

## Description
Two net-new root causes, both data planes of Lightsail resources the control plane reports as healthy and reachable. Each must be closed separately.

Cause 1 of 2: GetInstanceMetricData answers an EMPTY metricData for every instance, every metric and every window, so a running instance's CPUUtilization series never becomes available.
AWS: Smithy (com.amazonaws.lightsail#GetInstanceMetricData) — input instanceName (ResourceName, required), metricName (InstanceMetricName enum, required), period (MetricPeriod, required, range 60..86400), startTime/endTime (timestamp, required), unit (MetricUnit enum, required), statistics (MetricStatisticList, required); output metricName and metricData (MetricDatapointList). AWS publishes CloudWatch-backed datapoints for a running instance: after the instance reaches `running`, a request over a window that covers the elapsed minutes returns one or more MetricDatapoint entries, each carrying the requested statistic member (`average` for statistics=["Average"]) and the metric's unit. The datapoints can take a few minutes to appear, which is why the check polls, but they DO appear.
Emulator: the handler returns `metric_data: Some(Vec::new())` unconditionally for a stored instance (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_instance_metric_data.rs — the only outcomes are the not-found refusal, the input refusals, and a 200 whose metricData is always empty; the comment states "NONE, for every window and every instance"). The check consequently polls forever and the suite cut it off at its 60s ceiling; it can never terminate, since no window ever carries a datapoint.
Why: every metric consumer sees a permanently empty series. Dashboards, autoscaling-style polling loops and any caller that waits for a metric to land block indefinitely instead of making progress, and Lightsail alarms over these metrics can never leave INSUFFICIENT_DATA, so alarm behavior cannot be exercised at all. An authoritative-looking 200 with no data is indistinguishable, to a client, from "this instance has been idle forever".
Fix: the GetInstanceMetricData handler plus the state module that owns instance telemetry — synthesize a per-period series for the instance over the requested window (one datapoint per `period` bucket from instance creation to now, carrying the requested statistic members in the metric's published unit) so a running instance has data, instead of returning an always-empty list. The same owner feeds the other metric readers and the alarm evaluator, so it belongs in state, not in this one handler.
Traces: lightsail.instance-metric-data-availability.

Cause 2 of 2: a Lightsail managed relational database publishes a masterEndpoint but serves no SQL there, so a CREATE TABLE / INSERT / SELECT round-trip against the endpoint never completes.
AWS: Smithy (com.amazonaws.lightsail#CreateRelationalDatabase / GetRelationalDatabase) — RelationalDatabase carries `masterEndpoint` (address + port) and `state`; a MySQL-blueprint database created with publiclyAccessible=true reaches state `available` and its masterEndpoint then accepts MySQL protocol connections authenticated with masterUsername/masterUserPassword against masterDatabaseName, where DDL and DML round-trip. Recorded behavior only for the endpoint's wire protocol (the model describes the address, not the engine): the recording shows a client connecting to that address/port with pymysql and executing CREATE TABLE / INSERT / SELECT successfully.
Emulator: CreateRelationalDatabase stores a record whose endpoint_address is a minted host label (crate::state::relational_database_endpoint_address in src/ops/create_relational_database.rs) and whose state is owed `available` by the clock; nothing in the crate stands up a MySQL listener behind that address, and GetRelationalDatabase only renders the stored record. The check therefore never gets past bringing the database up and connecting to it — it was cut off at the suite's 60s ceiling with no SQL served. Hypothesis: the connect to the published address hangs or is refused because no process listens there; the control-plane record is served normally.
Why: the whole point of a managed database is the data plane. Any client that provisions a Lightsail database and then uses it — migrations, seeding, application boot — hangs or fails on connect even though the control plane reports the database `available` and hands out a masterEndpoint, which is a far worse failure mode than an honest refusal.
Fix: the relational-database data plane — either stand up a listener for the published masterEndpoint that speaks the blueprint's engine protocol and persists schema/rows per database record, or make the gap explicit rather than advertising an endpoint nothing answers on. Belongs alongside the relational-database state module and whatever mounts per-resource listeners (the same seam the bucket object store is served from), not in CreateRelationalDatabase's handler body.
Traces: lightsail.relational-database-native-sql-roundtrip.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (lightsail 15/25, 10 failed).

Cause 1 — lightsail.instance-metric-data-availability (mismatch): "Check timed out after 60.0s". The check creates an instance, waits for `running`, then polls get_instance_metric_data(instanceName=..., metricName="CPUUtilization", period=300, startTime=now-20min, endTime=now, unit="Percent", statistics=["Average"]) until metricData is non-empty, asserting a datapoint carrying `average`. The emulator answers HTTP 200 with metricData = [] on every poll; src/ops/get_instance_metric_data.rs line 303 sets `metric_data: Some(Vec::new())` with no code path that ever produces a datapoint, so the poll cannot succeed at any timeout.

Cause 2 — lightsail.relational-database-native-sql-roundtrip (mismatch): "Check timed out after 60.0s". The check creates a MySQL-blueprint database with publiclyAccessible=True, waits for state `available`, then connects to relationalDatabase.masterEndpoint (address, port) with pymysql and runs CREATE TABLE sim2real_roundtrip / INSERT / SELECT, expecting the row 'hello-lightsail' back. pymysql was present (a missing driver would have raised RequiredToolMissing instead of timing out). The emulator never completes the round-trip: the crate holds only the minted endpoint address string (src/ops/create_relational_database.rs, endpoint_address from crate::state::relational_database_endpoint_address) and serves no engine behind it.

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener in the emulator's region.

Cause 1: create_instances(instanceNames=[I], availabilityZone=<zone from get_regions>, blueprintId=<amazon_linux blueprint>, bundleId=<cheapest active bundle>); poll get_instance_state until `running`; then call get_instance_metric_data(instanceName=I, metricName="CPUUtilization", period=300, startTime=now-20min, endTime=now, unit="Percent", statistics=["Average"]). Expect at least one MetricDatapoint with an `average` member once the instance has been running for a few minutes. Observed metricData = [] on every call, indefinitely.

Cause 2: create_relational_database(relationalDatabaseName=D, relationalDatabaseBlueprintId=<default mysql blueprint>, relationalDatabaseBundleId=<cheapest active db bundle>, masterDatabaseName="roundtripdb", masterUsername="dbadmin", masterUserPassword=<valid password>, publiclyAccessible=True); poll get_relational_database until state == "available"; read relationalDatabase.masterEndpoint and connect to (address, port) with pymysql using the master credentials; run CREATE TABLE sim2real_roundtrip (id INT PRIMARY KEY, val VARCHAR(64)), INSERT (1,'hello-lightsail'), commit, SELECT val WHERE id=1. Expect the row back. Observed no SQL served at the published endpoint — the round-trip never completes.

Re-replay: lightsail.instance-metric-data-availability, lightsail.relational-database-native-sql-roundtrip.
