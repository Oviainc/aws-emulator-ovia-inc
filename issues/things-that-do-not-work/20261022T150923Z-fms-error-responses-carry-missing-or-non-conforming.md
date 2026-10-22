Issue-Source: trace-sweep-divergence
Issue-Trace: fms-delete-apps-list-without-admin-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fms-delete-apps-list-without-admin-rejected fms-no-default-admin-access-denied fms-admin-not-configured-reads fms-policy-not-found-reads fms-resource-tagging-not-found
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms|DeleteAppsList|4xx->4xx|$.Message
Issue-Key: fms|GetResourceSet|4xx->4xx|$.Message
Issue-Key: fms|ListAdminsManagingAccount|4xx->4xx|$.Message
Issue-Key: fms|GetPolicy|4xx->4xx|$.Message
Issue-Key: fms|DeletePolicy|4xx->4xx|$.Message
Issue-Key: fms|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: fms|TagResource|4xx->4xx|$.Message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# FMS error responses carry missing or non-conforming Message values

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAppsList

## Description
Seven replayed FMS steps diverge in the same place: the `Message` member of the service's 400 client-error bodies. The HTTP status (400) and the error code match AWS on every one of them; what differs is that the emulator either omits `Message` entirely on the administrator-gate rejection path, or replaces FMS's fixed ResourceNotFoundException text with a bespoke, identifier-embedding string of its own. This issue bundles SEVEN causes and all seven must be closed.

GROUP A - `Message` member absent from the no-default-admin rejection (causes 1-2).

Cause 1 of 7: DeleteAppsList, no Firewall Manager default administrator configured.
AWS: Smithy (services/service-com-amazonaws-fms-awsfms-20180101) models the rejection error as a client error defaulting to httpError 400 with a single member `Message` targeting `ErrorMessage`; the model states no literal text. Recorded behavior: the 400 body carries a populated `Message` built from the caller's request/credential context - it names the account id and the Region being called ('No default admin could be found for account <accountId> in Region <region>'). The requirement is that `Message` is PRESENT and is composed from the calling account and Region; the recorded account id 000000000000 and Region us-west-2 are the recording's example values, not a requirement.
Emulator: the response body has no `Message` key at all - replay reports `$.Message: missing (expected 'No default admin could be found for account 000000000000 in Region us-west-2', got None)`.
Why: AWS SDKs surface this field as the exception's message, so a client gets an error with an empty message and no indication that the failure is a missing-administrator configuration problem rather than a bad request; operators and retry/branching logic that inspects the message see nothing.
Fix: the administrator-gate rejection path in the DeleteAppsList handler (and the shared gate helper it calls) must construct the modeled error WITH its `Message` populated from the calling account and Region, and the error serializer must emit that member.
Traces: fms-delete-apps-list-without-admin-rejected#task#1.

Cause 2 of 7: GetResourceSet, same administrator gate.
AWS: same modeled error shape and same recorded rule as cause 1 - a populated `Message` naming the calling account and Region.
Emulator: `$.Message: missing (expected 'No default admin could be found for account 000000000000 in Region us-west-2', got None)`; status 400 expected and 400 returned.
Why: identical client impact - an empty exception message on a read operation, so a caller cannot distinguish an unconfigured administrator from a missing resource set.
Fix: same component as cause 1 - the shared administrator-gate rejection path and the error serializer, reached here from the GetResourceSet handler.
Traces: fms-no-default-admin-access-denied#task#1.

GROUP B - ResourceNotFoundException `Message` is a bespoke string instead of FMS's constant text (causes 3-7).

Cause 3 of 7: ListAdminsManagingAccount for an account no administrator manages.
AWS: Smithy `ResourceNotFoundException` - client error, httpError 400 (defaulted), members {Message: ErrorMessage}, doc 'The specified resource was not found.'; the model does not state the wire text. Recorded behavior: FMS returns the literal constant `The referenced item does not exist.` The recordings establish this as a rule rather than one sample: the SAME literal appears on five different operations (ListAdminsManagingAccount, GetPolicy, DeletePolicy, ListTagsForResource, TagResource) across three fixtures and across two different lookup keys (a policy UUID and a resource ARN), and it never embeds the identifier that was looked up. So the requirement is: this error's `Message` is a fixed constant that does not vary with the request.
Emulator: returns 400 with `Message` = 'no Firewall Manager administrator account manages the calling account' - a hand-written, path-specific sentence.
Why: a client that matches on or displays the error message sees a string no FMS Region ever produces; the message-is-constant invariant that distinguishes FMS's not-found from its other 400s is lost, and emulator-internal phrasing is exposed.
Fix: the not-found construction behind the ListAdminsManagingAccount handler must raise ResourceNotFoundException with FMS's constant Message rather than formatting its own detail text; this belongs in the shared error-mapping helper so all not-found sites get it.
Traces: fms-admin-not-configured-reads#task#3.

Cause 4 of 7: GetPolicy for an unknown policy id.
AWS: as cause 3 - 400 ResourceNotFoundException whose `Message` is the constant `The referenced item does not exist.`, independent of the policy id supplied.
Emulator: 400 with `Message` = 'no policy with the id <policyId>' (recorded example: 'no policy with the id 109c24c6-0d5e-407e-98b5-8619b6cf4907') - it interpolates the requested id into the message, which AWS does not do.
Why: besides the wrong text, the emulator makes the message request-dependent, so a client cannot rely on the stable not-found string and any comparison against AWS's message fails.
Fix: the policy lookup in the GetPolicy handler / fms state layer must surface the modeled not-found error through the shared mapping that supplies FMS's constant Message, dropping the interpolated id.
Traces: fms-policy-not-found-reads#task#1.

Cause 5 of 7: DeletePolicy for an unknown policy id.
AWS: as cause 3 - 400 ResourceNotFoundException with the same constant Message.
Emulator: 400 with `Message` = 'no policy with the id <policyId>' (same interpolated form as cause 4).
Why: same as cause 4, on the delete path - a caller that treats the documented not-found message as the signal that the policy is already gone (idempotent delete) does not match it.
Fix: same shared not-found mapping, reached from the DeletePolicy handler's state lookup.
Traces: fms-policy-not-found-reads#task#2.

Cause 6 of 7: ListTagsForResource for an unknown Firewall Manager resource ARN.
AWS: as cause 3 - 400 ResourceNotFoundException with the constant Message, which notably does NOT contain the ARN.
Emulator: 400 with `Message` = 'no Firewall Manager resource with the ARN <arn>' (recorded example ARN arn:aws:fms:us-west-2:000000000000:policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8).
Why: same wrong-text impact, and the emulator echoes the full ARN - including account id and Region - back into an error message where AWS emits none, so clients and tests calibrated on AWS diverge.
Fix: the tagging resource-resolution path must raise the modeled not-found error via the shared mapping with FMS's constant Message instead of formatting the ARN into it.
Traces: fms-resource-tagging-not-found#task#1.

Cause 7 of 7: TagResource for the same unknown resource ARN.
AWS: as cause 3 - 400 ResourceNotFoundException with the constant Message.
Emulator: 400 with `Message` = 'no Firewall Manager resource with the ARN <arn>'.
Why: same as cause 6, on the write path.
Fix: same shared not-found mapping, reached from the TagResource handler's resource resolution.
Traces: fms-resource-tagging-not-found#task#2.

No open issue covers any of these seven keys; the previously filed fms issues (the bootstrap issue, the DisassociateAdminAccount / ListMemberAccounts stub issues, and the two stub-group issues covering TagResource, ListTagsForResource, ListAdminsManagingAccount and the administrator-gated operations) are all RESOLVED - those operations now answer 400 with the right status and error code, and what remains is this Message-member divergence.

## Evidence
Replay of trace set simparity-wire-aws-v1: 14 steps, 1 passed, 13 mismatched, 7 divergence clusters - all seven on service `fms`, all with expected_status 400 and actual_status 400, all keyed on `$.Message`.

Group A (`Message` member absent; expected a populated message naming the calling account and Region):
- fms-delete-apps-list-without-admin-rejected#task#1 DeleteAppsList - detail: `$.Message: missing (expected 'No default admin could be found for account 000000000000 in Region us-west-2', got None)`
- fms-no-default-admin-access-denied#task#1 GetResourceSet - detail: `$.Message: missing (expected 'No default admin could be found for account 000000000000 in Region us-west-2', got None)`

Group B (ResourceNotFoundException Message is not FMS's constant text; expected the literal 'The referenced item does not exist.' on every one):
- fms-admin-not-configured-reads#task#3 ListAdminsManagingAccount - got 'no Firewall Manager administrator account manages the calling account'
- fms-policy-not-found-reads#task#1 GetPolicy - got 'no policy with the id 109c24c6-0d5e-407e-98b5-8619b6cf4907'
- fms-policy-not-found-reads#task#2 DeletePolicy - got 'no policy with the id 109c24c6-0d5e-407e-98b5-8619b6cf4907'
- fms-resource-tagging-not-found#task#1 ListTagsForResource - got 'no Firewall Manager resource with the ARN arn:aws:fms:us-west-2:000000000000:policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8'
- fms-resource-tagging-not-found#task#2 TagResource - got 'no Firewall Manager resource with the ARN arn:aws:fms:us-west-2:000000000000:policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8'

Model evidence (smithy_query on services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json):
- ResourceNotFoundException: structure, errorKind client, httpError 400 (defaulted), members {Message -> ErrorMessage}, doc 'The specified resource was not found.'
- InvalidOperationException / InvalidInputException: likewise client, httpError 400 (defaulted), single `Message` -> ErrorMessage member.
The model declares the `Message` member but no literal text, so Group B's requirement is grounded in the recordings' invariance (one identical constant across 5 operations, 3 fixtures and 2 kinds of lookup key) and Group A's in the message being composed from the request's account and Region. Specific account ids, ARNs, policy UUIDs and Regions quoted above are the recordings' example values, not requirements; the requirements are the 400 status, the error code, the PRESENCE of `Message`, and the constant-vs-request-derived rule for its content.

state_reset_failures: 0 and bootstrap_signal: false for this replay, so none of these steps ran against leaked prior-fixture state and the emulator was serving normally (health 200, data listener accepting).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from the checkout.
2. Replay trace set simparity-wire-aws-v1; all seven failures below reproduce with state_reset_failures 0.
3. Group A - administrator gate omits `Message`:
   a. With no Firewall Manager default administrator configured for the calling account/Region, call DeleteAppsList (fixture fms-delete-apps-list-without-admin-rejected, step task#1). Observed: HTTP 400 with the correct error code but NO `Message` key in the JSON body. Expected: 400 with `Message` populated and composed from the calling account and Region (recording: 'No default admin could be found for account 000000000000 in Region us-west-2').
   b. Same precondition, call GetResourceSet (fixture fms-no-default-admin-access-denied, step task#1). Same observed/expected.
4. Group B - ResourceNotFoundException message text:
   a. Call ListAdminsManagingAccount for an account no administrator manages (fixture fms-admin-not-configured-reads, step task#3). Observed 400 with Message 'no Firewall Manager administrator account manages the calling account'. Expected 400 with Message exactly 'The referenced item does not exist.'
   b. Call GetPolicy with an unknown policy id (fixture fms-policy-not-found-reads, step task#1), then DeletePolicy with the same unknown id (step task#2). Observed 400 with Message 'no policy with the id <id>' - the requested id interpolated. Expected 400 with the constant Message 'The referenced item does not exist.' in both cases.
   c. Call ListTagsForResource with an unknown Firewall Manager resource ARN (fixture fms-resource-tagging-not-found, step task#1), then TagResource with the same ARN (step task#2). Observed 400 with Message 'no Firewall Manager resource with the ARN <arn>'. Expected 400 with the constant Message 'The referenced item does not exist.' in both cases.
5. Re-replay all seven fixture#step keys (the representative fms-delete-apps-list-without-admin-rejected#task#1 plus the six in trace_ids) to confirm a fix; a fix that only populates Group A's Message leaves Group B's five causes failing, and vice versa.
