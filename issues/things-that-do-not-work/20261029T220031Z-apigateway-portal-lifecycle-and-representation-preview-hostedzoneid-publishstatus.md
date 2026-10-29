Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-create-update-delete-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-create-update-delete-lifecycle apigatewayv2-portal-multiple-isolation apigatewayv2-portal-disable-before-publish-conflict apigatewayv2-portal-preview-publish-disable apigatewayv2-portal-with-included-portal-product apigatewayv2-portal-with-included-product
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetPortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|GetPortal|2xx->2xx|$.lastModified
Issue-Key: apigateway|GetPortal|2xx->2xx|$.preview
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.lastModified
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.preview
Issue-Key: apigateway|DisablePortal|4xx->2xx|$
Issue-Key: apigateway|DisablePortal|4xx->2xx|$headers.content-length
Issue-Key: apigateway|DisablePortal|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|DisablePortal|4xx->2xx|status
Issue-Key: apigateway|GetPortal|2xx->2xx|$.endpointConfiguration.portalDomainHostedZoneId
Issue-Key: apigateway|GetPortal|2xx->2xx|$.publishStatus
Issue-Key: apigateway|DeletePortal|2xx->4xx|$
Issue-Key: apigateway|DeletePortal|2xx->4xx|status
Issue-Key: apigateway|CreatePortal|2xx->2xx|$.endpointConfiguration.portalDomainHostedZoneId
Issue-Key: apigateway|UpdatePortal|2xx->2xx|$.endpointConfiguration.portalDomainHostedZoneId
Issue-Key: apigateway|GetPortal|2xx->4xx|$.authorization
Issue-Key: apigateway|GetPortal|2xx->4xx|$.endpointConfiguration
Issue-Key: apigateway|GetPortal|2xx->4xx|$.includedPortalProductArns
Issue-Key: apigateway|GetPortal|2xx->4xx|$.lastModified
Issue-Key: apigateway|GetPortal|2xx->4xx|$.portalArn
Issue-Key: apigateway|GetPortal|2xx->4xx|$.portalContent
Issue-Key: apigateway|GetPortal|2xx->4xx|$.portalId
Issue-Key: apigateway|GetPortal|2xx->4xx|$.preview
Issue-Key: apigateway|GetPortal|2xx->4xx|$.publishStatus
Issue-Key: apigateway|GetPortal|2xx->4xx|$.tags
Issue-Key: apigateway|GetPortal|2xx->4xx|status
Issue-Related: 20261029T220031Z-apigateway-portal-product-and-vpc-link-timestamps-serialized.md
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway portal lifecycle and representation: preview, hostedZoneId, publishStatus, delete

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPortal

## Description
The developer-portal resource diverges from the recorded AWS portal on its representation (a required empty `preview` member, the regional Route 53 hosted-zone id) and on its lifecycle (publish/disable state transitions, delete idempotency, reading a deleted portal). Ten causes, each with its own fixture step. Note the per-run portal default domain name/host:port reported alongside some of these clusters is NOT part of this issue - an emulator must announce an address it is reachable on.

Cause 1 of 10: GetPortal omits `preview`.
AWS: the portal body always carries `preview`, an empty object `{}` when no preview has been requested (recorded behavior only - the model declares the member but not the default).
Emulator: `preview` is absent from the body.
Why: a client that reads portal.preview to decide whether a preview exists gets a null/undefined instead of an empty structure and faults.
Fix: the GetPortal response projection - populate `preview` with the empty structure by default.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#2.

Cause 2 of 10: UpdatePortal omits `preview`.
AWS: same empty-object default as cause 1.
Emulator: `preview` absent.
Why/Fix: the UpdatePortal response projection.
Traces: apigatewayv2-portal-create-update-delete-lifecycle#task#4.

Cause 3 of 10: DisablePortal does not refuse a portal that is not published.
AWS: 409 with x-amzn-errortype `ConflictException` and the message `Portal must be PUBLISHED, PUBLISH_FAILED, or DISABLE_FAILED to disable` - that whole sentence is fixed text with no request- or state-derived parts.
Emulator: 204 with an empty body; the disable is accepted.
Why: a client gets no signal that the disable was illegal, and the portal is left in a state the real service would never reach.
Fix: the DisablePortal handler's precondition check on publishStatus.
Traces: apigatewayv2-portal-disable-before-publish-conflict#task#2.

Cause 4 of 10: GetPortal reports a non-terminal publishStatus and the wrong hosted-zone id.
AWS: `publishStatus` is `DISABLED` once the disable has settled, and `endpointConfiguration.portalDomainHostedZoneId` is the fixed regional portal hosted-zone id recorded for us-west-2 (`Z091913938PZVWK6ZMXI7`); `preview` is `{}`.
Emulator: `publishStatus` stays `DISABLE_IN_PROGRESS`, hostedZoneId is an emulator-invented value, `preview` absent.
Why: a client polling for DISABLED never converges, and a client building a Route 53 alias record from the hosted-zone id writes an unusable record. The hosted-zone id is a per-region AWS constant, not a per-run value.
Fix: the portal state machine (settle the disable transition) and the portal endpoint-configuration projection (regional hosted-zone id table).
Traces: apigatewayv2-portal-disable-before-publish-conflict#task#3.

Cause 5 of 10: DeletePortal is not idempotent.
AWS: deleting a portal that is already gone answers 204 with an empty body.
Emulator: 404 with `{"__type":"NotFoundException","message":"Portal not found"}`.
Why: teardown and retry paths that delete twice fail against the emulator where they succeed on AWS.
Fix: the DeletePortal handler's absent-resource branch.
Traces: apigatewayv2-portal-multiple-isolation#teardown#8.

Cause 6 of 10: CreatePortal returns the wrong portalDomainHostedZoneId.
AWS: the fixed regional portal hosted-zone id (recorded for us-west-2 as `Z091913938PZVWK6ZMXI7`).
Emulator: an emulator-invented id.
Why/Fix: as cause 4, in the CreatePortal response projection.
Traces: apigatewayv2-portal-preview-publish-disable#setup#1.

Cause 7 of 10: GetPortal returns the wrong hosted-zone id and omits `preview`.
AWS: regional hosted-zone id plus `preview: {}`.
Emulator: invented hosted-zone id, `preview` absent.
Why/Fix: as causes 1 and 4.
Traces: apigatewayv2-portal-preview-publish-disable#task#2.

Cause 8 of 10: CreatePortal (with an included portal product) returns the wrong hosted-zone id.
AWS: regional hosted-zone id.
Emulator: invented id.
Why/Fix: as cause 6.
Traces: apigatewayv2-portal-with-included-portal-product#setup#2.

Cause 9 of 10: UpdatePortal returns the wrong hosted-zone id and omits `preview`.
AWS: regional hosted-zone id plus `preview: {}`.
Emulator: invented id, `preview` absent.
Why/Fix: as causes 2 and 4.
Traces: apigatewayv2-portal-with-included-product#task#4.

Cause 10 of 10: GetPortal on a deleted portal.
AWS: 200 with the full portal body and `publishStatus: DELETED` - a deleted portal remains readable in its terminal state.
Emulator: 404 and no body.
Why: a client that confirms deletion by reading the terminal DELETED status cannot complete its teardown verification.
Fix: the portal store's delete semantics (tombstone with publishStatus DELETED) and the GetPortal lookup.
Traces: apigatewayv2-portal-with-included-product#teardown#7.

## Evidence
Replay verdicts `mismatch`. Representative details: `$.preview: missing (expected {}, got None)`; `$.endpointConfiguration.portalDomainHostedZoneId: value_mismatch (expected 'Z091913938PZVWK6ZMXI7', got 'Z47A2501D8FE6F')`; `$.publishStatus: value_mismatch (expected 'DISABLED', got 'DISABLE_IN_PROGRESS')`; DisablePortal `status: expected 409, got 204` with `$headers.x-amzn-errortype: missing (expected 'ConflictException')`; DeletePortal `status: expected 204, got 404`; GetPortal-after-delete `status: expected 200, got 404` with `$.publishStatus: missing (expected 'DELETED')`.

## Steps to Reproduce
Replay the corpus. Cause 1: create a portal and GET /v2/portals/{portalId} (fixture apigatewayv2-portal-create-update-delete-lifecycle step task#2) - `preview` is absent. Cause 3: create a portal and immediately POST its disable action (apigatewayv2-portal-disable-before-publish-conflict#task#2) - the emulator answers 204 instead of 409 ConflictException. Cause 10: delete a portal then GET it (apigatewayv2-portal-with-included-product#teardown#7) - 404 instead of 200/DELETED.
