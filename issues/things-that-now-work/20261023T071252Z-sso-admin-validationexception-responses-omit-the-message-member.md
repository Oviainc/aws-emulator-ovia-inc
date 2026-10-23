Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2 sso-admin-instance-describe-update-and-quota-r2 sso-admin-tagging-error-paths-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|DescribeInstance|4xx->4xx|$.Message
Issue-Key: sso-admin|DeleteInstance|4xx->4xx|$.Message
Issue-Key: sso-admin|ListTagsForResource|4xx->4xx|$.Message
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# sso-admin ValidationException responses omit the Message member on malformed ARN inputs

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstance

## Description
When an sso-admin request carries an ARN that violates its modeled @pattern, the emulator correctly answers HTTP 400 but serializes the ValidationException with no Message member, so the client is told only 'invalid' and never which input failed which constraint. THREE distinct causes (three operations whose validation paths each drop the message) are bundled here; all three must be closed.

The message here IS a requirement, not an example: it is built from the request. AWS composes it from the offending member name and the constraint it violated — "Value of input '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — and the pattern quoted in every recording is exactly the @pattern the Smithy model puts on that member, with multiple failing members joined by '; '. Reproducing that rule is the fix; the literal recorded strings below are just the rule applied to these inputs.

Cause 1 of 3 — DescribeInstance with a malformed InstanceArn returns 400 with no Message.
AWS: Smithy DescribeInstance.InstanceArn is @required with pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$; errors include ValidationException (@httpError 400) whose shape declares a Message member (and an optional Reason). Recorded response: 400 with Message "Value of input 'instanceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}".
Emulator: 400 with $.Message absent.
Why: the status and error code are right, so a client retries nothing — but a developer or an SDK that surfaces err.message gets an empty string, and tooling that parses which member was rejected cannot tell a bad instance ARN from any other validation failure.
Fix: the shared input-validation layer / ValidationException serializer for the ssoadmin crate — populate Message from the failing member and its pattern.
Traces: sso-admin-describe-list-edge-cases-r2#task#3.

Cause 2 of 3 — DeleteInstance with a malformed InstanceArn returns 400 with no Message.
AWS: Smithy DeleteInstance.InstanceArn is @required with the same instance-ARN pattern; errors include ValidationException (400). Recorded response: 400 with the same member/pattern message for 'instanceArn'.
Emulator: 400 with $.Message absent.
Why: as Cause 1, on the delete path — teardown code that logs or matches the validation message to distinguish 'bad ARN' from 'instance busy' has nothing to match.
Fix: same validation/serializer path, exercised from the DeleteInstance handler.
Traces: sso-admin-instance-describe-update-and-quota-r2#task#8.

Cause 3 of 3 — ListTagsForResource with a malformed ResourceArn (and InstanceArn) returns 400 with no Message, and so cannot report the multiple failing members AWS reports.
AWS: Smithy ListTagsForResource.ResourceArn is @required with pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::((:instance/(sso)?ins-[a-zA-Z0-9-.]{16})|(:permissionSet/...)|(\\d{12}:application/...)|(\\d{12}:trustedTokenIssuer/...))$ and InstanceArn carries the instance-ARN pattern; errors include ValidationException (400). Recorded response: 400 with both failures joined — "Value of input 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <taggable-resource-arn pattern>; Value of input 'instanceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <instance-arn pattern>".
Emulator: 400 with $.Message absent, so neither member is named and the multi-failure join is absent too.
Why: tagging clients validate ARNs by calling and reading the message; with no Message they cannot tell which of the two ARN inputs was wrong, which is the whole diagnostic value of this error.
Fix: same validation/serializer path; it must accumulate all failing members for one request and join them in input order.
Traces: sso-admin-tagging-error-paths-r2#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up: true; state_reset_failures: 0). In all three clusters the HTTP status (400) and the error code matched — the only divergence is the missing $.Message.

1) sso-admin-describe-list-edge-cases-r2#task#3 DescribeInstance — expected 400, got 400; $.Message: missing (expected "Value of input 'instanceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}", got None).

2) sso-admin-instance-describe-update-and-quota-r2#task#8 DeleteInstance — expected 400, got 400; $.Message: missing (expected the same instanceArn member/pattern message, got None).

3) sso-admin-tagging-error-paths-r2#task#5 ListTagsForResource — expected 400, got 400; $.Message: missing (expected "Value of input 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]{1,5}){0,3}:sso::((:instance/(sso)?ins-[a-zA-Z0-9-.]{16})|(:permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16})|(\\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16})|(\\d{12}:trustedTokenIssuer/(sso)?ins-[a-zA-Z0-9-.]{16}/tti-[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})); Value of input 'instanceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}", got None).

Model consulted: Smithy com.amazonaws.ssoadmin — shape ValidationException (@httpError 400, members Message: ValidationExceptionMessage, Reason); DescribeInstance.InstanceArn / DeleteInstance.InstanceArn @required + instance-ARN @pattern; ListTagsForResource.ResourceArn @required + TaggableResourceArn @pattern and optional InstanceArn @pattern. Each recorded message quotes exactly the member's modeled pattern, which is why the message is treated as a rule rather than recorded text.

## Steps to Reproduce
1. git clone the repo at mainline and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Fixture sso-admin-describe-list-edge-cases-r2, step task#3: call DescribeInstance with an InstanceArn that does not match the modeled instance-ARN pattern — observe HTTP 400 with a body carrying no Message member.
4. Fixture sso-admin-instance-describe-update-and-quota-r2, step task#8: call DeleteInstance with a malformed InstanceArn — observe HTTP 400 and no Message.
5. Fixture sso-admin-tagging-error-paths-r2, step task#5: call ListTagsForResource with a malformed ResourceArn and a malformed InstanceArn — observe HTTP 400 and no Message, where AWS returns one message naming both members and their patterns joined by '; '.
