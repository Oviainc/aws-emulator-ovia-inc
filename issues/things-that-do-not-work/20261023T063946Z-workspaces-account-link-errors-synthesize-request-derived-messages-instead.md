Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-account-link-accept-reject-workflow
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-account-link-accept-reject-workflow workspaces-account-link-invitation-lifecycle
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|AcceptAccountLinkInvitation|4xx->4xx|$.message
Issue-Key: workspaces|RejectAccountLinkInvitation|4xx->4xx|$.message
Issue-Key: workspaces|CreateAccountLinkInvitation|4xx->4xx|$.message
Issue-Key: workspaces|GetAccountLink|4xx->4xx|$.message
Issue-Key: workspaces|DeleteAccountLinkInvitation|4xx->4xx|$.message
Issue-Commit: f6e4ec6cad9a7d9cb69a8c1b88646cc41a0ad580

# Account-link errors synthesize request-derived messages instead of fixed service text

> _Note: this issue was identified against the code base at commit `f6e4ec6cad9a7d9cb69a8c1b88646cc41a0ad580`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptAccountLinkInvitation

## Description
The five WorkSpaces account-link operations all return the correct HTTP status (400) and the correct error code; they diverge ONLY in the `message` member of the error body, and they diverge the same way: the emulator builds the text out of the request (echoing the caller's id, or rendering a generic constraint-validation envelope that names the member, the offending value and the regex), whereas the real service returns a fixed, request-INDEPENDENT string per error class.

The rule behind AWS's text is established by the corpus rather than by the model: two different link ids (link-doesnotexist0000, link-doesnotexist0001) produce the SAME ResourceNotFoundException message, and three different operations failing on two different members (targetAccountId, linkId) produce the SAME ValidationException message. That request-independence is the requirement here -- not the particular recorded sentences, which are quoted below only as examples of it. Smithy states nothing about the text: ValidationException and ResourceNotFoundException each model `message` as a plain ExceptionMessage, and ResourceNotFoundException's doc is only "The resource could not be found."

There are 5 causes. All share one fault, so a fix almost certainly lives in one place -- the account-link error construction -- but each cause must be closed and re-replayed.

Cause 1 of 5: AcceptAccountLinkInvitation on an unknown link id.
  AWS: recorded behavior only -- 400 ResourceNotFoundException whose message does not mention the requested link id (recorded example: 'Requested AccountLink could not be found.'). Smithy: AcceptAccountLinkInvitation.LinkId @required, @pattern ^link-.{8,24}$; modeled error ResourceNotFoundException (httpError 400, members message + ResourceId).
  Emulator: same 400 and same error code, but message is built from the request (recorded example: 'The account link link-doesnotexist0000 could not be found.').
  Why: a client or recorded-assertion suite that matches on the not-found text sees a different string for every link id, and the id leaks into error text the service keeps out of it -- the service carries resource identity in the modeled `ResourceId` member, not in prose.
  Fix: the AcceptAccountLinkInvitation handler's not-found path (error construction), not its status or code selection.
  Traces: workspaces-account-link-accept-reject-workflow#task#1.

Cause 2 of 5: RejectAccountLinkInvitation on an unknown link id.
  AWS: recorded behavior only -- 400 ResourceNotFoundException, request-independent message (recorded example: 'Requested AccountLink could not be found.'). Smithy: RejectAccountLinkInvitation.LinkId @required, @pattern ^link-.{8,24}$; modeled error ResourceNotFoundException.
  Emulator: same 400 and code, message echoes the id (recorded example: 'The account link link-doesnotexist0001 could not be found.').
  Why: identical to cause 1, on the reject half of the invitation workflow, so a client cannot rely on a stable not-found text on either side of the accept/reject pair.
  Fix: the RejectAccountLinkInvitation handler's not-found path (error construction).
  Traces: workspaces-account-link-accept-reject-workflow#task#2.

Cause 3 of 5: CreateAccountLinkInvitation with a TargetAccountId that violates its pattern.
  AWS: recorded behavior only -- 400 ValidationException with a fixed message that names neither member nor constraint (recorded example: 'The request is invalid.'). Smithy: CreateAccountLinkInvitation.TargetAccountId @required, target AwsAccount @pattern ^\d{12}$; modeled error ValidationException (httpError 400).
  Emulator: same 400 and code, but the message is a generic constraint-validation envelope rendered from the request (recorded example: "1 validation error detected: Value '12345' at 'targetAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$").
  Why: the emulator exposes framework validation detail -- member name in a casing that is not the modeled member (`targetAccountId` vs `TargetAccountId`), the rejected value, and the raw regex -- where the service deliberately returns one opaque sentence. Clients written against the opaque form, and fixtures asserting it, diverge.
  Fix: the shared account-link input validation layer, which should emit the service's ValidationException text rather than the generic constraint reporter.
  Traces: workspaces-account-link-invitation-lifecycle#task#2.

Cause 4 of 5: GetAccountLink with a LinkId that violates its pattern.
  AWS: recorded behavior only -- 400 ValidationException, fixed message (recorded example: 'The request is invalid.'). Smithy: GetAccountLink.LinkId (optional) target LinkId @pattern ^link-.{8,24}$; modeled error ValidationException.
  Emulator: same 400 and code, generic envelope naming member, value and regex (recorded example: "1 validation error detected: Value 'not-a-link' at 'linkId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^link-.{8,24}$").
  Why: same leak as cause 3, reached through a read operation, so even non-mutating probes get framework text instead of the service's.
  Fix: the GetAccountLink input validation path (shared account-link validation layer).
  Traces: workspaces-account-link-invitation-lifecycle#task#5 (cluster also covers task#6).

Cause 5 of 5: DeleteAccountLinkInvitation with a LinkId that violates its pattern.
  AWS: recorded behavior only -- 400 ValidationException, fixed message (recorded example: 'The request is invalid.'). Smithy: DeleteAccountLinkInvitation.LinkId @required, @pattern ^link-.{8,24}$; modeled error ValidationException.
  Emulator: same 400 and code, generic envelope naming member, value and regex (recorded example: "1 validation error detected: Value 'not-a-link' at 'linkId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^link-.{8,24}$").
  Why: same leak as causes 3 and 4, completing the pattern across create/read/delete of the invitation lifecycle.
  Fix: the DeleteAccountLinkInvitation input validation path (shared account-link validation layer).
  Traces: workspaces-account-link-invitation-lifecycle#task#7 (cluster also covers task#8).

Not covered by the three open workspaces issues (CreateWorkspaces FailedRequests, DescribeConnectionAliases, IP access control group mutations) -- none of those touches account-link error construction. Scoped LOW deliberately: every cause matches AWS on status and error code, so only error text diverges.

## Evidence
Replay of simparity-wire-aws-v1 (13 fixtures, 83 steps, score 33.7%, state_reset_failures 0, bootstrap_signal false). Five unowned mismatch clusters, each with expected_status 400 == actual_status 400 and the diverging key being $.message alone:

1. workspaces-account-link-accept-reject-workflow#task#1 AcceptAccountLinkInvitation -- $.message value_mismatch: expected 'Requested AccountLink could not be found.', got 'The account link link-doesnotexist0000 could not be found.'
2. workspaces-account-link-accept-reject-workflow#task#2 RejectAccountLinkInvitation -- $.message value_mismatch: expected 'Requested AccountLink could not be found.', got 'The account link link-doesnotexist0001 could not be found.'
3. workspaces-account-link-invitation-lifecycle#task#2 CreateAccountLinkInvitation -- $.message value_mismatch: expected 'The request is invalid.', got "1 validation error detected: Value '12345' at 'targetAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$"
4. workspaces-account-link-invitation-lifecycle#task#5 GetAccountLink (cluster also affects task#6) -- $.message value_mismatch: expected 'The request is invalid.', got "1 validation error detected: Value 'not-a-link' at 'linkId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^link-.{8,24}$"
5. workspaces-account-link-invitation-lifecycle#task#7 DeleteAccountLinkInvitation (cluster also affects task#8) -- $.message value_mismatch: expected 'The request is invalid.', got "1 validation error detected: Value 'not-a-link' at 'linkId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^link-.{8,24}$"

Request-independence evidence: causes 1 and 2 use distinct link ids yet AWS returns one identical sentence; causes 3, 4 and 5 span three operations and two members yet AWS returns one identical sentence. The emulator's text varies with the request in all five.

Model evidence (services/service-com-amazonaws-workspaces-workspacesservice/smithy-model/com-amazonaws-workspaces-workspacesservice.json): ValidationException and ResourceNotFoundException are both errorKind=client, httpError 400; ValidationException has only `message` (ExceptionMessage); ResourceNotFoundException has `message` plus `ResourceId` (NonEmptyString, the modeled place for the resource identity the emulator instead inlines into prose). Neither shape models the message text, hence the "recorded behavior only" basis above.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the booted emulator (trace set simparity-wire-aws-v1).
2. Not-found text: in fixture workspaces-account-link-accept-reject-workflow, step task#1 calls AcceptAccountLinkInvitation with an unknown link id (link-doesnotexist0000) and step task#2 calls RejectAccountLinkInvitation with link-doesnotexist0001. Both return 400 with the expected error code; compare $.message against the recording -- the emulator's text embeds the link id while the recording's does not, and the recording's text is byte-identical for the two different ids.
3. Validation text: in fixture workspaces-account-link-invitation-lifecycle, step task#2 calls CreateAccountLinkInvitation with TargetAccountId '12345' (violates ^\\d{12}$); steps task#5/task#6 call GetAccountLink and steps task#7/task#8 call DeleteAccountLinkInvitation with LinkId 'not-a-link' (violates ^link-.{8,24}$). All return 400 with the expected error code; compare $.message -- the emulator returns a '1 validation error detected: ...' envelope naming the member, the value and the regex, while the recording returns one fixed sentence for all three operations.
4. Confirm a fix by re-replaying all five bindings: workspaces-account-link-accept-reject-workflow#task#1 and #task#2, and workspaces-account-link-invitation-lifecycle#task#2, #task#5 and #task#7 (the last two clusters also cover task#6 and task#8).
