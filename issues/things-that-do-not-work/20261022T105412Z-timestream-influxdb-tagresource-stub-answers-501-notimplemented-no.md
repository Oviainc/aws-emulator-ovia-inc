Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.tag-resource-nonexistent-arn-not-found timestream-influxdb.create-db-parameter-group-missing-permission-denied timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.tag-resource-nonexistent-arn-not-found|run#6|AssertionError|assert err["Code"] == "ResourceNotFoundException", (
Issue-Key: timestream-influxdb.create-db-parameter-group-missing-permission-denied|_wait_for_access_denied#11|Timeout|time.sleep(interval_s)
Issue-Key: timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected|run#5|AssertionError|assert err.get("Message") == expected_message, (
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# timestream-influxdb: TagResource stub answers 501 NotImplemented, no identity-based authorization gate, and ValidationException message rule diverges (3 causes)

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three independent root causes in the Timestream for InfluxDB emulator, all grounded in failing behavioral checks of this suite. Causes are numbered; each must be closed before this issue is submitted.

=== Cause 1 of 3: TagResource is an unimplemented scaffold stub, so no tagging call can ever be answered (failing check: timestream-influxdb.tag-resource-nonexistent-arn-not-found) ===
AWS: Smithy (services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/smithy-model/com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb.json) models TagResource with required `resourceArn` (shape Arn, @length 1..1011, @pattern ^arn:aws[a-z\-]*:timestream\-influxdb:[a-z0-9\-]+:[0-9]{12}:(db\-instance|db\-cluster|db\-parameter\-group|db\-backup)/[a-zA-Z0-9]{3,64}$) and required `tags` (RequestTagMap, @length 1..200), an empty (Unit) output, and exactly two modeled errors: ResourceNotFoundException (HTTP 404) and ServiceQuotaExceededException (HTTP 402). A syntactically valid ARN that names no existing resource satisfies every modeled constraint, so AWS reaches the operation and refuses it with HTTP 404 / error code `ResourceNotFoundException`. Recorded message example: "Resource cannot be found." (the status and the code are the requirement; the text is an example). Tagging an ARN that does exist returns 200 with an empty body and the tag is then visible through ListTagsForResource.
Emulator: every TagResource request, for existent and nonexistent ARNs alike, is answered HTTP 501 with error code `NotImplemented` and message "operation TagResource is scaffolded but not implemented in this emulator". The check's first call (a well-formed db-parameter-group ARN with a nonexistent 20-character id) asserted `ResourceNotFoundException` and got `'NotImplemented'`.
Why: a client cannot tag any Timestream for InfluxDB resource at all, and cannot exercise the not-found branch. `NotImplemented` is not a modeled error of this operation, so SDK error handling falls through to the generic/unknown-error path instead of the ResourceNotFoundException branch; retry classification is also wrong (501 is not retryable where a 404 is terminal).
Fix: implement the handler for TagResource in services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/tag_resource.rs — resolve `resourceArn` against the service state's db-instance / db-cluster / db-parameter-group / db-backup collections, return `ResourceNotFoundException` (404) when nothing resolves, otherwise merge `tags` into that resource's tag set and answer 200 with the empty output; then delete the SCAFFOLDED marker line at the top of the file. Note this is NOT the already-filed CreateDbParameterGroup stub: that issue names a different operation and a different handler file.
Traces: timestream-influxdb.tag-resource-nonexistent-arn-not-found. Stub confirmed in the checkout at services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/tag_resource.rs lines 3-6 (SCAFFOLDED marker) and line 172 (`Err(crate::scaffolded::Scaffolded::not_implemented("TagResource"))`).

=== Cause 2 of 3: no identity-based authorization gate — a principal with no permissions is dispatched instead of denied (failing check: timestream-influxdb.create-db-parameter-group-missing-permission-denied) ===
AWS: authorization runs BEFORE the operation executes. A freshly created IAM user with no attached policies calling a timestream-influxdb action is refused with HTTP 403 and error code `AccessDeniedException` — a modeled error on these operations (Smithy: ListDbParameterGroups/CreateDbParameterGroup declare AccessDeniedException with httpError 403). The message is built from the request and is therefore a rule, not free text: "User: arn:aws:iam::<account>:user/<user> is not authorized to perform: timestream-influxdb:<Action> on resource: arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/* because no identity-based policy allows the timestream-influxdb:<Action> action". The denial is independent of whether the action would have succeeded.
Emulator: the request signed with that unprivileged user's access key reaches operation dispatch and is answered HTTP 501 `NotImplemented` — never 403 `AccessDeniedException`. The check retries for 60s waiting for the call to settle on AccessDeniedException and times out ("Check timed out after 60.0s"), so a no-permission principal is treated exactly like the test's own credentials. Hypothesis: the request walk has no authorization step in front of handler dispatch, so principal/policy is never consulted for this service.
Why: any client or test asserting least-privilege behavior sees no denial — unauthorized callers are indistinguishable from authorized ones. Code (`AccessDeniedException`), status (403) and the retry/terminal classification are all wrong, and IAM-policy regression tests built against AWS cannot run.
Fix: the service's request-pipeline authorization step (the authorization/middleware layer of the shared server runtime as mounted for this service crate), evaluated before the per-operation handler, so the 403 is returned regardless of whether the target operation is implemented. Note: this check's later positive control additionally needs CreateDbParameterGroup, whose stub is covered by the separately filed CreateDbParameterGroup issue; the missing authorization gate reported here is a distinct cause and is not fixed by implementing that handler.
Traces: timestream-influxdb.create-db-parameter-group-missing-permission-denied.

=== Cause 3 of 3: ValidationException message is built by the wrong rule — it echoes the submitted value and uses the legacy singular template (failing check: timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected) ===
AWS: Smithy models ListDbParameterGroups.maxResults with @range min 1, max 100, and declares ValidationException (HTTP 400). maxResults=101 is refused with HTTP 400 / code `ValidationException`, and the message is generated from the request by the service's constraint-validation layer — a known rule, naming the member and the violated constraint and NOT echoing the submitted value: "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: <constraint text>"; recorded form for this request: "1 validation error(s) detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Emulator: status and error code are correct (400 / ValidationException) but the message is rendered by the older Coral-style rule — singular "validation error detected" and the offending value interpolated before the member path: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: clients and conformance tests that parse validation messages by this service's documented rule (count-plus-"error(s)", member path, constraint text) mis-parse or mismatch, and the emulator discloses the submitted value in a message where this service does not — so request-value echoing differs from AWS for every constrained member of the service, not just maxResults.
Fix: the shared constraint-validation/serialization layer that renders ValidationException messages for this service (the centralized validate step of the request walk, not the operation handler) — emit the "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: <constraint>" form and stop interpolating the request value.
Traces: timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, timestream-influxdb 0/19 (0%).

Cause 1 — [mismatch] timestream-influxdb.tag-resource-nonexistent-arn-not-found:
  request: TagResource(resourceArn="arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/zzzzzzzzzzzzzzzzzzzz", tags={"env":"sim2real"})
  expected: HTTP 404, Error.Code "ResourceNotFoundException" (message example "Resource cannot be found.")
  actual:   HTTP 501, Error.Code "NotImplemented" — AssertionError: expected ResourceNotFoundException, got 'NotImplemented'
  checkout: services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/tag_resource.rs:172 returns Scaffolded::not_implemented("TagResource"); SCAFFOLDED marker at lines 3-6.

Cause 2 — [mismatch] timestream-influxdb.create-db-parameter-group-missing-permission-denied:
  setup: iam.CreateUser + CreateAccessKey (no policies attached), client re-signed with those credentials
  request: CreateDbParameterGroup(name=<unique>) as that principal, retried every 5s for 60s
  expected: HTTP 403, Error.Code "AccessDeniedException", Message built by the AWS rule "User: arn:aws:iam::<account>:user/<user> is not authorized to perform: timestream-influxdb:CreateDbParameterGroup on resource: arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/* because no identity-based policy allows the timestream-influxdb:CreateDbParameterGroup action"
  actual:   every attempt answered HTTP 501 "NotImplemented"; the call never settles on AccessDeniedException -> "Check timed out after 60.0s". The unprivileged principal is dispatched to the handler rather than refused by an authorization step.

Cause 3 — [mismatch] timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected:
  request: ListDbParameterGroups(maxResults=101)  (model @range 1..100)
  expected: HTTP 400 ValidationException, Message "1 validation error(s) detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  actual:   HTTP 400 ValidationException (code correct), Message "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" — wrong template ("error" vs "error(s)") and the request value '101' echoed back.

## Steps to Reproduce
Boot the emulator from mainline and point a botocore timestream-influxdb client at it (region/account as configured by the harness).

Cause 1:
  arn = f"arn:aws:timestream-influxdb:{region}:{account}:db-parameter-group/" + "z"*20
  influx.tag_resource(resourceArn=arn, tags={"env": "sim2real"})
  -> observe HTTP 501 / Error.Code "NotImplemented"; AWS answers HTTP 404 / "ResourceNotFoundException".

Cause 2:
  u = iam.create_user(UserName="deny-user"); k = iam.create_access_key(UserName="deny-user")["AccessKey"]
  denied = boto3 timestream-influxdb client signed with k.AccessKeyId / k.SecretAccessKey
  denied.create_db_parameter_group(name="anything")
  -> observe HTTP 501 "NotImplemented" (no authorization decision at all); AWS answers HTTP 403 "AccessDeniedException" with the not-authorized-to-perform message, before the operation runs.

Cause 3:
  influx.list_db_parameter_groups(maxResults=101)
  -> observe HTTP 400 ValidationException with Message "1 validation error detected: Value '101' at 'maxResults' failed ..."; AWS answers "1 validation error(s) detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" (no echoed value).

Re-replay these three checks to confirm: timestream-influxdb.tag-resource-nonexistent-arn-not-found, timestream-influxdb.create-db-parameter-group-missing-permission-denied, timestream-influxdb.list-db-parameter-groups-invalid-max-results-rejected.
