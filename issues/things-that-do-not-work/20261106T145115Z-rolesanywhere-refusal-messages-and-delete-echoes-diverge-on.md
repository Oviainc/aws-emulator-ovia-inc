Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-not-found-error-paths
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-not-found-error-paths rolesanywhere-trust-anchor-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|DisableTrustAnchor|4xx->4xx|$.message
Issue-Key: rolesanywhere|DeleteAttributeMapping|4xx->4xx|$.message
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.enabled
Issue-Commit: 02cf7c25cd5def225a777337e4860b499f867599

# rolesanywhere refusal messages and delete echoes diverge on trust-anchor and attribute-mapping paths

> _Note: this issue was identified against the code base at commit `02cf7c25cd5def225a777337e4860b499f867599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableTrustAnchor

## Description
Three root causes on the rolesanywhere trust-anchor and attribute-mapping paths: two refusals whose status and error class are right but whose message body diverges from AWS, and one success echo that reports the wrong resource state. Three causes, numbered below; a fixer must close all three before submitting.

Cause 1 of 3 (DisableTrustAnchor - the 404 message drops AWS's operation prefix):
AWS: Smithy binds DisableTrustAnchor to POST /trustanchor/{trustAnchorId}/disable (input ScalarTrustAnchorRequest, required @httpLabel trustAnchorId -> Uuid @length 36..36 @pattern [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}; output TrustAnchorDetailResponse; errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404). For a trustAnchorId that no stored trust anchor carries (never created, or already deleted) AWS answers 404 ResourceNotFoundException whose message is 'Error disabling TrustAnchor. TrustAnchor not found' (recorded behavior only - the model documents the error class, not the text). The whole message is FIXED text for this error class: an operation-scoped prefix 'Error disabling TrustAnchor. ' followed by this service's fixed trust-anchor not-found tail 'TrustAnchor not found'. No part of it is taken from the request or from state - the requested id is not interpolated.
Emulator: same 404 and the same error class, but the message is only the bare tail 'TrustAnchor not found' - the disable handler builds its refusal from the shared helper crate::state::addressed_trust_anchor_not_found_message(), which carries no per-operation prefix, so the refusal does not say which operation failed.
Why: the error message is part of the error contract. A client or test that compares the refusal text against AWS's never matches, and a human (or log scraper) reading the refusal cannot tell a failed disable apart from a failed read of the same missing anchor, because the operation-naming prefix is gone.
Fix: the DisableTrustAnchor handler's not-found error construction (and the shared trust-anchor not-found message helper it calls) must render AWS's operation-prefixed text for this path while leaving the bare-tail wording where AWS uses it.
Traces: rolesanywhere-not-found-error-paths#task#10, rolesanywhere-trust-anchor-lifecycle#teardown#9.

Cause 2 of 3 (DeleteAttributeMapping - the 404 carries a message body AWS does not send):
AWS: Smithy binds DeleteAttributeMapping to DELETE /profiles/{profileId}/mappings (required @httpLabel profileId -> Uuid @length 36..36 with the UUID pattern, required @httpQuery certificateField, optional @httpQuery specifiers; output required profile -> ProfileDetail; errors AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400). For a profileId no profile carries, AWS answers 404 ResourceNotFoundException with NO message member in the body at all (recorded behavior only: the recorded response's $.message is absent, the error class travels in x-amzn-errortype). There is therefore no message template for this error class on this operation - absence is the requirement.
Emulator: same 404 and the same error class, but it volunteers a message - "The item doesn't exist. arn: <profile ARN> ; aborting." - composed by crate::state::arn_not_found_message(crate::state::profile_arn(&region, &input.profile_id)) in the DeleteAttributeMapping handler. (The ARN in the recorded run is a per-run value filling that template, not a requirement; the defect is that any message member is serialized here.)
Why: a client that branches on whether a not-found refusal carries explanatory text, or that compares the refusal body against AWS's, sees a field AWS never sends; the emulator also echoes a composed profile ARN for a profile that does not exist, which AWS deliberately does not disclose on this path.
Fix: the DeleteAttributeMapping handler's not-found error construction - omit the message member for this operation's ResourceNotFoundException instead of routing it through the shared ARN-keyed not-found message helper.
Traces: rolesanywhere-not-found-error-paths#task#13.

Cause 3 of 3 (DeleteTrustAnchor - the deletion receipt reports a disabled anchor where AWS reports the live one):
AWS: Smithy binds DeleteTrustAnchor to DELETE /trustanchor/{trustAnchorId} (success 200, output TrustAnchorDetailResponse with required trustAnchor -> TrustAnchorDetail; errors AccessDeniedException 403, ResourceNotFoundException 404). The echoed TrustAnchorDetail reports the resource's state as it stood when the delete was accepted, including its enabled flag. In the recorded trust-anchor lifecycle the anchor is enabled at delete time, so AWS's delete response reports trustAnchor.enabled = true.
Emulator: 200 with the right shape, but trustAnchor.enabled = false. The delete handler itself is faithful - it removes the record and projects record.to_detail() without touching the flag - so the stored flag was already false when the delete ran: the earlier DisableTrustAnchor in the lifecycle does write enabled=false, and nothing ever writes it back to true. Hypothesis: the re-enable leg of the lifecycle never lands because EnableTrustAnchor is an unimplemented scaffold answering 501, so the anchor stays disabled for the rest of the fixture. Open issue 20261101T125141Z-rolesanywhere-enabletrustanchor-is-an-unimplemented-scaffold-stub-and.md describes that stub from the behavioral suite and carries no wire replay key, so it does not cover this step or this response divergence.
Why: the deletion receipt is how a client learns the final state of the resource it just removed; reporting a disabled anchor where AWS reports an enabled one inverts the trust anchor's kill switch in audit and teardown logic, and any client that re-enables an anchor before deleting it records the opposite of what happened.
Fix: the trust-anchor enabled-state transitions in service state - the enable path must write enabled=true (and bump updatedAt) the way the disable path writes false - so DeleteTrustAnchor's projection of the removed record reports the anchor's actual last state.
Traces: rolesanywhere-trust-anchor-lifecycle#teardown#7.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (score 56.5%; emulator up, /__admin/health 200). The three clusters below carry no stub_path and no owning issue.

Cause 1 - DisableTrustAnchor, expected 404 / actual 404, fixture rolesanywhere-not-found-error-paths step task#10 (cluster also covers rolesanywhere-trust-anchor-lifecycle#teardown#9):
  $.message: value_mismatch (expected 'Error disabling TrustAnchor. TrustAnchor not found', got 'TrustAnchor not found')
  Fixed text: the entire expected message ('Error disabling TrustAnchor. ' + 'TrustAnchor not found'); no variable part.
  Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/disable_trust_anchor.rs lines 149-155 (message: Some(crate::state::addressed_trust_anchor_not_found_message())).

Cause 2 - DeleteAttributeMapping, expected 404 / actual 404, fixture rolesanywhere-not-found-error-paths step task#13:
  $.message: type_mismatch (expected None, got "The item doesn't exist. arn: arn:aws:rolesanywhere:us-west-2:000000000000:profile/aac7ac34-8e10-4cb8-ad84-794ff5571bd7 ; aborting.")
  AWS sends no $.message for this error class on this operation; the ARN in the emulator's text is a per-run value from the request plus region/account, not a requirement.
  Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/delete_attribute_mapping.rs lines 152-160 (crate::state::arn_not_found_message(crate::state::profile_arn(...))).

Cause 3 - DeleteTrustAnchor, expected 200 / actual 200, fixture rolesanywhere-trust-anchor-lifecycle step teardown#7:
  $.trustAnchor.enabled: value_mismatch (expected True, got False)
  Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/delete_trust_anchor.rs lines 143-156 (projects the removed record unchanged); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/disable_trust_anchor.rs line 161 (record.enabled = false); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_trust_anchor.rs (scaffolded stub, never writes the flag back).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Cause 1: POST /trustanchor/<a well-formed UUID that was never created, or one just deleted>/disable (fixture rolesanywhere-not-found-error-paths, step task#10; also rolesanywhere-trust-anchor-lifecycle teardown#9 after the anchor is deleted). Expect 404 ResourceNotFoundException with $.message exactly 'Error disabling TrustAnchor. TrustAnchor not found'; observe 'TrustAnchor not found'.
3. Cause 2: DELETE /profiles/<a well-formed UUID with no profile>/mappings?certificateField=x509Subject (fixture rolesanywhere-not-found-error-paths, step task#13). Expect 404 ResourceNotFoundException with NO $.message member in the body; observe a $.message of the form "The item doesn't exist. arn: <profile arn> ; aborting.".
4. Cause 3: POST /trustanchors to create a trust anchor, POST /trustanchor/{id}/disable, then re-enable it via POST /trustanchor/{id}/enable, then DELETE /trustanchor/{id} (fixture rolesanywhere-trust-anchor-lifecycle, step teardown#7). Expect the delete response's $.trustAnchor.enabled to be true (the anchor's state at delete time); observe false.
