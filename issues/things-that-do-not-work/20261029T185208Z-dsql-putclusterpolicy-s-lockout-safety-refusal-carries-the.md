Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.put-cluster-policy-lockout-safety-rejected dsql.create-stream-lifecycle-to-kinesis
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.put-cluster-policy-lockout-safety-rejected|run#10|AssertionError|assert lockout_message == "The new policy would result in the calling principal being unable to further update and delete this cluster's policy. Set the 'bypass
Issue-Key: dsql.create-stream-lifecycle-to-kinesis|_connect_with_retry#34|Timeout|time.sleep(interval_s)
Issue-Commit: f85dfe40fdc33b8708513e38d3ef8c108b7b5586

# DSQL: PutClusterPolicy's lockout-safety refusal carries the wrong ValidationException message, and the cluster+stream deploy workflow still dead-ends at the missing PostgreSQL data plane / CDC delivery

> _Note: this issue was identified against the code base at commit `f85dfe40fdc33b8708513e38d3ef8c108b7b5586`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
This issue bundles 2 net-new root causes in the dsql service. Both must be closed before submitting.

Cause 1 of 2: PutClusterPolicy refuses a lockout policy with the right code but the wrong message text.
Failing check: dsql.put-cluster-policy-lockout-safety-rejected

AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) — PutClusterPolicy input `bypassPolicyLockoutSafetyCheck` (BypassPolicyLockoutSafetyCheck, default false), documented as "A flag that allows you to bypass the policy lockout safety check. When set to true, this parameter allows you to apply a policy that might lock you out of the cluster."; modeled error ValidationException, httpError 400. The model does not state the refusal's message, so for the wording this is recorded behavior only: the recording shows the lockout refusal is HTTP 400, Error Code `ValidationException`, Error Message exactly "The new policy would result in the calling principal being unable to further update and delete this cluster's policy. Set the 'bypassPolicyLockoutSafetyCheck' parameter to 'true' to confirm this intent." That sentence is entirely FIXED text for this error class — it interpolates no identifier, ARN, action, version or timestamp from the request or from state — and it states the rule in terms of the CALLING principal losing the ability to further update and delete the cluster's policy, naming the parameter in quotes ('bypassPolicyLockoutSafetyCheck', 'true').

Emulator: the status (400) and the code (ValidationException) match, but the message is a different sentence: "The policy would prevent every principal from calling dsql:PutClusterPolicy on this cluster. Set bypassPolicyLockoutSafetyCheck to true to apply it anyway." It is built in the shared document validator, which also interpolates the policy-management action constant into the fixed text — a variable part AWS's message does not have. The ValidationException `reason` is set to the service's "other" reason value.

Why: the error message is part of this error class's contract. A client (or an operator reading a log) that matches or displays the lockout refusal sees a sentence AWS never emits, and the emulator's sentence also misstates the rule: AWS's check is about the CALLING principal being unable to further update and delete this cluster's policy, while the emulator asserts the policy would block "every principal" from one named action — so a policy whose effect on the caller alone is what matters is described wrongly, and the remediation hint does not quote the parameter and value the way AWS words it.

Fix: the shared cluster-policy document validation in the state module (the lockout branch of `validate_cluster_policy`), which both policy writers call — the refusal's message text belongs there, not in either handler. Note: the open issue 20261028T155002Z-dsql-putclusterpolicy-wrong-behavior--r3.md concerns the same validator but a different defect (NotPrincipal is ignored by the lockout predicate); it does not cover this wording divergence. The earlier fixed issue 20261022T193920Z-dsql-putclusterpolicy-stale-expectedpolicyversion-conflict-carries-the-wrong.md made the deny-all policy be refused at all; the refusal it introduced carries this wrong message.

Traces: dsql.put-cluster-policy-lockout-safety-rejected — the check asserts the code first (ValidationException, which passes) and then the message, which fails with the emulator sentence above. File citations: services/service-com-amazonaws-dsql-dsql/src/state.rs lines 2158-2182 (`validate_cluster_policy`, lockout branch at 2170-2180); services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs lines 246-252 (the call site that lifts it into ValidationException).

Cause 2 of 2: the full cluster+policy+CDC-stream deploy workflow never completes — it blocks at the data-plane step and is killed by the harness.
Failing check: dsql.create-stream-lifecycle-to-kinesis

AWS: recorded behavior only (the Smithy model is control-plane only; CreateStream's documentation says the stream "captures database changes and delivers them to the specified target destination", with targetDefinition.kinesis{streamArn, roleArn}, ordering UNORDERED, format JSON). The recording shows the whole provisioning workflow completing: CreateCluster -> cluster ACTIVE with tags readable through ListTagsForResource -> PutClusterPolicy/GetClusterPolicy agree on document and policyVersion -> CreateStream against a real Kinesis stream and an account-local role settles CREATING -> ACTIVE and is listed -> a libpq client connects to the published endpoint on TCP 5432 (database postgres, user admin, admin auth token as password, TLS) and commits a row -> the resulting JSON change record is read off the target Kinesis stream with GetShardIterator/GetRecords -> teardown in reverse order removes stream, policy (GetClusterPolicy then 404 ResourceNotFoundException with "Aurora DSQL can't find the policy for the resource <identifier>. Verify that the policy exists.", where <identifier> is the cluster id from the request) and cluster.

Emulator: the check reports only "Check timed out after 60.0s" with no assertion reached. The control-plane prefix is served (those operations pass in other checks), and CreateStream does not refuse an account-local role (the pass-role gate denies only cross-account roleArns), so the run reaches the step that needs behavior the emulator does not have: the published cluster endpoint accepts no PostgreSQL wire connection, so the row is never committed, and even with a connection nothing in the crate ever writes to the named Kinesis stream, so no CDC record would ever arrive. The connect-retry budget (120 s) and the CDC poll (120 s) both exceed the 60 s harness cap, hence the timeout rather than an assertion failure.

Why: a client following the documented DSQL CDC provisioning flow gets a deployment that looks healthy at every control-plane read (cluster ACTIVE with an endpoint, stream ACTIVE and listed) but hangs on connect and delivers no change records, so the whole create-cluster -> create-stream -> change-data -> read-records workflow cannot be run and nothing signals the failure.

Fix: the service needs the data-plane component behind the endpoint the control plane publishes (a PostgreSQL wire listener at the advertised address on 5432 that requires TLS and authenticates a DSQL auth token) and the stream's delivery path into the emulated Kinesis service. Related already-open issues describe the same gaps but do not carry this check's replay key: 20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md (no PostgreSQL listener; it binds nine OTHER checks), 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md (same cause) and 20261024T074208Z-dsql-createstream-event-propagation.md (no Kinesis delivery). This check is additionally the only one that exercises them together with the control-plane deploy/teardown sequence, so it must be re-replayed here.

Traces: dsql.create-stream-lifecycle-to-kinesis — timed out at 60 s inside the data-plane phase. File citations: services/service-com-amazonaws-dsql-dsql/src/lib.rs (module list: ops, scaffolded, service, state — no data-plane listener); services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs lines 341-377 (the stream record is stored and queued to ACTIVE; no delivery machinery).

## Evidence
Cause 1 — dsql.put-cluster-policy-lockout-safety-rejected
Request: CreateCluster(deletionProtectionEnabled=false), wait ACTIVE, then PutClusterPolicy(identifier=<id>, policy={"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"dsql:*","Resource":<cluster arn>}]}) with bypassPolicyLockoutSafetyCheck defaulted to false.
Expected (recorded AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "The new policy would result in the calling principal being unable to further update and delete this cluster's policy. Set the 'bypassPolicyLockoutSafetyCheck' parameter to 'true' to confirm this intent." (all fixed text; no request- or state-derived values).
Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = "The policy would prevent every principal from calling dsql:PutClusterPolicy on this cluster. Set bypassPolicyLockoutSafetyCheck to true to apply it anyway."
Replay output: AssertionError at check_dsql_put_cluster_policy_lockout_safety_rejected.py line 42, asserting lockout_message; the asserted value printed is the emulator sentence above.
Source: services/service-com-amazonaws-dsql-dsql/src/state.rs, `validate_cluster_policy` lockout branch (lines 2170-2180) formats the message from a POLICY_MANAGEMENT_ACTION constant and VALIDATION_REASON_OTHER.

Cause 2 — dsql.create-stream-lifecycle-to-kinesis
Expected (recorded AWS): the workflow runs to completion — cluster ACTIVE with tags; policy attached and read back with the same policyVersion; CreateStream(targetDefinition.kinesis{streamArn, roleArn}, ordering=UNORDERED, format=JSON) CREATING -> ACTIVE and listed; a libpq session on the published endpoint:5432 commits CREATE TABLE + INSERT; a GetRecords on the target Kinesis stream returns a JSON change record carrying the inserted payload; then DeleteStream -> stream absent, DeleteClusterPolicy -> GetClusterPolicy 404 ResourceNotFoundException, DeleteCluster -> GetCluster 404.
Actual (emulator): the check never reaches any of those assertions and is killed by the harness:
  Error: Check timed out after 60.0s
No PostgreSQL wire connection to the advertised endpoint is ever accepted, and no record is ever written to the target Kinesis stream (no reference to the Kinesis emulator anywhere in services/service-com-amazonaws-dsql-dsql/src). CreateStream itself does not refuse the check's account-local roleArn (services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs `is_passable_role`/`cross_account_denial` deny only cross-account ARNs), so the control-plane prefix of the workflow is not what blocks.

## Steps to Reproduce
Cause 1:
1. POST /cluster {"deletionProtectionEnabled": false}; poll GET /cluster/{identifier} until status == "ACTIVE"; note the cluster `arn`.
2. POST /cluster/{identifier}/policy with {"policy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"dsql:*\",\"Resource\":\"<cluster arn>\"}]}"} and no bypassPolicyLockoutSafetyCheck.
3. Observe HTTP 400 ValidationException (correct) whose message is "The policy would prevent every principal from calling dsql:PutClusterPolicy on this cluster. Set bypassPolicyLockoutSafetyCheck to true to apply it anyway." where AWS answers "The new policy would result in the calling principal being unable to further update and delete this cluster's policy. Set the 'bypassPolicyLockoutSafetyCheck' parameter to 'true' to confirm this intent."
4. GET /cluster/{identifier}/policy still 404s (no policy attached) — that part is correct.

Cause 2:
1. Create a Kinesis stream in the emulator and wait for ACTIVE; create an IAM role in the emulator account trusting dsql.amazonaws.com with kinesis:PutRecord(s) on that stream.
2. POST /cluster {"deletionProtectionEnabled": false, "tags": {...}}; poll until ACTIVE; PutClusterPolicy + GetClusterPolicy agree.
3. POST /stream/{clusterIdentifier} with {"targetDefinition": {"kinesis": {"streamArn": <arn>, "roleArn": <arn>}}, "ordering": "UNORDERED", "format": "JSON"}; poll GetStream until ACTIVE.
4. Connect a PostgreSQL client to the cluster's published `endpoint` on port 5432 (dbname postgres, user admin, password = GenerateDbConnectAdminAuthToken, sslmode=require) and run CREATE TABLE / INSERT.
5. The connection is never accepted; the client retries until the check budget expires. Even if it were accepted, GetShardIterator/GetRecords on the target Kinesis stream return no records ever.
6. The teardown assertions (DeleteStream -> absent, DeleteClusterPolicy -> 404 with "Aurora DSQL can't find the policy for the resource <identifier>. Verify that the policy exists.", DeleteCluster -> 404) are never reached.
