Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-malformed-session-policy-rejected sts.assume-role-authorization-precedes-policy-parsing
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-malformed-session-policy-rejected|run#15|AssertionError|assert message == (
Issue-Key: sts.assume-role-authorization-precedes-policy-parsing|run#20|AssertionError|assert parse_message == (
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# MalformedPolicyDocument for a syntactically invalid AssumeRole session Policy carries a generic sentence instead of a message describing the specific parse error (1 cause)

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
This issue carries ONE net-new root cause in the STS service (service token `sts`). A fixer must close one cause before submitting.

Cause 1 of 1: the MalformedPolicyDocument refusal for an unparseable inline session Policy does not describe the specific parse error, so the message carries no information about WHAT is wrong with the document.

AWS: Smithy: AssumeRole error MalformedPolicyDocumentException, errorKind client, awsQueryError code `MalformedPolicyDocument`, HTTP 400; the shape is documented as "The request was rejected because the policy document was malformed. The error message describes the specific error." The message is therefore built from the request, and the rule that produces it — report the first offending character, its character code, what was expected there, and its line/column position in the submitted document — is the requirement. Two independent recordings in this suite exhibit exactly that rule with the request-dependent parts varying: for Policy='{this is not valid json' the recorded message is "Unexpected IOException: Unexpected character ('t' (code 116)): was expecting double-quote to start field name\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 2]", and for Policy='{not valid json' it is the same text with ('n' (code 110)) — i.e. the character and code track the actual first offending byte at column 2. The recorded wording itself is an example; the rule (name the offending character/code, the expectation, and the line and column) is the requirement.

Emulator: the error CODE and status are correct — both checks passed their `code == "MalformedPolicyDocument"` assertion — but the message is a fixed sentence that mentions no position and no offending character: "The inline session policy must be a JSON policy document".

Why: MalformedPolicyDocument is the only signal a caller gets about a policy document it generated programmatically. AWS's message pinpoints the byte that failed to parse, so a caller (or a human reading a deploy log) can locate the defect; the emulator's message only restates the member's type, so a developer who hits this against the emulator learns nothing about which part of their document is wrong and cannot reproduce the diagnosis they will get from AWS. It also makes the two distinct malformed documents exercised here indistinguishable.

Fix: the AssumeRole handler's session-policy parsing/validation step — propagate the JSON parser's diagnosis (offending character and its code, the expected token, and the line/column of the failure in the submitted Policy) into the MalformedPolicyDocumentException message rather than substituting a constant sentence. Status 400 and awsQuery code MalformedPolicyDocument are already correct and must not change.

Note on scope: this is adjacent to but distinct from the message-fidelity causes already filed as cause 4 and cause 5 of 20261024T065715Z-sts-sessions-carry-no-policy-scoping-packedpolicysize-oversized.md and 20261024T154246Z-sts-assumerolewithwebidentity-is-unroutable-unknownservice-trust-policy-conditions.md (SignatureDoesNotMatch text and the PackedPolicyTooLarge byte figures); neither of those open issues covers the malformed-session-policy parse diagnostic, and neither of these two checks is marked as owned by them.

Traces: sts.assume-role-malformed-session-policy-rejected, sts.assume-role-authorization-precedes-policy-parsing.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator; 2 failing checks, one cause. Both checks establish a positive control first (they create a role and poll until it is genuinely assumable, and retry past transient AccessDenied/InvalidClientTokenId), and both got the right error code — the failure is on the message assertion that follows it.

[mismatch] sts.assume-role-malformed-session-policy-rejected
  Policy='{this is not valid json'
  assert code == "MalformedPolicyDocument"  -> PASSED
  File "check_sts_assume_role_malformed_session_policy_rejected.py", line 123, in run: assert message == (...)
  AssertionError: The inline session policy must be a JSON policy document
  Expected (recorded): "Unexpected IOException: Unexpected character ('t' (code 116)): was expecting double-quote to start field name\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 2]"

[mismatch] sts.assume-role-authorization-precedes-policy-parsing
  Policy='{not valid json' against a role the caller CAN assume
  assert parse_code == "MalformedPolicyDocument"  -> PASSED
  File "check_sts_assume_role_authorization_precedes_policy_parsing.py", line 134, in run: assert parse_message == (...)
  AssertionError: The inline session policy must be a JSON policy document
  Expected (recorded): the same text with "Unexpected character ('n' (code 110))" — the offending character and its code follow the request, which is what shows the message is request-derived rather than a constant.

In both cases the emulator answers a single fixed sentence naming only the member and its type. The second check's remaining assertion (that an unauthorized role ARN with the same malformed policy is refused AccessDenied, i.e. authorization precedes policy parsing) was never reached, so the ordering contract is untested here and may surface once the message is fixed.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite).
2. Re-replay sts.assume-role-malformed-session-policy-rejected and sts.assume-role-authorization-precedes-policy-parsing.
3. Minimal manual repro: iam.create_role(RoleName='probe-role', AssumeRolePolicyDocument=<trust arn:aws:iam::<account>:root for sts:AssumeRole>), then poll sts.assume_role until it succeeds.
4. sts.assume_role(RoleArn=<probe-role arn>, RoleSessionName='probe', Policy='{this is not valid json') and read Error.Code and Error.Message.
5. Observe Error.Code == 'MalformedPolicyDocument' with HTTP 400 (correct) but Error.Message == 'The inline session policy must be a JSON policy document'. AWS reports the specific parse failure, naming the offending character and its code, the expected token, and line 1, column 2.
6. Repeat with Policy='{not valid json' and observe the emulator returns the identical message, where AWS's message changes with the offending character ('n' (code 110) instead of 't' (code 116)).
