Issue-Source: trace-sweep-divergence
Issue-Trace: dsdata-member-constraint-validation-envelopes
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsdata-member-constraint-validation-envelopes dsdata-sdk-unreachable-request-envelopes
Issue-Shape-Id: com.amazonaws.directoryservicedata#DirectoryServiceData
Issue-Key: ds-data|CreateGroup|4xx->4xx|$.Message
Issue-Key: ds-data|SearchUsers|4xx->4xx|$.Message
Issue-Key: ds-data|SearchGroups|4xx->4xx|$.Message
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ValidationException constraint messages rendered unlike AWS for enum and list members

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Directory Service Data returns HTTP 400 ValidationException for all three recorded constraint violations, and the emulator does too -- status and error code match on every step. What diverges is the $.Message body: the emulator has its own constraint-violation renderer instead of reproducing the AWS Smithy framework's message grammar. Three distinct rendering faults, all in the same renderer, so all three must be closed before this issue is done.

This issue is NOT covered by the existing open ds-data issues (20261022T085720Z-ds-data-no-directory-authorization-gate-absent-directoryid.md, 20261022T085720Z-ds-ds-data-cross-plane-contract-broken-ds.md, 20261023T134711Z-ds-data-user-objects-are-not-real-active.md): those concern authorization gating and user-object derivation, not validation-message rendering.

AWS renders every constraint violation with the fixed template: "<N> validation error(s) detected: Value '<value>' at '<path>' failed to satisfy constraint: <constraint phrase>". The fixed text is the wrapper; the variable parts are the violation count, the offending value, the member path and the constraint phrase, all derived from the request and the model. The three causes below are each a divergence in how a variable part is produced.

Cause 1 of 3: CreateGroup -- enum value set is sorted instead of emitted in AWS's order.
AWS: CreateGroup.GroupType targets the GroupType enum (Smithy: com.amazonaws.directoryservicedata#GroupType; CreateGroup errors include ValidationException, httpError 400). For an unmodeled value AWS answers 400 ValidationException with constraint phrase "Member must satisfy enum value set: [Security, Distribution]" -- the bracketed set is emitted in a FIXED per-enum order which the recording shows as Security then Distribution. Fixed text: "1 validation error detected: Value '<value>' at 'groupType' failed to satisfy constraint: Member must satisfy enum value set: [Security, Distribution]". Variable part: only the rejected value, taken from the request. Hypothesis: the order is the service's internal model declaration order; note the on-disk Smithy model lists the values as [Distribution, Security], so the order cannot be taken from that list either -- it must match the recorded AWS order per enum.
Emulator: emits "Member must satisfy enum value set: [Distribution, Security]" -- the set alphabetized/model-ordered rather than in AWS's order.
Why: a client or SDK protocol test that asserts on the enum-value-set message (and any operator reading it to learn the accepted values in AWS's documented order) sees a message AWS never produces; the accepted-value list is the actionable payload of this error, so a reordered list makes the emulator's error text non-substitutable for the real one.
Fix: the enum-constraint branch of ds-data's input validation / ValidationException message renderer -- the accepted-value list must come from a per-enum ordered table matching AWS, not from a sorted or model-iteration order.
Traces: dsdata-member-constraint-validation-envelopes#task#6 (CreateGroup, expected 400 / actual 400, $.Message value_mismatch).

Cause 2 of 3: SearchUsers -- list member violations reported per element with a single constraint instead of aggregated at the list with the full constraint set.
AWS: SearchUsers.SearchAttributes targets LdapDisplayNameList (Smithy: list, @length min 1 max 25; member LdapDisplayName, @length min 1 max 63, @pattern ^[A-Za-z*][A-Za-z-*]*$; SearchUsers errors include ValidationException, httpError 400). When an element violates a member constraint, AWS attributes the failure to the LIST member itself: the path is the list's wire member name ('searchAttributes'), the value is the whole list rendered bracketed and comma-joined ('[1BadAttribute]'), and the constraint phrase is the element's COMPLETE constraint set, bracketed and comma-joined: "Member must satisfy constraint: [Member must have length less than or equal to 63, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [A-Za-z*][A-Za-z-*]*]". Fixed text: that wrapper plus the three constraint phrases in that order; variable parts: the list contents (from the request), the member path (from the model), and the limits/pattern (from the member's constraint traits).
Emulator: answers "1 validation error detected: Value '1BadAttribute' at 'searchAttributes.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z*][A-Za-z-*]*" -- it indexes into the list for the path, renders the single bare element as the value, and names only the one constraint that actually failed rather than the member's whole constraint set.
Why: a client that locates the offending input from the error path is pointed at a path ('searchAttributes.1.member') that no AWS response ever uses and that its own request model has no field for, and it loses the other two constraints AWS discloses, so it cannot tell the user what a valid search attribute must look like.
Fix: the list-member branch of the same renderer -- aggregate member-constraint violations onto the containing list member, render the value as the bracketed list, and emit the member's full constraint set in AWS's order (max length, min length, pattern).
Traces: dsdata-member-constraint-validation-envelopes#task#7 (SearchUsers, expected 400 / actual 400, $.Message value_mismatch); same cluster also covers dsdata-sdk-unreachable-request-envelopes#task#2.

Cause 3 of 3: SearchGroups -- list-length violation summarizes the value as an item count instead of rendering the list.
AWS: SearchGroups.SearchAttributes targets LdapDisplayNameList (Smithy: @length min 1 max 25; SearchGroups errors include ValidationException, httpError 400). When the list is over-long AWS renders the offending VALUE as the actual list -- every element, bracketed and comma-joined -- then the length phrase: "1 validation error detected: Value '[<elem>, <elem>, ...]' at 'searchAttributes' failed to satisfy constraint: Member must have length less than or equal to 25". Fixed text: the wrapper and "Member must have length less than or equal to"; variable parts: the list contents from the request and the limit (25) from the model's @length max.
Emulator: answers "... Value '26 items' at 'searchAttributes' failed to satisfy constraint: Member must have length less than or equal to 25" -- it substitutes a count summary ('<n> items') for the value rendering. Path, limit and status are right; only the value rendering is wrong.
Why: the message no longer echoes the input AWS says it echoes, so a client that parses or logs the rejected value gets a synthesized phrase instead of the submitted list, and any assertion on AWS's value-echo fails.
Fix: the value-rendering helper of the same renderer -- collection values must be serialized as the bracketed comma-joined element list for every constraint kind, with no count shortcut for large collections.
Traces: dsdata-member-constraint-validation-envelopes#task#8 (SearchGroups, expected 400 / actual 400, $.Message value_mismatch).

## Evidence
Replay of trace set simparity-wire-aws-v1: 19 steps, 15 passed, 4 mismatched, score 78.9%. All four mismatches are ds-data ValidationException bodies at a MATCHING status (expected 400, actual 400) and a matching error shape; only $.Message diverges.

Cause 1 -- divergence_key service=ds-data|CreateGroup|status=400->400|keys=$.Message
  expected: "1 validation error detected: Value 'NotAGroupType' at 'groupType' failed to satisfy constraint: Member must satisfy enum value set: [Security, Distribution]"
  actual:   "1 validation error detected: Value 'NotAGroupType' at 'groupType' failed to satisfy constraint: Member must satisfy enum value set: [Distribution, Security]"
  (fixed text identical; the bracketed accepted-value set is in the wrong order. 'NotAGroupType' is the request's value, not part of the template.)

Cause 2 -- divergence_key service=ds-data|SearchUsers|status=400->400|keys=$.Message (affects 2 steps)
  expected: "1 validation error detected: Value '[1BadAttribute]' at 'searchAttributes' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 63, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [A-Za-z*][A-Za-z-*]*]"
  actual:   "1 validation error detected: Value '1BadAttribute' at 'searchAttributes.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z*][A-Za-z-*]*"
  (three independent differences: value not bracketed as a list, path indexed into the element, and only the violated constraint emitted instead of the member's whole set. '1BadAttribute' is the request's element; 63/1 and the pattern come from the LdapDisplayName constraint traits.)

Cause 3 -- divergence_key service=ds-data|SearchGroups|status=400->400|keys=$.Message
  expected: "1 validation error detected: Value '[Cn, Cn, Cn, ... ]' at 'searchAttributes' failed to satisfy constraint: Member must have length less than or equal to 25" (all 26 submitted elements rendered)
  actual:   "1 validation error detected: Value '26 items' at 'searchAttributes' failed to satisfy constraint: Member must have length less than or equal to 25"
  (the element list is the request's; the limit 25 is the model's @length max. '26 items' is emulator-synthesized text AWS never emits.)

Smithy evidence (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  GroupType = enum, values [Distribution, Security]
  LdapDisplayNameList = list, @length min 1 max 25, member LdapDisplayName @length min 1 max 63 @pattern ^[A-Za-z*][A-Za-z-*]*$
  CreateGroup / SearchUsers / SearchGroups all declare ValidationException (errorKind client, httpError 400)

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.

Cause 1 -- fixture dsdata-member-constraint-validation-envelopes, step task#6: POST /Groups/CreateGroup?DirectoryId=<valid d-xxxxxxxxxx> with SAMAccountName set and GroupType set to a value outside the GroupType enum (recording uses 'NotAGroupType'). Observe 400 ValidationException and compare $.Message: the accepted-value set is emitted as [Distribution, Security] instead of AWS's [Security, Distribution].

Cause 2 -- fixture dsdata-member-constraint-validation-envelopes, step task#7 (also dsdata-sdk-unreachable-request-envelopes, step task#2): POST /Users/SearchUsers?DirectoryId=<valid> with SearchString set and SearchAttributes a single-element list whose element violates the LdapDisplayName pattern (recording uses ['1BadAttribute']). Observe 400 ValidationException and compare $.Message: the emulator reports path 'searchAttributes.1.member' with the bare element and only the pattern constraint, instead of path 'searchAttributes' with the bracketed list and the member's full constraint set.

Cause 3 -- fixture dsdata-member-constraint-validation-envelopes, step task#8: POST /Groups/SearchGroups?DirectoryId=<valid> with SearchString set and SearchAttributes a list of 26 valid attribute names (over the @length max of 25). Observe 400 ValidationException and compare $.Message: the value is rendered as the summary '26 items' instead of the bracketed, comma-joined list of the 26 submitted elements.

All three pass when $.Message matches AWS's rendering byte-for-byte apart from the request-derived variable parts.
