Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.concurrent-analysis-template-name-race
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.concurrent-analysis-template-name-race|run#28|AssertionError|assert dead["Message"] == (
Issue-Commit: ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e

# DeleteAnalysisTemplate not-found refusal is emulator-worded instead of AWS's "No analysis template found with id <id> for membership <id>"

> _Note: this issue was identified against the code base at commit `ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAnalysisTemplate

## Description
One net-new root cause on the cleanrooms control plane, reached through the DeleteAnalysisTemplate not-found refusal path. 1 cause must be closed before this issue is submitted.

Cause 1 of 1: deleting an analysis template that has already been deleted is refused with the correct error code and status but with a wholly emulator-authored message sentence.

AWS: Smithy: DeleteAnalysisTemplate is DELETE /memberships/{membershipIdentifier}/analysistemplates/{analysisTemplateIdentifier}, @idempotent, success HTTP 204 with an empty output, and models ResourceNotFoundException (httpError 404) among AccessDeniedException(403) | InternalServerException(500) | ThrottlingException(429) | ValidationException(400). ResourceNotFoundException requires message + resourceId + resourceType (ResourceType enum = CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP | CONFIGURED_TABLE_ASSOCIATION). The model does not state the refusal text, so the recorded behavior is the authority here: AWS refuses the second delete 404 ResourceNotFoundException with the message template "No analysis template found with id <analysisTemplateIdentifier> for membership <membershipIdentifier>". The fixed text of that template is "No analysis template found with id " and " for membership " (no trailing period); the two variable parts are the analysis-template identifier and the membership identifier, both taken verbatim from the request path. The uuids seen in any single replay are per-run values and are not part of the requirement.

Emulator: answers 404 with Code 'ResourceNotFoundException' (code and status correct) but the message "analysis template <analysisTemplateId> not found in membership <membershipId>" — its own lower-cased sentence, with the two identifiers in the opposite grammatical frame ("<id> not found in membership <id>" rather than "No analysis template found with id <id> for membership <id>"). Everything else in this check passes: the concurrent create race produces exactly one winner with ConflictException for each loser, ListAnalysisTemplates shows exactly the winner, the first delete succeeds and the template disappears from the listing, and the repeat delete is classified ResourceNotFoundException. Only the refusal wording diverges.

Why: a client or operational tooling that matches, logs or displays AWS's documented refusal sentence to distinguish "analysis template already gone" (a benign idempotent-delete outcome) from other 404s never matches the emulator's string, so idempotent cleanup paths and message-keyed error tables behave differently against the emulator than against AWS. Message-asserting conformance tests fail even though the code and status are right.

Fix: the DeleteAnalysisTemplate handler's not-found path in the cleanrooms service crate (and the shared cleanrooms not-found message builder it uses) — emit AWS's message template keyed on (analysisTemplateIdentifier, membershipIdentifier) from the request path, and populate the modeled required resourceId/resourceType members consistently with the resource that was actually addressed. Note the already-fixed issue 20261029T213946Z-cleanrooms-deleteanalysistemplate-stub.md stood this operation up and the happy path now works, so this is NOT a reopening of it — the operation answers, it is only the refusal text that is emulator-worded. The open issue 20261029T213946Z-cleanrooms-refusal-fidelity-duplicate-analysis-rule-classified-conflictexception.md records the same class of emulator-worded not-found text for GetPrivacyBudgetTemplate and the tagging plane, but names neither DeleteAnalysisTemplate nor this check, so it does not cover this cause.

Traces: cleanrooms.concurrent-analysis-template-name-race.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator: cleanrooms 8/21 passed, 13 failed. Twelve of those failures are already filed (two unimplemented-stub clusters covering ListMembers and CreateConfiguredTableAssociationAnalysisRule, plus six checks marked OWNED by open issues). The failure below is the only net-new one.

Cause 1 — cleanrooms.concurrent-analysis-template-name-race [mismatch], check line 90:
  assert dead["Message"] == (
      f"No analysis template found with id {winner_id} for membership {membership_id}")
failed with
  AssertionError: {'Message': 'analysis template c7607fe8-1444-4a0b-b436-043c0564a0d3 not found in membership 44776bc0-7e20-455b-99c3-01d5f020d460', 'Code': 'ResourceNotFoundException'}

Expected: HTTP 404, Code 'ResourceNotFoundException', Message "No analysis template found with id <analysisTemplateIdentifier> for membership <membershipIdentifier>".
Actual:   HTTP 404, Code 'ResourceNotFoundException' (correct), Message "<analysisTemplateIdentifier> not found in membership <membershipIdentifier>" prefixed with the literal "analysis template ".

The assertion immediately before it, `assert dead["Code"] == "ResourceNotFoundException"` (check line 206), passed, as did every earlier assertion in the check: 6 concurrent CreateAnalysisTemplate calls with an identical name in one membership yielded exactly 1 success and 5 ConflictException refusals, ListAnalysisTemplates returned exactly the winning id, and DeleteAnalysisTemplate on the winner succeeded and removed it from the listing. So the divergence is isolated to the not-found message text.

Model evidence (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json): DeleteAnalysisTemplate — http {method: DELETE, uri: /memberships/{membershipIdentifier}/analysistemplates/{analysisTemplateIdentifier}, code: 204}, idempotent, errors include ResourceNotFoundException httpError 404; ResourceNotFoundException requires message, resourceId and resourceType; ResourceType = CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP | CONFIGURED_TABLE_ASSOCIATION.

## Steps to Reproduce
Boot the emulator and run the cleanrooms behavioral checks (suite simparity-behavioral-aws-v1), check cleanrooms.concurrent-analysis-template-name-race.

1. CreateCollaboration (members=[], creatorMemberAbilities=[CAN_QUERY, CAN_RECEIVE_RESULTS], queryLogStatus=DISABLED), then CreateMembership on it.
2. CreateAnalysisTemplate(membershipIdentifier, name=<any valid name>, format=SQL, source={text: "SELECT 1 AS n"}) and keep the returned analysisTemplate.id.
3. DELETE /memberships/{membershipIdentifier}/analysistemplates/{id} — succeeds (204) and the template leaves ListAnalysisTemplates.
4. Issue the SAME DeleteAnalysisTemplate call a second time. Observe HTTP 404 Code 'ResourceNotFoundException' with Message "analysis template <id> not found in membership <membershipId>" instead of AWS's "No analysis template found with id <id> for membership <membershipId>".
