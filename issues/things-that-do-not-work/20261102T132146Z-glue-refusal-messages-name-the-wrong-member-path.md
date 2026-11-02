Issue-Source: trace-sweep-divergence
Issue-Trace: glue-cflog-create-input-validation-mistakes
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-cflog-create-input-validation-mistakes glue-cflog-get-tables-paginated-and-filtered glue-tsd-attributes-to-get-names-then-describe glue-tsd-expression-filter-mistakes glue-gjwr-traj-hand-written-job-definition-refusals
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateDatabase|4xx->4xx|$.message
Issue-Key: glue|CreateTable|4xx->4xx|$.message
Issue-Key: glue|UpdateJob|4xx->4xx|$.Message
Issue-Key: glue|CreateJob|4xx->4xx|$.message
Issue-Related: 20261102T132146Z-glue-catalog-input-validation-returns-validationexception-instead-of.md
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue refusal messages name the wrong member path and the wrong rule

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatabase

## Description
Six Glue refusals carry the right HTTP status and the right error code but the wrong message text: the emulator names a different input member path, states a different rule, or validates the members in a different order than Glue does. The message is part of the contract, so each of these is a defect. Six causes.

Cause 1 of 6: CreateDatabase with an over-long DatabaseInput.Name. AWS: Smithy CreateDatabase.DatabaseInput.Name is @length max 255; the recording shows 400 ValidationException with message "1 validation error detected: Value '<name>' at 'database.name' failed to satisfy constraint: Member must have length less than or equal to 255". Fixed text is the "1 validation error detected: Value '...' at '...' failed to satisfy constraint: Member must have length less than or equal to 255" template and the member path is the literal 'database.name'; the only variable part is the rejected value, taken from the request. Emulator: same template but the member path is 'databaseInput.name', and it truncates the echoed value ("... [265 bytes]") instead of echoing it whole. Why: a client or test that matches on the member path cannot locate the offending field, and the truncation marker is not AWS text. Fix: CreateDatabase's validation message rendering (member path alias and full value echo). Traces: glue-cflog-create-input-validation-mistakes#task#4.

Cause 2 of 6: CreateTable with an over-long TableInput.Name — identical defect: AWS names the path 'table.name', the emulator names 'tableInput.name' and truncates the echoed value. Fix: CreateTable's validation message rendering. Traces: glue-cflog-create-input-validation-mistakes#task#7 (also #task#8).

Cause 3 of 6: CreateTable with OpenTableFormatInput present but its Iceberg input absent. AWS: Smithy CreateTable takes TableInput and OpenTableFormatInput and models InvalidInputException (400); the recording shows `Message` 'CreateIcebergTableInput cannot be equal to null' — fixed text naming the missing nested input shape. Emulator: 'TableInput is required unless OpenTableFormatInput is supplied', which states a different rule entirely (it blames the top-level TableInput rather than the missing Iceberg sub-input). Why: the client is told to supply TableInput when the actual fault is an incomplete OpenTableFormatInput. Fix: CreateTable's validation of OpenTableFormatInput. Traces: glue-cflog-create-input-validation-mistakes#task#5.

Cause 4 of 6: GetTables with MaxResults 0. AWS: Smithy GetTables.MaxResults targets CatalogGetterPageSize with @range min 1; the recording shows 400 InvalidInputException with `Message` 'MaxItems has to be a positive number' — entirely fixed text, with no value and no limit interpolated. Emulator: 'MaxResults 0 is below the minimum page size of 1', which interpolates the request value and the limit and uses the wire member name instead of Glue's internal 'MaxItems'. Fix: GetTables' page-size validation message. Traces: glue-cflog-get-tables-paginated-and-filtered#task#11 (also glue-tsd-attributes-to-get-names-then-describe#task#4 and #task#6, glue-tsd-expression-filter-mistakes#task#7).

Cause 5 of 6: UpdateJob with a JobUpdate that omits both Command and Role. AWS: Smithy UpdateJob.JobUpdate is required and models InvalidInputException (400); the recording shows `Message` 'Command should not be null.' — Glue validates JobUpdate.Command FIRST, and the message is entirely fixed text. Emulator: 'JobUpdate.Role is required and cannot be blank' — it validates Role first and uses a different sentence form. Why: validation ORDER is observable, and a client fixing the field the emulator names still gets refused by real Glue. Fix: UpdateJob's validation (order and message text). Traces: glue-gjwr-traj-hand-written-job-definition-refusals#task#12 (also #task#13).

Cause 6 of 6: CreateJob with GlueVersion '4' (no dot). AWS: the recording shows 400 ValidationException with message "1 validation error detected: Value '4' at 'glueVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((\\w+\\.)+\\w+|auto)$" — the quoted pattern includes the `|auto` alternative. Emulator: quotes '^(\\w+\\.)+\\w+$' (no `auto`). The checked-in Smithy model agrees with the emulator (CreateJob.GlueVersion targets GlueVersionString @pattern ^(\\w+\\.)+\\w+$), so this is recorded behavior only: live Glue now accepts the literal 'auto' and its refusal message quotes the wider pattern. Why: the quoted pattern is the client's only statement of what is acceptable, and the emulator's narrower one also means GlueVersion 'auto' is wrongly rejected. Fix: the GlueVersion constraint in CreateJob/UpdateJob validation (widen to ^((\\w+\\.)+\\w+|auto)$ and quote that pattern). Traces: glue-gjwr-traj-hand-written-job-definition-refusals#task#2 (also #task#4).

Related open issues (do not cover these steps): 20261102T114851Z-glue-8-net-new-root-causes-refusal-messages.md and 20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. All six clusters are status-matching (400 expected, 400 observed) message mismatches.

Cause 1 — glue-cflog-create-input-validation-mistakes#task#4 CreateDatabase: $.message value_mismatch, expected "... at 'database.name' failed to satisfy constraint: Member must have length less than or equal to 255", got "... at 'databaseInput.name' ..." with the value rendered as "...xxx... [265 bytes]".
Cause 2 — glue-cflog-create-input-validation-mistakes#task#7 CreateTable: expected "... at 'table.name' ...", got "... at 'tableInput.name' ..." plus the same [265 bytes] truncation. 2 steps in cluster.
Cause 3 — glue-cflog-create-input-validation-mistakes#task#5 CreateTable: $.Message expected 'CreateIcebergTableInput cannot be equal to null', got 'TableInput is required unless OpenTableFormatInput is supplied'.
Cause 4 — glue-cflog-get-tables-paginated-and-filtered#task#11 GetTables: $.Message expected 'MaxItems has to be a positive number', got 'MaxResults 0 is below the minimum page size of 1'. 4 steps in cluster.
Cause 5 — glue-gjwr-traj-hand-written-job-definition-refusals#task#12 UpdateJob: $.Message expected 'Command should not be null.', got 'JobUpdate.Role is required and cannot be blank'. 2 steps in cluster.
Cause 6 — glue-gjwr-traj-hand-written-job-definition-refusals#task#2 CreateJob: $.message expected pattern '^((\\w+\\.)+\\w+|auto)$' in the quoted constraint, got '^(\\w+\\.)+\\w+$'. 2 steps in cluster.

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-cflog-create-input-validation-mistakes#task#4 — CreateDatabase with a 265-char DatabaseInput.Name.
2. glue-cflog-create-input-validation-mistakes#task#7 — CreateTable with a 265-char TableInput.Name.
3. glue-cflog-create-input-validation-mistakes#task#5 — CreateTable with OpenTableFormatInput lacking IcebergInput.
4. glue-cflog-get-tables-paginated-and-filtered#task#11 — GetTables with MaxResults 0.
5. glue-gjwr-traj-hand-written-job-definition-refusals#task#12 — UpdateJob with JobUpdate missing Command.
6. glue-gjwr-traj-hand-written-job-definition-refusals#task#2 — CreateJob with GlueVersion '4'.
Compare the Message/message member against the expected text in each case.
