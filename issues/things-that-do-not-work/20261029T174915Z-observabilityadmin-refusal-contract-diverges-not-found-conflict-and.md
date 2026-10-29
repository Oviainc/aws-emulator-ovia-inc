Issue-Source: trace-sweep-divergence
Issue-Trace: observabilityadmin-s3-table-integration-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: observabilityadmin-s3-table-integration-error-paths observabilityadmin-telemetry-pipeline-error-paths observabilityadmin-telemetry-rule-error-paths observabilityadmin-telemetry-rule-lifecycle
Issue-Shape-Id: com.amazonaws.observabilityadmin#ObservabilityAdmin
Issue-Key: observabilityadmin|GetS3TableIntegration|4xx->4xx|$.Message
Issue-Key: observabilityadmin|CreateS3TableIntegration|4xx->2xx|$.Message
Issue-Key: observabilityadmin|CreateS3TableIntegration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: observabilityadmin|CreateS3TableIntegration|4xx->2xx|status
Issue-Key: observabilityadmin|GetTelemetryPipeline|4xx->4xx|$.Message
Issue-Key: observabilityadmin|GetTelemetryPipeline|4xx->4xx|$.ResourceType
Issue-Key: observabilityadmin|CreateTelemetryPipeline|4xx->4xx|$.Message
Issue-Key: observabilityadmin|CreateTelemetryPipeline|4xx->4xx|$.ResourceType
Issue-Key: observabilityadmin|GetTelemetryRule|4xx->4xx|$.Message
Issue-Key: observabilityadmin|DeleteTelemetryRule|4xx->4xx|$.Message
Issue-Key: observabilityadmin|CreateTelemetryRule|4xx->4xx|$.Message
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# observabilityadmin refusal contract diverges: not-found, conflict and validation messages are emulator prose and the one-integration-per-account conflict is unserved

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetS3TableIntegration

## Description
The observabilityadmin emulator authors its own refusal bodies instead of serving AWS's. Across every refusal path the corpus exercises, the HTTP status and the error code are mostly right but the `Message` is emulator prose that interpolates the request identifier and the region, and `ResourceNotFoundException`/`ConflictException`.`ResourceType` carries the emulator's internal resource token rather than AWS's. One refusal is not served at all (the account-level S3 Table integration singleton). This issue bundles 8 distinct causes; ALL EIGHT must be closed before submitting.

Cause 1 of 8 — GetS3TableIntegration not-found message (fixture observabilityadmin-s3-table-integration-error-paths, step task#2).
AWS: Smithy: GetS3TableIntegration input Arn -> ResourceArn (@length 1..1011, @pattern ^arn:...); modeled error ResourceNotFoundException (httpError 404) with members Message, ResourceId, ResourceType. Recorded behavior: an Arn naming no integration is answered HTTP 404 with Message exactly 'Integration not found' — the whole message is fixed text, there is no variable part and the requested ARN is NOT interpolated.
Emulator: HTTP 404 (correct) but Message "no S3 Table integration '<requested arn>' in <region>" — emulator wording whose variable parts are the request's ARN and the region.
Why: the message text is part of the error contract; a client (or a test/log matcher) keyed on AWS's fixed sentence never matches, and the emulator leaks an internal phrasing that AWS never emits.
Fix: the not-found path of the GetS3TableIntegration handler (services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/, one file per operation) must raise ResourceNotFoundException with AWS's fixed text.

Cause 2 of 8 — CreateS3TableIntegration does not enforce one integration per account (fixture observabilityadmin-s3-table-integration-error-paths, step task#5).
AWS: Smithy: CreateS3TableIntegration models ConflictException (httpError 409). Recorded behavior: a CreateS3TableIntegration call in an account that already holds an integration is refused HTTP 409 with ConflictException, Message exactly 'Only one S3 Table integration is allowed per account' (entirely fixed text) and an x-amzn-errortype response header naming the error type.
Emulator: HTTP 200 with a success body (a second integration is created and a fresh Arn returned); no Message, and no x-amzn-errortype header at all.
Why: the account-level singleton invariant is broken — a client that relies on the conflict to detect an existing integration instead creates a duplicate, and subsequent reads/deletes address an account state AWS cannot produce.
Fix: CreateS3TableIntegration handler plus the integration state: reject when an integration already exists for the account, with ConflictException and AWS's message.

Cause 3 of 8 — GetTelemetryPipeline not-found message and ResourceType (fixture observabilityadmin-telemetry-pipeline-error-paths, step task#2).
AWS: Smithy: GetTelemetryPipeline input PipelineIdentifier (required, @length 1..512); modeled error ResourceNotFoundException (404) whose ResourceType is documented as 'The type of the resource which could not be found'. Recorded behavior: an unknown PipelineIdentifier is answered 404 with Message exactly 'Pipeline not found' (all fixed text) and ResourceType 'Pipeline'.
Emulator: 404 with Message "no telemetry pipeline '<identifier>' in <region>" and ResourceType 'telemetry-pipeline'.
Why: both the message and the ResourceType discriminator diverge; clients branching on ResourceType see an internal hyphenated token instead of AWS's resource name.
Fix: the not-found path of the GetTelemetryPipeline handler and the shared ResourceNotFoundException construction (ResourceType must carry AWS's resource name, not the emulator's internal token).

Cause 4 of 8 — CreateTelemetryPipeline duplicate-name conflict message and ResourceType (fixture observabilityadmin-telemetry-pipeline-error-paths, step task#5).
AWS: Smithy: CreateTelemetryPipeline input Name (required, @length 3..28, @pattern [a-z][a-z0-9\-]+); modeled error ConflictException (409) with Message/ResourceId/ResourceType. Recorded behavior: creating a pipeline whose Name already exists is refused 409 with Message exactly 'The specified PipelineName already exists' — fixed text that names the INPUT MEMBER, not the value — and ResourceType 'PipelineName'.
Emulator: 409 (status correct) with Message "a telemetry pipeline named '<name>' already exists in <region>" and ResourceType 'telemetry-pipeline'.
Why: AWS's wording identifies the conflicting member; the emulator's substitutes its own sentence and interpolates the submitted name, so neither the message nor ResourceType matches what a client parses.
Fix: the duplicate-name path of the CreateTelemetryPipeline handler.

Cause 5 of 8 — GetTelemetryRule not-found message (fixture observabilityadmin-telemetry-rule-error-paths, step task#4; same cause also hit by observabilityadmin-telemetry-rule-lifecycle#task#13, a read after delete).
AWS: Smithy: GetTelemetryRule input RuleIdentifier (required, @length 1..1011); modeled error ResourceNotFoundException (404). Recorded behavior: an unknown RuleIdentifier is answered 404 with Message exactly 'Rule not found.' — fixed text including the trailing period, with no identifier interpolated.
Emulator: 404 with Message "no telemetry rule '<identifier>' in <region>".
Why: as above — the refusal text is part of the contract, and the emulator's form is unique to the emulator.
Fix: the not-found path of the GetTelemetryRule handler.

Cause 6 of 8 — DeleteTelemetryRule not-found message uses the WRONG wording family (fixture observabilityadmin-telemetry-rule-error-paths, step task#5).
AWS: Smithy: DeleteTelemetryRule input RuleIdentifier (required); modeled error ResourceNotFoundException (404). Recorded behavior: deleting an unknown rule is answered 404 with Message 'Resource <rule ARN> does not exist.' — the fixed text is 'Resource ' ... ' does not exist.' and the single variable part is the rule's full ARN, which AWS derives from the request's RuleIdentifier plus the partition/region/account (arn:<partition>:observabilityadmin:<region>:<account>:telemetry-rule/<rule name>). Note this is deliberately a DIFFERENT sentence from GetTelemetryRule's 'Rule not found.' in cause 5.
Emulator: 404 with the same generic "no telemetry rule '<identifier>' in <region>" it uses for the read path, so it neither matches AWS's wording nor preserves the per-operation difference, and it never resolves the identifier to an ARN.
Why: a client distinguishing the delete refusal from the read refusal (or extracting the ARN from the message) cannot.
Fix: the not-found path of the DeleteTelemetryRule handler, with its own message template that renders the resolved rule ARN.

Cause 7 of 8 — CreateTelemetryRule Regions/AllRegions mutual-exclusion message (fixture observabilityadmin-telemetry-rule-error-paths, step task#8).
AWS: Smithy: TelemetryRule.Regions (@length min 1, uniqueItems) and TelemetryRule.AllRegions are documented as 'Mutually exclusive with AllRegions' / 'Mutually exclusive with Regions'; CreateTelemetryRule models ValidationException (400). Recorded behavior: supplying both is refused HTTP 400 with Message exactly "Parameters 'regions' and 'allRegions' are mutually exclusive. Provide one or the other, not both." — entirely fixed text, and it names the parameters in lowerCamel wire form ('regions', 'allRegions'), not the Rust/shape path.
Emulator: 400 (status correct) with Message 'Rule.Regions and Rule.AllRegions are mutually exclusive; supply one of them' — the emulator's own sentence, naming the members by shape path.
Fix: the request-validation step of the CreateTelemetryRule handler.
Why: a client that surfaces the validation message to a user, or matches on it, sees text AWS never produces and member names that do not match its own wire payload.

Cause 8 of 8 — CreateTelemetryRule duplicate-rule conflict message (fixture observabilityadmin-telemetry-rule-error-paths, step task#9).
AWS: Smithy: CreateTelemetryRule models ConflictException (409). Recorded behavior: creating a rule whose RuleName already exists is refused 409 with Message exactly 'Telemetry Rule already exists.' — all fixed text, including the trailing period, with the rule name NOT interpolated.
Emulator: 409 with Message "a telemetry rule named '<name>' already exists in <region>".
Why: same contract break as the other refusals; the emulator interpolates request values into a message AWS keeps constant.
Fix: the duplicate-name path of the CreateTelemetryRule handler.

Across all eight causes the shared defect is that refusal bodies are composed by the emulator's own error helpers rather than by per-operation AWS message templates, and that ResourceType is populated from the emulator's internal resource token. Note the only other open observabilityadmin issue is '20261029T064523Z-observabilityadmin-lists3tableintegrations-stub.md' (ListS3TableIntegrations stub), which covers none of these.

## Evidence
Cause 1: GetS3TableIntegration — expected 404 / $.Message 'Integration not found'; actual 404 / $.Message "no S3 Table integration 'arn:aws:observabilityadmin:<region>:<account>:s3tableintegration/<id>' in <region>".
Cause 2: CreateS3TableIntegration (second create) — expected 409 with $.Message 'Only one S3 Table integration is allowed per account' and header x-amzn-errortype present; actual 200, $.Message missing, x-amzn-errortype missing.
Cause 3: GetTelemetryPipeline — expected $.Message 'Pipeline not found', $.ResourceType 'Pipeline'; actual "no telemetry pipeline '<name>' in <region>" and 'telemetry-pipeline'.
Cause 4: CreateTelemetryPipeline — expected $.Message 'The specified PipelineName already exists', $.ResourceType 'PipelineName'; actual "a telemetry pipeline named '<name>' already exists in <region>" and 'telemetry-pipeline'.
Cause 5: GetTelemetryRule — expected $.Message 'Rule not found.'; actual "no telemetry rule '<name>' in <region>" (2 affected steps).
Cause 6: DeleteTelemetryRule — expected $.Message 'Resource <rule arn> does not exist.'; actual "no telemetry rule '<name>' in <region>".
Cause 7: CreateTelemetryRule — expected $.Message "Parameters 'regions' and 'allRegions' are mutually exclusive. Provide one or the other, not both."; actual 'Rule.Regions and Rule.AllRegions are mutually exclusive; supply one of them'.
Cause 8: CreateTelemetryRule — expected $.Message 'Telemetry Rule already exists.'; actual "a telemetry rule named '<name>' already exists in <region>".
Identifiers, ARNs, region and account values above are the per-run values of the recording and are not themselves requirements; the fixed sentences and the ResourceType tokens are.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1/2: fixture observabilityadmin-s3-table-integration-error-paths — step task#2 (GetS3TableIntegration on an unknown integration ARN) and step task#5 (CreateS3TableIntegration when one already exists).
3. Cause 3/4: fixture observabilityadmin-telemetry-pipeline-error-paths — step task#2 (GetTelemetryPipeline for a missing pipeline) and step task#5 (CreateTelemetryPipeline with an existing Name).
4. Cause 5/6/7/8: fixture observabilityadmin-telemetry-rule-error-paths — step task#4 (GetTelemetryRule, missing rule), step task#5 (DeleteTelemetryRule, missing rule), step task#8 (CreateTelemetryRule with both Rule.Regions and Rule.AllRegions), step task#9 (CreateTelemetryRule with a duplicate RuleName). Cause 5 also reproduces at observabilityadmin-telemetry-rule-lifecycle step task#13.
5. Compare the response status, the x-amzn-errortype header and the $.Message / $.ResourceType members against the recorded responses.
