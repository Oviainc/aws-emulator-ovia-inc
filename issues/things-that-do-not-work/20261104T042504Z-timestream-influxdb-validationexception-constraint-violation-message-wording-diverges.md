Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-instance-invalid-input-boundaries-rejected timestream-influxdb.update-db-instance-storage-decrease-rejected-then-increase-succeeds timestream-influxdb.native-influxdb-http-protocol-round-trip
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-instance-invalid-input-boundaries-rejected|run#8|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: timestream-influxdb.update-db-instance-storage-decrease-rejected-then-increase-succeeds|run#14|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: timestream-influxdb.native-influxdb-http-protocol-round-trip|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 9b14a11a85da2dc3914d7c191991ddabd7c195c3

# timestream-influxdb: ValidationException constraint-violation message wording diverges (3 template defects) and the DB instance's native InfluxDB v2 HTTP data plane is never served

> _Note: this issue was identified against the code base at commit `9b14a11a85da2dc3914d7c191991ddabd7c195c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbInstance

## Description
Four net-new root causes in the timestream-influxdb emulator, all grounded in failing behavioral checks. Causes 1-3 are independent defects in the SAME rendered string (the service-side constraint-violation message that accompanies ValidationException); a fixer must close all three, each is separately observable. Cause 4 is a data-plane gap. Related but NOT covering: open issue 20261022T105412Z-timestream-influxdb-tagresource-stub-answers-501-notimplemented-no.md mentions a ValidationException message divergence for TagResource only; the causes below are on CreateDbInstance / UpdateDbInstance input validation and are not covered by it.

Cause 1 of 4: ValidationException message prefix is singular, not AWS's "N validation error(s) detected" form.
AWS: Smithy: CreateDbInstance.password @length 8..64, @pattern ^[a-zA-Z0-9]+$; CreateDbInstance/UpdateDbInstance.allocatedStorage @range min 20 max 15360; both operations model error ValidationException (client, httpError 400). Recorded behavior: for a server-side constraint violation AWS returns HTTP 400, Error.Code "ValidationException", and an Error.Message whose fixed leading text is "<N> validation error(s) detected: " — the literal parenthesised "(s)" is part of AWS's fixed text and is emitted even when N is 1. The only variable part of that prefix is the count N, taken from the number of violated members in the request.
Emulator: emits "1 validation error detected: ..." — it pluralises the word itself and drops the literal "(s)", observed on both CreateDbInstance (password) and UpdateDbInstance (allocatedStorage).
Why: clients and test harnesses that match on the documented AWS prefix (and operators reading logs) see a message AWS never produces; any consumer parsing "validation error(s) detected" to extract the violation list fails to match, so a real validation rejection is misread as an unknown error.
Fix: the shared constraint-violation message formatter used by the generated request-walk validate stage for this service (the component that renders the ValidationException Message), not the individual handlers.
Traces: timestream-influxdb.create-db-instance-invalid-input-boundaries-rejected, timestream-influxdb.update-db-instance-storage-decrease-rejected-then-increase-succeeds.

Cause 2 of 4: @pattern violation message echoes the anchored model regex instead of AWS's unanchored pattern text.
AWS: Smithy: CreateDbInstance.password @pattern ^[a-zA-Z0-9]+$. Recorded behavior: for a pattern violation AWS's message is "<N> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where the fixed text is everything except <N>, the member name ('password', taken from the request member that violated) and the pattern text — and the pattern text AWS prints for this member is the UNANCHORED body "[a-zA-Z0-9]+", without the ^ and $ anchors carried in the model.
Emulator: prints the raw anchored model pattern: "...Member must satisfy regular expression pattern: ^[a-zA-Z0-9]+$".
Why: the message no longer matches what AWS documents/returns, so a client that surfaces or compares the advertised pattern (or regenerates client-side validation from the error) gets a different regex string than AWS gives for the same rejection.
Fix: the pattern branch of that same constraint-violation message formatter — strip the anchors when rendering the pattern into the message (the stored model trait must stay anchored for enforcement).
Traces: timestream-influxdb.create-db-instance-invalid-input-boundaries-rejected.

Cause 3 of 4: @range violation message interpolates the offending value, which AWS omits for this member.
AWS: Smithy: UpdateDbInstance.allocatedStorage @range min 20 (also CreateDbInstance.allocatedStorage). Recorded behavior: a shrink below the floor is refused with HTTP 400 / "ValidationException" and the message "<N> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must have value greater than or equal to <min>" — fixed text apart from <N>, the member name from the request ('allocatedStorage') and the modeled minimum (20). AWS does NOT include the submitted value in this message.
Emulator: emits "1 validation error detected: Value '10' at 'allocatedStorage' failed to satisfy constraint: Member must have value greater than or equal to 20" — it inserts "'<submitted value>'" after "Value". (The literal 10 above is only this run's request value; the defect is the inserted value slot, not that number.)
Why: the message shape differs from AWS for every range rejection, so message-sensitive clients mis-parse it; it also echoes a request value AWS deliberately leaves out of this message class.
Fix: the range branch of the constraint-violation message formatter.
Traces: timestream-influxdb.update-db-instance-storage-decrease-rejected-then-increase-succeeds.

Cause 4 of 4: an AVAILABLE DB instance's advertised endpoint/port does not serve the native InfluxDB v2 HTTP API at all.
AWS: recorded behavior only (the Smithy model describes only the control plane: GetDbInstance output members `endpoint` (String) and `port` (Port, range 1024..65535, default 8086)). The recording shows that once GetDbInstance reports status AVAILABLE, the host in `endpoint` listening on `port` serves the InfluxDB v2 HTTP API: GET /health returns HTTP 200 with a JSON body whose "status" is "pass"; POST /api/v2/signin with the instance's username/password returns HTTP 204; POST /api/v2/write?org=<organization>&bucket=<bucket>&precision=ns with a line-protocol body returns HTTP 204; and POST /api/v2/query?org=<organization> with a Flux body and Accept: application/csv returns HTTP 200 containing the written measurement and value.
Emulator: the control plane reports the instance AVAILABLE and hands back an endpoint/port, but no InfluxDB v2 HTTP API answers there — the check never gets a 200 from /health and is killed by the harness ceiling ("Check timed out after 60.0s"), so the sign-in, write and query legs are never reachable. Hypothesis: the data plane for a created DB instance is not stood up (or is announced at an address other than the one the instance is reachable on).
Why: the entire data plane of Timestream for InfluxDB is unusable against the emulator. A client that creates an instance, waits for AVAILABLE and then writes/queries points through the InfluxDB v2 API — the normal use of this service — hangs instead of getting 204/200, while the control plane falsely reports the instance as ready to serve.
Fix: the instance data-plane listener for this service (stand up an InfluxDB v2 HTTP surface for an AVAILABLE instance at the endpoint/port the control-plane state announces: /health, /api/v2/signin, /api/v2/write, /api/v2/query) plus the instance state that records that address.
Traces: timestream-influxdb.native-influxdb-http-protocol-round-trip.

## Evidence
Cause 1 (CreateDbInstance, password):
  expected Error.Message: "1 validation error(s) detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+"
  actual   Error.Message: "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9]+$"
  (Error.Code "ValidationException" / HTTP 400 matched.) Fixed text: "validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the count (1), the member name from the request (password) and the member's pattern body ([a-zA-Z0-9]+).

Cause 2 is the pattern-text half of the same diff: "^[a-zA-Z0-9]+$" emitted where AWS emits "[a-zA-Z0-9]+".

Cause 3 (UpdateDbInstance, allocatedStorage=10 on a 20 GiB instance):
  expected Error.Message: "1 validation error(s) detected: Value at 'allocatedStorage' failed to satisfy constraint: Member must have value greater than or equal to 20"
  actual   Error.Message: "1 validation error detected: Value '10' at 'allocatedStorage' failed to satisfy constraint: Member must have value greater than or equal to 20"
  (Error.Code "ValidationException" matched.) Fixed text: "validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must have value greater than or equal to ". Variable parts: count, member name from the request, modeled minimum (20). The inserted "'10'" is the defect; 10 is merely this run's request value.

Cause 4 (native data plane): after CreateDbInstance + GetDbInstance reported status AVAILABLE with an endpoint and port, the check polled GET <endpoint>:<port>/health for a 200 and never received one; the harness terminated the check with "Check timed out after 60.0s". Expected: /health -> 200 {"status":"pass"}, /api/v2/signin -> 204, /api/v2/write -> 204, /api/v2/query -> 200 with the written measurement in the CSV body.

## Steps to Reproduce
Causes 1-2: call CreateDbInstance with a valid name, dbInstanceType db.influx.medium, two subnet ids, one security-group id, allocatedStorage 20, dbStorageType InfluxIOIncludedT1, deploymentType SINGLE_AZ and password "bad-pass!1" (violates the password @pattern). Inspect the returned error: expect HTTP 400 / ValidationException with Message "1 validation error(s) detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+"; the emulator returns "1 validation error detected: ... pattern: ^[a-zA-Z0-9]+$".

Cause 3: create a DB instance with allocatedStorage 20, wait for AVAILABLE, then (with client-side parameter validation disabled so the request reaches the service) call UpdateDbInstance(identifier=<id>, allocatedStorage=10). Expect ValidationException with Message "1 validation error(s) detected: Value at 'allocatedStorage' failed to satisfy constraint: Member must have value greater than or equal to 20"; the emulator inserts "Value '10' at ..." and uses the singular prefix.

Cause 4: create a DB instance (username, password matching ^[a-zA-Z0-9]+$, organization, bucket, publiclyAccessible false, networkType IPV4), poll GetDbInstance until status AVAILABLE, then GET http://<endpoint>:<port>/health. Expect 200 with {"status":"pass"}, then POST /api/v2/signin (basic auth) -> 204, POST /api/v2/write?org=<org>&bucket=<bucket>&precision=ns with a line-protocol point -> 204, POST /api/v2/query?org=<org> with a Flux query -> 200 containing the measurement. Against the emulator nothing answers on that endpoint/port and the /health poll never succeeds.
