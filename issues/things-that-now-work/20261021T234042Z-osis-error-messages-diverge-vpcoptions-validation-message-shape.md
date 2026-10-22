Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.create-pipeline-endpoint-invalid-vpc-options-rejected osis.delete-pipeline-not-found osis.get-pipeline-not-found
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.create-pipeline-endpoint-invalid-vpc-options-rejected|run#9|AssertionError|assert malformed.get("Message") == expected, (
Issue-Key: osis.delete-pipeline-not-found|run#6|AssertionError|assert error.get("Message") == expected, f"expected message {expected!r}, got {error.get('Message')!r}"
Issue-Key: osis.get-pipeline-not-found|run#6|AssertionError|assert error.get("Message") == expected, f"expected message {expected!r}, got {error.get('Message')!r}"
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# OSIS error messages diverge: VpcOptions validation message shape and generic pipeline-not-found template

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipelineEndpoint

## Description
Two net-new root causes in the OSIS emulator's error-message construction. Both return the correct error code and HTTP status, but build the message by rules AWS does not use. The messages at issue are request-derived (they name the offending member and its constraint, or the pipeline being operated on), so the construction rule — not the recorded text — is the requirement.

Cause 1 of 2: CreatePipelineEndpoint ValidationException for malformed VpcOptions list members is built per-index and omits the member length constraints.
AWS: Smithy (services/service-com-amazonaws-osis-amazonopensearchingestionservice smithy-model) — CreatePipelineEndpoint.VpcOptions targets com.amazonaws.osis#PipelineEndpointVpcOptions; SubnetIds member SubnetId has @length min 15 max 24 and @pattern ^subnet-\w{8}(\w{9})?$; SecurityGroupIds member SecurityGroupId has @length min 11 max 20 and @pattern ^sg-\w{8}(\w{9})?$; the operation models ValidationException with httpError 400. Recorded AWS response for SubnetIds=["not-a-valid-subnet-id"], SecurityGroupIds=["not-a-valid-sg-id"]: 400 / ValidationException, message built by this rule — one entry per offending list, the value rendered as the whole list in brackets, the path being the COLLECTION member path with no index or '.member' segment, ALL of the member's constraints enumerated in a bracketed list (length max, then length min, then pattern), the pattern rendered WITHOUT ^...$ anchors and with a trailing '.*', and the entries ordered securityGroupIds before subnetIds. Example of that rule: "2 validation errors detected: Value '[not-a-valid-sg-id]' at 'vpcOptions.securityGroupIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 20, Member must have length greater than or equal to 11, Member must satisfy regular expression pattern: sg-\\w{8}(\\w{9})?.*]; Value '[not-a-valid-subnet-id]' at 'vpcOptions.subnetIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 24, Member must have length greater than or equal to 15, Member must satisfy regular expression pattern: subnet-\\w{8}(\\w{9})?.*]".
Emulator: code ValidationException is correct, but the message reports each offending element individually at a per-index path, with the scalar value, only the pattern constraint, the raw anchored pattern, and subnetIds first — e.g. "2 validation errors detected: Value 'not-a-valid-subnet-id' at 'vpcOptions.subnetIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^subnet-\\w{8}(\\w{9})?$; Value 'not-a-valid-sg-id' at 'vpcOptions.securityGroupIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^sg-\\w{8}(\\w{9})?$".
Why: a client that parses the validation message to locate the bad field reads the "at '<path>'" token; the emulator emits a path AWS never produces ('vpcOptions.subnetIds.1.member' vs 'vpcOptions.subnetIds'), never reports the length bounds AWS reports, and reverses the entry order, so field-level error mapping and golden-message assertions both break.
Fix: the OSIS request-validation / ValidationException message builder for constrained list members (collection-level path, full constraint enumeration, anchor stripping, member ordering).
Traces: osis.create-pipeline-endpoint-invalid-vpc-options-rejected. Note the later assertions of that check (the control call with real subnet/SG ids expecting ResourceNotFoundException, and ListPipelineEndpoints leaving no endpoint behind) were never reached, so they are untested here.

Cause 2 of 2: pipeline-not-found uses one generic message template instead of AWS's per-operation messages.
AWS: Smithy — GetPipeline and DeletePipeline both model ResourceNotFoundException with httpError 404. DeletePipeline's recorded message is built from the request and is delete-specific: "The pipeline <PipelineName> could not be deleted because it does not exist". GetPipeline's recorded message (recorded behavior only) is a constant that does NOT echo the requested name: "The pipeline could not be found".
Emulator: both operations return the same template, 'Pipeline <PipelineName> could not be found' (code ResourceNotFoundException and status are correct). It uses the delete-neutral wording for DeletePipeline and leaks the requested name on GetPipeline.
Why: a client that distinguishes the two conditions by message, or asserts the delete-specific phrasing AWS builds from the request, sees wording AWS never emits; the shared template also exposes caller-supplied input where AWS deliberately does not.
Fix: ResourceNotFoundException message construction in the OSIS GetPipeline and DeletePipeline handlers — per-operation messages rather than one shared not-found template.
Traces: osis.get-pipeline-not-found, osis.delete-pipeline-not-found. The second half of the delete check (ValidationException for a pattern-invalid PipelineName) was never reached and is untested here.

## Evidence
osis.create-pipeline-endpoint-invalid-vpc-options-rejected [mismatch]:
AssertionError: expected message "2 validation errors detected: Value '[not-a-valid-sg-id]' at 'vpcOptions.securityGroupIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 20, Member must have length greater than or equal to 11, Member must satisfy regular expression pattern: sg-\\w{8}(\\w{9})?.*]; Value '[not-a-valid-subnet-id]' at 'vpcOptions.subnetIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 24, Member must have length greater than or equal to 15, Member must satisfy regular expression pattern: subnet-\\w{8}(\\w{9})?.*]", got "2 validation errors detected: Value 'not-a-valid-subnet-id' at 'vpcOptions.subnetIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^subnet-\\w{8}(\\w{9})?$; Value 'not-a-valid-sg-id' at 'vpcOptions.securityGroupIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^sg-\\w{8}(\\w{9})?$"

osis.delete-pipeline-not-found [mismatch]:
AssertionError: expected message 'The pipeline sim2real-behav-3dd89536a1e0 could not be deleted because it does not exist', got 'Pipeline sim2real-behav-3dd89536a1e0 could not be found'

osis.get-pipeline-not-found [mismatch]:
AssertionError: expected message 'The pipeline could not be found', got 'Pipeline sim2real-behav-020ae0bff8bc could not be found'

In all three the error Code (ValidationException / ResourceNotFoundException) and status matched; only the message construction diverged.

## Steps to Reproduce
1. Boot the emulator and point an OSIS client at it.
2. Cause 1: CreatePipelineEndpoint(PipelineArn=<well-formed but nonexistent pipeline ARN>, VpcOptions={SubnetIds:["not-a-valid-subnet-id"], SecurityGroupIds:["not-a-valid-sg-id"]}). Observe 400 ValidationException whose message uses per-index '<list>.1.member' paths, omits the member length constraints, keeps the ^...$ anchors, and lists subnetIds before securityGroupIds.
3. Cause 2a: GetPipeline(PipelineName="sim2real-behav-<rand>") for a name that was never created. Observe 404 ResourceNotFoundException with message 'Pipeline <name> could not be found' instead of the constant 'The pipeline could not be found'.
4. Cause 2b: DeletePipeline(PipelineName="sim2real-behav-<rand>") for a name that was never created. Observe the same generic message instead of 'The pipeline <name> could not be deleted because it does not exist'.
5. Replay the checks in trace_ids to confirm.
