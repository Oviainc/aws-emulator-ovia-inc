Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-component-rejects-a-two-part-semantic-version
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-component-rejects-a-two-part-semantic-version imagebuilder-ibp-traj-component-request-mistakes imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes imagebuilder-ibp-traj-component-listing-and-paging imagebuilder-ibp-traj-distribution-config-mistakes imagebuilder-ibp-traj-poll-before-start-and-start-mistakes imagebuilder-ibp-traj-recipe-reference-mistakes
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateComponent|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateComponent|4xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateComponent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateImagePipeline|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateImagePipeline|4xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateImagePipeline|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|ListComponents|4xx->4xx|$.message
Issue-Key: imagebuilder|ListComponents|4xx->4xx|$.requestId
Issue-Key: imagebuilder|ListComponents|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|GetComponent|4xx->4xx|$.message
Issue-Key: imagebuilder|GetComponent|4xx->4xx|$.requestId
Issue-Key: imagebuilder|GetComponent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->4xx|$.message
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->4xx|$.requestId
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->4xx|$.__type
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->4xx|$.message
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->4xx|$.requestId
Issue-Key: imagebuilder|GetImagePipeline|4xx->4xx|$.message
Issue-Key: imagebuilder|GetImagePipeline|4xx->4xx|$.requestId
Issue-Key: imagebuilder|GetImagePipeline|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|StartImagePipelineExecution|4xx->4xx|$.message
Issue-Key: imagebuilder|StartImagePipelineExecution|4xx->4xx|$.requestId
Issue-Key: imagebuilder|StartImagePipelineExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|GetImageRecipe|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# Image Builder parameter validation refuses with ValidationException instead of InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponent

## Description
ONE root cause with TEN causes: the imagebuilder emulator rejects constraint-violating input from the generic Smithy constraint layer, so every input-validation refusal carries the wrong error type (ValidationException), the generic "1 validation error detected: ..." sentence, a Smithy member path instead of the API parameter name, and no requestId. AWS Image Builder refuses these with InvalidParameterValueException and its own single-sentence template. There are 10 separate causes below; all 10 must be closed. Related open issues (neither covers these replay keys): 20261031T221631Z-ec2-iam-imagebuilder-image-builder-createimagepipeline-refuses-imagetestsconfiguration-timeoutminutes-above-the.md and 20261031T232326Z-imagebuilder-image-builder-error-responses-use-the-wrong-status.md.

Shared AWS requirement for all ten: Smithy com.amazonaws.imagebuilder#InvalidParameterValueException is a client error with @httpError 400, documented "The value that you provided for the specified parameter is invalid.", carrying one `message` member; the error type is surfaced in `$.__type` and the `x-amzn-errortype` header. It is not in these operations' modeled error lists (they list ClientException/InvalidRequestException/ForbiddenException/ServiceException/...), so the error-type choice is recorded behavior only. The recorded message template is: fixed text "The value supplied for parameter '", "' is not valid. " followed by a constraint clause; the VARIABLE parts are the API parameter path taken from the request (e.g. semanticVersion, owner, imageTestsConfiguration.timeoutMinutes, distributions[0].amiDistributionConfiguration.name — AWS indexes list members as `name[0]`, never Smithy's `name.1.member`) and the limit/pattern/enum set from the model. Every operation's output shape also models a `requestId` member (NonEmptyString, length 1..1024) and the recordings carry it on the error body; the emulator never emits it.

Cause 1 of 10: CreateComponent, invalid semanticVersion. AWS: 400 InvalidParameterValueException, message "The value supplied for parameter 'semanticVersion' is not valid. semanticVersion must match pattern ^[0-9]+\\.[0-9]+\\.[0-9]+$" (fixed text as above plus "<param> must match pattern <pattern>", pattern taken from CreateComponentRequest.semanticVersion @pattern), plus requestId. Emulator: 400 with x-amzn-errortype 'ValidationException' and message "1 validation error detected: Value '1.0' at 'semanticVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+\\.[0-9]+\\.[0-9]+$", no requestId. Traces: imagebuilder-create-component-rejects-a-two-part-semantic-version#task#1 (also task#3, and imagebuilder-ibp-traj-component-request-mistakes#task#2,#3,#4).

Cause 2 of 10: CreateImagePipeline, imageTestsConfiguration.timeoutMinutes above the ceiling. AWS: 400 with message "The value supplied for parameter 'imageTestsConfiguration.timeoutMinutes' is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than or equal to 1440" (fixed text plus "<param> must be smaller than or equal to <max>", max from the model's range) and a requestId. Emulator: ValidationException with "1 validation error detected: Value '1441' at 'imageTestsConfiguration.timeoutMinutes' failed to satisfy constraint: Member must have value less than or equal to 1440". Trace: imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes#task#7.

Cause 3 of 10: ListComponents, out-of-set `owner`. AWS: 400 with message "The value supplied for parameter 'owner' is not valid. owner must be one of ['Self', 'Shared', 'Amazon', 'ThirdParty', 'AWSMarketplace']" (fixed text plus "<param> must be one of [<quoted enum values>]", values from Smithy enum Ownership) and requestId. Emulator: ValidationException with "1 validation error detected: Value 'self' at 'owner' failed to satisfy constraint: Member must satisfy enum value set: [Self, Shared, Amazon, ThirdParty, AWSMarketplace]". Trace: imagebuilder-ibp-traj-component-listing-and-paging#task#3 (also task#5, task#6).

Cause 4 of 10: CreateComponent with neither/both of `data` and `uri`. AWS: 400 with message "The value supplied for parameter 'data' and 'uri' is not valid. You must specify either data or a uri." — entirely fixed text for this error class — plus requestId. Emulator: its own sentence 'The request must specify exactly one of data and uri.' and no requestId. Trace: imagebuilder-ibp-traj-component-request-mistakes#task#1.

Cause 5 of 10: GetComponent, malformed componentBuildVersionArn. AWS: 400 with "The value supplied for parameter 'componentBuildVersionArn' is not valid. componentBuildVersionArn must match pattern ^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws(?:-[a-z-]+)?|third-party):component/[a-z0-9-_]+/(?:(?:([0-9]+|x)\\.([0-9]+|x)\\.([0-9]+|x))|(?:[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+))$" and requestId. Note AWS reports its own ARN pattern text, not the model's `arn:aws[^:]*` shorthand. Emulator: ValidationException echoing the model pattern. Trace: imagebuilder-ibp-traj-component-request-mistakes#task#7.

Cause 6 of 10: CreateDistributionConfiguration, amiDistributionConfiguration.name violating its pattern inside a list member. AWS: 400 with "The value supplied for parameter 'distributions[0].amiDistributionConfiguration.name' is not valid. distributions[0].amiDistributionConfiguration.name must match pattern ^[-_A-Za-z0-9{][-_A-Za-z0-9\\s:{}\\.]+[-_A-Za-z0-9}]$" — the variable part is the zero-based bracket-indexed member path built from the request. Emulator: ValidationException naming 'distributions.1.member.amiDistributionConfiguration.name' (one-based, Smithy-internal) with the generic sentence. Trace: imagebuilder-ibp-traj-distribution-config-mistakes#task#2.

Cause 7 of 10: GetDistributionConfiguration, malformed distributionConfigurationArn. AWS: 400, `$.__type` 'InvalidParameterValueException', message "The value supplied for parameter 'distributionConfigurationArn' is not valid. distributionConfigurationArn does not match the required pattern" (this class uses the fixed clause "<param> does not match the required pattern" with no pattern echoed), plus requestId. Emulator: `$.__type` 'ValidationException' with the generic sentence echoing the model regex. Trace: imagebuilder-ibp-traj-distribution-config-mistakes#task#7.

Cause 8 of 10: GetImagePipeline, malformed imagePipelineArn. AWS: 400 with "The value supplied for parameter 'imagePipelineArn' is not valid. imagePipelineArn must match pattern ^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$" and requestId. Emulator: ValidationException, generic sentence, no requestId. Trace: imagebuilder-ibp-traj-poll-before-start-and-start-mistakes#task#10.

Cause 9 of 10: StartImagePipelineExecution, malformed imagePipelineArn. AWS: 400 with "The value supplied for parameter 'imagePipelineArn' is not valid. imagePipelineArn must match pattern ^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$" and requestId. Emulator: ValidationException, generic sentence. Trace: imagebuilder-ibp-traj-poll-before-start-and-start-mistakes#task#11.

Cause 10 of 10: GetImageRecipe, malformed imageRecipeArn. AWS: 400 with "The value supplied for parameter 'imageRecipeArn' is not valid. imageRecipeArn must match pattern ^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$" and requestId. Emulator: ValidationException, generic sentence. Trace: imagebuilder-ibp-traj-recipe-reference-mistakes#task#10.

Why: SDK callers discriminate on the error code, not on prose. Code that catches InvalidParameterValueException (retry/report/fall-back logic, and the AWS CLI's own error rendering) sees an unmodelled ValidationException and treats it as an unknown client error, so the branch never runs. Callers that parse the message to tell the user which field was wrong get a Smithy member path (`distributions.1.member....`) that does not exist in the API, and one-based list indices mislabel which list element was rejected. Missing requestId removes the correlation id every operation's response shape models and that AWS support flows depend on.

Fix: the imagebuilder request-validation layer. Constraint violations must be translated, per operation handler, into InvalidParameterValueException (HTTP 400, error type in $.__type and x-amzn-errortype) with AWS's message template and the API-level parameter path (bracket-indexed, zero-based for list members); the data/uri combination check in create_component must emit the same class; and the shared error serializer must populate requestId on every error body as the response shapes model it.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 10 mismatch clusters, all verdict=mismatch at the expected status 400:
- imagebuilder-create-component-rejects-a-two-part-semantic-version#task#1 (CreateComponent): $.message expected "The value supplied for parameter 'semanticVersion' is not valid. semanticVersion must match pattern ^[0-9]+\\.[0-9]+\\.[0-9]+$", got "1 validation error detected: Value '1.0' at 'semanticVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+\\.[0-9]+\\.[0-9]+$"; $.requestId missing; x-amzn-errortype expected InvalidParameterValueException, got ValidationException. Affects 5 steps.
- imagebuilder-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes#task#7 (CreateImagePipeline): expected "...'imageTestsConfiguration.timeoutMinutes' is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than or equal to 1440", got "1 validation error detected: Value '1441' ... Member must have value less than or equal to 1440"; errortype ValidationException; requestId missing.
- imagebuilder-ibp-traj-component-listing-and-paging#task#3 (ListComponents): expected "...'owner' is not valid. owner must be one of ['Self', 'Shared', 'Amazon', 'ThirdParty', 'AWSMarketplace']", got "1 validation error detected: Value 'self' at 'owner' ... Member must satisfy enum value set: [Self, Shared, Amazon, ThirdParty, AWSMarketplace]". Affects 3 steps.
- imagebuilder-ibp-traj-component-request-mistakes#task#1 (CreateComponent): expected "The value supplied for parameter 'data' and 'uri' is not valid. You must specify either data or a uri.", got 'The request must specify exactly one of data and uri.'; requestId missing.
- imagebuilder-ibp-traj-component-request-mistakes#task#7 (GetComponent): expected the 'componentBuildVersionArn ... must match pattern ...' template, got the generic ValidationException sentence.
- imagebuilder-ibp-traj-distribution-config-mistakes#task#2 (CreateDistributionConfiguration): expected parameter path 'distributions[0].amiDistributionConfiguration.name', got 'distributions.1.member.amiDistributionConfiguration.name'.
- imagebuilder-ibp-traj-distribution-config-mistakes#task#7 (GetDistributionConfiguration): $.__type expected 'InvalidParameterValueException', got 'ValidationException'; message expected "...'distributionConfigurationArn' is not valid. distributionConfigurationArn does not match the required pattern".
- imagebuilder-ibp-traj-poll-before-start-and-start-mistakes#task#10 (GetImagePipeline) and #task#11 (StartImagePipelineExecution): same substitution for imagePipelineArn.
- imagebuilder-ibp-traj-recipe-reference-mistakes#task#10 (GetImageRecipe): same substitution for imageRecipeArn.
Model: smithy_query shape InvalidParameterValueException -> client error, httpError 400, member `message`; every listed operation's output shape models `requestId`.

## Steps to Reproduce
1. CreateComponent(name=<unique>, semanticVersion='1.0', platform='Linux', data=<valid YAML>, clientToken=<uuid>) — inspect HTTP status, x-amzn-errortype/$.__type, $.message and $.requestId. Expect 400 InvalidParameterValueException with "The value supplied for parameter 'semanticVersion' is not valid. semanticVersion must match pattern ^[0-9]+\\.[0-9]+\\.[0-9]+$" and a requestId; the emulator answers ValidationException with the generic '1 validation error detected' sentence and no requestId.
2. CreateComponent(...) with neither `data` nor `uri` — expect the 'data' and 'uri' template above.
3. ListComponents(owner='self') — expect the enum template naming owner.
4. CreateImagePipeline(..., imageTestsConfiguration={'timeoutMinutes': 1441}) — expect the 'must be smaller than or equal to 1440' template.
5. CreateDistributionConfiguration(distributions=[{'region':'us-west-2','amiDistributionConfiguration':{'name':'custom al2 awscli v2!'}}], ...) — expect the parameter path 'distributions[0].amiDistributionConfiguration.name'.
6. GetComponent(componentBuildVersionArn='sim2real-probe-ibp-comp-xxxx'), GetImageRecipe(imageRecipeArn=<bare name>), GetDistributionConfiguration(distributionConfigurationArn=<bare name>), GetImagePipeline(imagePipelineArn=<bare name>), StartImagePipelineExecution(imagePipelineArn=<bare name>, clientToken=<uuid>) — each must refuse 400 InvalidParameterValueException with the per-parameter pattern template; all five answer ValidationException instead.
7. Re-replay the fixture steps listed in Traces.
