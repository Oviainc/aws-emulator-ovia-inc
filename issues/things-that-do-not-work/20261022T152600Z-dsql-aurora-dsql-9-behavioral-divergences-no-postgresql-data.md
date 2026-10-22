Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.get-cluster-not-found dsql.delete-cluster-lifecycle-to-gone dsql.delete-cluster-deleting-to-deleted-transition dsql.get-cluster-invalid-identifier-pattern-rejected dsql.get-cluster-malformed-and-missing-identifier-rejected dsql.list-clusters-max-results-range-rejected dsql.create-cluster-client-token-idempotency-and-conflict-rejected dsql.create-cluster-token-reuse-conflict-rejected dsql.update-cluster-client-token-reuse-rejected dsql.delete-protected-cluster-rejected dsql.delete-cluster-deletion-protection-conflict-rejected dsql.tag-resource-quota-exceeded-rejected dsql.cluster-policy-lifecycle dsql.put-cluster-policy-lockout-safety-rejected dsql.create-stream-invalid-target-arn-rejected dsql.native-sql-round-trip-create-insert-select dsql.connection-without-tls-or-admin-token-rejected dsql.expired-auth-token-rejected-open-session-survives dsql.dsql-occ-write-conflict-rejects-the-losing-commit dsql.locking-read-conflicts-rejected-at-commit dsql.transaction-constraints-rejected dsql.unsupported-postgres-statements-rejected dsql.sequence-and-identity-cache-rule-rejected dsql.iam-mapped-database-role-privileges-denied
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.get-cluster-not-found|run#14|AssertionError|assert message == (
Issue-Key: dsql.delete-cluster-lifecycle-to-gone|run#12|AssertionError|assert redelete_message == (
Issue-Key: dsql.delete-cluster-deleting-to-deleted-transition|run#14|AssertionError|assert message == (
Issue-Key: dsql.get-cluster-invalid-identifier-pattern-rejected|run#4|AssertionError|assert malformed_message == (
Issue-Key: dsql.get-cluster-malformed-and-missing-identifier-rejected|run#4|AssertionError|assert malformed_message == "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a
Issue-Key: dsql.list-clusters-max-results-range-rejected|run#5|AssertionError|assert code_zero_message == (
Issue-Key: dsql.create-cluster-client-token-idempotency-and-conflict-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: dsql.create-cluster-token-reuse-conflict-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: dsql|UpdateCluster|ConflictException
Issue-Key: dsql.delete-protected-cluster-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: dsql.delete-cluster-deletion-protection-conflict-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: dsql.tag-resource-quota-exceeded-rejected|run#8|AssertionError|assert quota_code == "BadRequestException", (
Issue-Key: dsql.cluster-policy-lifecycle|run#10|AssertionError|assert no_policy_code == "ResourceNotFoundException", (
Issue-Key: dsql.put-cluster-policy-lockout-safety-rejected|run#9|AssertionError|assert lockout_code == "ValidationException", (
Issue-Key: dsql.create-stream-invalid-target-arn-rejected|run#8|AssertionError|assert code == "AccessDeniedException", code
Issue-Key: dsql.native-sql-round-trip-create-insert-select|_connect_with_retry#12|Timeout|time.sleep(interval_s)
Issue-Key: dsql.connection-without-tls-or-admin-token-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.expired-auth-token-rejected-open-session-survives|_connect_ready#4|Timeout|time.sleep(5)
Issue-Key: dsql.dsql-occ-write-conflict-rejects-the-losing-commit|_connect_with_retry#7|Timeout|time.sleep(interval_s)
Issue-Key: dsql.locking-read-conflicts-rejected-at-commit|_connect_ready#6|Timeout|time.sleep(5)
Issue-Key: dsql.transaction-constraints-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.unsupported-postgres-statements-rejected|_connect_ready#5|Timeout|time.sleep(5)
Issue-Key: dsql.sequence-and-identity-cache-rule-rejected|_connect_ready#6|Timeout|time.sleep(5)
Issue-Key: dsql.iam-mapped-database-role-privileges-denied|_connect_ready#13|Timeout|time.sleep(5)
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Aurora DSQL: 9 behavioral divergences — no PostgreSQL data plane, clientToken idempotency ignored on CreateCluster/UpdateCluster, deletion-protection and tag-quota refusals misclassified, non-AWS not-found/validation message constructions, cluster-policy semantics missing

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster, DeleteCluster, CreateCluster, UpdateCluster, ListClusters, TagResource, PutClusterPolicy, GetClusterPolicy, DeleteClusterPolicy, CreateStream

## Description
Nine distinct root causes in the emulated Aurora DSQL service, found by replaying the behavioral suite (9/49 checks pass). All nine are in service `dsql`; each is numbered with its own failing check ids. Causes 7 and 8 touch operations that also have their own scaffold-stub issues filed by the replay harness (PutClusterPolicy / GetClusterPolicy / CreateStream "unimplemented scaffold stub"); those issues only ask for the operation to exist — the semantics below are what the implementation must satisfy and are NOT covered by them.

=== Cause 1 of 9: ResourceNotFoundException for a gone cluster uses a non-AWS message construction ===
AWS: GetCluster/DeleteCluster against an identifier whose cluster has been fully deleted answer 404 ResourceNotFoundException (Smithy: GetCluster/DeleteCluster errors include ResourceNotFoundException, httpError 404). Recorded behavior shows the message is built from the request's resource ARN: "The resource with ARN <cluster arn> doesn't exist. Verify the ARN and try again." — i.e. it names the ARN of the resource the call addressed.
Emulator: the code is right but the message is a bespoke construction naming the bare identifier, e.g. "cluster 01m5j2t9ne1n33yrsjs59wqhpw not found" — no ARN, no AWS sentence.
Why: clients and tooling that parse/surface DSQL not-found messages (and tests that assert the ARN is echoed, which is how a caller learns WHICH resource was missing) see a message that neither names the ARN nor matches AWS's wording.
Fix: the not-found error construction shared by the GetCluster/DeleteCluster handlers (and anything else raising ResourceNotFoundException for a cluster) must render "The resource with ARN {arn} doesn't exist. Verify the ARN and try again." from the addressed cluster's ARN.
Traces: dsql.get-cluster-not-found, dsql.delete-cluster-lifecycle-to-gone, dsql.delete-cluster-deleting-to-deleted-transition.

=== Cause 2 of 9: ValidationException constraint messages echo the offending value and the anchored regex ===
AWS: constraint violations are 400 ValidationException (Smithy: service-common error ValidationException, httpError 400) with the standard AWS validation sentence. Recorded behavior shows DSQL's construction omits the offending value and quotes the pattern unanchored: "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{26}" and "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" / "... less than or equal to 100" (Smithy: GetCluster.identifier pattern ^[a-z0-9]{26}$; ListClusters.maxResults @range min 1 max 100).
Emulator: the code and status are right, but the message interpolates the rejected value and the anchors: "1 validation error detected: Value 'TOO-SHORT-AND-UPPERCASE' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{26}$" and "Value '0' at 'maxResults' failed to satisfy constraint: ...".
Why: the rule producing the message is known (it is built mechanically from the member and its modeled constraint), so the construction is part of the contract; here it also echoes caller-supplied input back into the error text, which AWS deliberately does not do for these members.
Fix: the request-validation layer's message formatter for pattern and range violations (shared by all dsql operations) — drop the value and emit the pattern as modeled minus the ^...$ anchors.
Traces: dsql.get-cluster-invalid-identifier-pattern-rejected, dsql.get-cluster-malformed-and-missing-identifier-rejected, dsql.list-clusters-max-results-range-rejected.

=== Cause 3 of 9: CreateCluster ignores clientToken — token reuse with different parameters creates a second cluster ===
AWS: CreateCluster.clientToken is an @idempotencyToken (Smithy: CreateClusterInput.clientToken, idempotencyToken, length 1-128, pattern ^[!-~]+$). Replaying the same token with the same parameters is an idempotent no-op returning the original cluster; reusing the token with DIFFERENT parameters is refused with 409 ConflictException (Smithy: CreateCluster errors include ConflictException, httpError 409), recorded message example "Requests using a client token should supply identical request parameters", and no second cluster is created.
Emulator: the second call is accepted with HTTP 200 and mints a brand-new cluster (new identifier/arn, status CREATING), i.e. the token is not recorded at all.
Why: a retrying client (SDK retries, Step Functions, Terraform) silently provisions duplicate clusters instead of getting back the first one or a conflict — the exact failure idempotency tokens exist to prevent.
Fix: CreateCluster's handler/state must persist clientToken -> (request parameters, created cluster) and compare on replay.
Traces: dsql.create-cluster-client-token-idempotency-and-conflict-rejected, dsql.create-cluster-token-reuse-conflict-rejected.

=== Cause 4 of 9: UpdateCluster ignores clientToken and refuses replays because the cluster is UPDATING ===
AWS: UpdateCluster.clientToken is an @idempotencyToken (Smithy: UpdateClusterInput.clientToken, idempotencyToken). Replaying the same token with the same parameters is treated as the same request and answers 200 with the same identifier/arn; reusing the token with different parameters is 409 ConflictException (recorded message example "Requests using a client token should supply identical request parameters") and leaves the first request's setting in place.
Emulator: the immediate replay of the identical token+parameters is refused with 409 ConflictException and the message "cluster <id> cannot be updated while its status is UPDATING" — the token is never consulted; refusal is driven only by cluster status, so the idempotent replay cannot succeed and the token-reuse case is never distinguished.
Why: any client that retries UpdateCluster (or calls it twice through a workflow) gets a hard ConflictException instead of the idempotent success AWS gives, and a genuine parameter-mismatch reuse is indistinguishable from a transient status conflict.
Fix: UpdateCluster's handler/state — record the token with its request parameters and short-circuit a matching replay before the status gate.
Traces: dsql.update-cluster-client-token-reuse-rejected.

=== Cause 5 of 9: DeleteCluster on a deletion-protected cluster answers ConflictException instead of ValidationException ===
AWS: DeleteCluster against a cluster with deletionProtectionEnabled=true is refused as an input-constraint violation, 400 ValidationException (Smithy: DeleteCluster errors include both ValidationException httpError 400 and ConflictException httpError 409; recorded behavior selects ValidationException), recorded message example "Can't delete cluster: Deletion protection is enabled. To proceed, disable deletion protection and try again." The cluster is untouched, and the identical call succeeds once protection is disabled.
Emulator: answers ConflictException (409) for the protected cluster.
Why: callers branch on the error code — a 409 ConflictException is normally retryable/transient ("try again later"), while this condition is permanent until the caller changes the cluster, so a retry loop keyed on the code spins instead of surfacing the configuration error.
Fix: the deletion-protection guard in DeleteCluster's handler must raise ValidationException, not ConflictException.
Traces: dsql.delete-protected-cluster-rejected, dsql.delete-cluster-deletion-protection-conflict-rejected.

=== Cause 6 of 9: TagResource over the 200-tag limit answers ValidationException instead of BadRequestException ===
AWS: TagMap is @length max 200 (Smithy: TagResourceInput.tags -> TagMap, length min 0 max 200; TagResource also models ServiceQuotaExceededException httpError 402). Recorded behavior for a 201-entry TagResource request is error code BadRequestException with message "Invalid request body", and none of the oversized batch's tags are persisted.
Emulator: refuses with ValidationException instead of BadRequestException (the rejection itself happens, only the classification diverges).
Why: the error code is the contract a client switches on; a caller catching DSQL's BadRequestException for oversized tag batches never sees it and the exception escapes as an unmodeled ValidationException.
Fix: the tag-count guard in TagResource's validation path must emit BadRequestException (recorded message example "Invalid request body").
Traces: dsql.tag-resource-quota-exceeded-rejected.

=== Cause 7 of 9: cluster resource-policy semantics absent — policy operations answer 501 NotImplemented ===
AWS: GetClusterPolicy on a cluster with NO policy attached is 404 ResourceNotFoundException (Smithy: GetClusterPolicy errors include ResourceNotFoundException httpError 404), recorded message example "Aurora DSQL can't find the policy for the resource <identifier>. Verify that the policy exists."; PutClusterPolicy attaches a document and returns a required policyVersion token (Smithy: PutClusterPolicyOutput.policyVersion required), GetClusterPolicy then returns that same document and version; DeleteClusterPolicy with expectedPolicyVersion removes it so GetClusterPolicy reverts to ResourceNotFoundException and a second DeleteClusterPolicy on the gone policy is also ResourceNotFoundException (recorded message example "Aurora DSQL can't find policy version <v> for resource <identifier>. Verify that the policy exists and that the version is correct."). PutClusterPolicy with a policy that denies every principal every dsql action, while bypassPolicyLockoutSafetyCheck defaults to false (Smithy: PutClusterPolicyInput.bypassPolicyLockoutSafetyCheck default false), is refused 400 ValidationException and attaches nothing.
Emulator: GetClusterPolicy and PutClusterPolicy answer error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator"), so none of the above holds — no lockout-safety check, no version tokens, no not-found-policy classification.
Why: resource-policy-based access control on a DSQL cluster cannot be exercised at all, and a caller probing "is a policy attached?" gets NotImplemented rather than the 404 it branches on.
Fix: implement the policy state and semantics in the PutClusterPolicy / GetClusterPolicy / DeleteClusterPolicy handlers (services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs, get_cluster_policy.rs). The separately filed "unimplemented scaffold stub" issues for PutClusterPolicy and GetClusterPolicy cover only the operations' existence; the version-token, not-found and lockout-safety behavior above is this cause.
Traces: dsql.cluster-policy-lifecycle, dsql.put-cluster-policy-lockout-safety-rejected.

=== Cause 8 of 9: CreateStream with a cross-account roleArn is refused ValidationException instead of AccessDeniedException ===
AWS: CreateStream whose targetDefinition carries a roleArn in a foreign/nonexistent account cannot have its trust relationship established, and is refused 403 AccessDeniedException (Smithy: AccessDeniedException is a service-common error, httpError 403; CreateStream also models ValidationException 400), recorded message example "Cross-account pass role is not allowed." — not a shape-only ValidationException.
Emulator: answers ValidationException for the cross-account roleArn.
Why: the caller cannot distinguish "your request is malformed" from "you may not pass that role", and authorization-failure handling (which AWS marks with the 403 family) never triggers.
Fix: the roleArn/pass-role check in CreateStream's handler (services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs) must classify a foreign-account role as AccessDeniedException. The separately filed CreateStream scaffold-stub issue covers only standing the operation up.
Traces: dsql.create-stream-invalid-target-arn-rejected.

=== Cause 9 of 9: the published cluster endpoint serves no PostgreSQL data plane — every SQL session check hangs ===
AWS: an ACTIVE cluster publishes an endpoint (Smithy: GetClusterOutput.endpoint / CreateClusterOutput.endpoint) that accepts PostgreSQL wire connections on port 5432, mandates TLS, and authenticates the password as a DbConnectAdmin/DbConnect auth token; a valid admin token over TLS opens a session where SQL (DDL, DML, SELECT, transactions, sequences) executes and the documented refusals are raised as PostgreSQL FATAL lines (recorded examples: "unable to accept connection, SSL is mandatory. AWS Authentication is required.", "unable to accept connection, access denied").
Emulator: the control plane reports the cluster ACTIVE and hands out an endpoint, but nothing accepts a PostgreSQL connection there — every data-plane check never completes a session and is killed at the 60s check timeout. Nine checks fail this way, so no SQL behavior (OCC write conflicts, locking read conflicts, transaction constraints, unsupported-statement rejection, sequence/identity cache rule, IAM-mapped role privileges, token expiry) is observable at all.
Why: the emulator advertises a usable endpoint it does not serve, so any client that follows the documented flow (CreateCluster -> wait ACTIVE -> generate auth token -> psycopg/libpq connect) hangs instead of either connecting or being refused.
Fix: the data-plane listener for the published cluster endpoint (PostgreSQL wire protocol, TLS and auth-token validation) plus the SQL execution behind it; failing that, the endpoint it publishes must at least refuse with the documented FATAL lines rather than never answering.
Traces: dsql.native-sql-round-trip-create-insert-select, dsql.connection-without-tls-or-admin-token-rejected, dsql.expired-auth-token-rejected-open-session-survives, dsql.dsql-occ-write-conflict-rejects-the-losing-commit, dsql.locking-read-conflicts-rejected-at-commit, dsql.transaction-constraints-rejected, dsql.unsupported-postgres-statements-rejected, dsql.sequence-and-identity-cache-rule-rejected, dsql.iam-mapped-database-role-privileges-denied.

## Evidence
Suite: 9/49 checks pass (18.4%); dsql 9/49, 40 failed. 16 of the failures are blocked by already-filed scaffold-stub issues (PutClusterPolicy x6, ListTagsForResource x3, TagResource x2, GetVpcEndpointServiceName x2, GetClusterPolicy, CreateStream, UntagResource) and are not re-filed here.

Cause 1 — expected message "The resource with ARN arn:aws:dsql:us-west-2:<acct>:cluster/<id> doesn't exist. Verify the ARN and try again."; emulator: AssertionError: cluster 01m5j2t9ne1n33yrsjs59wqhpw not found (same for 01m5j2hydh0che4sdp8cg0b32r, 01m5j37vb1atg2bca123s5sdb2).
Cause 2 — expected "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{26}"; emulator: "1 validation error detected: Value 'not-a-valid-identifier!!' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{26}$" (and "Value 'TOO-SHORT-AND-UPPERCASE' at 'identifier' ..."). Expected "Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"; emulator: "Value '0' at 'maxResults' failed to satisfy constraint: ...".
Cause 3 — expected ConflictException (409) "Requests using a client token should supply identical request parameters"; emulator: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'identifier': '01m5j2xggdw430s22hfz1tsvmf', 'arn': 'arn:aws:dsql:us-west-2:000000000000:cluster/01m5j2xggdw430s22hfz1tsvmf', 'status': 'CREATING', ...} — a NEW cluster.
Cause 4 — replay of the identical UpdateCluster token+params: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the UpdateCluster operation: cluster 01m5j2v73d1d9z3xk95ecqrwf8 cannot be updated while its status is UPDATING; expected 200 with the same identifier/arn.
Cause 5 — expected ValidationException (400) "Can't delete cluster: Deletion protection is enabled. To proceed, disable deletion protection and try again."; emulator: AssertionError: expected ValidationException, got 'ConflictException'.
Cause 6 — expected BadRequestException ("Invalid request body") for a 201-tag TagResource; emulator: AssertionError: ... was expected to fail with BadRequestException, got 'ValidationException'.
Cause 7 — expected ResourceNotFoundException (404) from GetClusterPolicy on a policy-less cluster; emulator: AssertionError: ... got 'NotImplemented'. Expected ValidationException for the all-principals-Deny policy; emulator: AssertionError: expected ValidationException for a policy locking out every principal, got 'NotImplemented' (PutClusterPolicy/GetClusterPolicy answer "operation ... is scaffolded but not implemented in this emulator").
Cause 8 — expected AccessDeniedException (403) "Cross-account pass role is not allowed."; emulator: AssertionError: ValidationException.
Cause 9 — all nine dp:native / plane:data checks report "Error: Check timed out after 60.0s"; each connects with psycopg to ctx.endpoint_host(<cluster endpoint>, 5432) after GetCluster reported ACTIVE and published that endpoint. psycopg is importable in the runner (the checks loaded and their control-plane setup ran), so the stall is the endpoint never completing a PostgreSQL session.

## Steps to Reproduce
Boot the emulator and run the behavioral suite for service dsql (`run_behavioral_suite`), or exercise the causes directly with botocore/psycopg against the emulator endpoint:
1. Cause 1: create_cluster(deletionProtectionEnabled=False) -> delete_cluster -> poll get_cluster until ResourceNotFoundException -> inspect Error.Message (expect the "resource with ARN ... doesn't exist" sentence); then delete_cluster again and inspect its message.
2. Cause 2: get_cluster(identifier="not-a-valid-identifier!!") and, with Config(parameter_validation=False), list_clusters(maxResults=0) and list_clusters(maxResults=101); compare Error.Message to AWS's construction.
3. Cause 3: t=uuid4(); create_cluster(clientToken=t, deletionProtectionEnabled=False, tags={"a":"1"}); create_cluster(clientToken=t, deletionProtectionEnabled=False, tags={"a":"2"}) -> expect 409 ConflictException, observe a second cluster created.
4. Cause 4: create a cluster, wait ACTIVE, t=uuid4(); update_cluster(identifier=id, deletionProtectionEnabled=True, clientToken=t) twice -> expect the second to be an idempotent 200, observe 409 "cannot be updated while its status is UPDATING"; then update_cluster with the same t and deletionProtectionEnabled=False -> expect 409 with the client-token message.
5. Cause 5: create_cluster(deletionProtectionEnabled=True), wait ACTIVE, delete_cluster -> expect ValidationException, observe ConflictException.
6. Cause 6: create a cluster, tag_resource(resourceArn=arn, tags={201 entries}) -> expect BadRequestException, observe ValidationException.
7. Cause 7: create a cluster, get_cluster_policy(identifier=id) -> expect 404 ResourceNotFoundException; put_cluster_policy with a {"Effect":"Deny","Principal":"*","Action":"dsql:*"} document -> expect 400 ValidationException; both answer NotImplemented.
8. Cause 8: create_stream(clusterIdentifier=id, targetDefinition with a kinesis roleArn in a foreign account, ordering/format set) -> expect 403 AccessDeniedException, observe ValidationException.
9. Cause 9: create_cluster, wait ACTIVE, generate_db_connect_admin_auth_token(Hostname=endpoint), then psycopg.connect(host=<mapped endpoint host>, port=5432, dbname="postgres", user="admin", password=token, sslmode="require") and run "SELECT current_user, 1+1" -> expect ('admin', 2); observe the connection never completing.
