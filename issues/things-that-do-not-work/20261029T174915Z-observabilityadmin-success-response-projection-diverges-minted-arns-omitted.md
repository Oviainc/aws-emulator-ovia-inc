Issue-Source: trace-sweep-divergence
Issue-Trace: observabilityadmin-s3-table-integration-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: observabilityadmin-s3-table-integration-error-paths observabilityadmin-telemetry-pipeline-error-paths observabilityadmin-telemetry-enrichment-lifecycle observabilityadmin-telemetry-pipeline-workflow observabilityadmin-telemetry-rule-lifecycle
Issue-Shape-Id: com.amazonaws.observabilityadmin#ObservabilityAdmin
Issue-Key: observabilityadmin|CreateS3TableIntegration|2xx->2xx|$.Arn
Issue-Key: observabilityadmin|CreateTelemetryPipeline|2xx->2xx|$.Arn
Issue-Key: observabilityadmin|StartTelemetryEnrichment|2xx->2xx|$.AwsResourceExplorerManagedViewArn
Issue-Key: observabilityadmin|ValidateTelemetryPipelineConfiguration|2xx->2xx|$.Errors[*]
Issue-Key: observabilityadmin|GetTelemetryPipeline|2xx->2xx|$.Pipeline.ConfigurationSummary
Issue-Key: observabilityadmin|GetTelemetryPipeline|2xx->2xx|$.Pipeline.StatusReason
Issue-Key: observabilityadmin|GetTelemetryPipeline|2xx->2xx|$.Pipeline.Tags
Issue-Key: observabilityadmin|GetTelemetryRule|2xx->2xx|$.TelemetryRule.TelemetrySourceTypes
Issue-Key: observabilityadmin|ListTelemetryRules|2xx->2xx|$.TelemetryRuleSummaries[*]
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# observabilityadmin success-response projection diverges: minted ARNs, omitted pipeline and rule members, and pipeline-validation error vocabulary

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateS3TableIntegration

## Description
On the paths that SUCCEED, the observabilityadmin emulator's 2xx bodies are not the ones AWS serves: the ARNs it mints do not have AWS's shape, the telemetry-enrichment managed-view ARN does not correlate with the view the enrichment actually uses, pipeline-configuration validation reports a single emulator-authored error instead of AWS's per-problem schema errors, and the pipeline and rule read/list projections drop modeled members that the create call supplied. This issue bundles 8 distinct causes; ALL EIGHT must be closed before submitting.

Cause 1 of 8 — CreateS3TableIntegration mints an ARN that is not AWS's integration-ARN form (fixture observabilityadmin-s3-table-integration-error-paths, step task#4).
AWS: Smithy: CreateS3TableIntegration output Arn -> ResourceArn (@length 1..1011, @pattern ^arn:aws([a-z0-9\-]+)?:([a-zA-Z0-9\-]+):([a-z0-9\-]+)?:([0-9]{12})?:(.+)$). Recorded behavior: S3 Table integration ARNs use the resource-type segment 's3tableintegration' (unhyphenated) — the same fixture's GetS3TableIntegration step addresses the integration as arn:<partition>:observabilityadmin:<region>:<account>:s3tableintegration/<integration id>. The replay asserts the member's shape (not a literal value) and the emulator's value fails that assertion.
Emulator: mints arn:<partition>:observabilityadmin:<region>:<account>:s3-table-integration/<random uuid> — a hyphenated resource-type segment and a fresh random uuid per call. Hypothesis: the shape assertion keys on the resource-type segment, which is the only structural difference visible in the recording.
Why: the ARN is the handle a client stores, puts in IAM policies and passes back to GetS3TableIntegration/Delete; an ARN whose resource type differs from AWS's cannot be matched by a policy or constructed by a client, and the account holds at most one integration so a random uuid also makes the handle unreproducible.
Fix: ARN minting in the CreateS3TableIntegration handler / the integration state that stores it (and the Get path must address the same form).

Cause 2 of 8 — CreateTelemetryPipeline mints an Arn that fails the recorded member's shape (fixture observabilityadmin-telemetry-pipeline-error-paths, step task#4).
AWS: Smithy: CreateTelemetryPipeline output Arn -> ResourceArn (@length 1..1011, same @pattern); input Name @length 3..28 @pattern [a-z][a-z0-9\-]+. Recorded behavior only: the recording asserts the Arn member's shape rather than a literal, and AWS's pipeline ARN does not have the shape the emulator produces.
Emulator: returns arn:<partition>:observabilityadmin:<region>:<account>:telemetry-pipeline/<submitted Name> — the resource identifier is simply the request's Name. Hypothesis: as with the S3 Table integration ARN, AWS's pipeline ARN differs structurally (unhyphenated resource type and/or an opaque service-assigned pipeline id rather than the submitted name); the fixer should re-derive the ARN template from the recorded CreateTelemetryPipeline/GetTelemetryPipeline responses rather than from the request.
Why: same consequence as cause 1 — the minted ARN is the resource handle, and a non-AWS form breaks policy matching and any client that parses it.
Fix: ARN minting in the CreateTelemetryPipeline handler / pipeline state.

Cause 3 of 8 — StartTelemetryEnrichment returns a constant managed-view ARN that does not correlate with the enrichment's view (fixture observabilityadmin-telemetry-enrichment-lifecycle, step task#1).
AWS: Smithy: StartTelemetryEnrichment is HTTP 202 and returns AwsResourceExplorerManagedViewArn -> @length 20..2048, @pattern ^arn:aws([a-z0-9\-]+)?:resource-explorer-2:([a-z0-9\-]+)?:([0-9]{12})?:managed-view/(.+)$. Recorded behavior: the ARN returned is the ARN of the Resource Explorer managed view the enrichment actually uses, and the fixture binds it as a variable ({{managed_view_arn}}) to be matched against later steps — i.e. the value must be the one the service records in state, not a constant.
Emulator: answers 202 (status correct) with the hard-coded 'arn:<partition>:resource-explorer-2:<region>:<account>:managed-view/AWSManagedViewForResourceTagsForTelemetry', which does not equal the bound value. Hypothesis: AWS's managed-view ARN carries a service-assigned unique suffix after the view name, which the emulator's constant lacks.
Why: a client takes this ARN straight to resource-explorer-2 (Search/GetManagedView); an ARN that names no view it can resolve, and that does not match what the status read reports, breaks the enrichment workflow.
Fix: the StartTelemetryEnrichment handler and the enrichment state — mint/record one managed view per account and return its ARN, the same value GetTelemetryEnrichmentStatus reports.

Cause 4 of 8 — ValidateTelemetryPipelineConfiguration does not report AWS's schema-validation errors for a non-pipeline body (fixture observabilityadmin-telemetry-pipeline-error-paths, step task#6).
AWS: Smithy: ValidateTelemetryPipelineConfiguration returns 200 with Errors -> list of ValidationError { Message, Reason, FieldMap } ('A mapping of field names to specific validation issues within the configuration'). Recorded behavior: AWS returns ONE ValidationError per schema problem, with Reason drawn from a fixed vocabulary and schema-validator wording whose variable parts are the offending property name/path: Reason 'MISSING_FIELD' with Message "required property '<name>' not found" and FieldMap {'<name>': ...}, and Reason 'UNKNOWN_PROPERTY' with Message "property '<name>' is not defined in the schema and the schema does not allow additional properties" and FieldMap {'path': '<json path, e.g. root>'}.
Emulator: returns a single entry with Reason 'PIPELINE_CONFIGURATION_VALIDATION_ERROR' and its own sentence ('Configuration.Body declares no pipeline section carrying a source, a processor or a sink'), FieldMap keyed 'Configuration.Body'; AWS's second error entry is absent entirely, so even the number of Errors differs.
Why: the Reason codes and FieldMap paths are what a console or CLI uses to point the user at the offending line of the pipeline document; one opaque emulator reason makes a misconfiguration undiagnosable and no AWS Reason code is ever observable.
Fix: the configuration validator behind the ValidateTelemetryPipelineConfiguration handler — validate the configuration body against the pipeline schema and emit one ValidationError per problem using AWS's Reason vocabulary, message templates and FieldMap keys.

Cause 5 of 8 — ValidateTelemetryPipelineConfiguration does not report the per-source processor restrictions (fixture observabilityadmin-telemetry-pipeline-workflow, step task#3).
AWS: same operation/shape as cause 4. Recorded behavior: for a configuration whose sources carry disallowed or excess processors, AWS returns an entry per violation with Reason 'DATA_SOURCE_PROCESSOR_RESTRICTED' — FieldMap carrying the restriction's facts (source_type, data_source_name, allowed_processor / allowed_processors) and messages such as 'must have at most 1 items but found 2' or "required property '<name>' not found" — interleaved with Reason 'UNKNOWN_PROPERTY' entries whose FieldMap {'path': 'processor[<index>]'} names the offending processor position. Nine entries were expected for this body; the fixed text is the validator wording and the variable parts are the source type, data-source name, allowed processor list, property name, path index and item count.
Emulator: returns two entries, both Reason 'PIPELINE_CONFIGURATION_VALIDATION_ERROR', with its own 'declares no source' / 'declares no sink' sentences and FieldMap keyed 'source'/'sink'; entries 3..9 are absent.
Why: the restriction vocabulary is the only way a client learns WHICH processor is not permitted for WHICH source type; the emulator instead reports an unrelated 'no source/no sink' conclusion about a configuration that does declare them.
Fix: the per-source-type processor restriction rules in the same configuration validator.

Cause 6 of 8 — GetTelemetryPipeline omits Pipeline.ConfigurationSummary, Pipeline.StatusReason and Pipeline.Tags (fixture observabilityadmin-telemetry-pipeline-workflow, step task#5).
AWS: Smithy: TelemetryPipeline models StatusReason -> TelemetryPipelineStatusReason { Description } and Tags -> TagMapOutput; the model also defines ConfigurationSummary { Sources, DataSources, Processors, ProcessorCount, Sinks }. Recorded behavior: GetTelemetryPipeline for an active pipeline returns ConfigurationSummary derived from the submitted configuration (Sources[].Type, DataSources[].Name/.Type, the Processors list and its ProcessorCount, Sinks), StatusReason.Description describing the active state, and Tags as an EMPTY MAP when the pipeline has no tags.
Emulator: all three members are absent from the Pipeline structure.
Why: a client inspecting a pipeline's composition or the reason behind its status gets nothing, and an absent Tags is not the same observation as an empty tag map (a client cannot distinguish 'no tags' from 'not reported').
Fix: the GetTelemetryPipeline projection/serializer — derive ConfigurationSummary from the stored configuration and always project StatusReason and Tags.

Cause 7 of 8 — GetTelemetryRule drops TelemetryRule.TelemetrySourceTypes (fixture observabilityadmin-telemetry-rule-lifecycle, step task#4; the same cause is also hit at step task#9, the read after update).
AWS: Smithy: TelemetryRule.TelemetrySourceTypes -> TelemetrySourceTypes (list of the TelemetrySourceType enum), documented as 'The specific telemetry source types to configure for the resource, such as VPC_FLOW_LOGS or EKS_AUDIT_LOGS'. Recorded behavior: a rule created with TelemetrySourceTypes reads back with the same list on GetTelemetryRule.
Emulator: $.TelemetryRule.TelemetrySourceTypes is absent, although the create call supplied it — a round-trip loss of a modeled member.
Why: the source types are the substance of the rule; a client reading a rule to render or re-apply it loses the configuration it just submitted.
Fix: the telemetry-rule state (persist TelemetrySourceTypes) and the GetTelemetryRule projection.

Cause 8 of 8 — ListTelemetryRules summaries drop TelemetrySourceTypes (fixture observabilityadmin-telemetry-rule-lifecycle, step task#5).
AWS: Smithy: TelemetryRuleSummary models TelemetrySourceTypes alongside RuleName, RuleArn, CreatedTimeStamp, LastUpdateTimeStamp, ResourceType and TelemetryType. Recorded behavior: each summary carries the rule's TelemetrySourceTypes list.
Emulator: the summary carries RuleName, RuleArn, the timestamps, ResourceType and TelemetryType but no TelemetrySourceTypes. Same root as cause 7 (the member is not kept/projected), but it is a separate projection and must be fixed in the list path too.
Why: a client that lists rules to pick one by source type cannot.
Fix: the ListTelemetryRules summary projection.
(The recorded timestamps and ARNs in this step are per-run values, asserted only for shape; they are not requirements.)

The only other open observabilityadmin issue is '20261029T064523Z-observabilityadmin-lists3tableintegrations-stub.md' (ListS3TableIntegrations stub); it covers none of these causes.

## Evidence
Cause 1: CreateS3TableIntegration, expected 200/Arn shape-asserted; actual 200 with $.Arn 'arn:aws:observabilityadmin:<region>:<account>:s3-table-integration/<uuid>' failing the recorded shape — AWS's recorded integration ARNs use the resource type 's3tableintegration'.
Cause 2: CreateTelemetryPipeline, expected 200/Arn shape-asserted; actual $.Arn 'arn:aws:observabilityadmin:<region>:<account>:telemetry-pipeline/<submitted Name>' failing the recorded shape.
Cause 3: StartTelemetryEnrichment, expected 202 with $.AwsResourceExplorerManagedViewArn == the fixture-bound managed-view ARN ({{managed_view_arn}}); actual 202 with the constant 'arn:aws:resource-explorer-2:<region>:<account>:managed-view/AWSManagedViewForResourceTagsForTelemetry'.
Cause 4: ValidateTelemetryPipelineConfiguration, expected Errors[0] {Reason 'MISSING_FIELD', Message "required property '<name>' not found", FieldMap {'<name>': ...}} and Errors[1] {Reason 'UNKNOWN_PROPERTY', Message "property '<name>' is not defined in the schema and the schema does not allow additional properties", FieldMap {'path': 'root'}}; actual a single {Reason 'PIPELINE_CONFIGURATION_VALIDATION_ERROR', Message 'Configuration.Body declares no pipeline section carrying a source, a processor or a sink', FieldMap {'Configuration.Body': ...}} and Errors[1] = None.
Cause 5: ValidateTelemetryPipelineConfiguration, expected nine entries with Reasons 'DATA_SOURCE_PROCESSOR_RESTRICTED' (FieldMap source_type/data_source_name/allowed_processor(s), messages like 'must have at most 1 items but found 2') and 'UNKNOWN_PROPERTY' (FieldMap {'path': 'processor[0]'}, 'processor[1]'); actual two 'PIPELINE_CONFIGURATION_VALIDATION_ERROR' entries ('declares no source' / 'declares no sink') and Errors[2..8] = None.
Cause 6: GetTelemetryPipeline, expected $.Pipeline.ConfigurationSummary {Sources, DataSources, Processors, ProcessorCount, Sinks}, $.Pipeline.StatusReason {Description}, $.Pipeline.Tags {}; actual all three None.
Cause 7: GetTelemetryRule, expected $.TelemetryRule.TelemetrySourceTypes ['VPC_FLOW_LOGS'] (as created); actual None (2 affected steps).
Cause 8: ListTelemetryRules, expected $.TelemetryRuleSummaries[0] to include TelemetrySourceTypes ['VPC_FLOW_LOGS']; actual the summary without that member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture observabilityadmin-s3-table-integration-error-paths, step task#4 — CreateS3TableIntegration; inspect $.Arn's resource-type and identifier segments.
3. Cause 2: fixture observabilityadmin-telemetry-pipeline-error-paths, step task#4 — CreateTelemetryPipeline; inspect $.Arn.
4. Cause 3: fixture observabilityadmin-telemetry-enrichment-lifecycle, step task#1 — StartTelemetryEnrichment; compare $.AwsResourceExplorerManagedViewArn with the managed-view ARN the fixture binds.
5. Cause 4: fixture observabilityadmin-telemetry-pipeline-error-paths, step task#6 — ValidateTelemetryPipelineConfiguration with a body that is not a pipeline document; compare every Errors[] entry's Reason/Message/FieldMap and the entry count.
6. Cause 5: fixture observabilityadmin-telemetry-pipeline-workflow, step task#3 — ValidateTelemetryPipelineConfiguration with per-source processor restrictions; same comparison.
7. Cause 6: fixture observabilityadmin-telemetry-pipeline-workflow, step task#5 — GetTelemetryPipeline after a successful create; check Pipeline.ConfigurationSummary / StatusReason / Tags.
8. Causes 7 and 8: fixture observabilityadmin-telemetry-rule-lifecycle, steps task#4 (and task#9) and task#5 — create a rule with TelemetrySourceTypes, then GetTelemetryRule and ListTelemetryRules and check TelemetrySourceTypes.
