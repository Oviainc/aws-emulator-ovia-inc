Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.requester-pays-enforcement
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.requester-pays-enforcement|run#8|AssertionError|assert refusal == (
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# s3: GetObject refused by Requester Pays answers the generic "Access Denied" message instead of the Requester-Pays message "Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate."

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause on S3, grounded in one failing behavioral check whose id leads with `s3`. The replay report marks this check neither ALREADY FILED nor OWNED, so no open issue carries its replay key.

Cause 1 of 1: when a bucket's RequestPayment Payer is `Requester`, an anonymous (unsigned) GetObject of an existing, policy-permitted key is now correctly refused with HTTP 403 / Error.Code `AccessDenied`, but the error document carries the generic fallback message `Access Denied` instead of the Requester-Pays message AWS emits for this refusal class. The refusal classification is right; only the message produced on the Requester-Pays branch is wrong.
Failing check: s3.requester-pays-enforcement

AWS:      Smithy s3#GetObject is bound to `GET /{Bucket}/{Key+}` and declares `RequestPayer` as @httpHeader `x-amz-request-payer` (enum) on input and `RequestCharged` as @httpHeader `x-amz-request-charged` on output; s3#PutBucketRequestPayment is bound to `PUT /{Bucket}?requestPayment` with a required `RequestPaymentConfiguration` whose `Payer` is `BucketOwner` or `Requester`, and s3#GetBucketRequestPayment reads it back. GetObject's only modeled errors are `InvalidObjectState`/403 and `NoSuchKey`/404, so the authorization refusal itself is unmodeled and is recorded behavior: with `Payer=Requester`, an anonymous/unsigned GetObject is refused with HTTP 403, Error.Code `AccessDenied` and Error.Message `Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.` That message is entirely FIXED text for this error class — it has no variable parts taken from the request or from state (no bucket name, no key, no principal), which is exactly what distinguishes it from S3's generic authorization refusal whose message is the fixed text `Access Denied`. AWS applies it because an anonymous request has no account to bill, so the `x-amz-request-payer` acknowledgement cannot rescue it; the owning account's own principals stay exempt from the acknowledgement entirely. Resetting `Payer` to `BucketOwner` makes the byte-identical anonymous GetObject succeed with HTTP 200 and the object's bytes, which is what isolates Requester Pays as the cause.
Emulator: the anonymous GetObject against the `Payer=Requester` bucket is refused with Error.Code `AccessDenied` (so the poll waiting for that code now converges) but Error.Message `Access Denied` — i.e. the generic authorization-denied message is emitted on the Requester-Pays path, and the Requester-Pays message is never produced for any request. Everything before that point in the check passes: the bucket, the object, the public-access-block settings, the public-read bucket policy, PutBucketRequestPayment and the GetBucketRequestPayment read-back of `Requester` all behave correctly, and the anonymous request is now routed and answered with a real S3 error document. Hypothesis: the Requester-Pays guard reuses the handler's generic AccessDenied constructor rather than carrying its own message for this refusal class.
Why:      a client relying on AWS's behavior cannot tell a Requester-Pays refusal from an ordinary authorization refusal. The two demand opposite remediations: `Access Denied` tells the caller the policy/ACL does not permit the read (fix the grant), whereas the Requester-Pays message tells the caller the object IS readable but the request must be authenticated and must acknowledge the charge with `x-amz-request-payer` (fix the client, not the grant). Public-dataset consumers, data-sharing SDK wrappers and CLI tooling all branch on this message because the error CODE is identical for both cases, so against this emulator that branch is dead and Requester-Pays enforcement is indistinguishable from a missing grant. It also makes the enforcement unverifiable in the other direction: an emulator that simply denied every anonymous read would look identical to one that correctly implements Requester Pays.
Fix:      the GetObject handler's Requester-Pays enforcement branch and the error serializer it feeds — when the bucket's RequestPayment `Payer` is `Requester` and the caller is anonymous/unsigned, the 403 `AccessDenied` document must carry the fixed message `Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.` rather than falling through to the generic `Access Denied` text, and must do so whether or not `x-amz-request-payer` is present. The bucket's RequestPayment state is already stored and read back correctly, so no state change is implied.

Related issues, neither of which covers this: open issue 20261025T130004Z-s3-anonymous-unsigned-getobject-answers-a-bare-http.md records the EARLIER symptom of this same check (the anonymous GetObject answering a bodyless HTTP 404 with no S3 error code at all) — that symptom is gone, the call is now routed and refused with AccessDenied/403, and the suite no longer binds this check's replay key to that issue; what survives is only the message. Fixed issue 20261026T094158Z-s3-a-public-canned-acl-on-an-acl.md (Cause 2) is what made anonymous GetObject answer a proper 403 `AccessDenied` / `Access Denied` document; that generic message is correct for ITS scenario (IgnorePublicAcls suppressing a public ACL) and is being emitted here too, where the Requester-Pays message is required instead. This is not a reopen of either: the recorded response for this request is also a refusal, and the emulator's status and error code now match it.

Traces:   s3.requester-pays-enforcement

## Evidence
s3.requester-pays-enforcement [mismatch], verbatim replay tail:

  File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_s3_requester_pays_enforcement.py", line 154, in run
    assert refusal == (
  AssertionError: expected AccessDenied for GetObject without RequestPayer ack, got ('AccessDenied', 'Access Denied')

Expected (AWS) for an UNSIGNED GetObject of an existing, publicly-policy-permitted key on a bucket whose RequestPayment Payer is `Requester`: HTTP 403, Error.Code `AccessDenied`, Error.Message `Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.` — the whole message is fixed text for this error class, with no parts interpolated from the request or from emulator state.
Actual (emulator): Error.Code `AccessDenied` (correct) with Error.Message `Access Denied` — S3's generic authorization-denied text.

The check reaches line 154 only because its preceding `_poll_until` DID converge on Error.Code == 'AccessDenied' within the 30 s window, so the status/code half of the refusal is right and the divergence is isolated to the message. The legs before it all pass: CreateBucket, PutObject('payer.txt', b'large-shared-dataset'), PutPublicAccessBlock(BlockPublicAcls=true, IgnorePublicAcls=true, BlockPublicPolicy=false, RestrictPublicBuckets=false), PutBucketPolicy(Allow Principal '*' s3:GetObject on arn:aws:s3:::<bucket>/payer.txt), PutBucketRequestPayment(Payer='Requester') and the GetBucketRequestPayment read-back asserting 'Requester'. The check never reaches its BucketOwner reset leg, so whether the anonymous read succeeds again after Payer is reset to `BucketOwner` is not evidenced by this run.

Smithy citations (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): s3#GetObject GET /{Bucket}/{Key+}, input member RequestPayer = @httpHeader x-amz-request-payer (RequestPayer enum), output member RequestCharged = @httpHeader x-amz-request-charged, modeled errors = [InvalidObjectState/403, NoSuchKey/404] (AccessDenied is unmodeled, hence recorded behavior); s3#PutBucketRequestPayment PUT /{Bucket}?requestPayment with required RequestPaymentConfiguration (httpPayload); s3#GetBucketRequestPayment GET /{Bucket}?requestPayment returning Payer.

## Steps to Reproduce
1. s3:CreateBucket (supply CreateBucketConfiguration.LocationConstraint = the emulator's region unless it is us-east-1).
2. s3:PutObject Key='payer.txt', Body=b'large-shared-dataset'.
3. s3:PutPublicAccessBlock with BlockPublicAcls=true, IgnorePublicAcls=true, BlockPublicPolicy=false, RestrictPublicBuckets=false, so the public-read policy below is allowed to take effect.
4. s3:PutBucketPolicy allowing Effect=Allow, Principal='*', Action='s3:GetObject' on arn:aws:s3:::<bucket>/payer.txt.
5. s3:PutBucketRequestPayment with RequestPaymentConfiguration={'Payer': 'Requester'} and confirm s3:GetBucketRequestPayment reports 'Requester' (this already works).
6. s3:GetObject that key with an UNSIGNED client (botocore Config(signature_version=UNSIGNED)), with no x-amz-request-payer header. AWS refuses with HTTP 403 / Error.Code 'AccessDenied' / Error.Message 'Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.'; the emulator refuses with HTTP 403 / 'AccessDenied' but Error.Message 'Access Denied'.
7. Repeat step 6 with x-amz-request-payer='requester' present — AWS still refuses an anonymous caller with the same code and the same Requester-Pays message.
8. Control: s3:PutBucketRequestPayment with Payer='BucketOwner', then repeat the UNSIGNED GetObject — AWS returns HTTP 200 and b'large-shared-dataset', which isolates Requester Pays as the cause of the refusal in steps 6-7.
9. Control: as the bucket owner (signed, same account), GetObject without the acknowledgement header succeeds in every state above.
Or replay the check named in trace_ids.
