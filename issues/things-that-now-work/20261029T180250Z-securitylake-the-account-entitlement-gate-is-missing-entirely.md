Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securitylake.create-aws-log-source-enables-collection-status securitylake.create-data-lake-exception-subscription-duplicate-conflict-rejected securitylake.create-data-lake-reaches-completed-status securitylake.create-subscriber-duplicate-name-conflict-rejected securitylake.create-subscriber-nonexistent-source-not-found-rejected securitylake.create-subscriber-notification-sqs-creates-real-queue securitylake.create-subscriber-s3-access-activates securitylake.delete-aws-log-source-disables-collection securitylake.delete-custom-log-source-removes-glue-table securitylake.delete-data-lake-removes-resource securitylake.get-data-lake-sources-without-data-lake-not-found-rejected securitylake.tag-resource-and-untag-roundtrip securitylake.update-data-lake-lifecycle-configuration-applies securitylake.update-subscriber-sources-reflected
Issue-Shape-Id: com.amazonaws.securitylake#SecurityLake
Issue-Key: securitylake.create-aws-log-source-enables-collection-status|_client_error#5|AssertionError|raise AssertionError(f"real AWS rejects CreateDataLake here, but it SUCCEEDED and returned {result!r}")
Issue-Key: securitylake.create-data-lake-exception-subscription-duplicate-conflict-rejected|_error_code_and_message#7|AssertionError|raise AssertionError("expected ClientError, call unexpectedly succeeded")
Issue-Key: securitylake.create-data-lake-reaches-completed-status|_reject_create_data_lake#5|AssertionError|raise AssertionError("expected CreateDataLake to be rejected, but it succeeded")
Issue-Key: securitylake.create-subscriber-duplicate-name-conflict-rejected|_attempt#10|AssertionError|raise AssertionError(
Issue-Key: securitylake.create-subscriber-nonexistent-source-not-found-rejected|_error_detail#6|AssertionError|raise AssertionError("expected the call to raise a ClientError, but it succeeded")
Issue-Key: securitylake.create-subscriber-notification-sqs-creates-real-queue|_error_code_and_message#4|AssertionError|raise AssertionError(f"expected CreateDataLake to be refused, but it succeeded with {result!r}")
Issue-Key: securitylake.create-subscriber-s3-access-activates|run#5|AssertionError|raise AssertionError(
Issue-Key: securitylake.delete-aws-log-source-disables-collection|run#6|AssertionError|raise AssertionError(
Issue-Key: securitylake.delete-custom-log-source-removes-glue-table|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with: {result!r}")
Issue-Key: securitylake.delete-data-lake-removes-resource|_error_detail#5|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {result!r}")
Issue-Key: securitylake.get-data-lake-sources-without-data-lake-not-found-rejected|run#5|AssertionError|assert code == "UnauthorizedException", f"GetDataLakeSources({kwargs!r}): expected UnauthorizedException, got {code!r}"
Issue-Key: securitylake.tag-resource-and-untag-roundtrip|_attempt_create_data_lake#5|AssertionError|raise AssertionError(
Issue-Key: securitylake.update-data-lake-lifecycle-configuration-applies|_client_error#5|AssertionError|raise AssertionError(f"expected CreateDataLake to be refused, but it SUCCEEDED: {result!r}")
Issue-Key: securitylake.update-subscriber-sources-reflected|run#8|AssertionError|raise AssertionError(
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# securitylake: the account-entitlement gate is missing entirely — CreateDataLake accepts and persists a data lake AWS refuses with AccessDeniedException, and GetDataLakeSources answers 501 NotImplemented instead of UnauthorizedException (2 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataLake

## Description
The emulated Security Lake control plane performs no account-entitlement check at all, and its source-status read path is still a scaffold stub. Two independent root causes, both in the securitylake service crate. ALL 14 securitylake behavioral checks fail on one of them; no open issue exists for this service (list_existing_issues query='securitylake' returns nothing).

=== Cause 1 of 2: CreateDataLake (and the other entitlement-gated control-plane operations) accept a call AWS refuses ===

AWS: Security Lake's control plane is gated on the CALLER ACCOUNT's Security Lake role before any request-level validation. An account that is an AWS Organizations MEMBER and is neither the organization's delegated Security Lake administrator nor a standalone Security Lake account is refused permanently. Smithy (services/service-com-amazonaws-securitylake-securitylake/smithy-model/com-amazonaws-securitylake-securitylake.json): CreateDataLake models AccessDeniedException @httpError 403 — "You do not have sufficient access to perform this action... An implicit denial occurs when there is no applicable Deny statement and also no applicable Allow statement" — alongside BadRequestException/ConflictException/ResourceNotFoundException/ThrottlingException; http POST /v1/datalake, success 200. Recorded behavior supplies the concrete refusal the model does not spell out: HTTP 403, Code `AccessDeniedException`, Message exactly "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account." That message is ENTIRELY FIXED TEXT — it interpolates nothing from the request or from state (no account id, no Region, no role ARN), so it must be reproduced verbatim. The same recorded gate applies to the sibling mutating/reading operations the checks exercise: CreateDataLakeExceptionSubscription and GetDataLakeExceptionSubscription also answer AccessDeniedException with that identical fixed text, while DeleteDataLake answers Code `UnauthorizedException` with Message "Unauthorized" (recorded behavior only — UnauthorizedException is not in this service's modeled error set or common_errors, which are AccessDeniedException/BadRequestException/ConflictException/InternalServerException/ResourceNotFoundException/ThrottlingException). ListDataLakes is NOT gated: it succeeds and returns an empty `dataLakes` list, which the checks use as a positive control.

Emulator: CreateDataLake returns HTTP 200 with a fully minted resource — `{'dataLakes': [{'createStatus': 'INITIALIZED', 'dataLakeArn': 'arn:aws:securitylake:<region>:<account>:data-lake/default', 'region': '<region>', 's3BucketArn': 'arn:aws:s3:::aws-security-data-lake-<region>-<suffix>'}]}` — and PERSISTS it, so the subsequent effect-absent read (ListDataLakes) would also diverge. The handler in src/ops/create_data_lake.rs goes straight from state lookup to bounds checks (MAX_DATA_LAKES) and then upserts; there is no caller-entitlement branch anywhere before the write. No modeled error is ever produced for an ineligible account.

Why: a client relying on AWS's behavior sees the opposite verdict for every negative control on this service. Code that handles the delegated-administrator/standalone refusal (surfacing the onboarding error, falling back to a different account, aborting a provisioning run) never runs; worse, the emulator reports a data lake that AWS never created, so test suites and provisioning tooling conclude Security Lake is enabled for an account that cannot have it. An accepted mutating call AWS refuses is the strongest possible fidelity divergence: the checks' own harness refuses to launder it into an error-code mismatch.

Fix: the CreateDataLake handler's admission/authorization stage — services/service-com-amazonaws-securitylake-securitylake/src/ops/create_data_lake.rs, async fn handle (the gate belongs BEFORE the MAX_DATA_LAKES bound checks and before any store mutation, so a refused request half-applies nothing), with the caller's Security Lake role carried in the service state (src/state.rs) so the gate is consistent across operations. The same gate must be applied in src/ops/create_data_lake_exception_subscription.rs, src/ops/get_data_lake_exception_subscription.rs (AccessDeniedException + the fixed entitlement text) and src/ops/delete_data_lake.rs (UnauthorizedException + "Unauthorized"), and must NOT be applied to ListDataLakes.

Traces: securitylake.create-aws-log-source-enables-collection-status, securitylake.create-data-lake-exception-subscription-duplicate-conflict-rejected, securitylake.create-data-lake-reaches-completed-status, securitylake.create-subscriber-duplicate-name-conflict-rejected, securitylake.create-subscriber-nonexistent-source-not-found-rejected, securitylake.create-subscriber-notification-sqs-creates-real-queue, securitylake.create-subscriber-s3-access-activates, securitylake.delete-aws-log-source-disables-collection, securitylake.delete-custom-log-source-removes-glue-table, securitylake.delete-data-lake-removes-resource, securitylake.tag-resource-and-untag-roundtrip, securitylake.update-data-lake-lifecycle-configuration-applies, securitylake.update-subscriber-sources-reflected (13 checks).

=== Cause 2 of 2: GetDataLakeSources is an unimplemented scaffold stub, so its refusal is unreachable ===

AWS: GetDataLakeSources is POST /v1/datalake/sources, success 200 with `dataLakeArn`/`dataLakeSources`/`nextToken`, paginated (inputToken/outputToken nextToken, pageSize maxResults, items dataLakeSources); modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429. For an account that is not a delegated Security Lake administrator or standalone account, recorded behavior shows this read refusing with Code `UnauthorizedException` and Message "Unauthorized" — an all-fixed-text message with no interpolated values — and NOT with ResourceNotFoundException, both with no `accounts` member and with `accounts=[<caller account id>]`. UnauthorizedException is recorded behavior only; it is absent from the Smithy model's error list for this operation and from the service's common_errors.

Emulator: the request is answered with error code `NotImplemented` (the scaffold's 501 channel). src/ops/get_data_lake_sources.rs still carries the generated stub body — `let _ = (&input, ctx, &self.engine);` followed by the "Implement GetDataLakeSources here, then delete the SCAFFOLDED marker" comment — so no response, no pagination and no refusal of any kind is produced.

Why: every negative and positive contract on the data-lake source-status surface is unreachable. A client cannot distinguish "not entitled" (UnauthorizedException) from "entitled but no data lake yet" (ResourceNotFoundException), which is precisely the branch AWS makes observable here; and generic SDK retry/error handling treats an unmodeled 501 NotImplemented as a transport-level failure rather than a client error, so callers retry a call that can never succeed.

Fix: implement the GetDataLakeSources handler — services/service-com-amazonaws-securitylake-securitylake/src/ops/get_data_lake_sources.rs, async fn handle — reading the data-lake/source state from src/state.rs, and route the ineligible-account case through the same shared entitlement gate introduced for Cause 1 (answering UnauthorizedException/"Unauthorized" rather than AccessDeniedException, which is the divergence from CreateDataLake that these checks pin).

Traces: securitylake.get-data-lake-sources-without-data-lake-not-found-rejected (fails directly on this stub: "GetDataLakeSources({}): expected UnauthorizedException, got 'NotImplemented'"). securitylake.create-data-lake-exception-subscription-duplicate-conflict-rejected and securitylake.delete-data-lake-removes-resource assert the same UnauthorizedException/"Unauthorized" pair on this operation but currently fail earlier, on Cause 1, so they will reach this stub once Cause 1 is fixed.

## Evidence
Suite: securitylake 0/14 (0%), 14 mismatched, 0 errored, 0 skipped. No check was marked ALREADY FILED or OWNED; no open issue mentions securitylake.

Cause 1 — expected (real AWS, recorded): HTTP 403, Error.Code = "AccessDeniedException", Error.Message = "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account." (fixed text, no interpolation), and ListDataLakes still returning dataLakes == [].
Actual (emulator): HTTP 200 with
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'dataLakes': [{'createStatus': 'INITIALIZED', 'dataLakeArn': 'arn:aws:securitylake:us-west-2:000000000000:data-lake/default', 'region': 'us-west-2', 's3BucketArn': 'arn:aws:s3:::aws-security-data-lake-us-west-2-<suffix>'}]}
(the account id, Region and bucket suffix are per-environment values, not part of the requirement; the divergence is the 200-with-resource instead of the 403 refusal, and the persisted data lake).
Harness assertions raised:
  - "real AWS rejects CreateDataLake here, but it SUCCEEDED and returned {...}"
  - "expected CreateDataLake to be rejected, but it succeeded"
  - "real AWS accepted CreateDataLake in a non-delegated, non-standalone member account; expected the delegated-administrator refusal"
  - "expected CreateDataLake to be denied for a non-delegated AWS Organizations member account, but it succeeded"
  - "expected CreateDataLake to be denied for a non-administrator, non-standalone account, but it succeeded"
  - "CreateDataLake unexpectedly succeeded; this account is no longer topology-restricted..."
The checks assert, after that refusal, `code == "AccessDeniedException"` and `message == _ENTITLEMENT_MESSAGE`, plus (unreached) DeleteDataLake -> ("UnauthorizedException", "Unauthorized"), CreateDataLakeExceptionSubscription -> ("AccessDeniedException", entitlement message), GetDataLakeExceptionSubscription -> ("AccessDeniedException", entitlement message). ListDataLakes succeeds as the positive control in every check, so the service is otherwise reachable.

Cause 2 — expected: Error.Code = "UnauthorizedException", Error.Message = "Unauthorized" for both GetDataLakeSources({}) and GetDataLakeSources({'accounts': [<caller account id>]}).
Actual: Error.Code = 'NotImplemented'.
Assertion raised: "GetDataLakeSources({}): expected UnauthorizedException, got 'NotImplemented'".
Source confirmation: services/service-com-amazonaws-securitylake-securitylake/src/ops/get_data_lake_sources.rs lines ~174-184 are the unmodified scaffold stub body; services/service-com-amazonaws-securitylake-securitylake/src/ops/create_data_lake.rs lines 170-305 are implemented but contain no caller-entitlement branch.

## Steps to Reproduce
1. Boot the emulator and point botocore at the securitylake endpoint with an account configured as a plain AWS Organizations member (neither delegated Security Lake administrator nor standalone).
2. Cause 1: call ListDataLakes(regions=[<region>]) — it returns dataLakes == [] (positive control). Then call CreateDataLake(configurations=[{"region": <region>}], metaStoreManagerRoleArn=<any ARN matching ^arn:aws:iam::\d{12}:role/...>). Expect HTTP 403 AccessDeniedException with the message "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account."; observe HTTP 200 and a dataLakes entry with createStatus INITIALIZED. Call ListDataLakes again and observe the persisted data lake that AWS would never have created.
3. Cause 1, sibling gates: call DeleteDataLake(regions=[<region>]) and expect UnauthorizedException/"Unauthorized"; call CreateDataLakeExceptionSubscription(...) and GetDataLakeExceptionSubscription() and expect AccessDeniedException with the same entitlement message.
4. Cause 2: call GetDataLakeSources() and GetDataLakeSources(accounts=[<caller account id>]). Expect UnauthorizedException/"Unauthorized"; observe error code NotImplemented (501 scaffold channel).
5. Replay: run_behavioral_suite and re-run the 14 check ids listed in trace_ids.
