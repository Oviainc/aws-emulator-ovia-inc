Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-aorigin-stack-first-template-and-outputs
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-aorigin-stack-first-template-and-outputs cloudformation-cfns3cnt-deleted-bucket-stack-handles cloudformation-cfns3cnt-template-summary-and-describe-route cloudformation-dcsr-template-route cloudformation-tsrb-template-summary-type-route
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member[*].ResourceIdentifiers.member
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member.ResourceIdentifiers.member
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member[*]
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member[*].LogicalResourceIds.member[*]
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member[*].ResourceType
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member[*]
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFormation GetTemplateSummary synthesizes resource-identifier summaries and ResourceTypes wrongly

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplateSummary

## Description
GetTemplateSummary answers 200 but its ResourceIdentifierSummaries and ResourceTypes projections are synthesized from the template instead of from each resource type's importable identifier set: the emulator invents a single '<Type>Name' identifier per resource type, emits summaries for types that are not importable, and collapses/reorders the type list. Smithy: GetTemplateSummary output members ResourceIdentifierSummaries (list of ResourceIdentifierSummary) and ResourceTypes (list of ResourceType, a list and not a set); ResourceIdentifierSummary is documented as 'Describes the target resources of a specific type in your import template (for example, all AWS::S3::Bucket resources) and the properties you can provide during the import to identify resources of that type.' — i.e. the member names come from the resource type's own import identifiers, not from the type name. This bundle carries 5 distinct causes. Open issue 20261103T171935Z-cloudformation-template-validation-and-gettemplatesummary-diverge-on-limits.md covers neighbouring GetTemplateSummary checks (template-source refusals, limits, Warnings) and owns NONE of these replay keys (owned_by is empty for all five clusters), so each must be closed here.

Cause 1 of 5 — ResourceIdentifiers for a type whose import identifier is composite (AWS::Glue::Table).
AWS: the summary for AWS::Glue::Table lists that type's importable identifier members — CatalogId, DatabaseName and Name. Smithy: ResourceIdentifierSummary.ResourceIdentifiers is the set of properties a caller may supply to identify a resource of that type during import; recorded behavior confirms the three-member list for AWS::Glue::Table.
Emulator: a single identifier 'TableName', derived from the resource type's last name segment plus the literal 'Name'.
Why: a client building an import ChangeSet reads ResourceIdentifiers to know which keys the ResourceToImport entries must carry; with 'TableName' it sends a key AWS does not accept and omits the two keys AWS requires, so every resource import of that type is malformed.
Fix: GetTemplateSummary's ResourceIdentifierSummaries projection — the per-resource-type identifier table it reads, not the operation's validation.
Traces: cloudformation-aorigin-stack-first-template-and-outputs#task#9.

Cause 2 of 5 — ResourceIdentifiers for a type identified by ARN (AWS::S3Tables::TableBucket).
AWS: the identifier member is 'TableBucketARN'.
Emulator: 'TableBucketName'.
Why: same as cause 1 — the caller is told to identify the resource by name when AWS identifies it by ARN, and the import request is rejected or resolves nothing. It also hides the fact that the identifier is an ARN at all.
Fix: GetTemplateSummary's ResourceIdentifierSummaries projection (identifier table for AWS::S3Tables::TableBucket).
Traces: cloudformation-cfns3cnt-deleted-bucket-stack-handles#task#16.

Cause 3 of 5 — multi-identifier types are truncated and the summary list order differs.
AWS: for a template declaring both AWS::S3Vectors::VectorBucket and AWS::S3Tables::TableBucket, the S3Vectors summary carries BOTH of that type's identifier members (VectorBucketArn and VectorBucketName) and the S3Tables summary carries TableBucketARN; the two summaries appear in the recorded order with the S3Vectors entry first.
Emulator: emits one identifier per type ('VectorBucketArn' only, and 'TableBucketName' for the S3Tables type) and emits the two summaries in the opposite order.
Why: a client that can only supply one of several accepted identifiers (for example the name, where AWS accepts name or ARN) is told that identifier is unsupported; and a client that indexes ResourceIdentifierSummaries positionally pairs identifiers with the wrong resource type. Hypothesis: the projection keeps a single identifier per type and orders the list by its internal resource-type registry rather than preserving AWS's ordering.
Fix: GetTemplateSummary's ResourceIdentifierSummaries projection — emit the complete identifier set per type and preserve the ordering AWS reports.
Traces: cloudformation-cfns3cnt-template-summary-and-describe-route#task#5.

Cause 4 of 5 — non-importable resource types are reported as importable, and AWS::SSM::Parameter's identifier is renamed.
AWS: for a CDK-synthesized template, ResourceIdentifierSummaries contains exactly ONE summary — AWS::SSM::Parameter with identifier member 'Name' and LogicalResourceIds 'Param'. AWS::CDK::Metadata produces no summary, because a type with no import identifiers is not summarised.
Emulator: returns two summaries — an invented AWS::CDK::Metadata entry whose identifier is 'MetadataName', plus AWS::SSM::Parameter whose identifier is given as 'ParameterName' instead of 'Name'.
Why: the caller is told a metadata-only resource can be imported (it cannot) and is given the wrong key name for the one resource that can, so an import plan derived from the summary is wrong on both entries.
Fix: GetTemplateSummary's ResourceIdentifierSummaries projection — omit types with no importable identifiers and take identifier names from the type's identifier table.
Traces: cloudformation-dcsr-template-route#task#7.

Cause 5 of 5 — ResourceTypes is de-duplicated/reordered and WaitConditionHandle is summarised as importable.
AWS: ResourceTypes lists one entry per DECLARED resource (Smithy: ResourceTypes is a list of ResourceType, not a set), so a template declaring the same type several times repeats that type the same number of times — the recorded response repeats AWS::CloudFormation::WaitConditionHandle and AWS::SSM::Parameter at the positions where they are declared. In ResourceIdentifierSummaries the AWS::SSM::Parameter summary carries identifier 'Name' and groups all of that type's logical ids into one entry; AWS::CloudFormation::WaitConditionHandle contributes no summary.
Emulator: returns a shorter, collapsed ResourceTypes list (entries expected at positions 1, 3 and 5 are absent or replaced by a different type), and its first summary is an AWS::CloudFormation::WaitConditionHandle entry with identifier 'WaitConditionHandleName' listing the handle logical ids, with the SSM::Parameter summary pushed to a later position.
Why: callers size a deployment and build an import plan from these lists; a collapsed ResourceTypes understates the resource count, and a summary for a type AWS never offers for import sends the caller down an import path that cannot work.
Fix: GetTemplateSummary's ResourceTypes projection (one entry per declared resource, template order) and its ResourceIdentifierSummaries projection (skip non-importable types).
Traces: cloudformation-tsrb-template-summary-type-route#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 5 divergence clusters, all verdict=mismatch, status 200 expected and 200 returned, all with stub_path='' and owned_by=[].
Cause 1 — $.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceIdentifierSummaries.member[1].ResourceIdentifiers.member: value_mismatch, expected 'CatalogId,DatabaseName,Name', got 'TableName'.
Cause 2 — ...ResourceIdentifierSummaries.member.ResourceIdentifiers.member: value_mismatch, expected 'TableBucketARN', got 'TableBucketName'.
Cause 3 — ...ResourceIdentifierSummaries.member[1]: expected {ResourceType: 'AWS::S3Vectors::VectorBucket', LogicalResourceIds: {member: 'Vec'}, ResourceIdentifiers: {member: ['VectorBucketArn','VectorBucketName']}}, got {ResourceType: 'AWS::S3Tables::TableBucket', LogicalResourceIds: {member: 'Tab'}, ResourceIdentifiers: {member: 'TableBucketName'}}; member[2]: expected the AWS::S3Tables::TableBucket entry with ResourceIdentifiers 'TableBucketARN', got the AWS::S3Vectors::VectorBucket entry with ResourceIdentifiers 'VectorBucketArn'.
Cause 4 — ...ResourceIdentifierSummaries.member: type_mismatch, expected the single object {ResourceType: 'AWS::SSM::Parameter', LogicalResourceIds: {member: 'Param'}, ResourceIdentifiers: {member: 'Name'}}, got a two-element list [{ResourceType: 'AWS::CDK::Metadata', ResourceIdentifiers: {member: 'MetadataName'}}, {ResourceType: 'AWS::SSM::Parameter', ResourceIdentifiers: {member: 'ParameterName'}}].
Cause 5 — ...ResourceTypes.member[1]: expected 'AWS::CloudFormation::WaitConditionHandle', got 'AWS::Logs::LogGroup'; ResourceTypes.member[3] expected 'AWS::CloudFormation::WaitConditionHandle', got None; ResourceTypes.member[5] expected 'AWS::SSM::Parameter', got None; ResourceIdentifierSummaries.member[0].ResourceType: expected 'AWS::SSM::Parameter', got 'AWS::CloudFormation::WaitConditionHandle'; member[0].ResourceIdentifiers.member expected 'Name', got 'WaitConditionHandleName'; member[0].LogicalResourceIds expected the parameter logical ids, got the handle logical ids; member[2] extra (the AWS::SSM::Parameter summary with ResourceIdentifiers 'ParameterName').
Smithy (services/service-com-amazonaws-cloudformation-cloudformation): GetTemplateSummary output ResourceTypes -> ResourceTypes (list), ResourceIdentifierSummaries -> ResourceIdentifierSummaries (list); ResourceIdentifierSummary documents ResourceIdentifiers as the properties usable to identify resources of that type during import.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-aorigin-stack-first-template-and-outputs, step task#9 — GetTemplateSummary for a template declaring AWS::Glue::Table; compare ResourceIdentifierSummaries[*].ResourceIdentifiers against the recorded response.
3. Cause 2: fixture cloudformation-cfns3cnt-deleted-bucket-stack-handles, step task#16 — template declaring AWS::S3Tables::TableBucket.
4. Cause 3: fixture cloudformation-cfns3cnt-template-summary-and-describe-route, step task#5 — template declaring AWS::S3Vectors::VectorBucket and AWS::S3Tables::TableBucket; check both the identifier sets and the order of the summaries.
5. Cause 4: fixture cloudformation-dcsr-template-route, step task#7 — CDK-synthesized template with AWS::CDK::Metadata and AWS::SSM::Parameter; assert exactly one summary, for AWS::SSM::Parameter with identifier 'Name'.
6. Cause 5: fixture cloudformation-tsrb-template-summary-type-route, step task#4 — template declaring several WaitConditionHandles and several SSM parameters; assert ResourceTypes has one entry per declared resource and that no WaitConditionHandle summary is emitted.
7. For every step compare status, ResourceTypes and ResourceIdentifierSummaries (member names, grouping and order) against the recorded AWS response.
