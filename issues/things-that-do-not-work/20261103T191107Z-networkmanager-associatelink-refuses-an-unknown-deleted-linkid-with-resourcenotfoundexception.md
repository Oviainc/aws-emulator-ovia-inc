Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.link-delete-blocked-until-disassociated
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.link-delete-blocked-until-disassociated|run#20|AssertionError|assert dead_code == "ValidationException", (
Issue-Commit: f8bf972e45ad9cc3c3d155db5b98d6bbeb81e357

# AssociateLink refuses an unknown/deleted LinkId with ResourceNotFoundException (404) where AWS refuses with ValidationException (400) "Link not found."

> _Note: this issue was identified against the code base at commit `f8bf972e45ad9cc3c3d155db5b98d6bbeb81e357`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateLink

## Description
Cause 1 of 1: AssociateLink classifies a LinkId that names no stored link of the addressed global network as a not-found resource instead of an input-validation failure.

AWS: AssociateLink models both ResourceNotFoundException (404) and ValidationException (400) (Smithy: AssociateLink errors = AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400; input members GlobalNetworkId/DeviceId/LinkId all @required, length 0..50). The model does not say which error a link id naming nothing maps to, so this is recorded behavior only: against real AWS, AssociateLink on a global network that exists, with a device that exists, and a LinkId whose link has been deleted is refused with error code ValidationException and the message "Link not found." — that whole string is FIXED text for this error class, it carries no value from the request or from state (no id is interpolated). The emulator's own non-existent-link answer for the same request family must therefore be the ValidationException class, not ResourceNotFoundException.

Emulator: the same call is refused with error code 'ResourceNotFoundException' (HTTP 404). The handler looks the link up and, on a miss, returns AssociateLinkErrorModeled::ResourceNotFoundException(crate::state::resource_not_found(crate::state::LINK_KIND, &input.link_id)) — a 404 with a resource-not-found message built from the link kind and the requested id, rather than the 400 ValidationException with the fixed message "Link not found.". The rest of the state machine the check exercises is correct: DeleteLink is already refused with ValidationException while a device association exists, the link stays AVAILABLE, disassociation then lets DeleteLink succeed, and GetLinks drops the deleted link. Only the terminal mutation-on-a-dead-id classification diverges.

Why: a client that distinguishes a bad input (ValidationException, 400 — do not retry, surface a field/argument problem to the caller) from a missing resource (ResourceNotFoundException, 404 — often treated as "already gone", and swallowed idempotently) takes the wrong branch. Cleanup and reconciliation code in particular treats ResourceNotFoundException as success-equivalent (the suite's own helpers do exactly that), so against the emulator an AssociateLink naming a deleted link is silently tolerated, while against AWS it surfaces as a hard validation failure with a 400 status and a different code. Retry/error-metric classification also differs because the HTTP status differs (400 vs 404).

Hypothesis: the sibling branch immediately above it in the same handler classifies an unknown DeviceId the same way (ResourceNotFoundException via crate::state::resource_not_found(DEVICE_KIND, ...)), so the device arm is likely to diverge identically (AWS wording would be "Device not found."); only the link arm is exercised by a failing check, so only it is grounded here.

Fix: belongs in the AssociateLink handler's reference-resolution/validation step — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_link.rs, the link-lookup arm that currently builds a ResourceNotFoundException; it must raise the ValidationException variant with the fixed message "Link not found." (and the service's catch-all Reason, consistent with the handler's other state-level refusals, which carry no Fields entry). The handler's module doc also documents the diverging rule ("an unknown device or link inside it is a 404") and must be corrected with it.

Traces: behavioral check networkmanager.link-delete-blocked-until-disassociated, final assertion: "expected ValidationException associating a deleted link, got 'ResourceNotFoundException'".

## Evidence
Request (after the link was successfully deleted): AssociateLink { GlobalNetworkId: <existing gn id>, DeviceId: <existing device id>, LinkId: <id of the link just deleted> }.

Expected (AWS, recorded): HTTP 400, Error.Code = "ValidationException", Error.Message = "Link not found." (fixed text, no interpolated values).

Actual (emulator): Error.Code = "ResourceNotFoundException" (modeled HTTP 404), message produced by crate::state::resource_not_found(LINK_KIND, link_id).

Suite output: AssertionError: expected ValidationException associating a deleted link, got 'ResourceNotFoundException' (check_networkmanager_link_delete_blocked_until_disassociated.py line 65).

All preceding assertions of the same check passed, so DeleteLink's association guard, its message, the link remaining AVAILABLE, DisassociateLink, the subsequent DeleteLink and GetLinks filtering are all correct; only the error class of AssociateLink against a dead link id diverges.

## Steps to Reproduce
1. CreateGlobalNetwork, wait AVAILABLE.
2. CreateSite in it, wait AVAILABLE.
3. CreateDevice in that site, wait AVAILABLE; CreateLink in that site, wait AVAILABLE.
4. AssociateLink(GlobalNetworkId, DeviceId, LinkId); poll GetLinkAssociations until LinkAssociationState == AVAILABLE.
5. DisassociateLink(GlobalNetworkId, DeviceId, LinkId); poll GetLinkAssociations until the list is empty.
6. DeleteLink(GlobalNetworkId, LinkId); poll GetLinks(LinkIds=[LinkId]) until it is empty.
7. Call AssociateLink(GlobalNetworkId, DeviceId, LinkId) again with the now-deleted LinkId and inspect the error: AWS answers HTTP 400 ValidationException "Link not found."; the emulator answers ResourceNotFoundException.
