Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.untag-resource-empty-tag-keys-rejected
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup.untag-resource-empty-tag-keys-rejected|run#10|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r} ({err!r})"
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# REOPENS UntagResource: an empty TagKeys list is answered 501 NotImplemented instead of AWS's front-door refusal

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
REOPENS 20261024T040418Z-ssm-quicksetup-untagresource-stub.md: its fix left the call refused. 1 cause.

Cause 1 of 1: UntagResource with an EMPTY TagKeys list never reaches the implemented handler — it falls through to the scaffold's catch-all and is answered 501 with code 'NotImplemented'.
AWS: recorded behavior only — DELETE /tags/{ResourceArn} carrying no tagKeys query parameter (what an SDK sends for TagKeys=[]) is refused by the ssm-quicksetup REST front door with the fixed error code UnauthorizedException and the fixed, request-independent body message 'Unauthorized'; nothing in it is interpolated from the request. Hypothesis (not stated by the model or asserted by this check): the status is 401, matching the other recorded front-door refusals of this service already filed in 20261022T034519Z-ssm-quicksetup-error-paths-wrong-rejection-class-unvalidated.md causes 1-4. Smithy models no UnauthorizedException at all for this service, and UntagResourceInput.TagKeys is @required with no @length, so the refusal comes from the front door, not from the operation's own validation.
Emulator: code 'NotImplemented' with message 'operation DELETE /tags/arn%3Aaws%3Assm-quicksetup%3A<region>%3A<account>%3Aconfiguration-manager%2F<uuid> is not implemented in this emulator' — the generic unimplemented-route answer, naming the raw percent-encoded path. The crate's own handler documents the gap it leaves: src/ops/untag_resource.rs:135-149 states "An EMPTY list needs no refusal — the route matches only a request carrying the @required tagKeys parameter", i.e. the handler relies on routing to have rejected the empty case, and routing instead answers the scaffold 501.
Why: a client that computes the key set to remove and happens to compute an empty one is told the operation does not exist. 'NotImplemented' is not a modelled ssm-quicksetup error, so SDK error handling, retry classification and negative tests all break on it; it also masks the whole operation's fidelity for that input class, and callers reading the message learn about the emulator rather than about their request.
Fix: the request routing/front-door stage for the ssm-quicksetup DELETE /tags/{ResourceArn} route — a request missing the @required tagKeys query parameter must be refused with the service's front-door UnauthorizedException ('Unauthorized') rather than falling through to the scaffold's NotImplemented answer. Once that holds, the handler's documented assumption at src/ops/untag_resource.rs:135-149 is true again.
Traces: ssm-quicksetup.untag-resource-empty-tag-keys-rejected (the same check's positive control — UntagResource with a real key, then ListTagsForResource reporting the key gone — never runs, because the refusal assertion fails first).

## Evidence
ssm-quicksetup.untag-resource-empty-tag-keys-rejected: after creating a manager tagged {'keep': 'me'} and waiting past INITIALIZING/DEPLOYING, UntagResource(ResourceArn=<manager arn>, TagKeys=[]) is expected to raise code 'UnauthorizedException' with Message 'Unauthorized'. Actual botocore Error dict: {'Message': 'operation DELETE /tags/arn%3Aaws%3Assm-quicksetup%3Aus-west-2%3A000000000000%3Aconfiguration-manager%2F78462bd7-4263-40df-829d-000000000001 is not implemented in this emulator', 'Code': 'NotImplemented'} (assertion: "expected UnauthorizedException, got 'NotImplemented'").
The operation is otherwise implemented — src/ops/untag_resource.rs has a full handle() body (tag_keys_rejection, store lookup, reserved-prefix rule, tag removal) — so the 501 comes from the route not matching when tagKeys is absent, which is exactly the input class the fixed stub issue 20261024T040418Z-ssm-quicksetup-untagresource-stub.md left refused.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. POST /configurationManager with a valid AWSQuickSetupType-SSMHostMgmt definition and Tags={'keep': 'me'}; wait until the Deployment status summary leaves INITIALIZING/DEPLOYING.
3. Call UntagResource with ResourceArn=<the manager ARN> and TagKeys=[] (an SDK sends DELETE /tags/{ResourceArn} with no tagKeys query parameter).
4. Observe code 'NotImplemented' and the 'is not implemented in this emulator' message instead of the service's front-door UnauthorizedException / 'Unauthorized'.
5. For contrast, call UntagResource with TagKeys=['keep'] on the same manager: it is served normally, confirming the operation itself is implemented and only the empty-list request misses the route.
