Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.acl-disabled-bucket-rejects-put-object-acl s3.s3-public-read-object-acl-rejected-when-block-public-acls-enabled
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.acl-disabled-bucket-rejects-put-object-acl|run#12|AssertionError|assert code == "AccessDenied", (
Issue-Key: s3.s3-public-read-object-acl-rejected-when-block-public-acls-enabled|run#29|AssertionError|assert code == "AccessDenied", (
Issue-Commit: 8ac25d866c3499bf9b2f284cb79ae4218e5e0f1c

# s3: a public canned ACL on an ACL-disabled bucket is refused with the wrong error (Block Public Access is not evaluated first), and an anonymous unsigned GetObject answers a bare HTTP 404 instead of 403 AccessDenied (2 causes)

> _Note: this issue was identified against the code base at commit `8ac25d866c3499bf9b2f284cb79ae4218e5e0f1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObjectAcl

## Description
Two distinct net-new root causes in the S3 surface, each grounded in its own failing behavioral check whose check id leads with `s3`. Neither check is marked ALREADY FILED or OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 2: on a default (ObjectOwnership=BucketOwnerEnforced) bucket, PutObjectAcl with a PUBLIC canned ACL is refused with the ACLs-disabled error instead of the Block-Public-Access denial — the two guards are evaluated in the wrong order.
Failing check: s3.acl-disabled-bucket-rejects-put-object-acl

AWS:      Smithy s3#PutObjectAcl is bound to PUT /{Bucket}/{Key+}?acl with ACL as @httpHeader x-amz-acl targeting the ObjectCannedACL enum, and its ONLY modeled error is NoSuchKey/404 — so both refusals below are AWS's documented behavior rather than model constraints. On a bucket created with today's defaults (ObjectOwnership=BucketOwnerEnforced, Block Public Access all-true):
          (a) ACL="private" is refused with Error.Code AccessControlListNotSupported and the message "The bucket does not allow ACLs" — fixed text, no variable parts. The emulator already answers this leg correctly.
          (b) ACL="public-read" is refused with Error.Code AccessDenied, because Block Public Access is evaluated BEFORE the ACLs-disabled check. Its message's fixed text is 'User: <...> is not authorized to perform: s3:PutObjectAcl on resource: "<...>" because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.'; the variable parts are the caller's own principal ARN (from the request's credentials) and the object ARN arn:aws:s3:::<bucket>/<key> built from the request path — never a literal from one run.
          Neither refusal may change the object's ACL, and the identical PutObjectAcl must still work on a sibling bucket whose ObjectOwnership is ObjectWriter with BPA off.
Emulator: ACL="public-read" is refused with Error.Code AccessControlListNotSupported, i.e. the BucketOwnerEnforced guard answers first and the BlockPublicAcls guard is never reached, so the AccessDenied message naming BlockPublicAcls is never produced. Hypothesis: the handler short-circuits on object ownership before consulting the bucket's public-access block — consistent with open issue 20261025T181311Z-s3-new-buckets-have-no-block-public-access.md, which records that a freshly created bucket carries no Block Public Access configuration at all (GetPublicAccessBlock answers NoSuchPublicAccessBlockConfiguration instead of the all-true creation default); if there is no BPA state to read, no BPA refusal can be produced.
Why:      the two codes mean different things to a client and demand different remediations. AccessControlListNotSupported says "ACLs are off on this bucket, express the grant as a bucket policy"; AccessDenied naming BlockPublicAcls says "this grant is PUBLIC and the account/bucket forbids public grants". A client that branches on the code to decide whether to retry with a non-public ACL, to rewrite to a policy, or to surface a compliance failure takes the wrong branch. It also makes the precedence between the two guards — itself a behavior that security tooling asserts on — untestable against the emulator.
Fix:      the PutObjectAcl handler's validation order: evaluate the bucket's Block Public Access (BlockPublicAcls) against the requested canned ACL / grant set BEFORE the BucketOwnerEnforced ACLs-disabled refusal, and emit AccessDenied with the message above (templated on the caller ARN and the request's bucket/key) for a public grant. Related open issues: 20261025T181311Z-s3-new-buckets-have-no-block-public-access.md (missing BPA creation default and diverging BPA messages) and 20261025T121424Z-s3-6-net-new-root-causes-putbucketacl-ignores.md (PutBucketAcl ignores BucketOwnerEnforced). Neither carries this check's replay key, and neither states the precedence rule between the BPA check and the ACL-disabled check, which is what this check pins.

----------------------------------------------------------------
Cause 2 of 2: an anonymous (unsigned) GetObject of an existing key answers a bare HTTP 404 with no S3 error document, where AWS refuses it with HTTP 403 AccessDenied.
Failing check: s3.s3-public-read-object-acl-rejected-when-block-public-acls-enabled

AWS:      Smithy s3#GetObject is bound to GET /{Bucket}/{Key+} and models only InvalidObjectState/403 and NoSuchKey/404, so an authorization refusal is the unmodeled AccessDenied — recorded behavior: HTTP 403 with Error.Code AccessDenied and the message "Access Denied" (fixed text, no variable parts). The scenario: an ObjectOwnership=ObjectWriter bucket, PutPublicAccessBlock with BlockPublicAcls=false but IgnorePublicAcls=true / BlockPublicPolicy=true / RestrictPublicBuckets=true, and an object whose public-read ACL PutObjectAcl has just accepted. Because IgnorePublicAcls suppresses the AllUsers grant, the object is NOT public, so an unsigned GetObject of that existing key is refused AccessDenied/403 — the key exists, so 404 is never correct here.
Emulator: the unsigned GetObject produces no S3 <Error><Code> document at all; botocore surfaces the synthetic pair code='404'. Everything earlier in the check passes — BlockPublicAcls=true does reject the public-read PutObjectAcl with AccessDenied and the documented BlockPublicAcls message, the rejected call leaves no AllUsers grant, the bucket-owner-full-control control ACL applies, and lifting BlockPublicAcls lets the identical public-read call through — so the divergence is isolated to how the anonymous read is answered.
Why:      a client cannot distinguish "blocked" from "missing". Every public-S3 consumer path (browser, curl, unsigned SDK client) is told the object does not exist, so IgnorePublicAcls/RestrictPublicBuckets enforcement is unverifiable and an emulator that simply never serves anonymous requests looks indistinguishable from one that enforces the block correctly. A 404 with no error code also breaks retry/alarm logic that keys on AccessDenied, and any code treating 404 as "safe to recreate" can act on a false absence.
Fix:      the GetObject handler's anonymous/unsigned request admission and authorization path, plus its error serializer: an unsigned request must be routed, authorized against the bucket policy / ACL with IgnorePublicAcls and RestrictPublicBuckets applied, and refused with HTTP 403 and a proper S3 error document carrying Code AccessDenied and Message "Access Denied" — not a bodyless 404. Related open issue: 20261025T130004Z-s3-anonymous-unsigned-getobject-answers-a-bare-http.md records the same bare-404 symptom on the Requester-Pays / public-read-bucket-policy legs; it does not carry this check's replay key, and this check pins the ACL-plus-IgnorePublicAcls leg, where the required answer is a REFUSAL rather than a success. Same handler, so a fix there should be re-replayed against this check too.

----------------------------------------------------------------
Both checks declare SERVICES = ['s3'] and s3 is bootstrapped.

## Evidence
Cause 1 — s3.acl-disabled-bucket-rejects-put-object-acl [mismatch], verbatim replay tail:

  File "check_s3_acl_disabled_bucket_rejects_put_object_acl.py", line 91, in run
    assert code == "AccessDenied", (
  AssertionError: expected AccessDenied (Block Public Access precedes the ACL-disabled check), got 'AccessControlListNotSupported'

Expected for PutObjectAcl(Bucket=<default bucket>, Key=<key>, ACL="public-read"): Error.Code = AccessDenied with Error.Message = 'User: <caller principal ARN> is not authorized to perform: s3:PutObjectAcl on resource: "arn:aws:s3:::<bucket>/<key>" because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.' (fixed text as quoted; the principal ARN is read from the caller's own identity and the object ARN is built from the request's bucket and key). Actual: Error.Code = AccessControlListNotSupported.
The preceding leg — PutObjectAcl with ACL="private" on the same bucket — correctly answers AccessControlListNotSupported with "The bucket does not allow ACLs" and leaves the object's grants unchanged, which is what isolates the failure to the PUBLIC form and therefore to the ordering of the two guards. Smithy citation: s3#PutObjectAcl PUT /{Bucket}/{Key+}?acl, ACL = @httpHeader x-amz-acl (ObjectCannedACL enum), modeled errors = [NoSuchKey/404] only.

Cause 2 — s3.s3-public-read-object-acl-rejected-when-block-public-acls-enabled [mismatch], verbatim replay tail:

  File "check_s3_s3_public_read_object_acl_rejected_when_block_public_acls_enabled.py", line 115, in run
    assert code == "AccessDenied", (
  AssertionError: IgnorePublicAcls=True must keep an anonymous GET refused after the public-read call was accepted, got '404'

Expected for an UNSIGNED GetObject (botocore Config(signature_version=UNSIGNED)) of the existing key: HTTP 403, Error.Code = AccessDenied, Error.Message = "Access Denied" (fixed text, no variable parts). Actual: a response with no S3 <Error> document, which botocore reports as code='404'. Smithy citation: s3#GetObject modeled errors = InvalidObjectState/403 and NoSuchKey/404; AccessDenied is unmodeled, so the 403/AccessDenied refusal is recorded behavior.
Earlier legs of the same check pass, including the BlockPublicAcls=true rejection of PutObjectAcl(ACL="public-read") with the full documented BlockPublicAcls message — so the bucket's public-access block IS consulted by PutObjectAcl once it has been written explicitly by PutPublicAccessBlock, and only the anonymous GetObject path is wrong.

## Steps to Reproduce
Cause 1 (BPA must precede the ACL-disabled check):
1. s3:CreateBucket with no ObjectOwnership override (so it is BucketOwnerEnforced) and CreateBucketConfiguration.LocationConstraint = the emulator's region unless us-east-1.
2. s3:PutObject Key='acl-disabled.txt', Body=b'x'; s3:GetObjectAcl to record the baseline grants.
3. s3:PutObjectAcl(ACL="private") — AWS and the emulator both answer AccessControlListNotSupported / "The bucket does not allow ACLs", and the grants are unchanged.
4. s3:PutObjectAcl(ACL="public-read") — AWS answers Error.Code AccessDenied with 'User: <caller ARN> is not authorized to perform: s3:PutObjectAcl on resource: "arn:aws:s3:::<bucket>/acl-disabled.txt" because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.'; the emulator answers AccessControlListNotSupported.
5. s3:GetObjectAcl again — the grants must be unchanged (they are).
6. Positive control: CreateBucket with ObjectOwnership=ObjectWriter, PutPublicAccessBlock with all four flags false, PutObject, then the identical PutObjectAcl(ACL="public-read") must SUCCEED and leave an AllUsers grant.

Cause 2 (anonymous GetObject must be 403 AccessDenied):
1. s3:CreateBucket with ObjectOwnership=ObjectWriter (+ LocationConstraint unless us-east-1).
2. s3:PutPublicAccessBlock with BlockPublicAcls=true, IgnorePublicAcls=true, BlockPublicPolicy=true, RestrictPublicBuckets=true.
3. s3:PutObject Key='deals.csv', Body=b'deal_id\nD1\n'.
4. s3:PutObjectAcl(ACL="public-read") — refused AccessDenied with the BlockPublicAcls message (this already works).
5. s3:PutPublicAccessBlock again with BlockPublicAcls=false and IgnorePublicAcls / BlockPublicPolicy / RestrictPublicBuckets still true, then repeat s3:PutObjectAcl(ACL="public-read") — it now returns HTTP 200.
6. s3:GetObject the same key with an UNSIGNED client (botocore Config(signature_version=UNSIGNED)). AWS refuses with HTTP 403 / AccessDenied / "Access Denied" because IgnorePublicAcls suppresses the public grant; the emulator answers a bodyless HTTP 404 (botocore reports code='404').
Or replay the two checks named in trace_ids.
