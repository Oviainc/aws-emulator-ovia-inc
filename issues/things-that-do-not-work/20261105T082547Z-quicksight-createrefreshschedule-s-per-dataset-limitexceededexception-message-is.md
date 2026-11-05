Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.refresh-schedule-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.refresh-schedule-quota-exceeded-rejected|run#21|AssertionError|assert err["Message"] == (
Issue-Commit: 13785c293693d79d6d42c9472bc536582224542b

# quicksight: CreateRefreshSchedule's per-dataset LimitExceededException message is emulator-invented wording, not AWS's "Reached max limit of N refresh schedules for preparedDataSource <dataset-arn>"

> _Note: this issue was identified against the code base at commit `13785c293693d79d6d42c9472bc536582224542b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRefreshSchedule

## Description
Cause 1 of 1: CreateRefreshSchedule correctly refuses the sixth refresh schedule on a dataset with LimitExceededException, but the error MESSAGE is wording the emulator made up rather than the sentence AWS returns for that refusal class.

AWS: Smithy (services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json) models CreateRefreshSchedule with error LimitExceededException, errorKind client, httpError 409, documented only as "A limit is exceeded."; the operation's own doc states the rule — "Creates a refresh schedule for a dataset. You can create up to 5 different schedules for a single dataset." The model does not state the message text, so for the message this is recorded behavior only: the recording shows real QuickSight refusing the sixth CreateRefreshSchedule for one dataset with Code "LimitExceededException" and Message of the form

    Reached max limit of <max> refresh schedules for preparedDataSource <dataset-arn>

Fixed text: "Reached max limit of ", " refresh schedules for preparedDataSource ". Variable parts taken from the service limit and the request: <max> is the per-dataset refresh-schedule allowance (5), and <dataset-arn> is the ARN of the dataset named by the request's DataSetId and AwsAccountId in the serving region, i.e. arn:aws:quicksight:<region>:<account-id>:dataset/<data-set-id>. The region, account id and dataset id in any one recording are request/environment values that fill that template and are not themselves part of it.

Emulator: answers Code "LimitExceededException" (correct) with Message "You can create up to 5 different schedules for a single dataset." — a paraphrase of the operation's documentation prose. It neither uses AWS's fixed text nor names the dataset at all, so the ARN-valued variable part is absent entirely.

Why: a client that classifies or parses this refusal by message — log/alert matching, surfacing the offending dataset to a user, or any test asserting the AWS sentence — sees a message that never matches what AWS emits, and loses the dataset ARN AWS identifies the exhausted resource with. Error code and 409 status are right, so the divergence is silent to code-only handling and only bites message-dependent paths.

Fix: the per-dataset quota branch of the CreateRefreshSchedule handler's validation — the LimitExceededException it constructs when the dataset already holds the maximum number of schedules. It should format AWS's sentence, interpolating the allowance and the dataset ARN it already computes for the new schedule's parent dataset. The two neighbouring LimitExceededException branches in the same handler (the single-schedule-per-sub-daily-interval one and the emulator-wide total-records ceiling) are separate wordings and are not what this check exercised; only the per-dataset ceiling is grounded here.

Traces: behavioral check quicksight.refresh-schedule-quota-exceeded-rejected — creates five schedules (3 DAILY, 1 WEEKLY, 1 MONTHLY) on one SPICE dataset, then asserts the sixth CreateRefreshSchedule is refused with LimitExceededException and AWS's message; the Code assertion passes and the Message assertion fails. Handler citation: services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_refresh_schedule.rs lines 303-316.

## Evidence
Check: quicksight.refresh-schedule-quota-exceeded-rejected [mismatch]

Assertion that failed (check_quicksight_refresh_schedule_quota_exceeded_rejected.py:130):
    assert err["Message"] == (
        f"Reached max limit of {_MAX_SCHEDULES_PER_DATASET} refresh schedules for "
        f"preparedDataSource {dataset_arn}"
    ), err
where _MAX_SCHEDULES_PER_DATASET = 5 and
    dataset_arn = arn:aws:quicksight:<region>:<account-id>:dataset/<data-set-id>

Expected (AWS), template:
    Code    = LimitExceededException
    Status  = 409
    Message = "Reached max limit of 5 refresh schedules for preparedDataSource arn:aws:quicksight:<region>:<account-id>:dataset/<data-set-id>"
              fixed text: "Reached max limit of " / " refresh schedules for preparedDataSource "
              variable:   the allowance (5) and the dataset ARN built from the request's AwsAccountId + DataSetId and the serving region

Actual (emulator):
    AssertionError: {'Message': 'You can create up to 5 different schedules for a single dataset.', 'Code': 'LimitExceededException'}

Source of the wrong text — services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_refresh_schedule.rs:303-316:
    let existing = store.refresh_schedules_in(&input.aws_account_id, &input.data_set_id);
    if existing.len() >= crate::state::MAX_REFRESH_SCHEDULES_PER_DATA_SET {
        return Err(... LimitExceededException {
            message: Some(format!(
                "You can create up to {} different schedules for a single dataset.",
                crate::state::MAX_REFRESH_SCHEDULES_PER_DATA_SET
            )), ...

The handler already computes the schedule ARN for this dataset (crate::state::refresh_schedule_arn at lines 253-259), so the region/account/dataset components AWS's message needs are in hand at the point of refusal.

Correct in the emulator and NOT part of this finding: the error code (LimitExceededException), the 409 classification, the absence of the sixth schedule from ListRefreshSchedules, and the positive control (deleting one schedule lets a new one be created) — the check's later assertions were never reached because the Message assertion aborts first.

## Steps to Reproduce
1. git_clone the repo and boot the emulator.
2. As a subscribed QuickSight account, create an ATHENA data source and wait for DescribeDataSource to report CREATION_SUCCESSFUL.
3. CreateDataSet with ImportMode=SPICE backed by a CustomSql physical table on that data source.
4. Call CreateRefreshSchedule five times on that dataset with distinct ScheduleIds (e.g. three DAILY at different TimeOfTheDay, one WEEKLY with RefreshOnDay.DayOfWeek, one MONTHLY with RefreshOnDay.DayOfMonth), each RefreshType=FULL_REFRESH. All five succeed.
5. Call CreateRefreshSchedule a sixth time on the same dataset with a new ScheduleId.
6. Observe the refusal: Code is LimitExceededException (correct) but Message is "You can create up to 5 different schedules for a single dataset." instead of "Reached max limit of 5 refresh schedules for preparedDataSource arn:aws:quicksight:<region>:<account-id>:dataset/<data-set-id>".

Equivalently: re-replay behavioral check quicksight.refresh-schedule-quota-exceeded-rejected and observe the AssertionError on err["Message"] at line 130.
