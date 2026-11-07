Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant|run#31|AssertionError|assert message == (
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# CreateAlert refuses an un-assumable access profile with the wrong ValidationException message

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlert

## Description
One net-new root cause in cloudwatchomni.

Cause 1 of 1: CreateAlert correctly REFUSES an alert that names an access profile the alert is not authorized to assume (ValidationException, HTTP 400), but it words the refusal with the generic AssumeStatus=DENIED sentence instead of the alert-specific trust-grant message AWS returns, so the error message diverges.
Failing check: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant

AWS: Smithy (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json) models CreateAlert with required spaceId/profileId/name/rule and the error ValidationException (httpError 400, "A parameter is specified incorrectly"); CreateAlert.profileId is documented as "The ID of the access profile the alert uses to evaluate its query and execute notifications. The caller supplies it: there is no managed alert profile, and the service does not pick one on the caller's behalf." AccessGrantPrincipalType enumerates ALERT ("An alert.") as a distinct principal type from ACCESS_PROFILE ("An access profile."), i.e. a grant whose principal is the profile is NOT the grant that lets an alert assume that profile. The model does not fix the message string, so the message itself is recorded behavior only: AWS rejects CreateAlert for a profile that carries no ALERT-principal trust grant with ValidationException and the message

  "The alert cannot assume access profile <profileId>. Grant the profile a trust grant with principalType=ALERT (customAction cloudwatch:AssumeAccessProfile) before creating the alert."

Fixed text of that message, required verbatim: "The alert cannot assume access profile " ... ". Grant the profile a trust grant with principalType=ALERT (customAction cloudwatch:AssumeAccessProfile) before creating the alert." The ONLY variable part is <profileId>, taken from the request's profileId member, interpolated unquoted. No space id appears in the message. (Do not hard-code the profile id from any one run — it is a per-run uuid filling the template.)

Emulator: the request is refused with the right code (ValidationException) but the message is the generic profile-assume sentence with both ids single-quoted and the space id appended: "The calling principal is not authorized to assume the access profile '<profileId>' in space '<spaceId>'." — i.e. the AssumeStatus DENIED enum doc text, reused for the alert-creation refusal. The replayed check asserted the code successfully and then failed on the message: AssertionError: unexpected message "The calling principal is not authorized to assume the access profile '82e0753c-...' in space '6dd5fd30-...'."

Why: a client cannot act on the refusal. AWS's message names the remedy (create an ALERT-principal trust grant carrying customAction cloudwatch:AssumeAccessProfile on that profile), which is exactly the step the caller is missing; the emulator's message instead reads as a caller-identity/authorization problem with the calling principal, pointing an operator at IAM credentials or at the grant they already created with principalType=ACCESS_PROFILE. Anything matching on the message text (runbooks, log scrapers, tests that assert the remediation wording) diverges, and since the two error classes are worded identically the emulator gives no way to tell "your principal may not assume this profile" from "this profile has no ALERT trust grant".

Fix: CreateAlert's validation/refusal path in the operation handler — the un-assumable-profile branch that builds the ValidationException message (the `profile.assume_grant_ids.is_empty()` branch). It should emit the alert-specific trust-grant message above, interpolating only the request's profileId, rather than reusing the AssumeStatus DENIED doc sentence with the space id appended. The refusal condition itself (and its ValidationException/400 classification) already matches AWS and should not change; also confirm an ACCESS_PROFILE-principal grant on the profile does not satisfy the ALERT trust requirement (it must not, and today it correctly does not).

Traces: cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant (handler: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_alert.rs lines 274-293; the comment at lines 267-273 states the reuse of the AssumeStatus DENIED wording).

## Evidence
Check cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant, after CreateDomain/CreateSpace/CreateAccessProfile/CreateAccessGrant(principalType=ACCESS_PROFILE, permission=READ) all succeeded and round-tripped:

Call: CreateAlert(spaceId=<space>, profileId=<profile>, name=<alert>, rule={telemetryRule:{query:{language:SQL,expression:'SELECT 1'},condition:{thresholdMode:COUNT_OF_RESULTS,comparator:GT,criticalThreshold:90.0},evaluation:{intervalSeconds:300},noData:{treatAs:NODATA}}})

Expected (AWS): HTTP 400, Error.Code=ValidationException, Error.Message=
  "The alert cannot assume access profile <profileId>. Grant the profile a trust grant with principalType=ALERT (customAction cloudwatch:AssumeAccessProfile) before creating the alert."
  (fixed text as above; <profileId> is the request's profileId, unquoted)

Actual (emulator): HTTP 400, Error.Code=ValidationException (matches), Error.Message=
  "The calling principal is not authorized to assume the access profile '<profileId>' in space '<spaceId>'."

Harness output:
  File "check_cloudwatchomni_alert_requires_an_explicit_assume_access_profile_trust_grant.py", line 115, in run
    assert message == (
  AssertionError: unexpected message "The calling principal is not authorized to assume the access profile '82e0753c-17a8-4cab-b25b-371fa70b13cd' in space '6dd5fd30-5f95-4b2d-9cb0-f74ef00ce21a'."
(The uuids above are per-run values, not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator and point a botocore client at cloudwatchomni.
2. iam.CreateRole with a trust policy allowing cloudwatch.amazonaws.com to sts:AssumeRole; keep the role ARN.
3. cwo.CreateDomain(name=<n>, identityProviders=["IAM"]) -> domainId.
4. cwo.CreateSpace(name=<n>, domainId=<domainId>, dataAccessRoleArn=<roleArn>) -> spaceId.
5. cwo.CreateAccessProfile(spaceId=<spaceId>, name=<n>) -> profileId.
6. cwo.CreateAccessGrant(domainId=<domainId>, spaceId=<spaceId>, name=<n>, principal={principalType:"ACCESS_PROFILE", principalId:<profileId>}, permission="READ") — this grant names the profile as PRINCIPAL and is deliberately NOT an ALERT trust grant.
7. cwo.CreateAlert(spaceId=<spaceId>, profileId=<profileId>, name=<n>, rule={"telemetryRule": {"query": {"language": "SQL", "expression": "SELECT 1"}, "condition": {"thresholdMode": "COUNT_OF_RESULTS", "comparator": "GT", "criticalThreshold": 90.0}, "evaluation": {"intervalSeconds": 300}, "noData": {"treatAs": "NODATA"}}}).
8. Inspect the ClientError: Code is ValidationException (correct) but Message is "The calling principal is not authorized to assume the access profile '<profileId>' in space '<spaceId>'." instead of the trust-grant message AWS returns.

Replay: run the behavioral check cloudwatchomni.alert-requires-an-explicit-assume-access-profile-trust-grant.
