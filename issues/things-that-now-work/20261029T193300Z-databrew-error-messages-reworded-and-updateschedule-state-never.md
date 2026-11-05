Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-schedule-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-schedule-lifecycle databrew-dataset-lifecycle
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeSchedule|2xx->2xx|$.CronExpression
Issue-Key: databrew|DescribeSchedule|2xx->2xx|$.LastModifiedBy
Issue-Key: databrew|DescribeSchedule|2xx->2xx|$.LastModifiedDate
Issue-Key: databrew|CreateDataset|4xx->4xx|$.message
Issue-Key: databrew|CreateSchedule|4xx->4xx|$.Message
Issue-Key: databrew|DescribeSchedule|4xx->4xx|$.Message
Issue-Commit: 497ea4470f6a57da4d057b03de07c81ba40956c1

# DataBrew error messages reworded and UpdateSchedule state never applied

> _Note: this issue was identified against the code base at commit `497ea4470f6a57da4d057b03de07c81ba40956c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchedule

## Description
Four distinct causes outside the recipe-version area: one dropped write on schedules and three response messages whose text does not match AWS's wording for its error class. The statuses are already correct in the three message cases, so only the message templates diverge. All four must be closed before re-submitting.

Cause 1 of 4: UpdateSchedule does not change the schedule record, so DescribeSchedule serves the create-time CronExpression and never sets modification metadata.
AWS: Smithy: UpdateSchedule (PUT /schedules/{Name}, input {Name @required @httpLabel, CronExpression @required @length 1..512, JobNames @length 0..50}) replaces the schedule's CronExpression and JobNames; DescribeSchedule output then returns the new CronExpression together with LastModifiedBy (the calling identity's ARN) and LastModifiedDate (the update time).
Emulator: DescribeSchedule after a successful UpdateSchedule answers HTTP 200 with the CronExpression supplied at CreateSchedule time, and LastModifiedBy / LastModifiedDate are absent from the body - the update left no trace in state.
Why: a schedule edit is silently lost. A caller that reschedules a job and reads back the schedule believes the new cron is in effect while the service still reports the old one, and there is no LastModified* to detect the drift - jobs keep firing on the superseded cadence.
Fix: the UpdateSchedule handler and the schedule state record - apply CronExpression/JobNames to the stored schedule and stamp LastModifiedBy/LastModifiedDate, which DescribeSchedule then serializes.
Note: UpdateSchedule is an unimplemented scaffold stub (open issue "Implement databrew UpdateSchedule: unimplemented scaffold stub"); that issue owns standing the operation up, this cause owns the state mutation and the metadata DescribeSchedule must report.
Traces: databrew-schedule-lifecycle#task#5.

Cause 2 of 4: the enum-constraint ValidationException message lists the member's enum value set in the wrong order.
AWS: Smithy: CreateDataset input member Format -> InputFormat (enum), error ValidationException (httpError 400). Recorded response: HTTP 400 with Message "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [ORC, CSV, JSON, PARQUET, EXCEL]". Fixed text: the "1 validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy enum value set: [" / "]" scaffolding AND the bracketed value set itself, which AWS emits for this member in the order ORC, CSV, JSON, PARQUET, EXCEL; the rejected <value> and the <member> path come from the request. Note the wire order is NOT the model's declaration order (the model lists CSV, JSON, PARQUET, EXCEL, ORC), so rendering the enum in model order diverges.
Emulator: same 400 and same scaffolding, but the value set is rendered in the model's declaration order - "...Member must satisfy enum value set: [CSV, JSON, PARQUET, EXCEL, ORC]".
Why: the message is the only machine-readable detail a ValidationException carries (there is no per-member error field), so clients, SDK integration tests and operator tooling that assert on or parse AWS's sentence do not match, and a user reading the emulator's message sees a value set AWS never prints.
Fix: the shared enum-constraint validation message builder used by CreateDataset's input validation - emit the member's value set in AWS's wire order rather than the model's trait order.
Traces: databrew-dataset-lifecycle#task#7.

Cause 3 of 4: the CreateSchedule duplicate-name ConflictException message carries a trailing period AWS does not emit.
AWS: Smithy: CreateSchedule error ConflictException (httpError 409, "Updating or deleting a resource can cause an inconsistent state."). Recorded response for creating a schedule whose name already exists: HTTP 409 with Message "Schedule <Name> already exists" - fixed text "Schedule " and " already exists" with NO terminating period; <Name> is the request's Name.
Emulator: HTTP 409 (correct) with Message "Schedule <Name> already exists." - the extra '.' makes the string differ from AWS for every duplicate schedule.
Why: error text is part of the contract clients match on; equality assertions and log/alert matchers keyed on AWS's sentence fail on the trailing character even though the status and error class are right.
Fix: CreateSchedule's already-exists conflict construction (and the shared already-exists message helper it calls) - drop the terminating period.
Traces: databrew-schedule-lifecycle#task#8.

Cause 4 of 4: the DescribeSchedule not-found ResourceNotFoundException message uses the wrong template.
AWS: Smithy: DescribeSchedule error ResourceNotFoundException (httpError 404, "One or more resources can't be found."). Recorded response for a schedule name that does not exist: HTTP 404 with Message "Schedule <Name> not found" - fixed text "Schedule " and " not found", no trailing period and no "wasn't found" phrasing; <Name> is the request's Name.
Emulator: HTTP 404 (correct) with Message "Schedule <Name> wasn't found." - produced by the generic not-found helper, so the wording and punctuation differ for every schedule miss.
Why: same contract problem as Cause 3 - callers that distinguish miss reasons by message text, and humans reading the error, see wording the service does not use.
Fix: the DescribeSchedule not-found construction / the shared not-found message helper - use DataBrew's schedule template. Note this helper is per-resource in AWS: recipe-version misses use a different sentence (see the DescribeRecipe recipe-version issue), so do not unify them onto one string.
Traces: databrew-schedule-lifecycle#task#9.

## Evidence
Replay against the emulator (trace set simparity-wire-aws-v1), all four verdict mismatch:

databrew-schedule-lifecycle#task#5 - DescribeSchedule, expected 200 / actual 200: $.CronExpression value_mismatch (expected the cron supplied to UpdateSchedule, got the cron supplied to CreateSchedule - e.g. expected 'cron(30 6 * * ? *)', got 'cron(15 12 * * ? *)'); $.LastModifiedBy missing (expected the caller identity ARN, got None); $.LastModifiedDate missing (expected the update timestamp, got None).

databrew-dataset-lifecycle#task#7 - CreateDataset, expected 400 / actual 400, $.message value_mismatch:
  expected "1 validation error detected: Value 'PARQUETX' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ORC, CSV, JSON, PARQUET, EXCEL]"
  actual   "1 validation error detected: Value 'PARQUETX' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [CSV, JSON, PARQUET, EXCEL, ORC]"
  (only the bracketed set's order differs; 'PARQUETX' and 'format' are the request's value and member path).

databrew-schedule-lifecycle#task#8 - CreateSchedule, expected 409 / actual 409, $.Message value_mismatch: expected 'Schedule <Name> already exists', got 'Schedule <Name> already exists.'.

databrew-schedule-lifecycle#task#9 - DescribeSchedule, expected 404 / actual 404, $.Message value_mismatch: expected 'Schedule <Name> not found', got "Schedule <Name> wasn't found.".

Model evidence (smithy_query on services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): InputFormat enum values [CSV, JSON, PARQUET, EXCEL, ORC]; CreateDataset POST /datasets, errors [AccessDeniedException 403, ConflictException 409, ServiceQuotaExceededException 402, ValidationException 400]; CreateSchedule POST /schedules {Name @required, CronExpression @required}, errors [ConflictException 409, ServiceQuotaExceededException 402, ValidationException 400]; UpdateSchedule PUT /schedules/{Name} {Name @required @httpLabel, CronExpression @required, JobNames}; DescribeSchedule GET /schedules/{Name}, output {CreateDate, CreatedBy, JobNames, LastModifiedBy, LastModifiedDate, ResourceArn, CronExpression, Tags, Name @required}, errors [ResourceNotFoundException 404, ValidationException 400].

## Steps to Reproduce
1. Boot the emulator and point a databrew client at it.
2. Cause 1: create_schedule(Name=S, CronExpression="cron(15 12 * * ? *)", JobNames=[J]); update_schedule(Name=S, CronExpression="cron(30 6 * * ? *)", JobNames=[J]); describe_schedule(Name=S). Expected: CronExpression "cron(30 6 * * ? *)" with LastModifiedBy and LastModifiedDate set. Actual: 200 with "cron(15 12 * * ? *)" and no LastModified* members.
3. Cause 2: create_dataset(Name=D, Format="PARQUETX", Input={...}). Expected: 400 ValidationException, Message "1 validation error detected: Value 'PARQUETX' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ORC, CSV, JSON, PARQUET, EXCEL]". Actual: same status, value set printed as [CSV, JSON, PARQUET, EXCEL, ORC].
4. Cause 3: create_schedule(Name=S, CronExpression="cron(30 6 * * ? *)") again. Expected: 409 ConflictException, Message "Schedule S already exists". Actual: "Schedule S already exists." (trailing period).
5. Cause 4: describe_schedule(Name=<name that was never created>). Expected: 404 ResourceNotFoundException, Message "Schedule <name> not found". Actual: "Schedule <name> wasn't found.".
6. Re-replay databrew-schedule-lifecycle steps task#5, task#8, task#9 and databrew-dataset-lifecycle step task#7 to confirm all four causes are closed.
