Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected
Issue-Key: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected|run#12|AssertionError|assert err == ("InvalidRequestException", _NO_LOCATION), (
Issue-Commit: 2f9e8ff46e4d80859bfa918f10daaf4eec221ac1

# Athena StartQueryExecution: the no-output-location refusal message is emulator-authored, not AWS's

> _Note: this issue was identified against the code base at commit `2f9e8ff46e4d80859bfa918f10daaf4eec221ac1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
One (1) net-new root cause on Athena's locationless-query refusal path. No open issue carries this replay key (a keyword search for an Athena output-location issue returns none), and the replay report marked the check neither ALREADY FILED nor OWNED.

Cause 1 of 1: StartQueryExecution with no ResultConfiguration.OutputLocation, against a workgroup whose ResultConfigurationUpdates.RemoveOutputLocation cleared its default, is refused with the right class but the wrong message text.
AWS: Smithy (com.amazonaws.athena#AmazonAthena): StartQueryExecution(QueryString @required, ResultConfiguration, WorkGroup) models InvalidRequestException (client, httpError 400) as its only client refusal besides TooManyRequestsException; the model does not fix the text, so the recording does. Recorded behavior: HTTP 400, error code InvalidRequestException, message exactly "No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings." This whole string is FIXED text with no request- or state-derived parts (note the doubled space after "for", which is how the service emits it); nothing in it is a per-run value.
Emulator: HTTP 400 with the correct code InvalidRequestException but the message "No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input." — emulator-authored prose for the same error class. The rest of the check's flow is right: the query succeeds while the workgroup has an OutputLocation, UpdateWorkGroup/RemoveOutputLocation is honoured by GetWorkGroup, and the refusal is produced at the right moment.
Why: this is the one refusal Athena clients, CLIs and test harnesses match by text to tell "your workgroup has no result location" apart from any other InvalidRequestException (the code is shared by every malformed-request case, so the text is the only discriminator). Against the emulator that match fails, and the operator-facing advice AWS gives (specify an S3 location or enable Athena managed query results) is replaced by wording the service never emits.
Fix: the StartQueryExecution handler's missing-output-location refusal — the message constant it raises with InvalidRequestException when neither the request's ResultConfiguration nor the resolved workgroup configuration supplies an OutputLocation.
Traces: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator up; bootstrap_signal false; 272/611 checks passed).

Check athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected [mismatch]:
  AssertionError: a locationless query after the removal was answered ('InvalidRequestException', 'No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input.')
  expected (code, message) == ('InvalidRequestException', 'No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings.')

Status and error code agree (400 / InvalidRequestException); only the message diverges. The expected message is entirely fixed text — no bucket, workgroup, query id or timestamp appears in it — so there is no per-run value in the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run check athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected.
3. Manually: s3.create_bucket(<bucket>); athena.create_work_group(Name=<wg>, Configuration={'ResultConfiguration': {'OutputLocation': 's3://<bucket>/athena-results/'}}).
4. athena.start_query_execution(QueryString='SELECT 1', WorkGroup=<wg>) with no ResultConfiguration — succeeds (positive control).
5. athena.update_work_group(WorkGroup=<wg>, ConfigurationUpdates={'ResultConfigurationUpdates': {'RemoveOutputLocation': True}}); confirm GetWorkGroup no longer reports OutputLocation.
6. athena.start_query_execution(QueryString='SELECT 1', WorkGroup=<wg>) again and read Error.Code / Error.Message: the code is InvalidRequestException (correct) but the message must be AWS's fixed sentence quoted above.
