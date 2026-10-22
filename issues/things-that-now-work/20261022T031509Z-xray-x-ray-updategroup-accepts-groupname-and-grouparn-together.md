Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.update-group-conflicting-name-rejected
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.update-group-conflicting-name-rejected|_error_code#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# X-Ray UpdateGroup accepts GroupName and GroupARN together instead of refusing the conflicting identity with InvalidRequestException

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGroup

## Description
Cause 1 of 1: UpdateGroup does not reject a request that supplies BOTH group identifiers.
AWS: UpdateGroup identifies its target by exactly one of GroupName or GroupARN — GroupName is an identifier, not a mutable field, and there is no rename operation. Supplying both (here pointing at two different, existing groups) is a conflicting-identity boundary refused with error code InvalidRequestException at HTTP 400 and the fixed message 'Provide either GroupName or GroupARN'; because that message is a fixed AWS string for this rule it is a requirement, not an example. Neither group is mutated by the refused call, and UpdateGroup by a single identifier still succeeds. Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json): UpdateGroupRequest members GroupName (@length min 1 max 32, documented 'The case-sensitive name of the group.') and GroupARN (@length min 1 max 400) are both optional, and the operation models only InvalidRequestException (errorKind client, httpError 400) and ThrottledException (429) — there is no modeled outcome in which a conflicting identity is serviced. The exclusivity rule and its message text are recorded behavior.
Emulator: the call is serviced with HTTP 200. UpdateGroup(GroupARN=<group B's ARN>, GroupName=<group A's name>, FilterExpression='service("changed")') returned 200 with Group{GroupName: 'sim2real-ga-22927483d9ce', GroupARN: 'arn:aws:xray:us-west-2:000000000000:group/sim2real-ga-22927483d9ce/7d96220e110c', FilterExpression: 'service("changed")'} — the handler silently preferred the GroupName identifier (the echoed name and ARN are group A's, while the request's GroupARN named group B) and applied the mutation to that group.
Why: the request is ambiguous, and the emulator resolves the ambiguity by picking one side without telling the caller. A client that builds an UpdateGroup request from a cached object carrying both fields — a very common SDK pattern, since both are returned on every Group — gets a silent write to whichever group the emulator prefers instead of a 400 it can correct. Against the emulator that write lands on the wrong group (group A is mutated while the caller addressed group B by ARN), so state diverges permanently and no error is ever surfaced; the same code against AWS is refused outright.
Fix: input validation in the UpdateGroup handler of the xray crate (services/service-com-amazonaws-xray-awsxray, src/ops/update_group.rs) — refuse with InvalidRequestException/400 when both GroupName and GroupARN are present, before any state mutation, so the 'neither group touched' property holds.
Traces: xray.update-group-conflicting-name-rejected.

## Evidence
xray.update-group-conflicting-name-rejected [mismatch]: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Group': {'GroupName': 'sim2real-ga-22927483d9ce', 'GroupARN': 'arn:aws:xray:us-west-2:000000000000:group/sim2real-ga-22927483d9ce/7d96220e110c', 'FilterExpression': 'service("changed")'}}`.
Expected: HTTP 400, Error.Code 'InvalidRequestException', Error.Message 'Provide either GroupName or GroupARN'; group A keeps FilterExpression 'service("a")' and group B keeps 'service("b")'.
Actual: HTTP 200; the update was applied to the group named by GroupName (group A) while the request's GroupARN addressed group B.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it).
2. xray.create_group(GroupName='ga', FilterExpression='service("a")') and xray.create_group(GroupName='gb', FilterExpression='service("b")'); keep both returned GroupARNs.
3. Call xray.update_group(GroupARN=<gb ARN>, GroupName='ga', FilterExpression='service("changed")') — observe HTTP 200 with a Group echoing ga's name/ARN, instead of HTTP 400 InvalidRequestException 'Provide either GroupName or GroupARN'.
4. Call xray.get_group(GroupARN=<ga ARN>) — its FilterExpression is now 'service("changed")' rather than the untouched 'service("a")'.
5. Re-replay xray.update-group-conflicting-name-rejected to confirm the fix (its positive control also asserts single-identifier UpdateGroup still works).
