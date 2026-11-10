Issue-Source: trace-sweep-divergence
Issue-Trace: drs-replication-configuration-template-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: drs-replication-configuration-template-lifecycle drs-tag-resource-lifecycle drs-resource-not-found-and-tag-arn-errors drs-service-describes-and-source-server-validation
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs|CreateReplicationConfigurationTemplate|4xx->4xx|$.__type
Issue-Key: drs|CreateReplicationConfigurationTemplate|4xx->4xx|$.message
Issue-Key: drs|CreateReplicationConfigurationTemplate|4xx->4xx|$.reason
Issue-Key: drs|CreateReplicationConfigurationTemplate|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.arn
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.associateDefaultSecurityGroup
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.autoReplicateNewDisks
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.bandwidthThrottling
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.createPublicIP
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.dataPlaneRouting
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.defaultLargeStagingDiskType
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.ebsEncryption
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.pitPolicy
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.replicationConfigurationTemplateID
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.replicationServerInstanceType
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.replicationServersSecurityGroupsIDs
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.stagingAreaSubnetId
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.stagingAreaTags
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.tags
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|$.useDedicatedReplicationServer
Issue-Key: drs|CreateReplicationConfigurationTemplate|2xx->4xx|status
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|$.__type
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|$.message
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|$.resourceId
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|$.resourceType
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: drs|DeleteReplicationConfigurationTemplate|4xx->4xx|status
Issue-Key: drs|DescribeSourceServers|2xx->4xx|$.items
Issue-Key: drs|DescribeSourceServers|2xx->4xx|status
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# drs: account-initialization gate refuses every call, masking create, delete and describe behavior

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationConfigurationTemplate

## Description
Four net-new wire divergences in the Elastic Disaster Recovery (drs) emulator, all four reached through the same defect: an unconditional account-initialization gate that answers HTTP 400 UninitializedAccountException for every request, so the operation's real success path, its not-found path and its input-validation path are never reached. They are listed as separate causes because each names a different operation and a different expected response that must be made correct, and all four must be closed before submitting. The open issue '20261109T163249Z-drs-4-net-new-behavioral-root-causes-the' describes the same gate from the BEHAVIORAL suite; it carries no wire replay key, so none of the fixture steps below is covered by it and each must be re-replayed here.

Cause 1 of 4 — a CreateReplicationConfigurationTemplate request that omits required members is refused as an uninitialized account instead of a validation failure.
AWS: Smithy: CreateReplicationConfigurationTemplate (POST /CreateReplicationConfigurationTemplate, success code 201) marks stagingAreaSubnetId, replicationServersSecurityGroupsIDs, ebsEncryption, bandwidthThrottling, stagingAreaTags and pitPolicy @required, and declares ValidationException (HTTP 400) with members message/code/reason (ValidationExceptionReason) and fieldList. The recorded response is HTTP 400, x-amzn-errortype 'ValidationException', __type 'com.amazon.drs#ValidationException', reason 'FIELD_VALIDATION_FAILED', message 'Missing required parameters in input: <comma-separated, alphabetically ordered list of the absent @required member names>' — the fixed text is 'Missing required parameters in input: ' and the list is derived entirely from which @required members the request omitted.
Emulator: answers HTTP 400 with x-amzn-errortype 'UninitializedAccountException', __type 'UninitializedAccountException' and message 'Elastic Disaster Recovery has not been initialized for this account; call InitializeService first', and no 'reason' member at all. The input is never validated.
Why: a client that forgot a required field is told its account is not onboarded. It calls InitializeService, retries, gets the identical 400 forever, and never learns which members are missing — and an SDK branching on the error shape matches UninitializedAccountException rather than ValidationException, so its retry/report logic takes a permanent caller error for an account-setup problem.
Fix: the service's initialization state and the gate reading it (crate state's `initialized` flag and the per-handler check that raises UninitializedAccountException), which must present the emulated account as initialized; and then the operation's input validation, which must emit the modelled ValidationException with the message template and the 'reason' member above.
Traces: drs-replication-configuration-template-lifecycle#task#4 (cluster also covers task#5).

Cause 2 of 4 — a fully valid CreateReplicationConfigurationTemplate never creates a template.
AWS: Smithy: the operation's http trait gives code 201 and its output shape ReplicationConfigurationTemplate returns @required replicationConfigurationTemplateID (pattern ^rct-[0-9a-zA-Z]{17}$, length 21) plus arn, stagingAreaSubnetId, associateDefaultSecurityGroup, replicationServersSecurityGroupsIDs, replicationServerInstanceType, useDedicatedReplicationServer, defaultLargeStagingDiskType, ebsEncryption, bandwidthThrottling, dataPlaneRouting, createPublicIP, stagingAreaTags, tags, pitPolicy and autoReplicateNewDisks. The recording shows HTTP 201 with a freshly minted template id and every configuration member echoed back from the request (the exact id, arn, subnet and security-group values are per-run and are not requirements).
Emulator: HTTP 400 UninitializedAccountException with the fixed message above; the whole body is absent, so no template is created or stored.
Why: the primary DRS workflow is unusable — nothing can be created, so every later step that needs a template id (update, describe, delete, tag) has nothing to act on.
Fix: same initialization gate as Cause 1; the handler's create-and-store path and its 201 serializer must then run for a valid request.
Traces: drs-replication-configuration-template-lifecycle#task#7 (cluster also covers drs-tag-resource-lifecycle#task#4).

Cause 3 of 4 — DeleteReplicationConfigurationTemplate on an unknown id does not report not-found.
AWS: Smithy: DeleteReplicationConfigurationTemplate requires replicationConfigurationTemplateID (pattern ^rct-[0-9a-zA-Z]{17}$) and declares ResourceNotFoundException with httpError 404 and members message/code/resourceId/resourceType, documented 'The resource for this operation was not found.' The recorded response is HTTP 404, __type 'com.amazon.drs#ResourceNotFoundException', resourceType 'Replication Configuration Template', resourceId set to the requested template id, and message "Replication Configuration Template with Id: '<requested template id>' was not found" — fixed text "Replication Configuration Template with Id: '" … "' was not found" with the id taken verbatim from the request.
Emulator: HTTP 400 with __type/x-amzn-errortype 'UninitializedAccountException' and the uninitialized message; resourceId and resourceType are absent.
Why: a client cannot distinguish a missing template from an un-onboarded account. Idempotent delete and cleanup paths that treat 404 as success instead see a 400 they must surface as a failure.
Fix: same initialization gate as Cause 1, then this handler's state lookup and its ResourceNotFoundException path (status 404, resourceId/resourceType populated, message as templated above).
Traces: drs-resource-not-found-and-tag-arn-errors#task#2.

Cause 4 of 4 — DescribeSourceServers cannot list an empty inventory.
AWS: Smithy: DescribeSourceServers (POST /DescribeSourceServers, code 200, @readonly, paginated on nextToken/maxResults/items) returns output members items (SourceServersList) and nextToken. With no source servers registered the recording shows HTTP 200 and items as an empty list — the member is present and empty, not omitted.
Emulator: HTTP 400 UninitializedAccountException; items is absent entirely.
Why: a read-only inventory call fails on a fresh account, so discovery and poll loops that begin by listing source servers abort before doing anything, and a client cannot tell 'nothing registered' from 'service refuses'.
Fix: same initialization gate as Cause 1; the handler must then serve a 200 with an empty items list when its source-server state is empty.
Traces: drs-service-describes-and-source-server-validation#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 68 steps, 12 passed).

Cause 1 — drs-replication-configuration-template-lifecycle#task#4 (CreateReplicationConfigurationTemplate), status 400 vs 400 but wrong shape:
  expected $.message 'Missing required parameters in input: associateDefaultSecurityGroup, createPublicIP, dataPlaneRouting, defaultLargeStagingDiskType, replicationServerInstanceType, useDedicatedReplicationServer' (the member list is request-derived), $.reason 'FIELD_VALIDATION_FAILED', $.__type 'com.amazon.drs#ValidationException', header x-amzn-errortype 'ValidationException'
  actual   $.message 'Elastic Disaster Recovery has not been initialized for this account; call InitializeService first', $.reason missing, $.__type 'UninitializedAccountException', header x-amzn-errortype 'UninitializedAccountException'
  cluster also: drs-replication-configuration-template-lifecycle#task#5

Cause 2 — drs-replication-configuration-template-lifecycle#task#7 (CreateReplicationConfigurationTemplate):
  expected status 201 with replicationConfigurationTemplateID, arn, stagingAreaSubnetId, associateDefaultSecurityGroup, replicationServersSecurityGroupsIDs, replicationServerInstanceType, useDedicatedReplicationServer, defaultLargeStagingDiskType, ebsEncryption, bandwidthThrottling, dataPlaneRouting, createPublicIP, stagingAreaTags, tags, pitPolicy (3 rules) and autoReplicateNewDisks
  actual   status 400, every one of those members missing (UninitializedAccountException body)
  cluster also: drs-tag-resource-lifecycle#task#4

Cause 3 — drs-resource-not-found-and-tag-arn-errors#task#2 (DeleteReplicationConfigurationTemplate):
  expected status 404, $.message "Replication Configuration Template with Id: '<requested id>' was not found", $.resourceId = the requested id, $.resourceType 'Replication Configuration Template', $.__type 'com.amazon.drs#ResourceNotFoundException'
  actual   status 400, $.message 'Elastic Disaster Recovery has not been initialized for this account; call InitializeService first', $.resourceId/$.resourceType missing, $.__type and x-amzn-errortype 'UninitializedAccountException'

Cause 4 — drs-service-describes-and-source-server-validation#task#2 (DescribeSourceServers):
  expected status 200, $.items = []
  actual   status 400, $.items missing

## Steps to Reproduce
Boot the emulator from this checkout and send REST-JSON requests to the drs endpoint (region/account as the corpus configures them).

Cause 1: POST /CreateReplicationConfigurationTemplate with a body that omits several @required members (e.g. only stagingAreaSubnetId, ebsEncryption, pitPolicy, stagingAreaTags, replicationServersSecurityGroupsIDs present). Observe 400 UninitializedAccountException instead of 400 ValidationException with reason FIELD_VALIDATION_FAILED and the 'Missing required parameters in input: ...' message. Re-replay drs-replication-configuration-template-lifecycle#task#4.

Cause 2: POST /CreateReplicationConfigurationTemplate with a complete valid body (real subnet id, ebsEncryption 'DEFAULT', bandwidthThrottling 0, dataPlaneRouting 'PRIVATE_IP', stagingAreaTags {}, a 3-rule pitPolicy). Observe 400 instead of 201 with the created template. Re-replay drs-replication-configuration-template-lifecycle#task#7.

Cause 3: POST /DeleteReplicationConfigurationTemplate with a well-formed but unknown replicationConfigurationTemplateID (^rct-[0-9a-zA-Z]{17}$). Observe 400 UninitializedAccountException instead of 404 ResourceNotFoundException carrying resourceId/resourceType. Re-replay drs-resource-not-found-and-tag-arn-errors#task#2.

Cause 4: POST /DescribeSourceServers with an empty body on a fresh emulator. Observe 400 instead of 200 with items []. Re-replay drs-service-describes-and-source-server-validation#task#2.
