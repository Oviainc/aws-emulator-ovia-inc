Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.connection-same-device-rejected networkmanager.restore-core-network-policy-version-lifecycle networkmanager.delete-live-policy-version-rejected networkmanager.execute-change-set-already-executed-rejected networkmanager.delete-site-blocked-until-device-removed networkmanager.link-delete-blocked-until-disassociated networkmanager.get-network-resource-counts-invalid-filter-validation networkmanager.tag-resource-reserved-prefix-rejected
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.connection-same-device-rejected|_error_code#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: networkmanager.restore-core-network-policy-version-lifecycle|_expect_rejected#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: networkmanager.delete-live-policy-version-rejected|run#14|AssertionError|assert code == "ValidationException", f"expected ValidationException deleting LIVE version, got {code!r}: {message!r}"
Issue-Key: networkmanager.execute-change-set-already-executed-rejected|run#11|AssertionError|assert error["Code"] == "ValidationException", (
Issue-Key: networkmanager.delete-site-blocked-until-device-removed|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: networkmanager.link-delete-blocked-until-disassociated|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: networkmanager.get-network-resource-counts-invalid-filter-validation|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: networkmanager.tag-resource-reserved-prefix-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# networkmanager: two refusals missing entirely (CreateConnection same-device, RestoreCoreNetworkPolicyVersion of the LIVE version) and five refusals answered with the wrong error class or wrong message text

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Seven distinct root causes in the networkmanager crate's negative paths, found by replaying the behavioral suite (8 failing checks, 16/24 passed). Two causes are refusals the emulator does not perform at all; two answer the right refusal under the wrong modeled error shape; three answer with message text that is not AWS's wording for that error class. All seven are in this one service. A fixer must close all SEVEN before submitting.

=== Cause 1 of 7: CreateConnection does not reject DeviceId == ConnectedDeviceId ===
AWS: CreateConnection creates a connection between two devices; Smithy models DeviceId and ConnectedDeviceId as two required members both targeting DeviceId, with ConnectedDeviceId documented as "The ID of the second device in the connection", and models ValidationException (httpError 400) on the operation. Recorded behavior: naming the same device for both members is refused with HTTP 400, code ValidationException, message "Connection cannot be created between the same device." That message is entirely fixed text — it carries no value from the request.
Emulator: the call SUCCEEDS with HTTP 200 and persists a Connection whose DeviceId and ConnectedDeviceId are the same device id, in State PENDING, and that connection is then listed by GetConnections.
Why: a client that relies on AWS to reject a device connected to itself gets a nonsensical self-loop stored in its network inventory, and no error to handle. Topology-walking code that assumes the two endpoints differ can loop or double-count.
Fix: the same-device precondition belongs in the CreateConnection handler's input validation, before any state is written (the operation's handler under the networkmanager crate's ops/create_connection).
Traces: networkmanager.connection-same-device-rejected

=== Cause 2 of 7: RestoreCoreNetworkPolicyVersion does not reject restoring the currently-LIVE version ===
AWS: RestoreCoreNetworkPolicyVersion restores a previous policy version as a new LATEST. Smithy models ValidationException (400) and ConflictException (409) on the operation. Recorded behavior: restoring the version that the LIVE alias currently points at is refused with HTTP 400, code ValidationException, message "Live policy cannot be restored." — all fixed text, no request values. No new policy version is created by the refused call.
Emulator: the call SUCCEEDS with HTTP 200 and mints a NEW policy version (observed: a third version with Alias LATEST and ChangeSetState PENDING_GENERATION) that is a copy of the LIVE policy.
Why: a client gets a phantom extra policy version and a spurious change set to generate/execute for a policy that is already in force; version-id bookkeeping and "is there a pending change set" checks diverge from AWS.
Fix: the LIVE-alias precondition belongs in the RestoreCoreNetworkPolicyVersion handler's validation, before a new version is appended to core-network policy state.
Traces: networkmanager.restore-core-network-policy-version-lifecycle

=== Cause 3 of 7: DeleteCoreNetworkPolicyVersion refuses the LIVE version as ConflictException instead of ValidationException ===
AWS: Smithy models both ValidationException (400) and ConflictException (409) on DeleteCoreNetworkPolicyVersion. Recorded behavior: deleting the version the LIVE alias points at is refused as code ValidationException, HTTP 400, with a message naming it as the live version (the message's fixed text states that the live version cannot be deleted; its variable parts are the version id and core network id taken from the request/state). Deleting a never-LIVE draft, and deleting the same version once a newer one has been promoted to LIVE, both succeed.
Emulator: the refusal happens, but under the wrong shape: code 'ConflictException' (HTTP 409), message 'policy version 1 is the LIVE policy of core network core-network-00000000000000002 and cannot be deleted' (the version id and core network id there are this run's values; the fixed text is the emulator's own wording, not AWS's, and it does not contain "live version").
Why: clients branch on the error code. Code keyed on ValidationException/400 for "you may not delete the live policy" sees a 409 instead, and generic retry/backoff middleware commonly treats ConflictException as a transient, retryable conflict — so a permanent refusal is retried.
Fix: the error classification in the DeleteCoreNetworkPolicyVersion handler's LIVE-version guard (choose the modeled ValidationException, not ConflictException) plus its message text.
Traces: networkmanager.delete-live-policy-version-rejected

=== Cause 4 of 7: ExecuteCoreNetworkChangeSet refuses an already-executed version as ConflictException instead of ValidationException ===
AWS: Smithy models ValidationException (400) and ConflictException (409) on ExecuteCoreNetworkChangeSet. Recorded behavior: executing a policy version whose change set is already EXECUTION_SUCCEEDED is refused with code ValidationException (HTTP 400); a freshly put version on the same core network can still be executed afterwards, so the refusal is specific to the stale version id.
Emulator: refuses with code 'ConflictException' instead.
Why: same as cause 3 — the stale-version refusal is permanent, but a 409 ConflictException reads as a transient conflict to retrying clients, and code keyed on ValidationException never matches.
Fix: the error classification in the ExecuteCoreNetworkChangeSet handler's already-executed guard.
Traces: networkmanager.execute-change-set-already-executed-rejected

=== Cause 5 of 7: the "cannot be deleted, still associated" messages for DeleteSite and DeleteLink do not carry the resource id and do not use AWS's wording ===
AWS: both operations model ValidationException (400); the emulator already picks that code correctly. Recorded behavior: the message follows one template per resource kind — "Site with ID <SiteId> cannot be deleted due to existing association." and "Link with ID <LinkId> cannot be deleted due to existing association." The fixed text is "<Kind> with ID " / " cannot be deleted due to existing association."; the only variable part is the site or link id taken from the request. It does NOT name which kind of dependent object exists.
Emulator: emits constant strings that omit the id and name the dependent kind instead — 'Site cannot be deleted due to an existing device associated with it.' and 'Link cannot be deleted due to an existing link association associated with it.'
Why: clients and operators match on or log this message; the id is the only part that identifies which resource blocked the delete, and a batch teardown that parses the id out of the refusal cannot find it. Both handlers share the same defective template shape, so this is one cause.
Fix: the message construction in the association guards of the DeleteSite and DeleteLink handlers (the same template, parameterised by the resource id from the request).
Traces: networkmanager.delete-site-blocked-until-device-removed, networkmanager.link-delete-blocked-until-disassociated

=== Cause 6 of 7: GetNetworkResourceCounts' unsupported-ResourceType message is the emulator's own enumeration, not AWS's text ===
AWS: Smithy models ResourceType as an optional httpQuery member (ConstrainedString, length 0..256) and ValidationException (400) on the operation. Recorded behavior: an unsupported ResourceType is refused with HTTP 400, code ValidationException, message "ResourceType is invalid" — entirely fixed text, naming only the member; AWS neither echoes the offending value nor enumerates the supported set.
Emulator: code and status are right, but the message is 'ResourceType "not-a-real-type" is not one of the supported network resource types: attachment, connect-peer, connection, core-network, customer-gateway, device, dx-gateway, dx-vif, dxcon, link, peering, site, transit-gateway, transit-gateway-attachment, transit-gateway-connect-peer, transit-gateway-route-table, vpn-connection' (the quoted type is the request value; the rest is the emulator's own fixed text and the enumerated list).
Why: a client asserting on or matching the documented refusal text does not match, and the emulator leaks a supported-type list AWS does not publish in this error, so tests written against the emulator break on AWS.
Fix: the message text in the GetNetworkResourceCounts handler's ResourceType validation. Note: open issue 20261029T070623Z-networkmanager-getnetworkresourcecounts-error-response--r2 concerns the same refusal but only its Fields[0].Message being inconsistent with sibling operations; it does not cover the top-level Message wording diverging from AWS, which is this cause. Fixing them together is fine as long as the resulting top-level Message is AWS's text.
Fix note: whatever helper builds the refusal must end up with AWS's "ResourceType is invalid" as the Message.
Traces: networkmanager.get-network-resource-counts-invalid-filter-validation

=== Cause 7 of 7: TagResource's reserved-prefix refusal message is not AWS's text ===
AWS: Smithy models Tags as required on TagResource and ValidationException (400) on the operation. Recorded behavior: a tag key beginning with the reserved "aws:" prefix is refused with HTTP 400, code ValidationException, message "Tag cannot start with aws:" — entirely fixed text; the offending key is not echoed. A normal key/value is applied and is visible via ListTagsForResource, and the refused reserved tag is not applied.
Emulator: code and status and effect are right, but the message is 'tag key "aws:reserved" uses the aws: prefix, which is reserved for AWS' (the quoted key is the request value; the surrounding text is the emulator's own wording, and it is lower-cased prose rather than AWS's sentence).
Why: clients matching the documented refusal text do not match, so reserved-prefix handling written against AWS fails against the emulator and vice versa.
Fix: the message text in the TagResource handler's reserved-prefix validation.
Traces: networkmanager.tag-resource-reserved-prefix-rejected

## Evidence
Suite: simparity-behavioral-aws-v1 — networkmanager 16/24 (67%), 8 mismatches, 0 errors, 0 skipped.

Cause 1 — networkmanager.connection-same-device-rejected:
  Expected: ClientError, Code=ValidationException, Message="Connection cannot be created between the same device."
  Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned HTTPStatusCode 200 with Connection{ConnectionId: 'connection-...3', DeviceId: 'device-...2', ConnectedDeviceId: 'device-...2', State: 'PENDING'} (ids are this run's generated values).

Cause 2 — networkmanager.restore-core-network-policy-version-lifecycle:
  Expected: Code=ValidationException, Message="Live policy cannot be restored." and no new policy version created.
  Actual: AssertionError: expected the call to be rejected, but it succeeded and returned HTTPStatusCode 200 with CoreNetworkPolicy{PolicyVersionId: 3, Alias: 'LATEST', ChangeSetState: 'PENDING_GENERATION'} — i.e. restoring the LIVE version minted a new version.

Cause 3 — networkmanager.delete-live-policy-version-rejected:
  Expected: Code=ValidationException, message containing "live version".
  Actual: AssertionError: expected ValidationException deleting LIVE version, got 'ConflictException': 'policy version 1 is the LIVE policy of core network core-network-00000000000000002 and cannot be deleted'.
  (The positive controls in the same check pass: a never-LIVE draft deletes, and the demoted version deletes.)

Cause 4 — networkmanager.execute-change-set-already-executed-rejected:
  Expected: Code=ValidationException re-executing an EXECUTION_SUCCEEDED version.
  Actual: AssertionError: expected ValidationException re-executing already-EXECUTION_SUCCEEDED version_id=1, got 'ConflictException'.

Cause 5 — networkmanager.delete-site-blocked-until-device-removed:
  Expected message: 'Site with ID <SiteId> cannot be deleted due to existing association.'
  Actual message: 'Site cannot be deleted due to an existing device associated with it.'
  networkmanager.link-delete-blocked-until-disassociated:
  Expected message: 'Link with ID <LinkId> cannot be deleted due to existing association.'
  Actual message: 'Link cannot be deleted due to an existing link association associated with it.'
  (Code was ValidationException in both — only the message diverges; the <Id> in the expected text is the request's site/link id, not a literal.)

Cause 6 — networkmanager.get-network-resource-counts-invalid-filter-validation:
  Expected message: 'ResourceType is invalid'
  Actual message: 'ResourceType "not-a-real-type" is not one of the supported network resource types: attachment, connect-peer, connection, core-network, customer-gateway, device, dx-gateway, dx-vif, dxcon, link, peering, site, transit-gateway, transit-gateway-attachment, transit-gateway-connect-peer, transit-gateway-route-table, vpn-connection'

Cause 7 — networkmanager.tag-resource-reserved-prefix-rejected:
  Expected message: 'Tag cannot start with aws:'
  Actual message: 'tag key "aws:reserved" uses the aws: prefix, which is reserved for AWS'

Smithy (services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json):
  CreateConnection: POST /global-networks/{GlobalNetworkId}/connections; required DeviceId and ConnectedDeviceId both target DeviceId; ConnectedDeviceId doc "The ID of the second device in the connection."; errors include ValidationException(400), ConflictException(409).
  RestoreCoreNetworkPolicyVersion: POST /core-networks/{CoreNetworkId}/core-network-policy-versions/{PolicyVersionId}/restore; errors include ValidationException(400), ConflictException(409).
  DeleteCoreNetworkPolicyVersion / ExecuteCoreNetworkChangeSet: both model ValidationException(400) and ConflictException(409).
  DeleteSite / DeleteLink: both model ValidationException(400) and ConflictException(409).
  GetNetworkResourceCounts: ResourceType is httpQuery 'resourceType', ConstrainedString length 0..256; errors ValidationException(400) (no ConflictException).
  TagResource: Tags required; errors include ValidationException(400).
  ConflictException doc: "There was a conflict processing the request. Updating or deleting the resource can cause an inconsistent state." — a transient-looking conflict, not the permanent precondition failures of causes 3 and 4.

## Steps to Reproduce
Common setup: CreateGlobalNetwork and wait for State=AVAILABLE.

Cause 1 (CreateConnection same device):
1. CreateDevice in the global network; wait for AVAILABLE. Call it device_a.
2. CreateConnection(GlobalNetworkId=gn, DeviceId=device_a, ConnectedDeviceId=device_a).
3. Expected: HTTP 400 ValidationException "Connection cannot be created between the same device.", and GetConnections returns [].
4. Observed: HTTP 200 with a Connection whose DeviceId == ConnectedDeviceId, State PENDING, and GetConnections lists it.

Cause 2 (RestoreCoreNetworkPolicyVersion of LIVE):
1. CreateCoreNetwork in the global network; PutCoreNetworkPolicy + ExecuteCoreNetworkChangeSet twice so versions v1 and v2 exist and LIVE points at v2.
2. RestoreCoreNetworkPolicyVersion(CoreNetworkId=cn, PolicyVersionId=v2).
3. Expected: HTTP 400 ValidationException "Live policy cannot be restored.", and ListCoreNetworkPolicyVersions still shows exactly {v1, v2}.
4. Observed: HTTP 200 and a new version v3 (Alias LATEST, ChangeSetState PENDING_GENERATION).

Cause 3 (DeleteCoreNetworkPolicyVersion of LIVE):
1. CreateCoreNetwork; PutCoreNetworkPolicy + ExecuteCoreNetworkChangeSet to make v1 LIVE.
2. DeleteCoreNetworkPolicyVersion(CoreNetworkId=cn, PolicyVersionId=v1).
3. Expected: HTTP 400 ValidationException whose message names v1 as the live version.
4. Observed: HTTP 409 ConflictException 'policy version 1 is the LIVE policy of core network <cn> and cannot be deleted'.

Cause 4 (ExecuteCoreNetworkChangeSet replay):
1. CreateCoreNetwork; PutCoreNetworkPolicy; wait READY_TO_EXECUTE; ExecuteCoreNetworkChangeSet(v1); wait EXECUTION_SUCCEEDED.
2. ExecuteCoreNetworkChangeSet(CoreNetworkId=cn, PolicyVersionId=v1) again.
3. Expected: HTTP 400 ValidationException.
4. Observed: ConflictException.

Cause 5 (association-blocked delete messages):
1. CreateSite; CreateDevice(SiteId=site); wait AVAILABLE; DeleteSite(site).
   Expected message 'Site with ID <site> cannot be deleted due to existing association.'; observed 'Site cannot be deleted due to an existing device associated with it.'
2. CreateSite, CreateDevice, CreateLink, AssociateLink; wait LinkAssociationState=AVAILABLE; DeleteLink(link).
   Expected message 'Link with ID <link> cannot be deleted due to existing association.'; observed 'Link cannot be deleted due to an existing link association associated with it.'

Cause 6 (GetNetworkResourceCounts bad filter):
1. GET /global-networks/{gn}/network-resource-count?resourceType=not-a-real-type
2. Expected: HTTP 400 ValidationException, Message 'ResourceType is invalid'.
3. Observed: ValidationException whose Message echoes the value and enumerates all 17 supported types.

Cause 7 (TagResource reserved prefix):
1. TagResource(ResourceArn=<global network ARN>, Tags=[{Key:'sim2real-env',Value:'test'}]) — succeeds (control).
2. TagResource(ResourceArn=<same ARN>, Tags=[{Key:'aws:reserved',Value:'x'}]).
3. Expected: HTTP 400 ValidationException, Message 'Tag cannot start with aws:'.
4. Observed: ValidationException, Message 'tag key "aws:reserved" uses the aws: prefix, which is reserved for AWS'.
