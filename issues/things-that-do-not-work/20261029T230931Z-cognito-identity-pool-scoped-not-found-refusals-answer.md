Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-identity-delete-identity-pool-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-identity-delete-identity-pool-not-found cognito-identity-identity-pool-delete-then-reuse-errors cognito-identity-identity-pool-lifecycle cognito-identity-identity-pool-lifecycle-crud cognito-identity-validation-error-boundaries cognito-identity-identity-pool-tagging-workflow cognito-identity-tag-operations-error-paths cognito-identity-tag-operations-on-missing-or-malformed-resource cognito-identity-tag-operations-on-nonexistent-pool cognito-identity-create-tag-untag-lifecycle cognito-identity-get-openid-token-error-lifecycle cognito-identity-resource-tagging-round-trip cognito-identity-unlink-identity-login-management cognito-identity-identity-pool-roles-workflow cognito-identity-role-mapping-configuration cognito-identity-principal-tag-attribute-mapping cognito-identity-principal-tag-attribute-mapping-configuration cogid-login-list-identities-next-token-pagination
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|DeleteIdentityPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|DeleteIdentityPool|4xx->4xx|status
Issue-Key: cognito-identity|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|ListTagsForResource|4xx->4xx|status
Issue-Key: cognito-identity|DescribeIdentityPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|DescribeIdentityPool|4xx->4xx|status
Issue-Key: cognito-identity|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|TagResource|4xx->4xx|status
Issue-Key: cognito-identity|GetIdentityPoolRoles|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetIdentityPoolRoles|4xx->4xx|status
Issue-Key: cognito-identity|GetPrincipalTagAttributeMap|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|GetPrincipalTagAttributeMap|4xx->4xx|status
Issue-Key: cognito-identity|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|UntagResource|4xx->4xx|status
Issue-Key: cognito-identity|ListIdentities|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|ListIdentities|4xx->4xx|status
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: pool-scoped not-found refusals answer HTTP 404 instead of 400

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIdentityPool

## Description
This issue bundles 10 root causes, all the same fault seen on a different control-plane operation: when the addressed identity pool does not exist, the emulator answers the ResourceNotFoundException with HTTP 404 (the status the Smithy @httpError trait carries) instead of the HTTP 400 that AWS Cognito Identity actually puts on the wire for this error class. Smithy: ResourceNotFoundException is declared `@error("client") @httpError(404)` and is a modeled error of DeleteIdentityPool, DescribeIdentityPool, ListTagsForResource, TagResource, UntagResource, GetIdentityPoolRoles, GetPrincipalTagAttributeMap and ListIdentities; the recorded AWS responses for every one of these steps carry status 400 with the error carried in the awsJson1_1 body (`__type` = ResourceNotFoundException) and in the `x-amzn-errortype` header, so for this service the wire status for a client error is 400 and the @httpError trait does not describe it. Fix belongs in the shared awsJson1_1 error serializer / status mapping for this service (the place that turns a modeled error shape into a status line), not in each operation handler: the per-operation handlers already select the right error shape and message, only the status is wrong.

Cause 1 of 10: DeleteIdentityPool on a pool id that was never created (or already deleted). AWS: HTTP 400, `__type` ResourceNotFoundException, header `x-amzn-errortype: ResourceNotFoundException:`. Emulator: HTTP 404 and no `x-amzn-errortype` header. Why: a client that branches on the HTTP status (or a raw-HTTP consumer, or an SDK that falls back to status when no error type header is present) treats the refusal as a transport-level 404 rather than the modeled ResourceNotFoundException. Fix: status mapping in the error serializer. Traces: cognito-identity-delete-identity-pool-not-found#task#1 (6 steps in the cluster).

Cause 2 of 10: ListTagsForResource for a pool ARN whose pool does not exist. AWS: 400 + ResourceNotFoundException (+ `x-amzn-errortype: ResourceNotFoundException:`). Emulator: 404, header absent. Why/Fix: as cause 1. Traces: cognito-identity-delete-identity-pool-not-found#task#2 (7 steps).

Cause 3 of 10: ListTagsForResource in a teardown after the pool was deleted. AWS: 400. Emulator: 404 (status only diverges here). Why/Fix: as cause 1. Traces: cognito-identity-create-tag-untag-lifecycle#teardown#8.

Cause 4 of 10: DescribeIdentityPool on a deleted/unknown pool. AWS: 400 + ResourceNotFoundException. Emulator: 404. Why/Fix: as cause 1. Traces: cognito-identity-get-openid-token-error-lifecycle#task#8 (6 steps).

Cause 5 of 10: TagResource against a deleted pool. AWS: 400 + ResourceNotFoundException (+ errortype header). Emulator: 404, header absent. Why/Fix: as cause 1. Traces: cognito-identity-identity-pool-delete-then-reuse-errors#task#7 (5 steps).

Cause 6 of 10: DeleteIdentityPool repeated in teardown (idempotent-delete probe). AWS: 400. Emulator: 404. Why/Fix: as cause 1. Traces: cognito-identity-identity-pool-lifecycle-crud#teardown#11 (2 steps).

Cause 7 of 10: GetIdentityPoolRoles on an unknown pool. AWS: 400 + ResourceNotFoundException. Emulator: 404. Why/Fix: as cause 1. Traces: cognito-identity-identity-pool-roles-workflow#task#7 (2 steps).

Cause 8 of 10: GetPrincipalTagAttributeMap on an unknown pool. AWS: 400 + ResourceNotFoundException. Emulator: 404. Why/Fix: as cause 1. Traces: cognito-identity-principal-tag-attribute-mapping#task#9 (2 steps).

Cause 9 of 10: UntagResource against an unknown pool. AWS: 400 + ResourceNotFoundException. Emulator: 404. Why/Fix: as cause 1. Traces: cognito-identity-tag-operations-error-paths#task#4 (2 steps).

Cause 10 of 10: ListIdentities on an unknown pool id. AWS: 400 + ResourceNotFoundException. Emulator: 404. Why/Fix: as cause 1. Traces: cogid-login-list-identities-next-token-pagination#task#12.

Related open issues that do NOT carry these replay keys and do not cover these causes: '20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md' (error classification, not status mapping) and '20261024T145006Z-cognito-identity-listtagsforresource-ignores-iam-authorization-a-foreign.md' (authorization on ListTagsForResource).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200). Per cluster, expected vs actual:
- DeleteIdentityPool: expected status 400, got 404; `$headers.x-amzn-errortype` expected 'ResourceNotFoundException:', got None (cognito-identity-delete-identity-pool-not-found#task#1, 6 affected steps).
- ListTagsForResource: expected 400, got 404; x-amzn-errortype expected 'ResourceNotFoundException:', got None (cognito-identity-delete-identity-pool-not-found#task#2, 7 affected steps).
- ListTagsForResource: expected 400, got 404 (cognito-identity-create-tag-untag-lifecycle#teardown#8).
- DescribeIdentityPool: expected 400, got 404; errortype header missing (cognito-identity-get-openid-token-error-lifecycle#task#8, 6 steps).
- TagResource: expected 400, got 404; errortype header missing (cognito-identity-identity-pool-delete-then-reuse-errors#task#7, 5 steps).
- DeleteIdentityPool: expected 400, got 404 (cognito-identity-identity-pool-lifecycle-crud#teardown#11, 2 steps).
- GetIdentityPoolRoles: expected 400, got 404; errortype header missing (cognito-identity-identity-pool-roles-workflow#task#7, 2 steps).
- GetPrincipalTagAttributeMap: expected 400, got 404; errortype header missing (cognito-identity-principal-tag-attribute-mapping#task#9, 2 steps).
- UntagResource: expected 400, got 404; errortype header missing (cognito-identity-tag-operations-error-paths#task#4, 2 steps).
- ListIdentities: expected 400, got 404; errortype header missing (cogid-login-list-identities-next-token-pagination#task#12).
Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice): ResourceNotFoundException errorKind=client, httpError=404; protocol awsJson1_1. Recorded AWS wire status for this error in this service is 400.

## Steps to Reproduce
1. Replay fixture cognito-identity-delete-identity-pool-not-found, step task#1: DeleteIdentityPool with an IdentityPoolId that does not exist -> emulator answers 404, AWS recording answers 400 with __type ResourceNotFoundException.
2. Repeat for each cause's fixture#step listed under Traces (ListTagsForResource, DescribeIdentityPool, TagResource, UntagResource, GetIdentityPoolRoles, GetPrincipalTagAttributeMap, ListIdentities, and the teardown DeleteIdentityPool) using a pool id/ARN that was deleted or never created.
3. Compare the HTTP status line and the x-amzn-errortype header against the recorded response.
