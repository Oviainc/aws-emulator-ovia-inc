Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-dataset-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-dataset-lifecycle
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeDataset|2xx->2xx|$.CreatedBy
Issue-Key: databrew|DescribeDataset|2xx->2xx|$.FormatOptions.Csv.Delimiter
Issue-Key: databrew|DescribeDataset|2xx->2xx|$.FormatOptions.Csv.HeaderRow
Issue-Key: databrew|DescribeDataset|2xx->2xx|$.LastModifiedBy
Issue-Key: databrew|DescribeDataset|2xx->2xx|$.LastModifiedDate
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# DataBrew dataset state not tracked: DescribeDataset returns stale FormatOptions and premature LastModified fields

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataset

## Description
Two causes, both in the emulator's AWS Glue DataBrew dataset store: DescribeDataset does not reflect the dataset's real lifecycle state. The dataset created by CreateDataset is described with modification metadata it never acquired, and a later UpdateDataset's FormatOptions are dropped so DescribeDataset keeps serving the create-time values. Both are filed together because they share one root cause - the dataset record is not maintained across the create/update lifecycle - and both are observed on the same fixture.

Cause 1 of 2 (fixture databrew-dataset-lifecycle, step task#3): premature LastModifiedBy / LastModifiedDate on a never-updated dataset.
AWS: Smithy model com.amazonaws.databrew, DescribeDataset (GET /datasets/{Name}, 200) models LastModifiedDate (Date, timestamp) and LastModifiedBy (LastModifiedBy) as OPTIONAL output members; only Name and Input are required. Recorded behavior: for a dataset that has only been created (no UpdateDataset yet), AWS returns the 200 DescribeDataset body with CreatedBy/CreateDate set and LastModifiedBy and LastModifiedDate ABSENT.
Emulator: returns 200 but populates both - $.LastModifiedBy and $.LastModifiedDate are present where the recording has no member at all (replay reports type_mismatch, expected None, got a principal ARN and got a numeric timestamp).
Why: a client cannot distinguish a freshly created dataset from a modified one. Code that keys cache invalidation, drift detection or 'has this been edited since creation' logic off the presence of LastModifiedDate/LastModifiedBy sees every dataset as already modified, and SDK optionals that are expected to be unset are always set.
Fix: the DescribeDataset serializer / the dataset record in the DataBrew state - set the last-modified pair only when a mutating operation (UpdateDataset and friends) actually runs, and omit both members from the response while they are unset.

Cause 2 of 2 (fixture databrew-dataset-lifecycle, step task#5): UpdateDataset's FormatOptions are not persisted.
AWS: Smithy model, UpdateDataset (PUT /datasets/{Name}) accepts FormatOptions (FormatOptions -> Csv: CsvOptions{Delimiter @length 1..1, HeaderRow boolean}); DescribeDataset echoes the dataset's current FormatOptions. Recorded behavior: after an UpdateDataset that sets a CSV delimiter of '|' and HeaderRow false, the next DescribeDataset returns 200 with $.FormatOptions.Csv.Delimiter = '|' and $.FormatOptions.Csv.HeaderRow = false.
Emulator: returns 200 but with $.FormatOptions.Csv.Delimiter = ',' and $.FormatOptions.Csv.HeaderRow = true - i.e. the create-time/default CSV options, so the update was silently lost (UpdateDataset itself returned success).
Why: a lost update that reports success. A client that reconfigures a dataset's parsing options and then reads them back gets the old options, so downstream jobs parse the source file with the wrong delimiter and treat the first data row as a header (or vice versa), and any read-after-write check the client performs fails or, worse, confirms the wrong value.
Fix: the UpdateDataset handler and the DataBrew dataset state - apply the request's Format/FormatOptions/Input/PathOptions onto the stored dataset record (and stamp LastModifiedDate/LastModifiedBy there, which is what Cause 1 wants), so DescribeDataset serves the updated options.

Not filed from these same two steps: $.CreatedBy / $.LastModifiedBy differ only in the generated principal name inside the ARN ('arn:aws:iam::000000000000:user/emulator' vs 'arn:aws:iam::000000000000:user/EMULATOR930333B8B254'). The account and ARN form match and a generated principal name is environment-specific, so it is not treated as a requirement here; only the PRESENCE of the LastModified* members (Cause 1) is.

Traces: databrew-dataset-lifecycle#task#3 (Cause 1), databrew-dataset-lifecycle#task#5 (Cause 2).

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 34 steps, 5 divergence clusters, no state_reset_failures, emulator_up=true).

Cause 1 - databrew-dataset-lifecycle#task#3, DescribeDataset, expected status 200, actual 200:
  $.LastModifiedBy: type_mismatch (expected None, got 'arn:aws:iam::000000000000:user/EMULATOR930333B8B254')
  $.LastModifiedDate: type_mismatch (expected None, got 1792627062.6532927)
  (also $.CreatedBy value_mismatch on the generated principal name only - not filed)

Cause 2 - databrew-dataset-lifecycle#task#5, DescribeDataset, expected status 200, actual 200:
  $.FormatOptions.Csv.Delimiter: value_mismatch (expected '|', got ',')
  $.FormatOptions.Csv.HeaderRow: value_mismatch (expected False, got True)
  (also $.CreatedBy/$.LastModifiedBy value_mismatch on the generated principal name only - not filed)

Model evidence: Smithy DescribeDataset output members LastModifiedDate/LastModifiedBy are optional (only Name and Input are required); UpdateDataset input carries FormatOptions -> CsvOptions{Delimiter, HeaderRow}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture databrew-dataset-lifecycle.
2. Cause 1: CreateDataset a CSV dataset (POST /datasets), then DescribeDataset it (GET /datasets/{Name}) WITHOUT any intervening update - step task#3. Observe 200 with LastModifiedBy and LastModifiedDate present; AWS omits both until the dataset is modified.
3. Cause 2: UpdateDataset that dataset (PUT /datasets/{Name}) with FormatOptions.Csv.Delimiter='|' and FormatOptions.Csv.HeaderRow=false, then DescribeDataset again - step task#5. Observe 200 with Delimiter=',' and HeaderRow=true instead of the values just written.
4. Re-replay both databrew-dataset-lifecycle#task#3 and databrew-dataset-lifecycle#task#5 to confirm a fix; task#5 alone does not exercise Cause 1.
