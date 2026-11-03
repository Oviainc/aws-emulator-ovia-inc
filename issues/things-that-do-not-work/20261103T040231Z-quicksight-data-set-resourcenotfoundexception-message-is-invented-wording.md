Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.search-datasets-by-name-finds-created-dataset
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|DescribeDataSet|ResourceNotFoundException
Issue-Commit: 5e177a4f4d28f6dc4714f734dc35ed5ad192c177

# quicksight data-SET ResourceNotFoundException message is invented wording ("Data set with ID <id> not found.") instead of AWS's ARN-based "DataSet <arn> is not found"

> _Note: this issue was identified against the code base at commit `5e177a4f4d28f6dc4714f734dc35ed5ad192c177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataSet

## Description
One net-new root cause in the QuickSight emulator.

Cause 1 of 1: the ResourceNotFoundException the data-SET handlers raise for a dataset that does not exist carries emulator-invented message text, so a client that reads or matches on the message sees text real QuickSight never emits. Failing check: quicksight.search-datasets-by-name-finds-created-dataset.

AWS: DescribeDataSet (Smithy: GET /accounts/{AwsAccountId}/data-sets/{DataSetId}; errors AccessDeniedException 401, InternalFailureException 500, InvalidParameterValueException 400, ResourceNotFoundException httpError 404 documented "One or more resources can't be found.", ThrottlingException 429) and DeleteDataSet (Smithy: DELETE /accounts/{AwsAccountId}/data-sets/{DataSetId}; same error set, ResourceNotFoundException httpError 404). The model fixes the error shape and the 404 status but not the message text, so the text itself is recorded behavior only: for an absent or already-deleted dataset QuickSight answers HTTP 404 with Code `ResourceNotFoundException` and a message whose template is the fixed text `DataSet ` + the dataset's ARN + ` is not found`. Fixed text: "DataSet " and " is not found" (no trailing period). Variable part: the ARN, assembled from the request and service identity as arn:aws:quicksight:<region>:<AwsAccountId>:dataset/<DataSetId> — region from the endpoint the request was served on, account from the request's AwsAccountId path label, id from the DataSetId path label. The id/account seen in any one replay only fill that template and are not part of it. The same not-found template applies to DeleteDataSet called a second time on the same id, which the check asserts immediately after the describe.

Emulator: the Code and the 404 classification are correct (the check gets a ResourceNotFoundException from both the post-delete DescribeDataSet and the repeated DeleteDataSet), but the message is rendered from the template `Data set with ID {DataSetId} not found.` — lower-case "Data set", the words "with ID", a bare id instead of an ARN, and a trailing period. Replay output: `AssertionError: {'Message': 'Data set with ID s2r-search-ds-a9d48fab277c not found.', 'Code': 'ResourceNotFoundException'}` where the expected message was `DataSet arn:aws:quicksight:<region>:<account>:dataset/<id> is not found`.

Why: a client that surfaces, logs or pattern-matches QuickSight's dataset not-found message gets text that never appears in AWS; anything that parses the ARN out of the message (to identify which dataset of a batch is missing, or to recover region/account) finds no ARN at all and either misreports or fails outright. Because both the read and the delete path share the invented wording, no message-sensitive caller can treat the emulator as AWS.

Fix: the not-found error construction in the data-set operation handlers — render the dataset ARN (the same derivation the handlers already return in DataSet.Arn / DeleteDataSet.Arn) rather than a bare id, and share one not-found message builder across the data-set handlers so describe and delete (and any other data-set operation keyed on an absent id) agree. File/line citations under Traces.

Traces: failing check quicksight.search-datasets-by-name-finds-created-dataset (assertion at check line 84, `assert e.response["Error"]["Message"] == dataset_gone`; the same assertion is repeated for the second DeleteDataSet at check line 95). Message constructed in services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/describe_data_set.rs line 165 and, identically, services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/delete_data_set.rs line 165. This is the DATA-SET analogue of, and distinct from, open issue 20261031T224711Z-quicksight-data-source-resourcenotfoundexception-message-is-invented-wording.md, which covers the DATA-SOURCE handlers (describe_data_source.rs / update_data_source.rs); the dataset handlers are separate files with their own copy of the invented template and are not fixed by that issue.

## Evidence
Check quicksight.search-datasets-by-name-finds-created-dataset. The SearchDataSets portion passes (the created dataset is found by DATASET_NAME/StringLike and stops being found after delete); the divergence is in the not-found message afterwards.

Request: DescribeDataSet(AwsAccountId=<account>, DataSetId=<already-deleted id>)

Expected (AWS): HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "DataSet arn:aws:quicksight:<region>:<account>:dataset/<DataSetId> is not found" — fixed text "DataSet " ... " is not found"; the only variable part is the ARN, built from the serving region, the request's AwsAccountId and the request's DataSetId.

Actual (emulator): Error.Code = "ResourceNotFoundException" (correct), Error.Message = "Data set with ID <DataSetId> not found." — template `Data set with ID {DataSetId} not found.`, no ARN, trailing period.

Replay excerpt:
  AssertionError: {'Message': 'Data set with ID s2r-search-ds-a9d48fab277c not found.', 'Code': 'ResourceNotFoundException'}

Same divergence on the repeated delete:
Request: DeleteDataSet(AwsAccountId=<account>, DataSetId=<already-deleted id>)
Expected: Error.Message = "DataSet arn:aws:quicksight:<region>:<account>:dataset/<DataSetId> is not found"
Actual: Error.Message = "Data set with ID <DataSetId> not found."

Source of the divergence:
  describe_data_set.rs:165  message: Some(format!("Data set with ID {} not found.", key.1))
  delete_data_set.rs:165    message: Some(format!("Data set with ID {} not found.", key.1))

Smithy citation: DescribeDataSet errors -> ResourceNotFoundException (errorKind client, httpError 404, doc "One or more resources can't be found."); DeleteDataSet errors -> ResourceNotFoundException (httpError 404). The model does not fix the message text, so the expected template above is recorded behavior only.

## Steps to Reproduce
1. Ensure the QuickSight account subscription exists (DescribeAccountSubscription / CreateAccountSubscription) and a QuickSight service role ARN is available.
2. CreateDataSource(AwsAccountId=<account>, DataSourceId=<src-id>, Name=<src-id>, Type="ATHENA", DataSourceParameters={AthenaParameters:{WorkGroup:"primary", RoleArn:<role>}}).
3. CreateDataSet(AwsAccountId=<account>, DataSetId=<ds-id>, Name=<unique name>, ImportMode="DIRECT_QUERY", PhysicalTableMap={tbl1:{CustomSql:{DataSourceArn:<src arn>, Name:"q1", SqlQuery:"SELECT 1 AS col", Columns:[{Name:"col",Type:"INTEGER"}]}}}).
4. DeleteDataSet(AwsAccountId=<account>, DataSetId=<ds-id>).
5. Call DescribeDataSet(AwsAccountId=<account>, DataSetId=<ds-id>) and read Error.Message: the emulator answers "Data set with ID <ds-id> not found." where AWS answers "DataSet arn:aws:quicksight:<region>:<account>:dataset/<ds-id> is not found".
6. Call DeleteDataSet(AwsAccountId=<account>, DataSetId=<ds-id>) a second time and read Error.Message: the same invented wording is returned instead of the ARN-based template.
