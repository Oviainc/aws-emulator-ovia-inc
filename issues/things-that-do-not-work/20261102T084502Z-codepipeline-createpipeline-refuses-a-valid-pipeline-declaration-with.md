Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-pipeline-source-and-deploy-configuration-round-trips
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-pipeline-source-and-deploy-configuration-round-trips
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|CreatePipeline|2xx->4xx|$.pipeline
Issue-Key: codepipeline|CreatePipeline|2xx->4xx|status
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CodePipeline CreatePipeline refuses a valid pipeline declaration with HTTP 400 instead of creating the pipeline

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
Cause 1 of 1: CreatePipeline rejects a well-formed two-stage declaration (CodeCommit Source -> Lambda Invoke Deploy) that AWS accepts, answering HTTP 400 with no pipeline in the body.
  AWS: Smithy: CreatePipeline input pipeline -> PipelineDeclaration (@required), output pipeline -> PipelineDeclaration + tags. The modeled refusals are ConcurrentModificationException, InvalidActionDeclarationException, InvalidBlockerDeclarationException, InvalidStageDeclarationException, InvalidStructureException, InvalidTagsException, LimitExceededException, PipelineNameInUseException, TooManyTagsException and ValidationException -- every one of them 400. For the declaration in this step AWS returns 200 and echoes the stored declaration back: artifactStore {type S3, location <bucket>}, executionMode SUPERSEDED, name <pipeline-name>, pipelineType V2, roleArn <role arn>, version 1, a Source stage with an AWS/Source/CodeCommit action (configuration BranchName/PollForSourceChanges/RepositoryName, outputArtifacts [{name src}], runOrder 1) and a Deploy stage with an AWS/Invoke/Lambda action (configuration FunctionName/UserParameters, inputArtifacts [{name src}], runOrder 1). Only the names, bucket, role ARN and function/job names are request-derived; the structure is what the model requires.
  Emulator: the step answers 400 and the response carries no pipeline member at all (replay detail: "status: expected 200, got 400" and "$.pipeline: missing"). The replay record does not expose which error code the emulator chose, so the code and message must be re-checked against the recording when fixing. Hypothesis: this step re-creates a pipeline name used earlier in the same fixture after it was deleted, and the emulator's pipeline-name registry still holds the deleted name, so it refuses with a name-in-use/validation style 400; the expected response's version 1 is consistent with a fresh create.
  Why: the whole create/update round-trip for a Lambda-invoke deploy pipeline is unreachable -- a client (or an IaC layer) that creates this topology gets a hard 400 on a declaration real CodePipeline accepts, and every later step that depends on the pipeline fails too. A spurious 400 on a valid create is not retryable and has no client-side workaround.
  Fix: the CreatePipeline handler and its declaration validation / pipeline-name state in the codepipeline service crate -- the acceptance path must admit this declaration and echo the stored PipelineDeclaration (including pipelineType V2, executionMode SUPERSEDED, version 1 and per-action runOrder) with status 200, and deleted pipeline names must be released from the name registry.
  Traces: codepipeline-pipeline-source-and-deploy-configuration-round-trips#task#7.

Note: 20261029T183239Z-codepipeline-declaration-validation-and-response-shapes-10-net.md covers misclassified declaration errors for INVALID declarations and 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md covers omitted members of a SUCCESSFUL declaration response; neither covers a valid declaration being refused outright, which is this cause.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture codepipeline-pipeline-source-and-deploy-configuration-round-trips, step task#7, operation CreatePipeline:
  expected_status 200, actual_status 400
  detail: "status: expected 200, got 400"
         "$.pipeline: missing (expected {'artifactStore': {'location': '<bucket>', 'type': 'S3'}, 'executionMode': 'SUPERSEDED', 'name': '<pipeline>', 'pipelineType': 'V2', 'roleArn': 'arn:aws:iam::<account>:role/<role>', 'stages': [{'name': 'Source', 'actions': [{'actionTypeId': {'__type': 'ActionTypeId', 'category': 'Source', 'owner': 'AWS', 'provider': 'CodeCommit', 'version': '1'}, 'configuration': {'BranchName': 'main', 'PollForSourceChanges': 'false', 'RepositoryName': '<repo>'}, 'inputArtifacts': [], 'name': 'Source', 'outputArtifacts': [{'name': 'src'}], 'runOrder': 1}]}, {'name': 'Deploy', 'actions': [{'actionTypeId': {'__type': 'ActionTypeId', 'category': 'Invoke', 'owner': 'AWS', 'provider': 'Lambda', 'version': '1'}, 'configuration': {'FunctionName': '<function>', 'UserParameters': '<json>'}, 'inputArtifacts': [{'name': 'src'}], 'name': 'Deploy', 'outputArtifacts': [], 'runOrder': 1}]}], 'version': 1}, got None)"

Smithy (com-amazonaws-codepipeline-codepipeline-20150709): CreatePipeline input {pipeline: PipelineDeclaration @required, tags: TagList}, output {pipeline: PipelineDeclaration, tags: TagList}; all modeled errors (InvalidActionDeclarationException, InvalidStageDeclarationException, InvalidStructureException, PipelineNameInUseException, ValidationException, ...) are 400 client errors, so a 400 here claims the declaration or the name is invalid when it is not.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture codepipeline-pipeline-source-and-deploy-configuration-round-trips in order up to step task#7.
3. Step task#7 issues CreatePipeline with a two-stage declaration (AWS/Source/CodeCommit -> AWS/Invoke/Lambda, S3 artifact store, pipelineType V2, executionMode SUPERSEDED).
4. Observe HTTP 400 with no pipeline member where the recording has HTTP 200 and the echoed PipelineDeclaration at version 1; capture the error code/message the emulator returns and compare it against the recorded 200 response.
