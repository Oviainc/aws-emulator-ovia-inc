Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-identity-create-app-instance-describe-roundtrip
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-identity-create-app-instance-describe-roundtrip chime-sdk-identity-create-app-instance-user-with-expiration-settings-at-creation chime-sdk-identity-retention-settings-put-get-roundtrip chime-sdk-identity-tag-resource-lifecycle-roundtrip
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity|CreateAppInstance|2xx->5xx|status
Issue-Commit: 196bb4609762a23c3b17bc928bd901fcc92dc085

# CreateAppInstance returns HTTP 500 instead of 201, blocking every chime-sdk-identity fixture at setup

> _Note: this issue was identified against the code base at commit `196bb4609762a23c3b17bc928bd901fcc92dc085`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppInstance

## Description
Cause 1 of 1: the restJson1 operation CreateAppInstance (POST /app-instances) answers with HTTP 500 instead of the modeled 201 success, so no App Instance can ever be created and every recorded chime-sdk-identity fixture dies on its first setup call.

AWS: Smithy (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json) — service ChimeIdentityService, protocol restJson1. CreateAppInstance is bound to @http(method: POST, uri: "/app-instances", code: 201); input CreateAppInstanceRequest requires Name (NonEmptyResourceName, @length 1..256, @sensitive) and ClientRequestToken (@idempotencyToken, @length 2..64, pattern ^[-_a-zA-Z0-9]*$), with optional Metadata (@length 0..1024) and Tags (TagList, @length 1..50). On success AWS returns 201 with CreateAppInstanceResponse.AppInstanceArn (ChimeArn, @length 5..1600, pattern ^arn:...). The only modeled outcomes are that 201 and the modeled errors BadRequestException(400), ConflictException(409), ForbiddenException(403), ResourceLimitExceededException(400), ServiceFailureException(500), ServiceUnavailableException(503), ThrottledClientException(429), UnauthorizedClientException(401). A well-formed request as recorded in the fixture carries a valid Name and ClientRequestToken and therefore has no modeled path to a 5xx.

Emulator: the replayed step returns status 500 where the fixture recorded 201 (replay detail: "status: expected 201, got 500"). The failure is identical and total across all four replayed fixtures (affected_count 4, one cluster), and because the created AppInstanceArn never materializes, the 13 remaining steps of the corpus are reported blocked — 0 of 17 steps passed (score 0.0%).

Why: a client relying on AWS behavior cannot create an App Instance, which is the root resource of this entire service. Every downstream flow — DescribeAppInstance, CreateAppInstanceUser (including ExpirationSettings at creation), PutAppInstanceRetentionSettings/GetAppInstanceRetentionSettings, and the TagResource/ListTagsForResource/UntagResource lifecycle — is unreachable. Worse, a 500 is classified by AWS SDKs as a retryable server fault, so callers burn their whole retry budget and then surface a server error for a request that AWS would have accepted, instead of failing fast or succeeding.

Fix: the CreateAppInstance handler for this service and the front-door routing/dispatch layer that reaches it, not the fixture. The handler must create and persist the app-instance in service state and serialize a 201 with AppInstanceArn; a request that cannot be served must be rendered as one of the modeled error shapes with its modeled status, never as a bare 500. Note the related OPEN behavioral issue 20261022T042904Z-chime-sdk-identity-is-entirely-unreachable-the-router.md, which reports the same operation failing with an unmodeled AmbiguousService code because the router resolves on the shared sigv4 signing name 'chime' rather than the endpointPrefix 'identity-chime'; that issue carries only behavioral check ids and none of this corpus's replay keys, so this wire divergence is filed separately and must be re-replayed against the fixture step named below. If the router defect is the underlying cause, the 500 observed here is consistent with that refusal being emitted as a 5xx.

Traces: chime-sdk-identity-create-app-instance-describe-roundtrip#setup#1 (representative). The same cluster also covers chime-sdk-identity-create-app-instance-user-with-expiration-settings-at-creation#setup#1, chime-sdk-identity-retention-settings-put-get-roundtrip#setup#1 and chime-sdk-identity-tag-resource-lifecycle-roundtrip#setup#1.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true (admin health 200, data listener accepting), bootstrap_signal false, state_reset_failures 0; fixtures_replayed 4, steps_total 17, steps_passed 0, steps_mismatched 4, steps_blocked 13, score_pct 0.0.

Divergence cluster (divergence_key service=chime-sdk-identity|CreateAppInstance|status=201->500|keys=status, verdict mismatch, affected_count 4, stub_path empty, owned_by empty):
  Expected (recorded AWS / Smithy @http code): HTTP 201 with body {"AppInstanceArn": "arn:aws:chime:<region>:<account>:app-instance/<id>"}.
  Actual (emulator): HTTP 500. detail: "status: expected 201, got 500".

Status is the diverging requirement: 201 vs 500. 500 is reserved in this model for ServiceFailureException, which a valid CreateAppInstance request does not reach. The generated ARN value, region, account id and instance id in the recorded response are environment-specific examples, not requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire fixture corpus simparity-wire-aws-v1 and observe fixture chime-sdk-identity-create-app-instance-describe-roundtrip, step setup#1.
3. Equivalently, send the restJson1 request directly: POST /app-instances to the emulator's data listener with a sigv4-signed chime-sdk-identity request and body {"Name": "<1..256 chars>", "Metadata": "", "ClientRequestToken": "<2..64 chars matching ^[-_a-zA-Z0-9]*$>", "Tags": [{"Key": "k", "Value": "v"}]}.
4. Observe HTTP 500 instead of the modeled HTTP 201 with an AppInstanceArn body.
5. Because no AppInstanceArn is produced, the remaining steps of that fixture (DescribeAppInstance) and of the other three fixtures (CreateAppInstanceUser with ExpirationSettings, PutAppInstanceRetentionSettings/GetAppInstanceRetentionSettings, TagResource/ListTagsForResource/UntagResource) are all blocked.
6. After fixing, re-replay chime-sdk-identity-create-app-instance-describe-roundtrip#setup#1 and the other three fixtures listed in the description to confirm 201 and that the 13 blocked steps now run.
