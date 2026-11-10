Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it|_error_code#16|AssertionError|raise AssertionError(
Issue-Commit: bd416c0ab091bf65c958786969d0a3369dfa0e54

# CreateExportJob with a MetricsDataSource is accepted while account VdmEnabled=DISABLED instead of being refused BadRequestException "To use this feature you must enable Virtual Deliverability Manager" (1 cause)

> _Note: this issue was identified against the code base at commit `bd416c0ab091bf65c958786969d0a3369dfa0e54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportJob

## Description
One net-new behavioral cause on the SESv2 VDM metrics surface.

Cause 1 of 1: CreateExportJob does not gate a MetricsDataSource export on the account's Virtual Deliverability Manager state.

AWS: the VDM metrics surface is withdrawn for an account whose VdmAttributes.VdmEnabled is DISABLED. A CreateExportJob whose ExportDataSource is a MetricsDataSource (Namespace VDM) is then refused 400 BadRequestException (x-amzn-errortype: BadRequestException) with the fixed sentence 'To use this feature you must enable Virtual Deliverability Manager' — the whole message is fixed text, it has no variable part — and no export job is created (a subsequent ListExportJobs(ExportSourceType=METRICS_DATA) returns the same JobId set as before the refused call). Smithy (com-amazonaws-sesv2-simpleemailservice-v2): CreateExportJob errors are BadRequestException(400), LimitExceededException(400), NotFoundException(404), TooManyRequestsException(429); the same 'To use this feature you must enable Virtual Deliverability Manager' sentence is the recorded VDM-off refusal of the sibling metrics operations (see Cause 2 of the open issue 20261029T171854Z-sesv2-metrics-export-job-insights-and-certificate-paths-skip.md for BatchGetMetricData/GetMessageInsights). The order is a VDM pre-check ahead of any data-source validation, so the refusal does not depend on the request's dates or dimensions.

Emulator: after PutAccountVdmAttributes{VdmEnabled: DISABLED} (and GetAccount confirming VdmEnabled=DISABLED), the identical CreateExportJob(MetricsDataSource, ExportDestination{DataFormat: CSV}) succeeds — 200 with a JobId — so the check's helper raises 'the call succeeded with account VDM disabled; the VDM metrics surface must be withdrawn'. No refusal of any class is produced.

Why: a client that turns VDM off (or has never turned it on) is handed a job id for a metrics export the feature cannot produce, instead of the documented 400 that tells it to enable Virtual Deliverability Manager. Code that branches on that refusal to prompt an operator never runs, and the account accumulates METRICS_DATA export jobs AWS would never have created, so later ListExportJobs/GetExportJob reads diverge too.

Fix: the CreateExportJob handler's pre-validation — add the account-level VDM gate (read the account singleton's VdmAttributes.VdmEnabled and refuse a MetricsDataSource export with BadRequestException and AWS's fixed sentence before the job is allocated or any state is written).

Note: the open issue 20261029T171854Z-sesv2-metrics-export-job-insights-and-certificate-paths-skip.md covers CreateExportJob's inverted-date-range and custom-S3Url refusals and the VDM gate on BatchGetMetricData/GetMessageInsights; it does not cover CreateExportJob's VDM gate, which is the cause filed here.

Traces: behavioral check sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it (fails at the first refusal beat, _error_code around the CreateExportJob call).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check [mismatch] sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it:

  File "check_sesv2_disabling_account_vdm_withdraws_the_metrics_surface_then_enabling_restores_it.py", line 62, in run
    export_code = _error_code(lambda: sesv2.create_export_job(ExportDataSource=_metrics_data_source(), ExportDestination={"DataFormat": "CSV"}))
  AssertionError: the call succeeded with account VDM disabled; the VDM metrics surface must be withdrawn

Expected (AWS): CreateExportJob refused 400 BadRequestException, Error.Code='BadRequestException', Error.Message='To use this feature you must enable Virtual Deliverability Manager' (entire message is fixed text), and no new METRICS_DATA job in ListExportJobs.
Actual (emulator): HTTP 200 with a JobId; no ClientError raised.

Preconditions the check established before the call, all of which the emulator satisfied: PutAccountVdmAttributes{VdmEnabled:'ENABLED'} -> BatchGetMetricData answers the query and CreateExportJob(MetricsDataSource) creates a job that GetExportJob reports with ExportSourceType=METRICS_DATA and ListExportJobs(ExportSourceType=METRICS_DATA) lists; then PutAccountVdmAttributes{VdmEnabled:'DISABLED'} with GetAccount polled until it reports VdmEnabled='DISABLED'. So the single changed condition is the account VDM flag.

Smithy: CreateExportJob (POST /v2/email/export-jobs, success 200) declares BadRequestException(400), LimitExceededException(400), NotFoundException(404), TooManyRequestsException(429).

## Steps to Reproduce
1. Boot the emulator and create an sesv2 client.
2. PutAccountVdmAttributes(VdmAttributes={'VdmEnabled': 'ENABLED'}) and poll GetAccount until VdmAttributes.VdmEnabled == 'ENABLED'.
3. Positive control: CreateExportJob(ExportDataSource={'MetricsDataSource': {'Dimensions': {'ISP': ['*']}, 'Namespace': 'VDM', 'Metrics': [{'Name': 'SEND', 'Aggregation': 'VOLUME'}], 'StartDate': <UTC midnight - 2d>, 'EndDate': <UTC midnight>}}, ExportDestination={'DataFormat': 'CSV'}) -> 200 with a JobId; note the JobId set from ListExportJobs(ExportSourceType='METRICS_DATA').
4. PutAccountVdmAttributes(VdmAttributes={'VdmEnabled': 'DISABLED'}) and poll GetAccount until VdmAttributes.VdmEnabled == 'DISABLED'.
5. Snapshot ListExportJobs(ExportSourceType='METRICS_DATA') JobIds, then repeat the exact CreateExportJob call from step 3.
   Expected: ClientError 400 BadRequestException, message 'To use this feature you must enable Virtual Deliverability Manager', and the JobId set unchanged.
   Observed: 200 with a new JobId (no error raised).
6. Re-run the whole behavioral check: sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it.
