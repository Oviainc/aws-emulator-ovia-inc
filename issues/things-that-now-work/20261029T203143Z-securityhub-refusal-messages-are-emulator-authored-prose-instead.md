Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityhub.batch-enable-standards-invalid-arn-rejected securityhub.create-action-target-duplicate-id-rejected securityhub.delete-action-target-removes-from-list securityhub.enable-security-hub-double-enable-rejected
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub.batch-enable-standards-invalid-arn-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: securityhub.create-action-target-duplicate-id-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: securityhub.delete-action-target-removes-from-list|run#14|AssertionError|assert dup_message == expected, f"expected message {expected!r}, got {dup_message!r}"
Issue-Key: securityhub.enable-security-hub-double-enable-rejected|run#8|AssertionError|assert message2 == expected, f"expected message {expected!r}, got {message2!r}"
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# securityhub refusal messages are emulator-authored prose instead of AWS error text (4 operations)

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchEnableStandards

## Description
Four securityhub refusal paths return the right HTTP status and the right modeled error code, but the Message member is emulator-authored prose rather than the message AWS emits for that error class. In each case the status/code contract is already satisfied, so the defect is isolated to the message the refusal path composes. 4 causes.

Note: the open issue 20261029T180317Z-securityhub-refusal-responses-missing-modeled-client-errors-and.md describes the same DeleteActionTarget message rewrite (its Cause 2) from the WIRE corpus; Cause 3 below is the behavioral-suite grounding for it and should be closed together with it. The other three causes are not covered by any open issue.

Cause 1 of 4: BatchEnableStandards rejects a bogus StandardsArn with a re-worded message.
AWS: Smithy: BatchEnableStandards POST /standards/register, input StandardsSubscriptionRequests (@length 1..25, required), error InvalidInputException (errorKind client, httpError 400, members Message + Code). Recorded behavior: when any entry of the batch names a StandardsArn that is not an available standard, the whole call is rejected with HTTP 400 InvalidInputException and Message of the form 'Invalid StandardsSubscriptionRequest(s): [{"standardsArn":"<arn>"}]'. The fixed text is 'Invalid StandardsSubscriptionRequest(s): ' followed by a JSON array of the rejected request objects, each rendered with the lower-camel member name 'standardsArn'; the only variable part is the ARN list, taken verbatim from the offending StandardsSubscriptionRequests entries of the request. AWS does not name the Region and does not reformat the entry into prose.
Emulator: status 400 and code InvalidInputException are correct, but Message is 'StandardsArn: <arn> is not the ARN of a standard available in Region <region>' — entirely different fixed text, and it interpolates the Region, which AWS's message does not contain.
Why: a caller that submits a mixed batch and parses Message to recover WHICH entries were rejected (the JSON array is the machine-readable part of AWS's message) gets prose it cannot parse, so it cannot retry the batch with only the valid entries; assertions and log matchers written against AWS text do not match.
Fix: the BatchEnableStandards handler's request-validation path — it must collect every invalid StandardsSubscriptionRequest and render the AWS message template with the JSON array of rejected entries.
Traces: securityhub.batch-enable-standards-invalid-arn-rejected.

Cause 2 of 4: CreateActionTarget duplicate-Id conflict message names the generated ARN instead of the requested Id.
AWS: Smithy: CreateActionTarget POST /actionTargets, input Id (NonEmptyString, required, pattern \\S), error ResourceConflictException (errorKind client, httpError 409, members Message + Code). Recorded behavior: creating a custom action target with an Id that already exists is refused with 409 ResourceConflictException and Message of the form 'Action with Id: <Id> already exists' — fixed text 'Action with Id: ' and ' already exists', with the value taken verbatim from the request's Id member.
Emulator: status 409 and code ResourceConflictException are correct, but Message is 'Custom action target <action-target-arn> already exists' — different fixed text, and the variable part is the server-constructed ActionTargetArn rather than the Id the client sent.
Why: the client only knows the Id it supplied; a message keyed on a derived ARN forces it to re-derive the ARN to correlate the conflict, and idempotent-create wrappers that match on AWS's text treat the refusal as an unknown conflict.
Fix: the CreateActionTarget handler's duplicate-Id refusal path — emit the AWS message template interpolating the request's Id.
Traces: securityhub.create-action-target-duplicate-id-rejected.

Cause 3 of 4: DeleteActionTarget not-found message is re-worded.
AWS: Smithy: DeleteActionTarget DELETE /actionTargets/{ActionTargetArn+}, input ActionTargetArn (httpLabel, required), error ResourceNotFoundException (errorKind client, httpError 404, members Message + Code). Recorded behavior: deleting a custom action target that does not exist (e.g. a repeat delete) is refused with 404 ResourceNotFoundException and Message of the form 'Given Arn: <ActionTargetArn> does not exist' — fixed text 'Given Arn: ' and ' does not exist', with the ARN taken verbatim from the request's ActionTargetArn URI label.
Emulator: status 404 and code ResourceNotFoundException are correct, but Message is 'Custom action target <arn> does not exist' — different fixed text around the same ARN value.
Why: callers that distinguish securityhub 404 subclasses by message text, and tests/log matchers built on AWS wording, do not match; this is the behavioral grounding of the already-open wire issue named above.
Fix: the DeleteActionTarget handler's not-found refusal path — emit the AWS 'Given Arn: ... does not exist' template.
Traces: securityhub.delete-action-target-removes-from-list.

Cause 4 of 4: EnableSecurityHub double-enable conflict message is re-worded and leaks account/Region.
AWS: Smithy: EnableSecurityHub POST /accounts, error ResourceConflictException (errorKind client, httpError 409, members Message + Code). Recorded behavior: a second EnableSecurityHub call for an account whose hub is already enabled is refused with 409 ResourceConflictException and the wholly FIXED Message 'Account is already subscribed to Security Hub' — it interpolates nothing from the request or from state.
Emulator: status 409 and code ResourceConflictException are correct, but Message is 'Security Hub CSPM is already enabled for account <account-id> in Region <region>' — different fixed text, and it interpolates an account id and a Region that AWS's fixed message does not contain.
Why: enable-if-not-enabled bootstrap code commonly swallows exactly AWS's fixed string and re-raises anything else, so the emulator's variant turns an expected idempotent no-op into a hard failure; the interpolated account/Region also make the message per-environment, so no stable assertion can be written against it.
Fix: the EnableSecurityHub handler's already-enabled refusal path — emit the fixed AWS message with no interpolation.
Traces: securityhub.enable-security-hub-double-enable-rejected.

## Evidence
Cause 1 — securityhub.batch-enable-standards-invalid-arn-rejected (mismatch): code InvalidInputException as expected; Message value_mismatch — expected 'Invalid StandardsSubscriptionRequest(s): [{"standardsArn":"<bogus-standards-arn>"}]', got 'StandardsArn: <bogus-standards-arn> is not the ARN of a standard available in Region <region>'. (Recorded run used arn:aws:securityhub:us-west-2:000000000000:standards/sim2real-does-not-exist/v/1.0.0 and Region us-west-2; those values fill the template, they are not part of it.)

Cause 2 — securityhub.create-action-target-duplicate-id-rejected (mismatch): code ResourceConflictException as expected; Message value_mismatch — expected 'Action with Id: <requested-Id> already exists', got 'Custom action target <action-target-arn> already exists'. (Recorded run: Id 'sim2real43f63243e00f', arn:aws:securityhub:us-west-2:000000000000:action/custom/sim2real43f63243e00f.)

Cause 3 — securityhub.delete-action-target-removes-from-list (mismatch): code ResourceNotFoundException as expected; Message value_mismatch — expected 'Given Arn: <action-target-arn> does not exist', got 'Custom action target <action-target-arn> does not exist'. (Recorded run: arn:aws:securityhub:us-west-2:000000000000:action/custom/sim2realb662dfc3834e.)

Cause 4 — securityhub.enable-security-hub-double-enable-rejected (mismatch): code ResourceConflictException as expected; Message value_mismatch — expected the fixed string 'Account is already subscribed to Security Hub', got 'Security Hub CSPM is already enabled for account <account-id> in Region <region>'. (Recorded run: account 000000000000, Region us-west-2.)

## Steps to Reproduce
Boot the emulator and replay the simparity-behavioral-aws-v1 checks listed in trace_ids.

Cause 1: EnableSecurityHub(EnableDefaultStandards=false); pick a StandardsArn from DescribeStandards that is not in GetEnabledStandards; call BatchEnableStandards with two entries — a bogus ARN (arn:aws:securityhub:<region>:<account>:standards/does-not-exist/v/1.0.0) and the real unenabled one. Expect 400 InvalidInputException with Message 'Invalid StandardsSubscriptionRequest(s): [{"standardsArn":"<bogus-arn>"}]' and neither standard enabled.

Cause 2: CreateActionTarget(Id=<id>, Name, Description), then CreateActionTarget again with the same Id. Expect 409 ResourceConflictException with Message 'Action with Id: <id> already exists'.

Cause 3: CreateActionTarget, DeleteActionTarget(ActionTargetArn=<arn>), then DeleteActionTarget on the same ARN. Expect 404 ResourceNotFoundException with Message 'Given Arn: <arn> does not exist'.

Cause 4: EnableSecurityHub, then EnableSecurityHub again on the same account/Region. Expect 409 ResourceConflictException with the fixed Message 'Account is already subscribed to Security Hub'.
