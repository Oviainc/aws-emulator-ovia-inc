Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.list-tags-for-resource-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.list-tags-for-resource-malformed-arn-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got code={code!r}"
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# greengrassv2 ListTagsForResource refuses a malformed resourceArn with ValidationException instead of BadRequestException / "Invalid input resource arn: bad syntax"

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
1 net-new root cause in greengrassv2.

Cause 1 of 1: ListTagsForResource rejects a resourceArn that is not an ARN at all with the wrong error code and the wrong message.
AWS: Smithy (services/service-com-amazonaws-greengrassv2-greengrassv2/smithy-model): ListTagsForResource GET /tags/{resourceArn}, `resourceArn` is GenericV2ARN, @required, @httpLabel, pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices): ; modeled errors ValidationException (HTTP 400), ResourceNotFoundException (404), InternalServerException (500). Recorded behavior only for the code and the message: a value that matches no ARN syntax at all (e.g. a bare token with no `arn:` prefix) is refused with HTTP 400, error code `BadRequestException` — a code that is NOT in this operation's modeled error list but is what the service answers — and message `Invalid input resource arn: bad syntax`. That message is entirely FIXED text: it has no variable part and does NOT echo the offending value. It is the syntax-level sibling of the vendor-level sentence `Invalid input resource arn: expected vendor greengrass` that TagResource answers for a well-formed ARN of another service.
Emulator: the request is refused by the pre-handler @pattern constraint check on `resourceArn`, so the response carries error code `ValidationException` (the generic modeled-constraint refusal) rather than `BadRequestException`; the replay stops at the code assertion, so the message assertion was never reached, but the emitted code is `ValidationException` where AWS emits `BadRequestException`.
Why: a client branching on the error code for tag-API input errors (BadRequestException for a syntactically bad ARN, ResourceNotFoundException for a well-formed but absent greengrass resource) never sees the AWS branch and treats a permanent input error as a generic validation failure; message-matching clients and operators also never see AWS's `Invalid input resource arn: bad syntax` sentence.
Fix: ListTagsForResource's resourceArn validation — the ARN-syntax check must raise the service's own BadRequestException with that fixed sentence instead of letting the modeled `GenericV2ARN` pattern constraint reject the request generically, mirroring the vendor check on the same ARN member.
Related but not covering this: open issue 20261103T065241Z-greengrassv2-refusal-responses-diverge-from-aws-10-net.md files the same defect for TagResource (its Cause 10, vendor-mismatch sentence) and for the component-ARN validators; it does not carry ListTagsForResource or the `bad syntax` sentence.
Traces: greengrassv2.list-tags-for-resource-malformed-arn-rejected.

## Evidence
Check greengrassv2.list-tags-for-resource-malformed-arn-rejected (mismatch):

  File "check_greengrassv2_list_tags_for_resource_malformed_arn_rejected.py", line 28, in run
    assert code == "BadRequestException", f"expected BadRequestException, got code={code!r}"
  AssertionError: expected BadRequestException, got code='ValidationException'

Request: list_tags_for_resource(resourceArn="clearly-not-an-arn") after creating and tagging a real component version (the positive control).

Expected (AWS): HTTP 400, Error.Code `BadRequestException`, Error.Message `Invalid input resource arn: bad syntax` (entirely fixed text — no value from the request appears in it).
Actual (emulator): HTTP 400 with Error.Code `ValidationException` from the generic `resourceArn` @pattern constraint refusal; the AWS code/message pair is never produced.

(The component name/ARN used by the check is a per-run generated value and is not part of the requirement.)

## Steps to Reproduce
Point a greengrassv2 client at the emulator.
1. created = create_component_version(inlineRecipe=<minimal valid JSON recipe>) ; arn = created["arn"]
2. tag_resource(resourceArn=arn, tags={"env": "sim2real-behavioral"})
3. list_tags_for_resource(resourceArn="clearly-not-an-arn")
   -> expect HTTP 400 with Error.Code `BadRequestException` and Error.Message `Invalid input resource arn: bad syntax`; observe Error.Code `ValidationException` with the generic pattern-constraint message.
4. list_tags_for_resource(resourceArn=arn) still returns {"env": "sim2real-behavioral"} (positive control).
