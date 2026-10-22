Issue-Source: trace-sweep-divergence
Issue-Trace: dsdata-member-constraint-validation-envelopes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsdata-member-constraint-validation-envelopes dsdata-sdk-unreachable-request-envelopes dsdata-request-layer-ordering-and-http-surface
Issue-Shape-Id: com.amazonaws.directoryservicedata#DirectoryServiceData
Issue-Key: ds-data|DescribeUser|4xx->4xx|$.Message
Issue-Key: ds-data|DescribeGroup|4xx->4xx|$.Message
Issue-Key: ds-data|ListUsers|4xx->4xx|$.Message
Issue-Key: ds-data|CreateGroup|4xx->4xx|$.Message
Issue-Key: ds-data|SearchUsers|4xx->4xx|$.Message
Issue-Key: ds-data|SearchGroups|4xx->4xx|$.Message
Issue-Key: ds-data|ListGroups|4xx->4xx|$.Message
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# ValidationException body omits Message for every member-constraint refusal

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
Directory Service Data refuses a request whose input member violates a modeled constraint with HTTP 400 ValidationException whose body carries a `Message` built FROM THE REQUEST: it names the rejected value, the member path and the constraint that was violated ("1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: <constraint>"). Because that text is derived from the request and the model's own constraint, the rule producing it is known and is a requirement; the exact recorded strings below are examples of that rule.

Across seven operations the emulator returns the correct 400 status and the correct `ValidationException` error type but NO `Message` member at all — the body's `Message` key is absent, so an SDK surfaces a constraint refusal with an empty reason and the client cannot tell which member it got wrong. The root cause is shared: the framework-level refusal wording is never rendered into the ds-data ValidationException envelope (service.rs `override_refusal` returns `None` and the default rendering drops the message for this service). There are 7 distinct causes, one per constrained member family; all 7 must be closed.

Cause 1 of 7 — DescribeUser, DirectoryId pattern.
AWS: Smithy DescribeUserRequest.DirectoryId -> DirectoryId @pattern ^d-[0-9a-f]{10}$, @required; error ValidationException (errorKind client, httpError 400, member Message). AWS answers 400 ValidationException with a Message naming the value, 'directoryId' and the pattern (recording: "1 validation error detected: Value 'not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: d-[0-9a-f]{10}").
Emulator: 400 ValidationException, `$.Message` missing (got None).
Why: a client logging or branching on the exception message gets nothing and cannot report which field is malformed.
Fix: the ds-data refusal-to-envelope rendering — service.rs `override_refusal` / the ValidationException serializer must populate `Message` from the refused member and constraint.
Traces: dsdata-member-constraint-validation-envelopes#task#1 (also dsdata-sdk-unreachable-request-envelopes#task#1).

Cause 2 of 7 — DescribeGroup, SAMAccountName pattern.
AWS: Smithy DescribeGroupRequest.SAMAccountName -> GroupName @required @length 1..64 @pattern ^[^:;|=+"*?<>/\\,\[\]@]+$; 400 ValidationException with a Message naming the value, 'sAMAccountName' and the pattern (recording: "... Value 'sim2real;group' at 'sAMAccountName' failed to satisfy constraint: Member must satisfy regular expression pattern: ...").
Emulator: 400 ValidationException, `$.Message` missing.
Why: same — no reason text for a rejected group name.
Fix: same refusal rendering, pattern constraints on string members.
Traces: dsdata-member-constraint-validation-envelopes#task#4.

Cause 3 of 7 — ListUsers, MaxResults range.
AWS: Smithy ListUsersRequest.MaxResults -> MaxResults @range min 1 max 250; a value of 251 is refused 400 ValidationException with a Message naming the value, 'maxResults' and the upper bound (recording: "... Value '251' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 250").
Emulator: 400 ValidationException, `$.Message` missing.
Why: a paginating client that over-asks cannot learn the page-size ceiling from the error.
Fix: same refusal rendering, numeric @range constraints.
Traces: dsdata-member-constraint-validation-envelopes#task#5 (also dsdata-request-layer-ordering-and-http-surface#task#1, #task#2, dsdata-sdk-unreachable-request-envelopes#task#5).

Cause 4 of 7 — CreateGroup, GroupType enum.
AWS: Smithy CreateGroupRequest.GroupType -> GroupType enum values [Distribution, Security]; an unmodeled value is refused 400 ValidationException with a Message naming the value, 'groupType' and the admissible set (recording: "... Value 'NotAGroupType' at 'groupType' failed to satisfy constraint: Member must satisfy enum value set: [Security, Distribution]").
Emulator: 400 ValidationException, `$.Message` missing.
Why: the client never learns which enum values are accepted.
Fix: same refusal rendering, enum constraints.
Traces: dsdata-member-constraint-validation-envelopes#task#6.

Cause 5 of 7 — SearchUsers, SearchAttributes member constraints.
AWS: Smithy SearchUsersRequest.SearchAttributes -> LdapDisplayNameList @required @length 1..25, member LdapDisplayName @length 1..63 @pattern ^[A-Za-z*][A-Za-z-*]*$; a bad element is refused 400 ValidationException with a Message naming the offending list value, 'searchAttributes' and the member constraint set (recording: "... Value '[1BadAttribute]' at 'searchAttributes' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 63, ... pattern: [A-Za-z*][A-Za-z-*]*]").
Emulator: 400 ValidationException, `$.Message` missing.
Why: list-member constraint violations become unattributable to an element.
Fix: same refusal rendering, constraints on list MEMBERS.
Traces: dsdata-member-constraint-validation-envelopes#task#7 (also dsdata-sdk-unreachable-request-envelopes#task#2).

Cause 6 of 7 — SearchGroups, SearchAttributes list length.
AWS: Smithy SearchGroupsRequest.SearchAttributes -> LdapDisplayNameList @length min 1 max 25; an over-long list is refused 400 ValidationException with a Message naming the list and the length ceiling (recording: "... at 'searchAttributes' failed to satisfy constraint: Member must have length less than or equal to 25").
Emulator: 400 ValidationException, `$.Message` missing.
Why: the client cannot learn the 25-attribute ceiling.
Fix: same refusal rendering, aggregate @length constraints.
Traces: dsdata-member-constraint-validation-envelopes#task#8.

Cause 7 of 7 — ListGroups, Realm pattern.
AWS: Smithy ListGroupsRequest.Realm -> Realm @length 1..255 @pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$; a malformed realm is refused 400 ValidationException with a Message naming the value, 'realm' and the pattern (recording: "... Value 'not a realm!' at 'realm' failed to satisfy constraint: Member must satisfy regular expression pattern: ...").
Emulator: 400 ValidationException, `$.Message` missing.
Why: same — no reason text for a rejected optional realm.
Fix: same refusal rendering, optional-member pattern constraints.
Traces: dsdata-member-constraint-validation-envelopes#task#9.

No open issue carries these replay keys (every cluster reported owned_by empty); the open issues 'Validation error messages inject a rejected-value rendering AWS does not emit' (controlcatalog) and 'ValidationException envelope and input validation diverge across wellarchitected operations' are other services and do not cover ds-data.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false, 19/19 steps mismatched, state_reset_failures 0). Seven clusters, all service=ds-data, verdict=mismatch, expected_status 400 / actual_status 400, each reporting only `$.Message: missing`:
- DescribeUser  dsdata-member-constraint-validation-envelopes#task#1 — expected "1 validation error detected: Value 'not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: d-[0-9a-f]{10}", got None (4 affected steps)
- DescribeGroup dsdata-member-constraint-validation-envelopes#task#4 — expected "... 'sim2real;group' at 'sAMAccountName' ... pattern: [^:;|=+\"*?<>/\\,\\[\\]@]+", got None
- ListUsers     dsdata-member-constraint-validation-envelopes#task#5 — expected "... Value '251' at 'maxResults' ... less than or equal to 250", got None (4 affected steps)
- CreateGroup   dsdata-member-constraint-validation-envelopes#task#6 — expected "... 'NotAGroupType' at 'groupType' ... enum value set: [Security, Distribution]", got None
- SearchUsers   dsdata-member-constraint-validation-envelopes#task#7 — expected "... '[1BadAttribute]' at 'searchAttributes' ... [length <= 63, length >= 1, pattern [A-Za-z*][A-Za-z-*]*]", got None (2 affected steps)
- SearchGroups  dsdata-member-constraint-validation-envelopes#task#8 — expected "... at 'searchAttributes' ... length less than or equal to 25", got None
- ListGroups    dsdata-member-constraint-validation-envelopes#task#9 — expected "... 'not a realm!' at 'realm' ... pattern: ([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?", got None
Expected status, error type and (where checked) all other body keys matched; the ValidationException `Message` member is absent in every case. Model grounding: services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json (ValidationException: client error, httpError 400, members Message/Reason; constraints as cited per cause). Component: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/service.rs lines 98-110 (`override_refusal` returns None, so the framework default wording is used and `Message` is not rendered).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect fixture dsdata-member-constraint-validation-envelopes, steps task#1, task#4, task#5, task#6, task#7, task#8, task#9 (one per cause) — each sends a ds-data request with a single constraint-violating member (bad DirectoryId pattern, ';' in SAMAccountName, MaxResults 251, GroupType 'NotAGroupType', SearchAttributes element '1BadAttribute', 26-element SearchAttributes, Realm 'not a realm!').
4. Observe each response: HTTP 400, x-amzn-errortype ValidationException, body WITHOUT a `Message` key.
5. Expected: the same 400/ValidationException with `Message` = "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: <the model constraint>".
6. Also re-replay the cluster members dsdata-sdk-unreachable-request-envelopes#task#1, #task#2, #task#5 and dsdata-request-layer-ordering-and-http-surface#task#1, #task#2, which fail the same way.
