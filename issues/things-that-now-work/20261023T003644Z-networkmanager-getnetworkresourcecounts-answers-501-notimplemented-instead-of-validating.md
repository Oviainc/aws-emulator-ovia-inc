Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.get-network-resource-counts-invalid-filter-validation networkmanager.global-network-lifecycle-delete-transition networkmanager.site-lifecycle-state-transition
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.get-network-resource-counts-invalid-filter-validation|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: networkmanager.global-network-lifecycle-delete-transition|run#16|AssertionError|assert update_message == expected_message, f"expected message {expected_message!r}, got {update_message!r}"
Issue-Key: networkmanager.site-lifecycle-state-transition|run#19|AssertionError|assert tag_err == "ResourceNotFoundException", (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# networkmanager: GetNetworkResourceCounts answers 501 NotImplemented instead of validating ResourceType, and ResourceNotFoundException carries a per-request message instead of AWS's constant text (2 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkResourceCounts

## Description
Two distinct root causes in the bootstrapped networkmanager emulator, both grounded in failing behavioral checks. The service does answer (CreateGlobalNetwork / CreateSite / DescribeGlobalNetworks / UpdateGlobalNetwork all respond), so these are fidelity defects, not bootstrap gaps.

Cause 1 of 2: GetNetworkResourceCounts is an unimplemented scaffold stub, so neither its rejection of an unsupported ResourceType nor its success path exists.
AWS: Smithy (services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json) models GetNetworkResourceCounts as GET /global-networks/{GlobalNetworkId}/network-resource-count, success 200 with NetworkResourceCounts (list of NetworkResourceCount{ResourceType, Count}) and NextToken; modeled errors are AccessDeniedException(403), InternalServerException(500), ThrottlingException(429) and ValidationException(400, "The input fails to satisfy the constraints"). ResourceType is a ConstrainedString whose documentation enumerates the supported values (Network Manager: attachment, connect-peer, connection, core-network, device, link, peering, site; plus the Direct Connect and VPC types). A value outside that set is refused with HTTP 400 / error code ValidationException — recorded behavior for the refusal shows message "ResourceType is invalid" as an example. A supported type such as "site" must be accepted and must count the global network's existing sites (>= 1 after a site is created).
Emulator: the handler returns Scaffolded::not_implemented("GetNetworkResourceCounts"), i.e. HTTP 501 with error code NotImplemented, for every request — both for the invalid ResourceType (observed: expected ValidationException, got 'NotImplemented') and for the valid "site" query.
Why: a client probing resource inventory, or one relying on input validation to surface a typo in ResourceType, sees a 501/NotImplemented it cannot classify: botocore raises a ClientError with a code no AWS SDK retry/validation path recognises, so negative-path handling never runs and the counting path returns nothing.
Fix: the operation's handler plus its validation of ResourceType against the documented value set, and the read of site/device/link/etc. counts out of service state.
Traces: networkmanager.get-network-resource-counts-invalid-filter-validation.

Cause 2 of 2: ResourceNotFoundException is serialized with an emulator-authored, per-request Message instead of the constant text AWS returns.
AWS: recorded behavior only — the model declares ResourceNotFoundException (HTTP 404, required members Message, ResourceId, ResourceType) but does not state the message text. Two independent recordings of different resources and different operations both show the identical constant string "Resource not found.": UpdateGlobalNetwork on a deleted global-network id, and TagResource on the ARN of a deleted site. The rule those recordings establish is that NetworkManager's ResourceNotFoundException message is a fixed, resource-independent string rather than one built from the request; the identifying detail travels in ResourceId/ResourceType, not in the prose.
Emulator: UpdateGlobalNetwork on a deleted id correctly answers 404 / ResourceNotFoundException but with a message formatted from the request — observed 'global network global-network-00000000000000001 does not exist' where AWS returns 'Resource not found.'. The same construction is used for the service's other not-found refusals.
Why: a client that matches on the error message (log assertions, error-catalogue mapping, or a UI that surfaces the service message verbatim) sees text AWS never produces, and the emulator additionally leaks an internal id-generation format into user-visible prose.
Fix: the shared not-found construction / error serializer for this service — emit the constant message and carry the identifying detail in ResourceId and ResourceType.
Traces: networkmanager.global-network-lifecycle-delete-transition (observed on UpdateGlobalNetwork; the check's code and state assertions all pass, only the message diverges). networkmanager.site-lifecycle-state-transition asserts the same constant message for TagResource on a deleted site but currently stops earlier on 501/NotImplemented from the TagResource scaffold stub, which is covered by the already-filed TagResource stub issue — that check must be re-replayed after TagResource is implemented to confirm this message rule too.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), networkmanager 1/24 passed:

1) [mismatch] networkmanager.get-network-resource-counts-invalid-filter-validation
   nm.get_network_resource_counts(GlobalNetworkId=gn, ResourceType="not-a-real-type")
   expected: HTTP 400, Error.Code == "ValidationException" (recorded message example "ResourceType is invalid")
   actual:   AssertionError: expected ValidationException, got 'NotImplemented' (HTTP 501, "operation GetNetworkResourceCounts is scaffolded but not implemented in this emulator")
   The follow-on positive control — ResourceType="site" returning NetworkResourceCounts with Count >= 1 — is therefore also unreachable.
   Source: services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resource_counts.rs:169 returns crate::scaffolded::Scaffolded::not_implemented("GetNetworkResourceCounts").

2) [mismatch] networkmanager.global-network-lifecycle-delete-transition
   nm.update_global_network(GlobalNetworkId=<deleted id>, Description="post-delete")
   expected: HTTP 404, Error.Code == "ResourceNotFoundException", Error.Message == "Resource not found."
   actual:   HTTP 404, Error.Code == "ResourceNotFoundException" (correct), Error.Message == 'global network global-network-00000000000000001 does not exist'
   AssertionError: expected message 'Resource not found.', got 'global network global-network-00000000000000001 does not exist'
   Source: services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_global_network.rs:146-149 builds the message with format!("global network {} does not exist", input.global_network_id).
   Corroborating recording of the same constant for a different resource/operation: networkmanager.site-lifecycle-state-transition asserts Error.Message == "Resource not found." for TagResource on a deleted site ARN.

## Steps to Reproduce
Cause 1:
1. CreateGlobalNetwork, wait for State == AVAILABLE; CreateSite in it, wait for AVAILABLE.
2. Call GetNetworkResourceCounts(GlobalNetworkId=<gn>, ResourceType="not-a-real-type").
   Expect HTTP 400 / ValidationException; emulator answers HTTP 501 / NotImplemented.
3. Call GetNetworkResourceCounts(GlobalNetworkId=<gn>, ResourceType="site").
   Expect 200 with a NetworkResourceCounts entry {ResourceType: "site", Count >= 1}; emulator answers HTTP 501 / NotImplemented.

Cause 2:
1. CreateGlobalNetwork, wait for State == AVAILABLE, then DeleteGlobalNetwork and poll DescribeGlobalNetworks until the id is gone (empty list).
2. Call UpdateGlobalNetwork(GlobalNetworkId=<deleted id>, Description="post-delete").
   Expect HTTP 404, Error.Code ResourceNotFoundException, Error.Message exactly "Resource not found." (with ResourceId/ResourceType carrying the detail); emulator returns the per-request prose "global network <id> does not exist".
3. After TagResource is implemented, repeat with TagResource against the ARN of a deleted site and check the same constant message.

Replay: networkmanager.get-network-resource-counts-invalid-filter-validation, networkmanager.global-network-lifecycle-delete-transition, networkmanager.site-lifecycle-state-transition.
