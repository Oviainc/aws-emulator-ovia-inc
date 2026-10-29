Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# TagResource's reserved 'aws:' prefix ValidationException carries an emulator-authored message instead of AWS's "System tags cannot be added/updated by requester"

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new behavioral root cause on bcm-pricing-calculator.

Cause 1 of 1: the ValidationException message for a tag key using the AWS-reserved 'aws:' prefix is written by the emulator rather than being AWS's wording for that refusal class.
Failing check: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected

AWS: TagResource takes a required `tags` map (Smithy: TagResource.tags -> Tags, @length 0..200; key ResourceTagKey @length 1..128 @pattern ^[\w\s:+=@/-]+$, value ResourceTagValue @length 0..256 @pattern ^[\w\s:+=@/-]*$) and models ValidationException with @httpError 400 and a required `message` member (Smithy: ValidationException{message required, reason: ValidationExceptionReason, fieldList}). Note that the reserved prefix is NOT a modelled constraint — `aws:reserved` satisfies ResourceTagKey's pattern and length — so this refusal is service behavior, recorded behavior only: real AWS refuses a caller-supplied key in the reserved `aws:` namespace with HTTP 400, error code `ValidationException`, and the message `System tags cannot be added/updated by requester`. That message is entirely FIXED text for this error class: it has no variable parts, it does not echo the offending key, the member name, the prefix, or the ARN. Any key in the reserved namespace earns that same sentence.

Emulator: the status (400) and the error code (ValidationException) are correct, and the refusal is correctly applied before anything is written (the check's follow-up ListTagsForResource shows no tag was applied, and the non-reserved positive control tags fine). Only the message diverges. For tags={"aws:reserved": "x"} the emulator answers the message:
  'tags: the aws: prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: aws:reserved'
where AWS answers:
  'System tags cannot be added/updated by requester'
This is authored in the handler, not accidental: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/tag_resource.rs builds the clause with a format! over `crate::state::RESERVED_TAG_KEY_PREFIX` and a sorted join of the offending keys, so the message is a template over request data where AWS's is a constant. The emulator's text names the input member (`tags:`), the reserved prefix, and echoes every offending key — three variable/extra segments AWS's wording does not contain at all.

Why: the error message is part of the refusal contract. A client that surfaces the service message to a user, matches on it, or asserts AWS's wording for the system-tag refusal (a common case for tagging tooling and policy-compliance harnesses, since the reserved-prefix rule is not expressible from the model) reads text that never appears in real AWS and passes against AWS while failing against the emulator. Because the emulator's text additionally echoes the submitted keys, a caller cannot rely on the message being a stable constant for this class as AWS makes it.

Fix: this operation's reserved-key refusal wording — the ValidationException clause builder in the TagResource handler (the `crate::state::is_reserved_tag_key` branch that calls `crate::scaffolded::Scaffolded::validation(...)`). The status, the error code, the refusal ORDER (reserved key refused before ARN resolution) and the no-write-on-refusal behavior are all already correct and must not change; only the message string must become AWS's fixed sentence. If any sibling tagging operation (UntagResource) renders the same reserved-prefix clause from the same helper, it needs the same wording.

Note: the one open bcm-pricing-calculator issue (20261029T170742Z-bcm-pricing-calculator-announces-awsjson1-1-on-every.md) does NOT cover this. That issue is a wire-corpus finding about (a) the response content-type being announced as application/x-amz-json-1.1 for an awsJson1_0 service and (b) the per-field `$.fieldList[*].message` subject dropping the rejected value on constraint violations. This cause is the TOP-LEVEL ValidationException `message` for a non-modelled, service-level reserved-tag rule, which neither of its two causes nor any of its Issue-Keys names.

Traces: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 16/17 passed, 1 mismatch, 0 errored, 0 skipped, 0 unloadable, bootstrap_signal false. The single failure is on bcm-pricing-calculator (16/17, 94%).

[mismatch] bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected
  AssertionError: expected message 'System tags cannot be added/updated by requester',
                  got 'tags: the aws: prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: aws:reserved'

What matched (so the divergence is the message and nothing else):
  - error code: ValidationException  (expected == actual; the check's `code == "ValidationException"` assertion passed before the message assertion)
  - HTTP status: 400 (ValidationException @httpError 400)
  - effect absent: the assertion on the message fired BEFORE the follow-up checks, but the handler refuses prior to any write (see source citation), and the positive control path is unaffected

Expected vs actual, message only:
  expected (AWS, fixed text, no variable parts): "System tags cannot be added/updated by requester"
  actual   (emulator, template over request data): "tags: the <RESERVED_TAG_KEY_PREFIX> prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: <sorted offending keys joined by ', '>"
    - instantiated in this run as: "tags: the aws: prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: aws:reserved"
    - `aws:reserved` is the key the CHECK submitted (request data, not part of any requirement); `aws:` is the emulator's RESERVED_TAG_KEY_PREFIX constant

Model corroboration (smithy_query on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json):
  TagResource input: arn (Arn, required, @length 20..2048, @pattern ^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$), tags (Tags, required, @length 0..200)
  TagResource errors: ResourceNotFoundException(404), ServiceQuotaExceededException(402), AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400)
  ValidationException: errorKind client, @httpError 400, members { message: String required, reason: ValidationExceptionReason, fieldList: ValidationExceptionFieldList }
  Tags: map, @length 0..200, key ResourceTagKey @length 1..128 @pattern ^[\w\s:+=@/-]+$, value ResourceTagValue @length 0..256 @pattern ^[\w\s:+=@/-]*$
  => `aws:reserved` SATISFIES ResourceTagKey, so the reserved-namespace refusal is service behavior the model does not express; its wording is known only from recorded behavior.

Source of the divergence in the checkout:
  services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/tag_resource.rs lines 195-209 — collects reserved keys via crate::state::is_reserved_tag_key, sorts them, and returns crate::scaffolded::Scaffolded::validation(format!("tags: the {} prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: {}", crate::state::RESERVED_TAG_KEY_PREFIX, reserved.join(", "))). The module header (lines 14-23) documents the refusal class and its ordering as intended; only the rendered text is wrong.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the behavioral suite; or run the single check bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected.
3. Manually: with an sts caller identity for the account id, call bcm-pricing-calculator CreateWorkloadEstimate(name=<a name matching ^[a-zA-Z0-9-]+$>) and keep the returned `id`.
4. Build the resource ARN: arn:aws:bcm-pricing-calculator::<accountId>:workload-estimate/<id>
5. Call TagResource(arn=<that ARN>, tags={"aws:reserved": "x"}).
6. Observe: HTTP 400 with error code ValidationException (correct), but Error.Message is "tags: the aws: prefix is reserved for AWS use and cannot be set by a caller, so these keys are not allowed: aws:reserved" instead of AWS's fixed "System tags cannot be added/updated by requester". Any key in the reserved namespace (e.g. "aws:anything") reproduces it, with that key echoed into the emulator's message — AWS's message is identical regardless of the key.
7. Confirm the rest of the contract is already right: ListTagsForResource(arn) returns {} (the refused tag was not applied), and TagResource(arn, tags={"env":"ok"}) followed by ListTagsForResource(arn) returns {"env":"ok"}.
8. After fixing, re-replay bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected.
