Issue-Source: trace-sweep-divergence
Issue-Trace: glue-aorigin-get-tags-resource-arn-forms
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-aorigin-get-tags-resource-arn-forms glue-aorigin-search-tables-resolves-the-database glue-tsd-search-tables-route glue-cflog-create-input-validation-mistakes glue-cflog-table-lookup-mistakes glue-tsd-page-size-and-parameter-shape-mistakes glue-tsd-attributes-to-get-names-then-describe glue-tsd-foreign-and-malformed-catalog-id
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTags|4xx->4xx|$.message
Issue-Key: glue|SearchTables|4xx->4xx|$.Message
Issue-Key: glue|SearchTables|4xx->4xx|$.__type
Issue-Key: glue|CreateDatabase|4xx->4xx|$.Message
Issue-Key: glue|CreateDatabase|4xx->4xx|$.__type
Issue-Key: glue|CreateTable|4xx->4xx|$.Message
Issue-Key: glue|CreateTable|4xx->4xx|$.__type
Issue-Key: glue|GetTable|4xx->4xx|$.Message
Issue-Key: glue|GetTable|4xx->4xx|$.__type
Issue-Key: glue|GetTables|4xx->4xx|$.Message
Issue-Key: glue|GetTables|4xx->4xx|$.__type
Issue-Related: 20261029T201859Z-glue-tagging-rejects-most-taggable-arns-and-resource.md
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue catalog input validation returns ValidationException instead of InvalidInputException

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTags

## Description
Six distinct Glue request-validation paths are answered by a generic constraint validator that emits `__type: ValidationException` with a lowercase `message` member, where real Glue emits its own modeled `InvalidInputException` with a `Message` member. The HTTP status (400) is right in every case; the error CODE and the error MESSAGE are both wrong, so a client that switches on the Glue error code sees an error class Glue never returns for that operation. Six causes, one root cause: the generic constraint validator runs ahead of (and in place of) each operation's own Glue-specific validation.

Cause 1 of 6: GetTags with a ResourceArn that is not an ARN at all. AWS: Smithy GetTags.ResourceArn targets GlueResourceArn (@length 1..10240, @pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:) and GetTags models InvalidInputException (httpError 400); the recording shows AWS answering 400 with `__type` InvalidInputException and NO lowercase `message` member. Emulator: 400 with `__type` ValidationException and a `message` of the generic form "1 validation error detected: Value '<value>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:" (fixed text is the "1 validation error detected: Value ... at ... failed to satisfy constraint: ..." template; the value and the member name come from the request). Why: a client catching InvalidInputException to distinguish a malformed ARN from a service fault does not catch ValidationException, and the `message`/`Message` member name differs too, so error text extraction returns null. Fix: the GetTags handler's input validation must raise Glue's InvalidInputException rather than let the generic constraint validator answer. Traces: glue-aorigin-get-tags-resource-arn-forms#task#5.

Cause 2 of 6: SearchTables with an unsupported ResourceShareType. AWS: Smithy SearchTables.ResourceShareType targets the ResourceShareType enum and SearchTables models InvalidInputException (httpError 400); the recording shows 400 `__type` InvalidInputException with `Message` 'Resource share type foreign is not valid' (fixed text 'Resource share type ' + the requested value + ' is not valid'). Emulator: 400 with `__type` ValidationException and NO `Message` member at all. Why: the refusal is unidentifiable — neither the Glue error code nor the message that names the offending value is present. Fix: SearchTables' validation of ResourceShareType. Traces: glue-aorigin-search-tables-resolves-the-database#task#7 (also glue-tsd-search-tables-route#task#10).

Cause 3 of 6: CreateDatabase with an empty DatabaseInput.Name. AWS: Smithy CreateDatabase models InvalidInputException (httpError 400) and DatabaseInput.Name is required with @length min 1; the recording shows 400 `__type` InvalidInputException with `Message` 'Database name cannot be equal to null or empty' (entirely fixed text). Emulator: 400 with `__type` ValidationException and no `Message`. Fix: CreateDatabase's validation. Traces: glue-cflog-create-input-validation-mistakes#task#3.

Cause 4 of 6: CreateTable with an empty TableInput.Name. AWS: Smithy CreateTable models InvalidInputException (httpError 400); the recording shows 400 `__type` InvalidInputException with `Message` 'Table name cannot be equal to null or empty' (entirely fixed text). Emulator: 400 with `__type` ValidationException and no `Message`. Fix: CreateTable's validation. Traces: glue-cflog-create-input-validation-mistakes#task#6.

Cause 5 of 6: GetTable with an empty Name. AWS: Smithy GetTable.Name is required, @length 1..255, and GetTable models InvalidInputException (httpError 400); the recording shows 400 `__type` InvalidInputException with `Message` 'Table name cannot be equal to null or empty' (entirely fixed text). Emulator: 400 with `__type` ValidationException and no `Message`. Fix: GetTable's validation. Traces: glue-cflog-table-lookup-mistakes#task#4 (also glue-cflog-table-lookup-mistakes#task#6, glue-tsd-page-size-and-parameter-shape-mistakes#task#8).

Cause 6 of 6: GetTables with an AttributesToGet entry that is not a member of the TableAttributes enum. AWS: Smithy GetTables.AttributesToGet targets TableAttributesList over the TableAttributes enum (values NAME, TABLE_TYPE, DEFAULT, LATEST_ICEBERG_METADATA) and GetTables models InvalidInputException (httpError 400); the recording shows 400 `__type` InvalidInputException with `Message` 'Attribute name is not a valid attribute to get.' (entirely fixed text — it does NOT echo the offending value). Emulator: 400 with `__type` ValidationException and no `Message`. Fix: GetTables' validation of AttributesToGet. Traces: glue-tsd-attributes-to-get-names-then-describe#task#5 (also glue-tsd-foreign-and-malformed-catalog-id#task#7, glue-tsd-page-size-and-parameter-shape-mistakes#task#5).

Related open issue (does not cover these steps): 20261029T185330Z-glue-7-net-new-root-causes-in-input.md describes the same generic-validator-instead-of-InvalidInputException pattern on other operations.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue, verdict mismatch on all six clusters (status 400 expected and 400 observed in each; the error code and message diverge).

Cause 1 — glue-aorigin-get-tags-resource-arn-forms#task#5 GetTags: expected $.__type 'InvalidInputException', got 'ValidationException'; $.message unexpected_present (expected absent, got "1 validation error detected: Value 'sim2real-probe-aorigin-tag-tbl-46e3e53f' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:").
Cause 2 — glue-aorigin-search-tables-resolves-the-database#task#7 SearchTables: expected $.__type 'InvalidInputException', got 'ValidationException'; $.Message missing (expected 'Resource share type foreign is not valid').
Cause 3 — glue-cflog-create-input-validation-mistakes#task#3 CreateDatabase: expected $.__type 'InvalidInputException', got 'ValidationException'; $.Message missing (expected 'Database name cannot be equal to null or empty').
Cause 4 — glue-cflog-create-input-validation-mistakes#task#6 CreateTable: expected $.__type 'InvalidInputException', got 'ValidationException'; $.Message missing (expected 'Table name cannot be equal to null or empty').
Cause 5 — glue-cflog-table-lookup-mistakes#task#4 GetTable: expected $.__type 'InvalidInputException', got 'ValidationException'; $.Message missing (expected 'Table name cannot be equal to null or empty'). 3 steps in cluster.
Cause 6 — glue-tsd-attributes-to-get-names-then-describe#task#5 GetTables: expected $.__type 'InvalidInputException', got 'ValidationException'; $.Message missing (expected 'Attribute name is not a valid attribute to get.'). 3 steps in cluster.

## Steps to Reproduce
Replay the wire corpus against the emulator and inspect these glue steps:
1. glue-aorigin-get-tags-resource-arn-forms#task#5 — GetTags with ResourceArn set to a bare table name.
2. glue-aorigin-search-tables-resolves-the-database#task#7 — SearchTables with ResourceShareType 'foreign'.
3. glue-cflog-create-input-validation-mistakes#task#3 — CreateDatabase with DatabaseInput.Name ''.
4. glue-cflog-create-input-validation-mistakes#task#6 — CreateTable with TableInput.Name ''.
5. glue-cflog-table-lookup-mistakes#task#4 — GetTable with Name ''.
6. glue-tsd-attributes-to-get-names-then-describe#task#5 — GetTables with AttributesToGet ['name'].
Each returns 400 with __type ValidationException instead of InvalidInputException and the Glue Message text.
