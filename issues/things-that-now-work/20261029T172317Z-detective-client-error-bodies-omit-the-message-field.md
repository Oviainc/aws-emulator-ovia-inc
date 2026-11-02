Issue-Source: trace-sweep-divergence
Issue-Trace: detective-invitations-and-membership-negative
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: detective-invitations-and-membership-negative detective-member-error-paths
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective|RejectInvitation|4xx->4xx|$.message
Issue-Key: detective|StartMonitoringMember|4xx->4xx|$.message
Issue-Key: detective|GetMembers|4xx->4xx|$.message
Issue-Key: detective|DeleteMembers|2xx->2xx|$.AccountIds
Issue-Key: detective|DeleteMembers|2xx->2xx|$.UnprocessedAccounts[*].Reason
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# Detective client-error bodies omit the message field and reword AWS's refusal text

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RejectInvitation

## Description
Four net-new root causes in how Detective words its refusals: the 404 bodies carry no message at all, the boundary validation refusal is the emulator's own generic prose instead of Detective's wording, and the partial-success body for a member removal both drops a modeled member and restates the per-account reason in its own words. Every cause below MUST be closed before submitting; they are four distinct gaps, not one.

Cause 1 of 4: RejectInvitation's 404 has no message member.
AWS: Smithy: RejectInvitation errors include ResourceNotFoundException (errorKind client, httpError 404) whose only member is Message -> ErrorMessage, serialised on the wire as the body key 'message'; the model documents the error as 'The request refers to a nonexistent resource.'. The recording shows Detective refusing a reject against a graph for which the caller holds no invitation with 404 and the fixed text 'No such membership' - no variable part, no echoed graph ARN or account id.
Emulator: the status is right (404->404) but $.message is missing - the body carries no message member at all, so the fixed text 'No such membership' is absent.
Why: every AWS SDK surfaces the exception message as the error's user-facing text; with the member absent the client raises ResourceNotFound with an empty message, so a caller cannot tell a missing invitation from any other 404 and operators get a blank error in logs. Clients and tests that match on the documented wording never match.
Fix: the RejectInvitation handler's not-found refusal and the shared ResourceNotFoundException serializer - populate Message with the fixed text 'No such membership'.
Traces: detective-invitations-and-membership-negative#task#3.

Cause 2 of 4: StartMonitoringMember's 404 has no message member either.
AWS: Smithy: StartMonitoringMember errors include the same ResourceNotFoundException (404, member Message). The recording shows a start-monitoring call naming an account that is not a member of the graph refused with 404 and the same fixed text 'No such membership' - again entirely fixed, with no request value interpolated.
Emulator: 404->404 with $.message missing.
Why: as Cause 1 - an empty exception message on the path a client uses to enable a member, so the "this account was never invited" case is indistinguishable from any other not-found and cannot be branched on.
Fix: the StartMonitoringMember handler's member-lookup refusal, through the same ResourceNotFoundException serializer. That both operations exhibit it points at the serializer dropping Message, not at two independent handler omissions.
Traces: detective-member-error-paths#task#3.

Cause 3 of 4: GetMembers' boundary validation refusal is generic framework prose instead of Detective's wording.
AWS: Smithy: GetMembers.AccountIds -> AccountIdList, whose member AccountId is @length min 12 max 12 @pattern ^[0-9]+$; the refusal is ValidationException (errorKind client, httpError 400, members Message/ErrorCode/ErrorCodeReason), 'The request parameters are invalid.'. The recording shows Detective stating a constraint violation on a list member as a single clause naming the shape and member path and the constraint class: 'GetMembersRequest.AccountIds members did not match the pattern.' - the fixed parts are '<InputShapeName>.<memberName> members did not match the pattern.' and the only variable parts are the input shape and member names; the offending value is NOT echoed and the failing index is NOT named.
Emulator: 400->400 but $.message is "1 validation error detected: Value '<the offending account id>' at 'accountIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$" - a different error-message template entirely (the generic AWS-framework 'N validation errors detected' form), lower-cased member paths, and the rejected value echoed.
Why: the message is the only machine-readable detail Detective gives for a validation failure (ValidationException's ErrorCode/ErrorCodeReason are unset), so a client parsing or asserting on it breaks; and echoing the rejected account id back leaks a request value this error class does not include.
Fix: the GetMembers input validation and the ValidationException message builder for list-member constraint failures - emit Detective's '<InputShape>.<member> members did not match the pattern.' clause rather than the generic validation-error template.
Traces: detective-member-error-paths#task#4.

Cause 4 of 4: DeleteMembers' partial-success body omits AccountIds and rewords the per-account reason.
AWS: Smithy: DeleteMembers output is {AccountIds -> AccountIdList, UnprocessedAccounts -> UnprocessedAccountList}, where UnprocessedAccount is {AccountId, Reason -> UnprocessedReason (a string), 'The reason that the member account request could not be processed.'}. The recording shows a removal naming an account that is not a member answering 200 with AccountIds PRESENT and empty (the list of accounts actually removed - here none) and one UnprocessedAccounts entry whose Reason is the same fixed text Detective uses for the missing-membership class, 'No such membership'.
Emulator: $.AccountIds is missing where the recording has an empty list, and $.UnprocessedAccounts[0].Reason is 'The account is not a member account in the behavior graph.' where the recording has 'No such membership'. 200->200.
Why: a client reads AccountIds to learn which removals succeeded; when the member is absent entirely it cannot distinguish "nothing was removed" from "the field was not served", and iterating it requires a null guard AWS never requires. The reworded reason breaks the same matching as Causes 1-2 and makes the unprocessed reason inconsistent with the 404 text for the identical condition.
Fix: the DeleteMembers handler's response builder (always serialise AccountIds, empty when no account was removed) and the single place the missing-membership reason text is produced, so the 404 message and the UnprocessedAccount reason are one string.
Traces: detective-member-error-paths#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 56.2%, 12 divergence clusters, all service=detective, none stub-blocked, none owned by an open issue - the 9 prior detective issues are all resolved).

Cause 1 - detective-invitations-and-membership-negative#task#3, RejectInvitation, expected 404 / actual 404:
  $.message: missing (expected 'No such membership', got None)

Cause 2 - detective-member-error-paths#task#3, StartMonitoringMember, expected 404 / actual 404:
  $.message: missing (expected 'No such membership', got None)

Cause 3 - detective-member-error-paths#task#4, GetMembers, expected 400 / actual 400:
  $.message: value_mismatch
    expected: 'GetMembersRequest.AccountIds members did not match the pattern.'
    got:      "1 validation error detected: Value '1234abcd9012' at 'accountIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$"
  (the offending value '1234abcd9012' and the index in 'accountIds.1.member' are per-request values; the requirement is the expected template, which names the input shape and member and echoes neither.)

Cause 4 - detective-member-error-paths#task#2, DeleteMembers, expected 200 / actual 200:
  $.AccountIds: missing (expected [], got None)
  $.UnprocessedAccounts[0].Reason: value_mismatch (expected 'No such membership', got 'The account is not a member account in the behavior graph.')

Status codes agree on all four steps; the divergence is the response body in each case.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture detective-invitations-and-membership-negative to task#3 - RejectInvitation against a graph the caller holds no invitation for. Status is 404 as recorded; inspect the body and observe there is no 'message' member (expected the fixed text 'No such membership').
3. Cause 2: replay fixture detective-member-error-paths to task#3 - StartMonitoringMember naming an account that is not a member of the graph. Status 404, body again carries no 'message'.
4. Cause 3: in the same fixture, task#4 - GetMembers whose AccountIds list contains an entry that violates AccountId's @pattern ^[0-9]+$. Status 400, but the message is the generic '<n> validation error(s) detected: ...' template with the rejected value echoed, instead of 'GetMembersRequest.AccountIds members did not match the pattern.'.
5. Cause 4: in the same fixture, task#2 - DeleteMembers naming a non-member account. Status 200, but the response has no AccountIds member (expected an empty list) and UnprocessedAccounts[0].Reason is reworded away from 'No such membership'.
