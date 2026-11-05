Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-component-rejects-a-two-part-semantic-version
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-component-rejects-a-two-part-semantic-version imagebuilder-ibp-traj-component-request-mistakes imagebuilder-ibp-traj-distribution-config-mistakes imagebuilder-ibp-traj-pipeline-create-mistakes imagebuilder-ibp-traj-component-listing-and-paging
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateComponent|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateComponent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateImagePipeline|4xx->4xx|$.message
Issue-Key: imagebuilder|ListComponents|4xx->4xx|$.message
Issue-Key: imagebuilder|ListComponents|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateImagePipeline|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 4f520f1edeaf87046d1e59f82118bd1eb9149075

# Image Builder refusal error types and messages are emulator-authored, not AWS wording

> _Note: this issue was identified against the code base at commit `4f520f1edeaf87046d1e59f82118bd1eb9149075`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponent

## Description
This bundle carries SEVEN distinct causes. All of them are the same class of defect: the Image Builder emulator refuses the request with the right HTTP status but invents its own error type and/or its own message text instead of the error class and message AWS returns for that refusal. Every cause must be closed before this issue is submitted. (Related but NOT covering: open issues 20261102T085510Z-imagebuilder-image-builder-not-found-and-already-exists-refusals.md and 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md — each carries its own replay keys and none of them carry the fixture steps listed below.)

Cause 1 of 7: CreateComponent duplicate-component refusal uses ResourceInUseException and an invented message.
AWS: the recorded 400 response carries x-amzn-errortype ResourceAlreadyExistsException and the message "The following resource 'Component' already exists: '<componentBuildVersionArn>'." The fixed text is "The following resource 'Component' already exists: '" ... "'."; the only variable part is the component build-version ARN of the already-existing component, taken from the emulator's state (account, region, component name, semantic version, build number). Smithy: CreateComponent -> ResourceAlreadyExistsException (client, httpError 400, "The resource that you are trying to create already exists.") and ResourceInUseException (client, 400, "The resource that you are trying to operate on is currently in use.") are distinct shapes.
Emulator: 400 with x-amzn-errortype ResourceInUseException and message "The component <arn> already exists with identical content." — a different error class and wording invented by the emulator.
Why: a client that branches on ResourceAlreadyExistsException (the normal create-idempotency path) never sees it and instead gets ResourceInUseException, which clients treat as a retryable in-use condition; the retry loops and the create never resolves.
Fix: the CreateComponent handler's duplicate-detection branch in the imagebuilder service crate — it must raise the already-exists error shape and format AWS's message template.
Traces: imagebuilder-create-component-rejects-a-two-part-semantic-version#task#3 (also imagebuilder-ibp-traj-component-request-mistakes#task#2, #task#4).

Cause 2 of 7: CreateDistributionConfiguration duplicate-name refusal message is emulator-authored.
AWS: 400 with message "The following resource 'DistributionConfiguration' already exists: '<distributionConfigurationArn>'." Fixed text as above with 'DistributionConfiguration' as the resource label; the variable part is the ARN of the existing distribution configuration, from state.
Emulator: 400 with "A distribution configuration named <name> already exists in this Amazon Web Services Region." — AWS's template is ARN-based and resource-labelled, not name-and-region prose.
Why: callers and test harnesses that parse the ARN out of the already-exists message to adopt the existing resource cannot find it, so the conflict is unrecoverable from the message alone.
Fix: the CreateDistributionConfiguration handler's conflict branch / its error message formatting.
Traces: imagebuilder-ibp-traj-distribution-config-mistakes#task#5.

Cause 3 of 7: CreateImagePipeline duplicate-name refusal message is emulator-authored.
AWS: 400 with message "The following resource 'Pipeline' already exists: '<imagePipelineArn>'." Note the resource label AWS uses here is 'Pipeline' (not 'ImagePipeline'); the variable part is the existing pipeline's ARN, from state.
Emulator: 400 with "An image pipeline named <name> already exists in this Amazon Web Services Region."
Why: same as cause 2 — the ARN is absent and the fixed text differs, so message-based handling diverges.
Fix: the CreateImagePipeline handler's conflict branch / its error message formatting.
Traces: imagebuilder-ibp-traj-pipeline-create-mistakes#task#13.

Cause 4 of 7: ListComponents unsupported-filter refusal uses InvalidRequestException and lists the allowed filters in the wrong form.
AWS: 400 with message "The value supplied for parameter 'filters' is not valid. Allowed filters for ListComponents are: name, version, description, platform, type, supportedOsVersion, productCodes, status." The whole string is fixed text for this error class: the parameter named is the member 'filters' (not the offending filter's name), and the allowed-filter list is given in AWS's own order. The recorded x-amzn-errortype is a different error shape than the one the emulator sends. Smithy: ListComponents.filters -> FilterList; modeled errors include InvalidRequestException ("The request is malformed or otherwise invalid") and the service models InvalidParameterValueException ("The value that you provided for the specified parameter is invalid"). Hypothesis: the "value supplied for parameter ... is not valid" wording is InvalidParameterValueException's message class, so that is the error type AWS returns here.
Emulator: 400 with x-amzn-errortype InvalidRequestException and message "The filter name <filter> is not supported. The supported filter names are description, name, platform, productCodes, status, supportedOsVersion, type, version." — wrong error class, wrong fixed text, and the filter list re-sorted alphabetically.
Fix: ListComponents' input validation in the imagebuilder service crate.
Traces: imagebuilder-ibp-traj-component-listing-and-paging#task#4 (also #task#7).

Cause 5 of 7: ListComponents maxResults refusal message wording.
AWS: 400 with message "The value supplied for parameter 'maxResults' is not valid. maxResults must be bigger than or equal to 1" — the fixed text uses "bigger than or equal to"; the member name and the bound come from the model (Smithy: ListComponents.maxResults -> RestrictedInteger, range min 1 max 25).
Emulator: same status, but the message reads "... maxResults must be greater than or equal to 1".
Why: the message is a requirement; a client or recorded-expectation check that matches AWS's fixed text fails on an otherwise correct refusal.
Fix: ListComponents' range validation message in the imagebuilder service crate.
Traces: imagebuilder-ibp-traj-component-listing-and-paging#task#5.

Cause 6 of 7: CreateImagePipeline missing-required-member refusal uses a generic ValidationException shape and message.
AWS: 400 with message "Your request is invalid. Request must contain ['infrastructureConfigurationArn'] properties. Verify your request and try again." Fixed text is "Your request is invalid. Request must contain [" ... "] properties. Verify your request and try again."; the variable part is the list of missing required member names, quoted, taken from the request. The recorded x-amzn-errortype is a different shape than the emulator's. Smithy: CreateImagePipeline.infrastructureConfigurationArn is required; the operation's modeled errors are CallRateLimitExceededException, ClientException, DryRunOperationException, ForbiddenException, IdempotentParameterMismatchException, InvalidRequestException, ResourceAlreadyExistsException, ResourceInUseException, ServiceException, ServiceQuotaExceededException, ServiceUnavailableException — there is no ValidationException in the Image Builder model at all.
Emulator: 400 with x-amzn-errortype ValidationException and message "1 validation error detected: Value null at 'infrastructureConfigurationArn' failed to satisfy constraint: Member must not be null" — a Coral-style validation message from a shape this service does not model.
Why: the error type is not in the service's error set, so generated SDK clients fall through to a generic/unmodeled-error path instead of the modeled InvalidRequestException branch.
Fix: CreateImagePipeline's required-member validation — it must emit the service's own error shape and AWS's missing-properties message template.
Traces: imagebuilder-ibp-traj-pipeline-create-mistakes#task#9.

Cause 7 of 7: CreateComponent request-mistake refusal carries the wrong x-amzn-errortype.
AWS: the recorded 400 response's message matches the emulator's, but its x-amzn-errortype is a different error shape than the emulator's InvalidParameterValueException (recorded behavior only; the replay asserts the recorded header's shape, which did not match). Smithy: CreateComponent models ClientException, IdempotentParameterMismatchException, InvalidParameterCombinationException, InvalidRequestException, InvalidVersionNumberException and ResourceInUseException at httpError 400 — InvalidParameterValueException is NOT among CreateComponent's modeled errors, so it cannot be the type AWS returns for this operation.
Emulator: 400 with x-amzn-errortype InvalidParameterValueException.
Why: the status and message are right but the error class is not one CreateComponent can return, so SDK clients cannot match the modeled exception and error-type-driven retry/branch logic diverges.
Fix: CreateComponent's input validation — select a modeled CreateComponent error shape for this refusal.
Traces: imagebuilder-ibp-traj-component-request-mistakes#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — imagebuilder-create-component-rejects-a-two-part-semantic-version#task#3, CreateComponent, 400 vs 400: $.message value_mismatch (expected "The following resource 'Component' already exists: 'arn:aws:imagebuilder:<region>:<account>:component/<name>/1.0.0/1'.", got 'The component <arn> already exists with identical content.'); $headers.x-amzn-errortype shape_mismatch (expected 'ResourceAlreadyExistsException', got 'ResourceInUseException'). Cluster also covers imagebuilder-ibp-traj-component-request-mistakes#task#2 and #task#4.

Cause 2 — imagebuilder-ibp-traj-distribution-config-mistakes#task#5, CreateDistributionConfiguration, 400 vs 400: $.message value_mismatch (expected "The following resource 'DistributionConfiguration' already exists: '<dist_arn>'.", got 'A distribution configuration named <name> already exists in this Amazon Web Services Region.').

Cause 3 — imagebuilder-ibp-traj-pipeline-create-mistakes#task#13, CreateImagePipeline, 400 vs 400: $.message value_mismatch (expected "The following resource 'Pipeline' already exists: 'arn:aws:imagebuilder:<region>:<account>:image-pipeline/<name>'.", got 'An image pipeline named <name> already exists in this Amazon Web Services Region.').

Cause 4 — imagebuilder-ibp-traj-component-listing-and-paging#task#4 (and #task#7), ListComponents, 400 vs 400: $.message value_mismatch (expected "The value supplied for parameter 'filters' is not valid. Allowed filters for ListComponents are: name, version, description, platform, type, supportedOsVersion, productCodes, status.", got 'The filter name <filter> is not supported. The supported filter names are description, name, platform, productCodes, status, supportedOsVersion, type, version.'); $headers.x-amzn-errortype shape_mismatch (got 'InvalidRequestException').

Cause 5 — imagebuilder-ibp-traj-component-listing-and-paging#task#5, ListComponents, 400 vs 400: $.message value_mismatch (expected "... maxResults must be bigger than or equal to 1", got "... maxResults must be greater than or equal to 1").

Cause 6 — imagebuilder-ibp-traj-pipeline-create-mistakes#task#9, CreateImagePipeline, 400 vs 400: $.message value_mismatch (expected "Your request is invalid. Request must contain ['infrastructureConfigurationArn'] properties. Verify your request and try again.", got "1 validation error detected: Value null at 'infrastructureConfigurationArn' failed to satisfy constraint: Member must not be null"); $headers.x-amzn-errortype shape_mismatch (got 'ValidationException', a shape the Image Builder model does not define).

Cause 7 — imagebuilder-ibp-traj-component-request-mistakes#task#1, CreateComponent, 400 vs 400: $headers.x-amzn-errortype shape_mismatch (got 'InvalidParameterValueException', which is not a modeled CreateComponent error).

## Steps to Reproduce
1. Build the emulator from this checkout and boot it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture imagebuilder-create-component-rejects-a-two-part-semantic-version, step task#3 — CreateComponent for a name/semanticVersion that already exists; inspect the 400 body's message and the x-amzn-errortype header.
4. Cause 2: fixture imagebuilder-ibp-traj-distribution-config-mistakes, step task#5 — CreateDistributionConfiguration reusing an existing name.
5. Cause 3: fixture imagebuilder-ibp-traj-pipeline-create-mistakes, step task#13 — CreateImagePipeline reusing an existing pipeline name.
6. Cause 4: fixture imagebuilder-ibp-traj-component-listing-and-paging, step task#4 — ListComponents with an unsupported filter name.
7. Cause 5: same fixture, step task#5 — ListComponents with maxResults below 1.
8. Cause 6: fixture imagebuilder-ibp-traj-pipeline-create-mistakes, step task#9 — CreateImagePipeline omitting infrastructureConfigurationArn.
9. Cause 7: fixture imagebuilder-ibp-traj-component-request-mistakes, step task#1 — CreateComponent with an invalid parameter; inspect x-amzn-errortype.
10. Re-replay all fixture steps listed in Traces to confirm every cause is closed.
