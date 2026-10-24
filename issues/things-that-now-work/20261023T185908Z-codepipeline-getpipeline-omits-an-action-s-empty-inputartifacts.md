Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips|run#24|KeyError|assert source['inputArtifacts'] == [], (
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# CodePipeline GetPipeline omits an action's empty inputArtifacts list instead of serializing it as an empty array

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
One net-new root cause in the codepipeline crate, grounded in a failing behavioral check the replay report left unmarked (not ALREADY FILED, not OWNED). The check id's leading token is the multi-service suite token codecommit__codepipeline__glue__iam__lambda; the diverging call is codepipeline.GetPipeline, and the check's own SERVICES list names codepipeline, so it is filed against codepipeline.

Cause 1 of 1: GetPipeline omits ActionDeclaration.inputArtifacts when the action consumes no artifacts, instead of returning it as an empty list.
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) ActionDeclaration has name (@required), actionTypeId (@required), configuration (map), outputArtifacts (OutputArtifactList) and inputArtifacts (InputArtifactList) — 'The name or ID of the artifact consumed by the action'. Recorded behavior: GetPipeline round-trips the declaration a CreatePipeline/UpdatePipeline supplied, and a Source action (which consumes nothing) comes back with inputArtifacts present as an empty array, so a client can read it unconditionally.
Emulator: the member is absent from the GetPipeline response body, so the SDK-parsed action declaration has no 'inputArtifacts' key and the client raises KeyError: 'inputArtifacts' at `assert source['inputArtifacts'] == []`.
Why: every tool that reads a pipeline definition back — to edit it and call UpdatePipeline, to diff it against the definition it submitted, or to render the stage graph — indexes inputArtifacts/outputArtifacts on each action because AWS always sends them. Omitting the empty list turns a normal round-trip into a crash in the caller, and an edit-then-UpdatePipeline cycle silently loses the member.
Fix: the pipeline-declaration output projection in the codepipeline crate (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_pipeline.rs and the shared ActionDeclaration serializer it uses with update_pipeline.rs / create_pipeline.rs) — emit inputArtifacts and outputArtifacts as empty arrays rather than skipping empty collections.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips.

## Evidence
Verbatim from the simparity-behavioral-aws-v1 replay report, check codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips [error]:

  File "check_codecommit__codepipeline__glue__iam__lambda_refarch_glue_pipeline_source_and_deploy_configuration_round_trips.py", line 88, in run
    assert source['inputArtifacts'] == [], (
  KeyError: 'inputArtifacts'

The check had already created the pipeline and read it back, so CreatePipeline/GetPipeline answered 200; the missing member is in the response shape, not an error path.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. cp = client('codepipeline').
2. cp.create_pipeline(pipeline={... a Source stage with one action of actionTypeId {category:'Source', owner:'AWS', provider:'CodeCommit', version:'1'}, configuration naming a repository and branch, outputArtifacts=[{'name':'SourceArtifact'}] and NO inputArtifacts ..., roleArn=R, artifactStore={...}}).
3. resp = cp.get_pipeline(name=P); source = the Source stage's single action declaration.
4. Expect source['inputArtifacts'] to be present and equal to [] (AWS returns the empty list); observe the key absent, so source['inputArtifacts'] raises KeyError: 'inputArtifacts'.
5. Re-replay codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips.
