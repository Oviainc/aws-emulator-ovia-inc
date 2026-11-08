Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: controltower.access-denied-without-permission-rejected
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower.access-denied-without-permission-rejected|_poll#16|AssertionError|raise AssertionError(
Issue-Commit: 5597ec87f107cb3357fdc24d00e3a7ec2ba6fc42

# controltower front door never evaluates identity policies: an explicit IAM Deny on controltower:ListLandingZones is ignored, so AccessDeniedException (403) is unreachable

> _Note: this issue was identified against the code base at commit `5597ec87f107cb3357fdc24d00e3a7ec2ba6fc42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLandingZones

## Description
One cause. NOTE: open issue 20261029T195533Z-controltower-requests-are-never-authorized-an-explicit-iam.md describes the same symptom and carries this check id, but the replay report did not mark this failing check as OWNED or ALREADY FILED, so it is re-filed here against the current checkout; if that issue is still the live work item, close them together.

Cause 1 of 1 - authorization is not evaluated on the controltower request path, so an explicit Deny in an identity-based policy has no effect and every signed caller is treated as fully authorized.
AWS: Smithy (com.amazonaws.controltower#AWSControlTowerApis): ListLandingZones is POST /list-landingzones and declares AccessDeniedException with httpError 403 among its modelled errors (with ValidationException 400, ThrottlingException 429, InternalServerException 500). AWS evaluates the signing principal's identity policies before the operation runs, and an explicit Deny overrides any Allow for the same action. Recorded behavior: with one inline role policy that Allows controltower:ListLandingZones on Resource '*' and a second inline policy that Denies the same action, the call issued under that assumed role is refused with HTTP 403, error code AccessDeniedException, and the message 'User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: controltower:ListLandingZones on resource: arn:aws:controltower:<region>:<account>:landingzone/* with an explicit deny in an identity-based policy'. In that message the FIXED text is 'User: ', ' is not authorized to perform: ', ' on resource: ' and ' with an explicit deny in an identity-based policy'; the VARIABLE parts are taken from the request's signing identity (account id, role name, role session name), the operation's IAM action name, and the operation's resource ARN built from the caller's region and account. Removing the Deny while leaving the Allow in place must make the identical call succeed again.
Emulator: the call continues to succeed after the Deny is attached. The check assumes the role afresh and re-issues the identical signed ListLandingZones call every 2s for 60s waiting for AccessDeniedException; every probe returns HTTP 200 with a landingZones list ('<call succeeded>'), and the poll exhausts its budget. The positive control before the Deny also succeeded, so the operation itself works - what is missing is the authorization gate: AccessDeniedException is not reachable through policy at all.
Why: a client relying on AWS's behavior gets a security-relevant divergence. Least-privilege and guardrail tests that must fail pass against the emulator; error-handling paths that only run on a 403 are never exercised; and a caller asserting that a role CANNOT list landing zones is told it can, so every negative authorization assertion is silently unverifiable.
Fix: the controltower front door's request authorization - evaluate the signing principal's identity policies (explicit Deny over Allow) before dispatching the operation, and serialize the modelled AccessDeniedException as HTTP 403 with AWS's wording assembled from the signing identity, the operation's IAM action and the operation's resource ARN. The gate belongs to all operations of the service, not only ListLandingZones; ListLandingZones is just where the check observes it.
Traces: controltower.access-denied-without-permission-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1, service controltower, check controltower.access-denied-without-permission-rejected [mismatch]:

  AssertionError: the explicit Deny takes effect for controltower:ListLandingZones not reached within 60.0s; last observation ('<call succeeded>', '')

Expected (AWS): once the Deny policy is attached, the identical signed call returns HTTP 403 with Error.Code 'AccessDeniedException' and Error.Message 'User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: controltower:ListLandingZones on resource: arn:aws:controltower:<region>:<account>:landingzone/* with an explicit deny in an identity-based policy' (fixed text as quoted; account id, role name, session name, region taken from the request and state).
Actual (emulator): HTTP 200 with a landingZones list on every probe for the full 60s window, i.e. no error code and no error message at all. Smithy declares AccessDeniedException/403 for this operation, so the 200 is the divergence. This is the emulator's own answer to a well-formed signed request - not a missing runner tool, not a transport or name-resolution failure, and not an assertion on an account-specific value (the expected message is templated over the request's own identity).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, check controltower.access-denied-without-permission-rejected.
2. iam.CreateRole with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root; iam.PutRolePolicy attaching {"Effect":"Allow","Action":"controltower:ListLandingZones","Resource":"*"}.
3. sts.AssumeRole into that role and call controltower.ListLandingZones with the returned credentials - it succeeds (positive control).
4. iam.PutRolePolicy attaching a second inline policy {"Effect":"Deny","Action":"controltower:ListLandingZones","Resource":"*"}.
5. Re-assume the role and re-issue the identical ListLandingZones call, polling every 2s: observe HTTP 200 with landingZones indefinitely instead of 403 AccessDeniedException with the explicit-deny message.
6. iam.DeleteRolePolicy for the Deny policy and re-issue the call; against AWS this restores success and proves the Deny caused the refusal - the emulator cannot show the transition because it never refuses.
