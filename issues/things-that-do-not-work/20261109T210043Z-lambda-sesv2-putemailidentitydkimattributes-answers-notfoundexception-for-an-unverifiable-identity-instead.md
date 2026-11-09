Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative|run#12|AssertionError|assert invalid_body["errorCode"] == "BadRequestException", (
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# PutEmailIdentityDkimAttributes answers NotFoundException for an unverifiable identity instead of BadRequestException

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEmailIdentityDkimAttributes

## Description
Cause 1 of 1: a DKIM write against an email identity that cannot exist is classified as a missing resource rather than a bad request.

AWS: recorded behavior only for the message; the SES v2 Smithy model declares both BadRequestException (400) and NotFoundException (404) for this operation but not their text. AWS refuses PutEmailIdentityDkimAttributes for an identity name that is not a valid, usable email identity with BadRequestException (HTTP 400) — the request itself is malformed/unacceptable, not a reference to a resource that merely has not been created yet.

Emulator: answers NotFoundException (HTTP 404) with `Email identity <identity> does not exist.` — the fixed text being `Email identity ` and ` does not exist.`, the variable part the identity from the request. It is the right template for a genuinely absent but well-formed identity, and the wrong classification here.

Why: a client branches on these two codes differently: a 404 NotFoundException invites "create the identity and retry", while a 400 BadRequestException says the request will never succeed as written. Returning 404 makes a client retry forever against an identity AWS would never accept, and a negative test that asserts the 400 class fails.

Fix: PutEmailIdentityDkimAttributes' validation/error classification in the SES v2 handler — validate the identity before the existence lookup and raise BadRequestException (400) for an unacceptable identity, reserving NotFoundException for a well-formed identity that is absent.

Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative.

## Evidence
Check lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative:
AssertionError: expected BadRequestException against the fake identity, got {'ok': False, 'errorCode': 'NotFoundException', 'message': "An error occurred (NotFoundException) when calling the PutEmailIdentityDkimAttributes operation: Email identity <identity> does not exist."}
(the identity string is a per-run generated name; the requirement is the 400/BadRequestException classification.)

## Steps to Reproduce
1. Deploy the stack's function and role.
2. From the handler call sesv2 PutEmailIdentityDkimAttributes with EmailIdentity set to an identity that is not a usable email identity.
3. Observe NotFoundException (404) where AWS answers BadRequestException (400).
