Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-recipe-job-writes-transformed-csv
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-recipe-job-writes-transformed-csv
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeJobRun|2xx->2xx|$.CompletedOn
Issue-Key: databrew|DescribeJobRun|2xx->2xx|$.Outputs[*].FormatOptions
Issue-Key: databrew|DescribeJobRun|2xx->2xx|$.Outputs[*].Location.BucketOwner
Issue-Key: databrew|DescribeJobRun|2xx->2xx|$.StartedOn
Issue-Commit: 00353ccf65811f65ad7f73dbcefa75ea121211d3

# DataBrew DescribeJobRun drops the job's output configuration (FormatOptions, Location.BucketOwner) and serializes run timestamps with non-AWS precision

> _Note: this issue was identified against the code base at commit `00353ccf65811f65ad7f73dbcefa75ea121211d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobRun

## Description
Three net-new root causes in the DataBrew DescribeJobRun response for a completed recipe job run. A fixer must close all three. Common context: the fixture creates a recipe job whose single Output names an S3 location (bucket, key, bucket owner) and CSV FormatOptions, starts it, and polls DescribeJobRun until the run is SUCCEEDED. The emulator answers 200 as AWS does, but the projected job-run record is lossy.

Cause 1 of 3: DescribeJobRun omits Outputs[*].FormatOptions.
AWS: Smithy: DescribeJobRun output member Outputs -> OutputList -> com.amazonaws.databrew#Output, whose member FormatOptions targets OutputFormatOptions { Csv: CsvOutputOptions { Delimiter (@length min 1 max 1) } }. The job run echoes the output configuration that was persisted when the job was created/updated, so a run of a job created with CSV format options reports those same options back, member for member.
Emulator: $.Outputs[0].FormatOptions is absent entirely (expected the job's own options object, in the recording {"Csv": {"Delimiter": ","}}; the delimiter character is the value taken from the CreateRecipeJob request, not a fixed part of the contract).
Why: a client that reads back how its job writes output — delimiter, format options — to parse the produced file, or that round-trips DescribeJobRun into an UpdateRecipeJob, silently loses the format configuration and either mis-parses the output or rewrites the job with no FormatOptions at all.
Fix: the databrew DescribeJobRun handler/serializer and the job-run state it projects — the Output list must be persisted whole at CreateRecipeJob/StartJobRun time and re-serialized, not rebuilt from location only.
Traces: databrew-recipe-job-writes-transformed-csv#task#10.

Cause 2 of 3: DescribeJobRun omits Outputs[*].Location.BucketOwner.
AWS: Smithy: com.amazonaws.databrew#S3Location members Bucket (required), Key, BucketOwner (@length min 12 max 12, @pattern ^[0-9]{12}$) — "The Amazon Web Services account ID of the bucket owner." When the output location was supplied with a bucket owner, DescribeJobRun returns it; the value is the 12-digit account id of the bucket's owner (the recording's 000000000000 is this environment's account id, not part of the contract).
Emulator: $.Outputs[0].Location.BucketOwner is absent.
Why: a cross-account consumer uses BucketOwner to confirm which account owns the destination before reading or granting on it; with the member dropped it cannot distinguish "no owner constraint" from "owner recorded".
Fix: as cause 1 — the S3Location serializer in the databrew job-run projection must carry BucketOwner through from stored state.
Traces: databrew-recipe-job-writes-transformed-csv#task#10.

Cause 3 of 3: StartedOn/CompletedOn are serialized in a shape AWS does not produce.
AWS: Smithy: DescribeJobRun output members StartedOn and CompletedOn target com.amazonaws.databrew#Date (timestamp), carried in the restJson1 body as an epoch-seconds number. The recorded response's timestamp values are volatile and therefore shape-asserted rather than value-asserted, and the emulator's values fail that shape assertion: AWS emits these epoch-seconds numbers with at most millisecond fractional precision.
Emulator: $.StartedOn = 1794232198.8054497 and $.CompletedOn = 1794232199.4054496 — a plain decimal carrying sub-millisecond (microsecond) fractional digits straight from the host clock.
Hypothesis: the handler serializes a floating-point wall-clock reading directly instead of normalizing it to the epoch-seconds form AWS's restJson1 timestamp serializer emits (millisecond precision), so the number's shape, not its instant, is what diverges.
Why: a client that parses the timestamp with a strict epoch-seconds reader, or that re-sends the value back to the service, sees a number of a form AWS never produces; the extra precision is also not meaningful state, so two runs are ordered on digits the service does not define.
Fix: the databrew job-run timestamp serialization (the DescribeJobRun/ListJobRuns response serializer and the clock reading stored on the job-run record).
Traces: databrew-recipe-job-writes-transformed-csv#task#10.

No open issue carries this replay key: the two open databrew issues (20261029T193300Z-databrew-recipe-versions-are-not-tracked-describerecipe-serves.md and 20261107T074006Z-databrew-listrecipeversions-reports-the-publish-request-description-instead.md) concern the recipe-version surface (DescribeRecipe / ListRecipeVersions), not job runs.

## Evidence
Replay verdict 'mismatch', expected_status 200, actual_status 200 (status matches; the body diverges).
Fixture databrew-recipe-job-writes-transformed-csv, step task#10, operation DescribeJobRun, service databrew, cluster size 1.
detail:
  $.Outputs[0].FormatOptions: missing (expected {'Csv': {'Delimiter': ','}}, got None)
  $.Outputs[0].Location.BucketOwner: missing (expected '000000000000', got None)
  $.StartedOn: value_mismatch (expected '<shaped-not-asserted>', got 1794232198.8054497)
  $.CompletedOn: value_mismatch (expected '<shaped-not-asserted>', got 1794232199.4054496)
divergence_key: service=databrew|DescribeJobRun|status=200->200|keys=$.CompletedOn,$.Outputs[0].FormatOptions,$.Outputs[0].Location.BucketOwner,$.StartedOn
Model evidence: smithy_query operation=DescribeJobRun on services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json (output members Outputs, StartedOn, CompletedOn; http GET /jobs/{Name}/jobRun/{RunId}, code 200) and smithy_query shape=Output,S3Location,OutputFormatOptions,CsvOutputOptions,Date (Output.FormatOptions -> OutputFormatOptions.Csv -> CsvOutputOptions.Delimiter; S3Location.BucketOwner @pattern ^[0-9]{12}$; Date is a timestamp).

## Steps to Reproduce
1. Replay the wire corpus and re-run the binding in Issue-Trace/Issue-Step: databrew-recipe-job-writes-transformed-csv#task#10.
2. By hand: CreateDataset over a CSV object; CreateRecipe + PublishRecipe; CreateRecipeJob with Outputs=[{Location:{Bucket:<b>,Key:<k>,BucketOwner:<12-digit account id>}, Format:CSV, FormatOptions:{Csv:{Delimiter:","}}}]; StartJobRun; then GET /jobs/<name>/jobRun/<runId>.
3. Assert the 200 body carries Outputs[0].FormatOptions.Csv.Delimiter equal to the delimiter supplied at job creation, Outputs[0].Location.BucketOwner equal to the supplied 12-digit account id, and that StartedOn/CompletedOn are epoch-seconds numbers in the form AWS emits (no sub-millisecond digits).
