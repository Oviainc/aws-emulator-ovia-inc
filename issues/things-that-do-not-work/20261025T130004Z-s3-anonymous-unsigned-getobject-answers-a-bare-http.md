Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.requester-pays-enforcement s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.requester-pays-enforcement|_poll_until#8|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected|_poll#8|Timeout|time.sleep(2.0)
Issue-Commit: 4051ebf28f0bd776eb1ce6243425fd1600c846dd

# S3 anonymous (unsigned) GetObject answers a bare HTTP 404 'Not Found' with no S3 error code, so a public-read bucket policy is never honoured and Requester Pays is never enforced

> _Note: this issue was identified against the code base at commit `4051ebf28f0bd776eb1ce6243425fd1600c846dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause on S3, grounded in two failing behavioral checks. Both exercise the SAME request: an UNSIGNED (anonymous) GetObject against an existing key of an existing bucket that carries a public-read bucket policy, with Block Public Policy / Restrict Public Buckets switched off, while the bucket's RequestPayment Payer is toggled between BucketOwner and Requester.

Cause 1 of 1: the anonymous GetObject path is not served — it answers an HTTP 404 with an empty/unparseable body (botocore surfaces the synthetic pair code='404', message='Not Found'), rather than evaluating the bucket policy and the Requester-Pays setting.

AWS: Smithy s3#GetObject is bound to GET /{Bucket}/{Key+} and declares RequestPayer (@httpHeader x-amz-request-payer) on input and RequestCharged (@httpHeader x-amz-request-charged) on output; s3#PutBucketRequestPayment is bound to PUT /{Bucket}?requestPayment with a required RequestPaymentConfiguration whose Payer is BucketOwner or Requester. The model does not enumerate AccessDenied on GetObject (its modeled errors are only InvalidObjectState/403 and NoSuchKey/404), so the two outcomes below are AWS's documented Requester-Pays behavior rather than model constraints. (a) With Payer=BucketOwner and a bucket policy allowing s3:GetObject to Principal "*" on that object ARN, an anonymous GetObject SUCCEEDS with HTTP 200 and the object's bytes. (b) With Payer=Requester, the byte-identical anonymous GetObject is REFUSED with HTTP 403, error code AccessDenied and the fixed message 'Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.' — this message is a constant for this error class, with no variable parts; the x-amz-request-payer acknowledgement header does NOT rescue an unsigned caller, because an anonymous request has no account to charge. The owning account's own principals stay exempt from the acknowledgement and read the object without the header either way.

Emulator: the anonymous GetObject never produces an S3 error document at all. Under Payer=Requester the check that polls for the refusal keeps observing ('404', 'Not Found') for its whole 30 s window, i.e. a 404 status with no <Error><Code> body, where AccessDenied/403 is required. Under the positive control (Payer=BucketOwner plus the same public-read policy) the same anonymous GET also never returns the bytes, so the sibling check cannot even get past its first convergence poll and is killed by the harness at the 60 s cap. PutBucketRequestPayment and GetBucketRequestPayment themselves round-trip correctly (the Payer=Requester read-back assertion passes before the failing poll), so the configuration is stored and only the GetObject authorization/enforcement path is wrong.

Why: a client relying on AWS's behavior gets two distinct wrong answers. A public dataset consumer that reads an object through an anonymous/unsigned client (the normal way public S3 content is fetched, including every browser and curl download) is told the object does not exist, so public-read bucket policies are untestable against this emulator. A client that handles Requester Pays — branching on AccessDenied/403 to add the x-amz-request-payer acknowledgement, or to report that the caller must authenticate — instead sees a 404 with no error code, concludes the key is missing, and silently takes a 'not found' path for what AWS calls an authentication/payment failure. It also means an emulator implementing Requester Pays as 'header present => allow' would look correct on the header-bearing probe while still being wrong, so the enforcement is unverifiable.

Fix: the GetObject handler's request-authorization path — admitting unsigned/anonymous requests and evaluating the bucket policy for Principal "*" instead of falling through to a 404, plus the Requester-Pays check that must refuse an anonymous caller with AccessDenied/403 and the documented message regardless of x-amz-request-payer. The bucket's RequestPayment state is already stored correctly, so no state change is implied.

Traces: s3.requester-pays-enforcement, s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected

No open issue carries this: the suite marked neither check as OWNED or ALREADY FILED, and the nearest-looking open issues are about other planes — 20261025T032458Z-s3-request-auth-and-eventing-plane-10-root.md (signing/eventing causes, not anonymous authorization) and 20261023T142021Z-sso-oidc-is-unroutable-every-unsigned-anonymous-operation.md / 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md (unsigned requests in other services, which answer UnknownService rather than a 404).

## Evidence
s3.requester-pays-enforcement (mismatch): with Payer=Requester read back successfully, the anonymous probe loop fails with `AssertionError: anonymous GetObject denied for a Requester-Pays bucket without a RequestPayer ack not observed within 30.0s; last saw ('404', 'Not Found')`. Expected: a ClientError with Error.Code == 'AccessDenied', HTTP 403, and Error.Message == 'Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.' (fixed text, no variable parts). Actual: HTTP 404 with no S3 <Error> document, which botocore reports as code='404', message='Not Found'.

s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected (mismatch): `Error: Check timed out after 60.0s`. The check's FIRST convergence poll is the positive control — an anonymous GetObject carrying RequestPayer='requester' under the default Payer=BucketOwner plus a public-read bucket policy, which AWS answers 200 with the object bytes — so the harness cap is reached because that anonymous read never succeeds. The same check would then require the Payer=Requester leg to answer ('AccessDenied', 'Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.') even though x-amz-request-payer IS present.

Both checks declare SERVICES = ['s3'] and differ only in whether the anonymous request carries x-amz-request-payer, which is why they are one cause.

## Steps to Reproduce
1. CreateBucket (with CreateBucketConfiguration.LocationConstraint = the emulator's region unless it is us-east-1).
2. PutObject Key='payer.txt', Body=b'large-shared-dataset'.
3. PutPublicAccessBlock with BlockPublicAcls=true, IgnorePublicAcls=true, BlockPublicPolicy=false, RestrictPublicBuckets=false.
4. PutBucketPolicy allowing Effect=Allow, Principal='*', Action='s3:GetObject' on arn:aws:s3:::<bucket>/payer.txt.
5. With the default Payer=BucketOwner, GetObject that key using an UNSIGNED client (botocore Config(signature_version=UNSIGNED)), both with and without RequestPayer='requester'. AWS returns HTTP 200 and the bytes; the emulator returns HTTP 404 'Not Found'.
6. PutBucketRequestPayment with RequestPaymentConfiguration={'Payer': 'Requester'} and confirm GetBucketRequestPayment reports 'Requester' (this part already works).
7. Repeat the UNSIGNED GetObject, with and without the x-amz-request-payer header. AWS refuses both with HTTP 403 / AccessDenied / 'Anonymous users cannot invoke requests against Requester Pays buckets. Please authenticate.'; the emulator still returns HTTP 404 with no error code.
8. As the bucket owner (signed, same account), GetObject without the header — AWS returns the bytes in all of the above states.
Or replay the two checks named in trace_ids.
