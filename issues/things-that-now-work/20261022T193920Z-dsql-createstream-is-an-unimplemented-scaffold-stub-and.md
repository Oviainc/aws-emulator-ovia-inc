Issue-Severity: HIGH
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# dsql CreateStream is an unimplemented scaffold stub, and rejects a cross-account target roleArn as ValidationException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
Two distinct root causes behind the dsql CDC-stream operations. Neither is covered by the two open dsql issues (`dsql-getclusterpolicy-stub`, `dsql-untagresource-stub`), which name different operations, and the replay report did not mark either of these failures as already filed.

Cause 1 of 2: CreateStream is scaffolded but not implemented, so the whole stream lifecycle is unreachable.
AWS: Smithy models CreateStream as POST /stream/{clusterIdentifier} with required `targetDefinition` (union TargetDefinition with a `kinesis` member), `ordering` (StreamOrdering), `format` (StreamFormat), optional `tags` and `@idempotencyToken` `clientToken`; success HTTP 200 returning required `clusterIdentifier`, `streamIdentifier`, `arn` (StreamArn pattern ^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}/stream/[a-z0-9]{26}$), `status` (StreamStatus), `creationTime`, `ordering`, `format`. Modeled errors: ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400, AccessDeniedException 403, InternalServerException 500, ThrottlingException 429. Recorded behavior: a stream against a real ACTIVE cluster is created as CREATING and settles to ACTIVE; CreateStream naming a non-existent cluster is `ResourceNotFoundException`.
Emulator: every CreateStream call returns the scaffold error — `NotImplemented`, "operation CreateStream is scaffolded but not implemented in this emulator" — which is not one of the modeled errors and not a status AWS ever returns for this operation. Nothing is stored, so GetStream/ListStreams/DeleteStream can never be exercised either.
Why: a client cannot provision a DSQL change-data-capture pipeline at all against the emulator, and the `NotImplemented` code is not in the operation's modeled error set, so SDK error handling (retry classification, `except ClientError` branching on the modeled codes) takes the wrong branch instead of seeing ResourceNotFoundException/ValidationException/AccessDeniedException.
Fix: implement the CreateStream handler (services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs) against the cluster's `streams` store that crate::state::Cluster already carries, minting a StreamId and StreamArn, starting at CREATING with a clock-owed transition to ACTIVE the way CreateCluster does, and answering ResourceNotFoundException for an unknown clusterIdentifier.
Traces: failing checks dsql.create-stream-lifecycle-to-kinesis and dsql.stream-lifecycle-transitions, both erroring on the first CreateStream call. The stream store exists but is never written: services/service-com-amazonaws-dsql-dsql/src/state.rs (Cluster.streams, around line 170) and the stream ARN/not-found helpers around lines 685-868.

Cause 2 of 2: a CreateStream target whose `roleArn` lives in a foreign account is refused as a shape-only ValidationException instead of AccessDeniedException.
AWS: KinesisTargetDefinition models `streamArn` with pattern ^arn:aws[a-zA-Z-]*:kinesis:[a-z0-9-]*:[0-9]{12}:stream/[a-zA-Z0-9+=,.@_/\-]+$ and `roleArn` with pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_-]+)+$, and CreateStream models AccessDeniedException (HTTP 403). Recorded behavior only (the model fixes no evaluation order): the check records that real AWS, given a target whose `roleArn` names an account the caller cannot pass a role from, answers code `AccessDeniedException` — recorded message example "Cross-account pass role is not allowed." — i.e. the cross-account pass-role authorization decision is reached ahead of the member pattern validation, and no stream is created. The requirement graded here is the HTTP 403 / `AccessDeniedException` classification and the absent effect, not the sentence.
Emulator: the request is rejected by the centralized model-constraint validation before the handler is reached, so it answers code `ValidationException` (HTTP 400). The effect is correctly absent, but the classification is wrong, and it is wrong in the permissive-looking direction: a client is told its input was malformed rather than that the cross-account role pass was denied.
Why: a client distinguishing "fix your ARN" from "you may not pass this role" branches on the error code; against the emulator a cross-account pass-role denial is indistinguishable from a typo, so an authorization-boundary test written against AWS fails here even though the call was refused. Hypothesis: AWS evaluates the target's pass-role authorization before validating the sibling `streamArn` member's pattern.
Fix: the CreateStream handler's target validation — once the operation is implemented, the cross-account/pass-role decision on `targetDefinition.kinesis.roleArn` has to be reached and answered as AccessDeniedException ahead of the shape-level pattern rejection of the sibling members.
Traces: failing check dsql.create-stream-invalid-target-arn-rejected.

## Evidence
Cause 1 — expected (AWS/Smithy): HTTP 200 with streamIdentifier/arn/status, status in (CREATING, ACTIVE); or, for an unknown cluster, HTTP 404 `ResourceNotFoundException`.
Actual (emulator), from both dsql.create-stream-lifecycle-to-kinesis and dsql.stream-lifecycle-transitions:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator
`NotImplemented` is not among the operation's modeled errors (ConflictException, ResourceNotFoundException, ServiceQuotaExceededException, ValidationException, AccessDeniedException, InternalServerException, ThrottlingException).

Cause 2 — request: CreateStream(clusterIdentifier=<ACTIVE cluster>, ordering="UNORDERED", format="JSON", targetDefinition={"kinesis": {"streamArn": "not-a-kinesis-arn-at-all", "roleArn": "arn:aws:iam::111122223333:role/sim2real-does-not-matter"}}).
Expected (recorded AWS): HTTP 403, code `AccessDeniedException` (recorded message example "Cross-account pass role is not allowed."), no stream created.
Actual (emulator): code `ValidationException` (HTTP 400). Replay output: AssertionError: ValidationException, at `assert code == "AccessDeniedException", code`.

## Steps to Reproduce
Cause 1:
1. CreateCluster(deletionProtectionEnabled=false); poll GetCluster until status=ACTIVE.
2. Create a Kinesis stream and an IAM role trusted by dsql.amazonaws.com in the same account.
3. CreateStream(clusterIdentifier=<id>, targetDefinition={"kinesis":{"streamArn":<kinesis arn>,"roleArn":<role arn>}}, ordering="UNORDERED", format="JSON").
4. Observe ClientError code `NotImplemented` instead of a 200 with streamIdentifier/arn/status=CREATING. Also call CreateStream against a 26-char clusterIdentifier that does not exist: AWS answers ResourceNotFoundException, the emulator answers NotImplemented.

Cause 2:
1. CreateCluster(deletionProtectionEnabled=false); poll GetCluster until status=ACTIVE.
2. CreateStream(clusterIdentifier=<id>, ordering="UNORDERED", format="JSON", targetDefinition={"kinesis":{"streamArn":"not-a-kinesis-arn-at-all","roleArn":"arn:aws:iam::111122223333:role/sim2real-does-not-matter"}}).
3. Observe HTTP 400 / `ValidationException`; AWS answers HTTP 403 / `AccessDeniedException`.
4. ListStreams(clusterIdentifier=<id>) confirms no stream was created in either case (the effect is correct; only the classification diverges).
