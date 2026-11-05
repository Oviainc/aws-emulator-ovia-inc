Issue-Source: trace-sweep-divergence
Issue-Trace: s3-oc-expected-bucket-owner-guard
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-oc-expected-bucket-owner-guard s3-pab-block-public-policy-classifies-the-document
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketOwnershipControls|4xx->4xx|$.Error.Code
Issue-Key: s3|GetBucketOwnershipControls|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketOwnershipControls|4xx->4xx|status
Issue-Key: s3|GetBucketPolicyStatus|2xx->4xx|$.PolicyStatus
Issue-Key: s3|GetBucketPolicyStatus|2xx->4xx|status
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: bac10eb1ee4725027fa554575afc99d177c37f90

# s3 bucket sub-resource guards answer the wrong class: ExpectedBucketOwner format refusal and GetBucketPolicyStatus read

> _Note: this issue was identified against the code base at commit `bac10eb1ee4725027fa554575afc99d177c37f90`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketOwnershipControls

## Description
Two net-new root causes in the s3 emulator's bucket sub-resource guards, both from wire-fixture steps that no open issue's replay key covers. Each makes a bucket sub-resource read answer a different status class than AWS does, so a client branching on the error class takes the wrong branch. A fixer must close BOTH causes before submitting.

Cause 1 of 2: a syntactically invalid x-amz-expected-bucket-owner is treated as an owner mismatch instead of a malformed parameter.
AWS: x-amz-expected-bucket-owner carries an AWS account id. When the supplied value is not an account id at all (not a 12-digit account number), S3 refuses the request as a malformed parameter BEFORE any ownership comparison: HTTP 400 with error code InvalidBucketOwnerAWSAccountID and a message whose fixed text is "The value of the expected bucket owner parameter must be an AWS Account ID..." and whose variable part is the rejected header value taken verbatim from the request (in the recording, the value `not-an-account`; the recorded expectation elides the sentence's tail with "..." and carries the rejected value in brackets). AccessDenied/403 is reserved for a WELL-FORMED account id that does not match the bucket's owner. Smithy: GetBucketOwnershipControls input ExpectedBucketOwner -> httpHeader x-amz-expected-bucket-owner, target com.amazonaws.s3#AccountId, a plain string with NO pattern/length constraint, and the operation models no errors — so both the 400/InvalidBucketOwnerAWSAccountID class and the 403/AccessDenied class are recorded behavior only, taken from the recorded responses of this fixture (task#2 correctly 403 AccessDenied for a well-formed foreign account id; task#3 400 InvalidBucketOwnerAWSAccountID for the non-account value).
Emulator: GetBucketOwnershipControls with x-amz-expected-bucket-owner set to a non-account string answers HTTP 403, error code 'AccessDenied', message 'Access Denied' — i.e. it string-compares the header against the bucket owner's account id and reports every non-equal value as an ownership failure, with no syntactic validation of the header at all.
Why: a client cannot distinguish "I sent a bad value for this header" from "this bucket belongs to another account". A typo, an unexpanded template variable or an ARN pasted where an account id belongs all surface as AccessDenied, sending the caller into a cross-account permissions investigation (bucket policy, ownership controls, credentials) for what is a client-side request-formatting bug that AWS reports as 400 on the spot. Retry/backoff layers also treat 403 and 400 differently.
Fix: the shared ExpectedBucketOwner guard used by the bucket control-plane handlers (GetBucketOwnershipControls and its siblings) — validate the header's syntax first and refuse 400 InvalidBucketOwnerAWSAccountID naming the supplied value, and only compare a well-formed account id against the resolved bucket's owner (403 AccessDenied) afterwards. Note this is NOT the ordering defect already filed as 20261104T193610Z-s3-2-net-new-root-causes-restoreobject-refuses.md (cause 2 there: existence check vs owner guard ordering, a missing bucket reported AccessDenied instead of NoSuchBucket); that issue's replay key does not cover this step and a well-formed-but-foreign id is handled correctly here.
Traces: s3-oc-expected-bucket-owner-guard#task#3

Cause 2 of 2: GetBucketPolicyStatus answers 404 for a bucket that has a policy, instead of 200 with PolicyStatus.IsPublic=false.
AWS: GetBucketPolicyStatus on a bucket that HAS a bucket policy answers HTTP 200 with a PolicyStatus document reporting whether that policy makes the bucket public. A policy whose only Allow statement grants Principal "*" but is restricted by a condition that confines access to a known account (here StringEquals on aws:PrincipalAccount) is NOT public, so the answer is 200 with <PolicyStatus><IsPublic>false</IsPublic></PolicyStatus>; such a policy is therefore also accepted by PutBucketPolicy even while the bucket's PublicAccessBlock has BlockPublicPolicy=true, because BlockPublicPolicy only rejects policies the classifier deems public. 404 (NoSuchBucketPolicy) is correct only when the bucket has no policy at all. Smithy: GetBucketPolicyStatus http GET /{Bucket}?policyStatus, code 200, output PolicyStatus (httpPayload) with member IsPublic (boolean, "TRUE indicates that this bucket is public. FALSE indicates that the bucket is not public."); the operation models no errors, so the 404 NoSuchBucketPolicy class is recorded behavior only.
Emulator: at this point in the fixture the bucket has no policy stored, because the emulator refused the preceding PutBucketPolicy of exactly that condition-restricted document with 403 instead of accepting it (204) — its BlockPublicPolicy check classifies any Allow to Principal "*" as public and ignores the statement's conditions. GetBucketPolicyStatus consequently answers HTTP 404 with no PolicyStatus element, where the recording has 200 and {IsPublic: false}: the public-policy classifier never gets to report a non-public verdict, neither on write nor on read.
Why: the documented way to confirm a policy is not public is to read its policy status. A client that applies a condition-scoped policy under BlockPublicPolicy gets its write refused and, when it asks the service to classify the policy, gets "no such policy" — so a correct, non-public, same-account policy is unusable and unverifiable, and any tooling that gates on IsPublic=false (compliance checks, CI guardrails) cannot pass against the emulator.
Fix: the shared public-policy classifier behind PutBucketPolicy's BlockPublicPolicy evaluation and the GetBucketPolicyStatus handler — a statement's Condition must be taken into account so an Allow to "*" scoped by aws:PrincipalAccount (and the equivalent account-scoping conditions) is classified non-public, the write accepted (204), and the read answer 200 with IsPublic=false. Adjacent open issues do not cover this step: 20261102T111455Z-s3-bucket-policy-plane-accepts-malformed-policies-and.md owns the PutBucketPolicy refusal step (task#7) and the GetBucketPolicy read (task#9) in this same fixture, and 20261103T071133Z-s3-getbucketpolicy-answers-404-xml-for-a-bucket.md covers GetBucketPolicy, not the policyStatus sub-resource — the GetBucketPolicyStatus step carries no owner in the replay report.
Traces: s3-pab-block-public-policy-classifies-the-document#task#8

## Evidence
Cause 1 — fixture s3-oc-expected-bucket-owner-guard, step task#3, operation GetBucketOwnershipControls (verdict: mismatch)
Expected: HTTP 400; $.Error.Code = 'InvalidBucketOwnerAWSAccountID'; $.Error.Message = 'The value of the expected bucket owner parameter must be an AWS Account ID...' with the rejected header value from the request (recorded as '[not-an-account]').
Actual:   HTTP 403; $.Error.Code = 'AccessDenied'; $.Error.Message = 'Access Denied'.
Control in the same fixture: task#2 (same operation, a well-formed foreign account id) matches AWS at 403/AccessDenied — only its RequestId diverges, which is owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md. So the guard is reached; it is the syntactic validation that is missing.
Note: the $.Error.RequestId mismatch reported alongside this step is the known request-id defect and is not part of this issue.

Cause 2 — fixture s3-pab-block-public-policy-classifies-the-document, step task#8, operation GetBucketPolicyStatus (verdict: mismatch)
Expected: HTTP 200 with $.PolicyStatus = {'IsPublic': 'false'}.
Actual:   HTTP 404, $.PolicyStatus missing (no PolicyStatus element in the body).
Upstream in the same fixture: task#7 PutBucketPolicy — the condition-scoped policy (Version 2012-10-17; one Allow of s3:GetObject to Principal "*" on arn:aws:s3:::<bucket>/* with Condition StringEquals aws:PrincipalAccount = the caller's own account) — expected 204, got 403; task#9 GetBucketPolicy expected 200 with that document, got 404. Those two steps are owned by 20261102T111455Z-s3-bucket-policy-plane-accepts-malformed-policies-and.md; the policyStatus read at task#8 is not, and it is the step that shows the classifier never produces an IsPublic=false verdict.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect the two steps named in Traces.

Cause 1: create a bucket owned by the caller, then GET /<bucket>?ownershipControls with header x-amz-expected-bucket-owner: not-an-account (any value that is not a 12-digit AWS account id). Expect HTTP 400 with error code InvalidBucketOwnerAWSAccountID and the message naming the supplied value; observe HTTP 403 AccessDenied / 'Access Denied'. Control: repeat with a well-formed but foreign account id (e.g. 111122223333) — that one must stay 403 AccessDenied.

Cause 2: create a bucket; PutPublicAccessBlock with BlockPublicPolicy=true; PUT /<bucket>?policy with a policy whose single statement is Allow s3:GetObject to Principal "*" on arn:aws:s3:::<bucket>/* with Condition {StringEquals: {"aws:PrincipalAccount": "<caller account>"}} — expect HTTP 204 (the policy is not public, so BlockPublicPolicy does not reject it); then GET /<bucket>?policyStatus — expect HTTP 200 with <PolicyStatus><IsPublic>false</IsPublic></PolicyStatus>, observe HTTP 404 with no PolicyStatus element. GET /<bucket>?policy must likewise return the stored document.
