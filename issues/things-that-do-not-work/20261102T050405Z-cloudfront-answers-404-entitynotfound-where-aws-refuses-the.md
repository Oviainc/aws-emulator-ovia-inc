Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-anycast-ip-list-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-anycast-ip-list-lifecycle cloudfront-resource-policy-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|status
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->4xx|status
Issue-Commit: 7216446cc3c31175158854d642481a94c8b1c026

# CloudFront answers 404 EntityNotFound where AWS refuses the argument first with 400 InvalidArgument

> _Note: this issue was identified against the code base at commit `7216446cc3c31175158854d642481a94c8b1c026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAnycastIpList

## Description
Two causes, one shared fault: on these two refusal paths the emulator resolves the request's identifier against its store FIRST and reports the resource absent, where AWS refuses the ARGUMENT first. Both answers are refusals, so nothing is over-permissively accepted, but the emulator returns the wrong HTTP status (404 instead of 400), the wrong Error.Code (EntityNotFound instead of InvalidArgument) and a different sentence — exactly the three fields a client's error handling branches on (a 404 branch means "gone, recreate/stop polling"; a 400 InvalidArgument branch means "the request itself is wrong, do not retry"). Related open issues cover neighbouring refusal-wording gaps on these same surfaces but carry neither cause: 20261029T212223Z-cloudfront-refusal-codes-and-messages-diverge-from-aws.md (causes 9/10 there are Put/GetResourcePolicy, where the emulator does the OPPOSITE — it answers 400 InvalidArgument "resource type isn't valid" where AWS answers 404 EntityNotFound) and 20261031T221122Z-cloudfront-refuses-well-formed-but-absent-distribution-tenant.md (the well-formed-but-ABSENT side of the Anycast/tenant id gate, i.e. the opposite side of cause 1). Neither owns these two steps.

Cause 1 of 2: UpdateAnycastIpList does not apply the Anycast-IP-list id format gate before resolving the id.
AWS: recorded behavior only for the ordering (the Smithy model declares both errors but not their precedence) — the recording shows UpdateAnycastIpList refusing an Id that is not in the Anycast-IP-list id namespace with HTTP 400, Error.Type Sender, Error.Code `InvalidArgument` and the message `Id format is invalid`, i.e. the same pre-lookup identifier-format gate GetAnycastIpList applies, run BEFORE any store lookup and before the If-Match precondition. The message is entirely fixed text: it names no member value and echoes no part of the request. Smithy: UpdateAnycastIpList errors = AccessDenied 403, EntityNotFound 404, InvalidArgument 400, InvalidIfMatchVersion 400, PreconditionFailed 412, UnsupportedOperation 400; Id is a required httpLabel on PUT /2020-05-31/anycast-ip-list/{Id}. AWS keeps the two conditions distinct: a malformed id is 400 InvalidArgument `Id format is invalid`, while an id of the real shape that simply does not exist is 404 EntityNotFound `The specified Anycast IP list does not exist`.
Emulator: the handler goes straight to `store.anycast_ip_list(&input.id)` and, on a miss, returns EntityNotFound via `crate::state::anycast_ip_list_not_found()` — HTTP 404, Error.Code `EntityNotFound`, message `The specified Anycast IP list does not exist` — whatever shape the id takes. The operation's module doc even asserts this is intended ("An id no stored list carries is EntityNotFound WHATEVER shape it takes"), so the format gate is unreachable here and InvalidArgument can never be produced for a malformed Id.
Why: a client cannot tell a mistyped/garbage id from a list that was deleted. Code that treats 404 EntityNotFound as "already gone" silently succeeds on a request that was never valid, and the 400 InvalidArgument branch that should stop a bad id from being retried never fires.
Fix: UpdateAnycastIpList's validation — run the Anycast-IP-list id-namespace predicate (`crate::state::is_anycast_ip_list_id`, with the refusal built by `crate::state::invalid_id_format("Id")`) before the store lookup and before the If-Match check, so an id outside the namespace is 400 InvalidArgument `Id format is invalid` and only an in-namespace unknown id falls through to 404 EntityNotFound. The module-level comment claiming the reverse ordering is part of the defect.
Traces: cloudfront-anycast-ip-list-lifecycle#task#4.

Cause 2 of 2: DeleteResourcePolicy reports an ARN of an unsupported resource type as absent instead of refusing the resource type.
AWS: recorded behavior only for the ordering — DeleteResourcePolicy answers HTTP 400, Error.Type Sender, Error.Code `InvalidArgument` with the message `The specified resource type isn't valid. Specify a VPC origin and try again.` for a well-formed ARN whose resource type is not one that takes a CloudFront resource policy, i.e. the resource-TYPE check precedes any existence lookup. That whole sentence is fixed text; it takes no value from the request and names the type the operation demands (a VPC origin). Smithy: DeleteResourcePolicy input = ResourceArn (required); errors = AccessDenied 403, EntityNotFound 404, IllegalDelete 400, InvalidArgument 400, PreconditionFailed 412, UnsupportedOperation 400.
Emulator: `crate::state::resource_policy_arn_refusal` classifies such an ARN as `ResourceArnRefusal::AbsentResource` (the store holds no resource for it) rather than `WrongType`, so the handler returns EntityNotFound via `crate::state::resource_policy_resource_absent()` — HTTP 404, Error.Code `EntityNotFound`, message `The specified resource doesn't exist. Update your request and try again.` The `WrongType` arm (which already carries AWS's sentence, `crate::state::INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE`) is reached only for an ARN of a resource the store happens to hold, so for any unsupported type the caller has not created it is dead code.
Why: a caller that passed an ARN of the wrong kind of resource is told the resource does not exist, so it retries after "creating" it or treats the policy as already removed, instead of being told the parameter is of the wrong resource type. The status/code pair also mis-routes generic SDK retry and error-classification logic (4xx-not-found vs invalid-parameter).
Fix: the resource-policy ARN classifier and its precedence in DeleteResourcePolicy's validation — decide the resource TYPE from the parsed ARN before consulting the store, so a non-VPC-origin ARN is 400 InvalidArgument with the fixed sentence above, and 404 EntityNotFound `The specified resource doesn't exist. Update your request and try again.` is reserved for an ARN of the supported type that names nothing.
Traces: cloudfront-resource-policy-not-found-errors#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (176 fixtures, 1143 steps) — these are the only two divergence clusters not already owned by an open issue or blocked by an unimplemented operation.

Cause 1 — fixture cloudfront-anycast-ip-list-lifecycle, step task#4, operation UpdateAnycastIpList, verdict mismatch:
  expected status 400, actual 400->404 (got 404)
  $.ErrorResponse.Error.Code: expected 'InvalidArgument', got 'EntityNotFound'
  $.ErrorResponse.Error.Message: expected 'Id format is invalid', got 'The specified Anycast IP list does not exist'
  Emulator code path: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_anycast_ip_list.rs lines 174-185 (store lookup, then EntityNotFound via crate::state::anycast_ip_list_not_found()); the id-format predicate crate::state::is_anycast_ip_list_id is never called here, and the module doc at lines 8-11 states the lookup-first ordering as intended.

Cause 2 — fixture cloudfront-resource-policy-not-found-errors, step task#3, operation DeleteResourcePolicy, verdict mismatch:
  expected status 400, actual 400->404 (got 404)
  $.ErrorResponse.Error.Code: expected 'InvalidArgument', got 'EntityNotFound'
  $.ErrorResponse.Error.Message: expected "The specified resource type isn't valid. Specify a VPC origin and try again.", got "The specified resource doesn't exist. Update your request and try again."
  Emulator code path: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_resource_policy.rs lines 190-212 — crate::state::resource_policy_arn_refusal returns AbsentResource, so the WrongType arm (crate::state::invalid_resource_policy_resource_type()) is not taken.

Both expected bodies also carry a RequestId, which is a per-run value and is not asserted; the requirement is the status, the Error.Code and the fixed message text of each class.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: call UpdateAnycastIpList (PUT /2020-05-31/anycast-ip-list/{Id}) with an Id that is not in the Anycast-IP-list id namespace and any If-Match value. The emulator answers 404 EntityNotFound / 'The specified Anycast IP list does not exist'; AWS answers 400 InvalidArgument / 'Id format is invalid'. Re-replay fixture cloudfront-anycast-ip-list-lifecycle, step task#4.
3. Cause 2: call DeleteResourcePolicy with a well-formed ResourceArn whose resource type is not a VPC origin. The emulator answers 404 EntityNotFound / "The specified resource doesn't exist. Update your request and try again."; AWS answers 400 InvalidArgument / "The specified resource type isn't valid. Specify a VPC origin and try again." Re-replay fixture cloudfront-resource-policy-not-found-errors, step task#3.
4. Both causes must be closed: re-replay BOTH fixture steps above (cloudfront-anycast-ip-list-lifecycle#task#4 and cloudfront-resource-policy-not-found-errors#task#3) and confirm each returns the expected status, Error.Code and message.
