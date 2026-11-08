Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-media.get-media-unauthorized-role-denied-rejected
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesis-video-media.get-media-unauthorized-role-denied-rejected|_poll_until#23|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# kinesis-video-media GetMedia performs no IAM authorization: an explicit Deny on kinesisvideo:GetMedia is still served 200 instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMedia

## Description
Cause 1 of 1: GetMedia on the Kinesis Video Streams data plane never evaluates the signing principal's identity-based IAM policy, so an explicit Deny attached to the calling role has no effect and the media stream is served anyway.

AWS: recorded behavior only — the kinesis-video-media Smithy model (services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice) binds GetMedia to POST /getMedia with errors ClientLimitExceededException(400), ConnectionLimitExceededException(400), InvalidArgumentException(400), InvalidEndpointException(400), NotAuthorizedException(401) and ResourceNotFoundException(404); it models no AccessDeniedException, because the refusal is produced by the sigv4/IAM authorization layer in front of the operation and not by the operation's modelled error set. The recording shows that for a role whose inline policy Allows kinesisvideo:GetMedia the call succeeds, and that once a second inline policy with Effect=Deny on Action kinesisvideo:GetMedia, Resource "*" is attached to that same role, GetMedia by a session of that role is refused with error code AccessDeniedException and the message: "User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: kinesisvideo:GetMedia on resource: <stream-name>". Fixed text of that message: "User: ", " is not authorized to perform: kinesisvideo:GetMedia on resource: "; the variable parts are the assumed-role session ARN of the signing credentials (account id, role name and RoleSessionName from the AssumeRole that minted them) and the stream the request names (the StreamName member, not an ARN). An explicit Deny beats any Allow, so no amount of permission elsewhere may let the call through.

Emulator: the restricted session's GetMedia keeps returning HTTP 200 with a payload for the full 90 s propagation budget after the Deny policy is attached — the replay's probe reports the sentinel {'Code': 'UNEXPECTED_SUCCESS'} on every poll and never observes any error. The request is authenticated but never authorized: the emulator does not consult the caller's attached role policies for kinesisvideo:GetMedia at all, so neither the explicit-deny refusal nor its message is ever produced.

Why: a client that relies on IAM to fence off media access gets the opposite of the contract. Code written and tested against the emulator believes a least-privilege or break-glass Deny is in force while every denied principal can still pull live media, so authorization regressions pass locally and only appear against real AWS; conversely error-handling paths for AccessDeniedException (re-auth, alarm, fallback) are never exercised and cannot be tested.

Fix: the GetMedia handler's authorization step for the kinesis-video-media data plane — the request must be evaluated against the signing principal's identity-based policies (explicit Deny wins over Allow) before the stream is resolved and the payload streamed, and on denial must answer the AccessDeniedException code with the message template above, built from the assumed-role session ARN of the signing credentials, the action kinesisvideo:GetMedia, and the StreamName from the request.

Traces: kinesis-video-media.get-media-unauthorized-role-denied-rejected (the positive-control leg of the same check — the allow-only role getting 200 — passes; only the post-Deny leg diverges).

## Evidence
Check kinesis-video-media.get-media-unauthorized-role-denied-rejected [mismatch]:

AssertionError: condition not met within 90.0s (last={'Code': 'UNEXPECTED_SUCCESS'})
  at check_kinesis_video_media_get_media_unauthorized_role_denied_rejected.py:112 -> _poll_until(... lambda c: c["Code"] == "AccessDeniedException", timeout_s=90.0, interval_s=3.0)

Expected (recorded AWS): after put_role_policy attaches {"Effect":"Deny","Action":"kinesisvideo:GetMedia","Resource":"*"} to the role, GetMedia by that role's session raises a ClientError with
  Error.Code    = "AccessDeniedException"
  Error.Message = "User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: kinesisvideo:GetMedia on resource: <stream-name>"
(fixed text: "User: " ... " is not authorized to perform: kinesisvideo:GetMedia on resource: "; variables: the session ARN of the signing credentials and the request's StreamName)

Actual (emulator): every GetMedia call in the 90 s window returns HTTP 200 with a readable Payload — no ClientError at all, so the probe only ever yields the harness sentinel {'Code': 'UNEXPECTED_SUCCESS'}. The same session's pre-Deny call also returns 200, i.e. the outcome is identical with and without the explicit Deny.

## Steps to Reproduce
1. kinesisvideo:CreateStream a stream and poll DescribeStream until StreamInfo.Status == ACTIVE; GetDataEndpoint(APIName=GET_MEDIA) for its data endpoint.
2. iam:CreateRole with a trust policy allowing sts:AssumeRole by arn:aws:iam::<account-id>:root, then iam:PutRolePolicy with {"Effect":"Allow","Action":"kinesisvideo:GetMedia","Resource":"*"}.
3. sts:AssumeRole that role and build a kinesis-video-media client against the data endpoint with the returned session credentials.
4. Call GetMedia(StreamName=<stream>, StartSelector={"StartSelectorType":"EARLIEST"}) — succeeds with 200 (correct, this is the positive control).
5. iam:PutRolePolicy a second inline policy on the same role: {"Effect":"Deny","Action":"kinesisvideo:GetMedia","Resource":"*"}.
6. Re-call GetMedia with the same session credentials, polling for up to 90 s.
   Expected: ClientError AccessDeniedException with the message naming arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>, the action kinesisvideo:GetMedia and the stream name.
   Actual: HTTP 200 with a payload on every attempt; the explicit Deny is never enforced.
