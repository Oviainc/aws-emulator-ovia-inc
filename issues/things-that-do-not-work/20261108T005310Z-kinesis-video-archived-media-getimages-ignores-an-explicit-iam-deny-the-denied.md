Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-archived-media.get-images-not-authorized-without-permission
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media.get-images-not-authorized-without-permission|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: f8ada2b69833d1b930b89852ccac9f51cdbe0c71

# GetImages ignores an explicit IAM Deny: the denied identity is refused with ResourceNotFoundException (fragment absence) instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `f8ada2b69833d1b930b89852ccac9f51cdbe0c71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImages

## Description
One net-new root cause in the kinesis-video-archived-media (KVAM) data plane: the GetImages request path performs no IAM authorization of the signing identity, so an identity an inline policy explicitly DENIES for kinesisvideo:GetImages on a stream is served by the operation body and refused only by the archive lookup.

Related open issue: 20261029T202349Z-kinesis-video-archived-media-data-plane-streams-never.md describes this same emulator gap as its 'Cause 5 of 5', but it was raised when GetImages was still an unimplemented scaffold and its recorded replay key for this check is a poll TIMEOUT (`...|_poll_until#24|Timeout|time.sleep(interval_s)`); the check now fails with a different key, an AssertionError raised at _poll_until's deadline carrying a concrete last result, so that issue does not own this failure and its pointer does not re-run it. The symptom has also MOVED since that issue was written: the GetImages stub has since been implemented (see fixed 20261029T202349Z-kinesis-video-archived-media-getimages-stub.md and 20261029T235218Z-...-getimages-returns-notimplemented-501-instead-of-resourcenotfoundexception), so the denied identity no longer times out against a 501 — it now receives a well-formed 404 ResourceNotFoundException, i.e. the unauthorized caller is affirmatively told the window holds no media. The authorization gate is still absent.

Cause 1 of 1: GetImages is not authorization-checked before the operation body runs.
AWS: Smithy GetImages (POST /getImages, success 200) models ClientLimitExceededException (400), InvalidArgumentException (400), NoDataRetentionException (400), NotAuthorizedException (401) and ResourceNotFoundException (404) — Smithy: GetImages.errors; the operation's own modeled errors do not include the IAM-layer refusal, which is raised by the request pipeline BEFORE the operation body and is recorded behavior only. The recording shows an IAM user whose inline policy Allows kinesisvideo:GetDataEndpoint/DescribeStream/ListFragments/GetImages on '*' and carries an explicit Deny of kinesisvideo:GetImages on one stream's ARN being refused with error code AccessDeniedException and message 'User: arn:aws:iam::<account-id>:user/<user-name> is not authorized to perform: kinesisvideo:GetImages on resource: <stream-name>'. Fixed text: 'User: ', ' is not authorized to perform: ', ' on resource: '. Variable parts, taken from the signing identity and the request: the caller's user ARN (account id + user name), the action 'kinesisvideo:GetImages', and the stream the request addresses. The deny is SCOPED, and the recording proves both sides of that scope: (a) the same restricted identity's ListFragments, which the policy does not deny, still succeeds 200 with an empty Fragments list on the same fresh stream, and (b) the account's own undenied credentials reach GetImages' own refusal path and are refused 404 ResourceNotFoundException 'No fragments found in the stream for the GetImages request.'. Ordering is part of the requirement: authorization precedes the archive/fragment lookup, so the denied identity never learns whether the window holds media.
Emulator: the denied identity's GetImages is never refused with AccessDeniedException. It is answered by the fragment-absence path instead — ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.') — identical to what the stream OWNER receives, repeated on every attempt across a 60s / 3s-interval poll, so this is not an eventual-consistency lag. No part of the signing identity's policy is consulted for the call.
Why: this is an overly-permissive authorization failure. A caller real AWS refuses outright is served the operation's normal answer, so (i) IAM policy tests — explicit-Deny guardrails, scoped-read boundaries, least-privilege regression suites — pass against the emulator on a configuration AWS rejects, and the gap is only discovered in production; (ii) the client gets the wrong remediation signal: 'no media in this window, keep polling / ingest media' instead of 'fix the policy'; (iii) the 404 answer itself leaks the archive's state (window emptiness) to an identity that is not authorized to read it, and once the stream does hold fragments the emulator will hand that identity actual image data.
Fix: the KVAM data-plane request pipeline's authorization step, applied to GetImages and the other stream-addressed reads — evaluate the signing identity's IAM policy (explicit Deny winning over Allow) against kinesisvideo:<action> and the addressed stream's ARN, and raise AccessDeniedException with AWS's sentence BEFORE dispatching into the operation handler, so the refusal is independent of the fragment index. Keep the two controls intact: a non-denied action for the same identity must still succeed, and the owner's credentials must still reach the fragment-absence refusal.
Traces: kinesis-video-archived-media.get-images-not-authorized-without-permission.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check kinesis-video-archived-media.get-images-not-authorized-without-permission, verdict mismatch (17/18 passed overall; this is the only failure).

Replay output:
  AssertionError: GetImages refused with AccessDeniedException for the denied identity not met within 60s. Last result: ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.')

Expected (code, message) for the explicitly-denied identity:
  ('AccessDeniedException', 'User: arn:aws:iam::<account-id>:user/<user-name> is not authorized to perform: kinesisvideo:GetImages on resource: <stream-name>')
Observed, on every one of the ~20 polled attempts over 60s:
  ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.')

Requirement vs per-run values: the requirement is the IAM-layer refusal — error code AccessDeniedException and the message's fixed text 'User: ' / ' is not authorized to perform: ' / ' on resource: '. The account id, the generated user name (sim2real-kvam-deny-*) and the generated stream name (sim2real-kvam-*) are per-run values from the identity and the request that fill the template; they are not part of it. The denial must also precede the fragment lookup, which is what makes it distinguishable from the owner's refusal.

Controls recorded in the same check (both must keep passing after the fix): the same restricted identity's ListFragments on the same stream returns 200 with Fragments == [] (the policy does not deny it), and the account's own credentials calling GetImages are refused ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.') — i.e. the emulator's current answer to the DENIED identity is exactly the answer reserved for the authorized owner.

Model: smithy_query(query="operation", operation="GetImages", model_path="services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/smithy-model/com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader.json") — GetImages: POST /getImages, code 200; errors ClientLimitExceededException(400), InvalidArgumentException(400), NoDataRetentionException(400), NotAuthorizedException(401), ResourceNotFoundException(404). The AccessDeniedException refusal is the pipeline-level IAM refusal (recorded behavior only), not one of the operation's modeled errors.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, and replay the behavioral suite simparity-behavioral-aws-v1 for service kinesis-video-archived-media.
2. Run check kinesis-video-archived-media.get-images-not-authorized-without-permission. It: creates a kinesisvideo stream (CreateStream with DataRetentionInHours=1, MediaType=video/x-matroska) and waits for ACTIVE; creates an IAM user with an inline policy that Allows kinesisvideo:GetDataEndpoint, DescribeStream, ListFragments and GetImages on '*' and adds an explicit Deny statement for kinesisvideo:GetImages on that stream's ARN; creates an access key for the user and waits until sts:GetCallerIdentity with those credentials succeeds.
3. With the restricted credentials, resolve the data endpoints via GetDataEndpoint(APIName='GET_IMAGES') and GetDataEndpoint(APIName='LIST_FRAGMENTS'), then call GetImages(StreamName=<stream>, ImageSelectorType='PRODUCER_TIMESTAMP', StartTimestamp=now-10, EndTimestamp=now, Format='JPEG'), polling for up to 60s at 3s intervals.
   Expected: error code AccessDeniedException with message 'User: arn:aws:iam::<account-id>:user/<user-name> is not authorized to perform: kinesisvideo:GetImages on resource: <stream-name>'.
   Observed: ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.') on every attempt until the 60s deadline.
4. Control A (must keep passing): with the SAME restricted credentials call ListFragments(StreamName=<stream>, FragmentSelector={SERVER_TIMESTAMP, TimestampRange now-60..now}) — it must still answer 200 with Fragments == [], proving the refusal added in step 3 is scoped to the denied action.
5. Control B (must keep passing): with the account's own credentials call the same GetImages — it must still be refused ('ResourceNotFoundException', 'No fragments found in the stream for the GetImages request.'), proving the deny is scoped to the restricted identity and that the authorization gate did not replace the fragment-absence path.
6. Re-replay kinesis-video-archived-media.get-images-not-authorized-without-permission to confirm the cause is closed.
