Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.enforce-workgroup-configuration-overrides-client-result-config athena.workgroup-result-configuration-enforced athena.op-contract-update-workgroup-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.enforce-workgroup-configuration-overrides-client-result-config|run#27|AssertionError|assert _object_count(s3, client_bucket, "clientchosen/") > 0, (
Issue-Key: athena.workgroup-result-configuration-enforced|run#32|AssertionError|assert landed, (
Issue-Key: athena.op-contract-update-workgroup-nonexistent-rejected|run#5|AssertionError|assert message == "WorkGroup is not found.", message
Issue-Commit: 1cef0270414d463e8e4cd5e638402f2f0968b2f1

# Athena never writes the query-result object to a client-supplied OutputLocation, and the WorkGroup not-found message interpolates the workgroup name (2 causes)

> _Note: this issue was identified against the code base at commit `1cef0270414d463e8e4cd5e638402f2f0968b2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Two net-new root causes in the Athena emulator, both reached through the workgroup/result-configuration path. 2 causes.

Cause 1 of 2: a SUCCEEDED query writes no result object under the client-supplied ResultConfiguration.OutputLocation when EnforceWorkGroupConfiguration is false.
AWS: StartQueryExecution takes an optional ResultConfiguration (Smithy: StartQueryExecution.ResultConfiguration -> ResultConfiguration, with OutputLocation; StartQueryExecution.WorkGroup -> WorkGroupName, pattern ^[a-zA-Z0-9._-]{1,128}$). When the referenced workgroup has Configuration.EnforceWorkGroupConfiguration=false, the client's OutputLocation wins, and once GetQueryExecution reports Status.State=SUCCEEDED, Athena has materialised the result bytes under that prefix: the object key is derived from the QueryExecutionId (`<client prefix>/<QueryExecutionId>.csv`, plus its `.csv.metadata` sibling), so a ListObjectsV2 of the client prefix is non-empty. The prefix, the bucket and the QueryExecutionId are values from the request and from the emulator's state; the requirement is that SOME result object exists under the resolved prefix after the execution reaches SUCCEEDED, not any one key.
Emulator: GetQueryExecution reports Status.State=SUCCEEDED and ResultConfiguration.OutputLocation correctly echoes the client prefix (both checks get past that assertion), but ListObjectsV2 on that prefix returns no Contents at all. The replay fails on `assert landed` / `assert _object_count(s3, client_bucket, "clientchosen/") > 0` with AssertionError: 'with enforcement off the query results must actually appear under the client prefix'. No object is written anywhere for that execution.
Why: a client that reads Athena output from S3 instead of through GetQueryResults — an ETL step, a manifest consumer, a CTAS/UNLOAD downstream, a crawler pointed at the result prefix — sees an empty location even though the API reported SUCCEEDED and named that location. The two halves of the EnforceWorkGroupConfiguration contract also become indistinguishable: nothing lands under either prefix, so a tenant cannot tell enforcement on from enforcement off.
Fix: the athena crate's query-execution result writer (it must materialise the CSV result object and its metadata sibling in the S3 store at the resolved output location) together with the output-location resolution in the StartQueryExecution handler on the enforcement-OFF branch, where the client-supplied ResultConfiguration.OutputLocation must be the resolved prefix. Related open issue 20261104T163600Z-athena-writes-no-query-result-object-to-s3.md describes the same writer gap for the workgroup-default and enforcement-on branches; the suite did not mark these two checks as owned by it, so they are filed here — a fix for that issue that does not also write bytes on the enforcement-off branch leaves these two checks failing.
Traces: athena.enforce-workgroup-configuration-overrides-client-result-config, athena.workgroup-result-configuration-enforced.

Cause 2 of 2: the InvalidRequestException message for an unknown workgroup interpolates the requested name instead of using AWS's fixed text.
AWS: UpdateWorkGroup (and GetWorkGroup) on a WorkGroup name that does not exist is refused with HTTP 400 and error code InvalidRequestException (Smithy: UpdateWorkGroup errors = InternalServerException, InvalidRequestException; InvalidRequestException is a client error, httpError 400). The message is entirely FIXED text with no variable part: `WorkGroup is not found.` — the requested workgroup name is NOT part of the message.
Emulator: the call is refused with the right code (InvalidRequestException) but the message is `WorkGroup <name> is not found.`, i.e. the requested WorkGroup name from the request is interpolated into the middle of the sentence. Recorded replay: AssertionError: WorkGroup sim2real-behav-wg-missing-0f14d17f41fd is not found. (the hex suffix is the per-run unique name from the request and is not part of the requirement; the defect is the presence of ANY name between 'WorkGroup' and 'is not found.').
Why: Athena only models InvalidRequestException for both 'not found' and genuine validation failures, so clients and the AWS CLI/SDK error-handling code commonly discriminate on the message text. An exact-match comparison against 'WorkGroup is not found.' fails, so a caller's not-found branch never fires and the condition is surfaced as an unclassified request error.
Fix: the UpdateWorkGroup handler's not-found refusal path (and the GetWorkGroup path that shares the same message, asserted immediately afterwards in the same check) in the athena crate — emit the fixed message 'WorkGroup is not found.' with no interpolation.
Traces: athena.op-contract-update-workgroup-nonexistent-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. 48/419 passed; these are the only three failing checks not already marked ALREADY FILED or OWNED.

Cause 1:
- [mismatch] athena.enforce-workgroup-configuration-overrides-client-result-config — check_athena_enforce_workgroup_configuration_overrides_client_result_config.py line 154: `assert _object_count(s3, client_bucket, "clientchosen/") > 0` -> AssertionError: 'with enforcement off the query results must actually appear under the client prefix'. Setup: CreateWorkGroup(Configuration.ResultConfiguration.OutputLocation='s3://<enforced-bucket>/enforced/', EnforceWorkGroupConfiguration=True), then UpdateWorkGroup(ConfigurationUpdates={EnforceWorkGroupConfiguration: False}), then StartQueryExecution(QueryString='SELECT 3 AS a', WorkGroup=<wg>, ResultConfiguration.OutputLocation='s3://<client-bucket>/clientchosen/'). GetQueryExecution reached SUCCEEDED and ResultConfiguration.OutputLocation started with the client prefix (that assertion passed); the S3 listing of the client prefix was empty.
- [mismatch] athena.workgroup-result-configuration-enforced — check_athena_workgroup_result_configuration_enforced.py line 86: `assert landed` -> AssertionError: 'with enforcement off the query results must actually appear under the client prefix'. Same shape, single bucket with prefixes 'wgout/' and 'clientout/', query 'SELECT 9 AS n'; list_objects_v2(Bucket=<bucket>, Prefix='clientout/') returned no Contents.
Expected: non-empty listing under the resolved (client) prefix once Status.State=SUCCEEDED. Actual: zero objects.

Cause 2:
- [mismatch] athena.op-contract-update-workgroup-nonexistent-rejected — check_athena_op_contract_update_workgroup_nonexistent_rejected.py line 27: `assert message == "WorkGroup is not found."` -> AssertionError: WorkGroup sim2real-behav-wg-missing-0f14d17f41fd is not found.
Expected: HTTP 400, Error.Code='InvalidRequestException', Error.Message='WorkGroup is not found.' (fixed text, no variable parts).
Actual: HTTP 400, Error.Code='InvalidRequestException' (correct), Error.Message='WorkGroup <requested WorkGroup name> is not found.' — the request's WorkGroup value is interpolated.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the three athena checks listed in trace_ids.
3. Minimal manual repro, cause 1: CreateBucket B. CreateWorkGroup(Name=W, Configuration={ResultConfiguration:{OutputLocation:'s3://B/wgout/'}, EnforceWorkGroupConfiguration:true}). UpdateWorkGroup(WorkGroup=W, ConfigurationUpdates={EnforceWorkGroupConfiguration:false}); confirm GetWorkGroup reports EnforceWorkGroupConfiguration=false. StartQueryExecution(QueryString='SELECT 9 AS n', WorkGroup=W, ResultConfiguration={OutputLocation:'s3://B/clientout/'}). Poll GetQueryExecution to SUCCEEDED and note ResultConfiguration.OutputLocation starts with 's3://B/clientout/'. Then ListObjectsV2(Bucket=B, Prefix='clientout/') — expect at least the '<QueryExecutionId>.csv' result object; observe an empty listing.
4. Minimal manual repro, cause 2: pick a WorkGroup name N that does not exist. UpdateWorkGroup(WorkGroup=N, Description='should not apply', ConfigurationUpdates={EnforceWorkGroupConfiguration:true}) — expect HTTP 400 InvalidRequestException with Message exactly 'WorkGroup is not found.'; observe 'WorkGroup N is not found.'. GetWorkGroup(WorkGroup=N) shows the same interpolated message.
