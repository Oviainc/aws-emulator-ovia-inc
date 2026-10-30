Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.artifact-lifecycle-create-describe-delete sagemaker.artifact-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.artifact-lifecycle-create-describe-delete|run#18|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Key: sagemaker.artifact-lifecycle-transitions|run#18|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Commit: bd260fbb92f697f6565f73ef409cec21a7b2614a

# SageMaker UpdateArtifact answers 501 NotImplemented, so a deleted artifact is not refused with 400 ResourceNotFound (1 cause)

> _Note: this issue was identified against the code base at commit `bd260fbb92f697f6565f73ef409cec21a7b2614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateArtifact

## Description
Cause 1 of 1: UpdateArtifact is an unimplemented scaffold stub, so a mutation aimed at an artifact that has been deleted (or that never existed) is answered 501 NotImplemented instead of AWS's modeled not-found refusal. Both failing checks reach the same point: CreateArtifact, DescribeArtifact and DeleteArtifact all behave, DescribeArtifact then correctly refuses the dead ARN, but the follow-up UpdateArtifact against the same ARN returns NotImplemented.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) models UpdateArtifact with input ArtifactArn (required, pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:artifact/) and errors ConflictException (HTTP 400) and ResourceNotFound (HTTP 400, documented "Resource being access is not found"). Updating an artifact whose ARN no longer resolves is the ResourceNotFound case: HTTP 400 with error code `ResourceNotFound`. The recorded message is the same sentence DescribeArtifact already produces for the same dead ARN: fixed text `Artifact '` + `' does not exist.` with the single variable part being the artifact ARN taken verbatim from the request's ArtifactArn; nothing else in the message varies. A successful UpdateArtifact instead answers HTTP 200 with only ArtifactArn.

Emulator: UpdateArtifact is still the generated stub — services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_artifact.rs:161 returns `crate::scaffolded::Scaffolded::not_implemented("UpdateArtifact")`, i.e. HTTP 501 with error code `NotImplemented`, for every request. The handler never looks the ArtifactArn up in state, so it can neither succeed on a live artifact nor refuse a deleted one. Replay shows: `expected UpdateArtifact on a deleted artifact to fail ResourceNotFound, got NotImplemented`.

Why: a client cannot distinguish "this artifact is gone" from "this service is broken". 501/NotImplemented is not a modeled error for the operation, so botocore surfaces an unmodeled code; retry and error-handling logic keyed on ResourceNotFound (the normal way to detect a lineage entity that was already deleted) never fires, and SDK retry policy may treat the 501 as a server fault and retry a request that can never succeed. It also means "deleted" is only half-enforced in this emulator: the artifact is unreadable but no mutation path proves it is unusable, so a lineage client can believe an update was merely unimplemented rather than rejected.

Fix: implement the operation's handler, services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_artifact.rs — resolve ArtifactArn against the artifact store reached through crate::state::service_state, return ResourceNotFound (400) with the same wording the artifact read path already uses when it does not resolve, otherwise apply ArtifactName/Properties/PropertiesToRemove and return the ArtifactArn; then delete the SCAFFOLDED marker line at the top of the file.

Related already-open issues that do NOT cover these two checks: 20261029T222441Z-sagemaker-lineage-update-projection-and-scaling-recommendation-paths.md (other Update* stubs and lineage projections) and 20261029T192819Z-sagemaker-update-operations-are-accepted-but-never-mutate.md (Update ops that are accepted but do not mutate — UpdateArtifact is not accepted at all). The suite's auto-filed UpdateArtifact stub entry was attributed only to sagemaker.artifact-create-describe-delete; the two checks listed here were reported unowned and are the ones this issue is grounded in.

Traces: sagemaker.artifact-lifecycle-create-describe-delete (asserts ResourceNotFound + message `Artifact '<arn>' does not exist.` for update_artifact(ArtifactArn=<deleted arn>, ArtifactName=<new name>)); sagemaker.artifact-lifecycle-transitions (same assertion for update_artifact(ArtifactArn=<deleted arn>, Properties={...})).

## Evidence
Expected (AWS, per the Smithy model's modeled ResourceNotFound on UpdateArtifact and the read path's recorded wording): HTTP 400, Error.Code `ResourceNotFound`, Error.Message `Artifact '<ArtifactArn from the request>' does not exist.`

Actual (emulator): HTTP 501, Error.Code `NotImplemented` for every UpdateArtifact call.

Replay output:
- sagemaker.artifact-lifecycle-create-describe-delete — AssertionError: expected UpdateArtifact on a deleted artifact to fail ResourceNotFound, got NotImplemented (check line 48)
- sagemaker.artifact-lifecycle-transitions — AssertionError: expected UpdateArtifact on a deleted artifact to fail with ResourceNotFound, got 'NotImplemented' (check line 62)

Source: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_artifact.rs lines 4-6 (SCAFFOLDED, NOT IMPLEMENTED marker) and line 161 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateArtifact"))`).

## Steps to Reproduce
1. CreateArtifact(ArtifactName=<name>, Source={SourceUri: s3://<bucket>/<key>}, ArtifactType="Model") and keep the returned ArtifactArn.
2. DescribeArtifact(ArtifactArn=<arn>) — succeeds and echoes the name/type/source.
3. DeleteArtifact(ArtifactArn=<arn>).
4. DescribeArtifact(ArtifactArn=<arn>) — correctly refuses 400 ResourceNotFound, message `Artifact '<arn>' does not exist.`
5. UpdateArtifact(ArtifactArn=<arn>, ArtifactName=<other name>) (or UpdateArtifact(ArtifactArn=<arn>, Properties={"probe":"post-delete"})).
   Expected: 400 ResourceNotFound with `Artifact '<arn>' does not exist.`
   Observed: 501 NotImplemented.
6. Equivalent replay: run behavioral checks sagemaker.artifact-lifecycle-create-describe-delete and sagemaker.artifact-lifecycle-transitions.
