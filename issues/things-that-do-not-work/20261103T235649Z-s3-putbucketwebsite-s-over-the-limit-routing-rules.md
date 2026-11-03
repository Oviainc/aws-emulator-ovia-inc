Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-website-hosting-put-rejected-for-fifty-one-routing-rules
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-website-hosting-put-rejected-for-fifty-one-routing-rules|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: a84989a361e8c7c49cb35cc41e86e09526be932e

# s3: PutBucketWebsite's over-the-limit routing-rules refusal is misworded — the message does not name the number of rules provided or the 50-rule limit as AWS words it

> _Note: this issue was identified against the code base at commit `a84989a361e8c7c49cb35cc41e86e09526be932e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketWebsite

## Description
ONE net-new root cause of service `s3`. The grounding check was not marked ALREADY FILED or OWNED by the replay report.

=== Cause 1 of 1: the 'too many routing rules' refusal carries emulator wording instead of AWS's sentence ===
AWS: `com.amazonaws.s3#WebsiteConfiguration.RoutingRules` targets the list `com.amazonaws.s3#RoutingRules` and the model declares NO length constraint, so the 50-rule ceiling is recorded behavior only. Recorded behavior: a WebsiteConfiguration carrying more than 50 RoutingRules is refused HTTP 400 with Error.Code `InvalidRequest` and Error.Message `<n> routing rules provided, the number of routing rules in a website configuration is limited to 50.` — the fixed text is ` routing rules provided, the number of routing rules in a website configuration is limited to 50.`, and the one variable part is the leading count, which is the number of RoutingRules in the REQUEST (51 in the recording, because 51 rules were sent; the literal 51 is not part of the requirement). The refusal is whole-document: a previously stored 50-rule configuration survives it unchanged, and the control arm — exactly 50 rules — is accepted and read back in declared order, unaltered.
Emulator: the error code and status are right, but the message is `The number of routing rules must not exceed 50.` — a sentence AWS never produces, which drops the count the caller supplied and therefore tells the caller nothing about how far over the limit its document is.
Why: the message is part of the refusal contract. A client that matches on AWS's sentence (log scrapers, retry/repair logic, test fixtures asserting the refusal) does not recognise this refusal, and an operator trimming a generated routing table loses the one number — how many rules were actually sent — that says how many to remove. A related resolved issue, 20261102T111455Z-s3-bucket-website-refusals-and-stored-configuration-diverge.md, re-worded several other PutBucketWebsite refusals (missing IndexDocument, malformed ErrorDocument key, HttpErrorCodeReturnedEquals) but did not touch the routing-rule COUNT refusal and does not carry this check's key; the open issue 20261103T040901Z-s3-10-more-net-new-root-causes-bucket.md covers other bucket sub-resource validation and does not carry it either.
Fix: the routing-rule validation branch of the PutBucketWebsite handler in the s3 crate — build the InvalidRequest message from AWS's template, interpolating the count of RoutingRules taken from the request, and keep the whole-document refusal semantics (the stored configuration is not modified).
Traces: s3.s3-website-hosting-put-rejected-for-fifty-one-routing-rules (its 50-rule control put, the ordered read-back and the Error.Code assertion all passed; only the message diverges).

## Evidence
s3.s3-website-hosting-put-rejected-for-fifty-one-routing-rules [mismatch]:
  AssertionError: expected the message '51 routing rules provided, the number of routing rules in a website configuration is limited to 50.', got 'The number of routing rules must not exceed 50.'
  expected: HTTP 400, Error.Code 'InvalidRequest' (this assertion PASSED), Error.Message '<count-of-RoutingRules-in-the-request> routing rules provided, the number of routing rules in a website configuration is limited to 50.'
  actual: Error.Message 'The number of routing rules must not exceed 50.'
  Control arm, passed: PutBucketWebsite with IndexDocument.Suffix 'index.html' and exactly 50 RoutingRules (each {Condition.KeyPrefixEquals: 'oldNN/', Redirect.ReplaceKeyPrefixWith: 'newNN/'}) is accepted and GetBucketWebsite reads the 50 rules back in declared order, unaltered.
  Smithy: com.amazonaws.s3#WebsiteConfiguration.RoutingRules -> com.amazonaws.s3#RoutingRules (list of RoutingRule, xmlName 'RoutingRule'); no @length constraint is modelled, so the 50-rule limit and its sentence are recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay s3.s3-website-hosting-put-rejected-for-fifty-one-routing-rules from suite simparity-behavioral-aws-v1.

By hand:
1. CreateBucket (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. PutBucketWebsite with WebsiteConfiguration = {IndexDocument:{Suffix:'index.html'}, RoutingRules: 50 rules of the form {Condition:{KeyPrefixEquals:'oldNN/'}, Redirect:{ReplaceKeyPrefixWith:'newNN/'}}} — accepted; GetBucketWebsite returns the 50 rules in order.
3. PutBucketWebsite again with the same IndexDocument and 51 such rules. The call is refused 400 InvalidRequest (correct), but Error.Message reads 'The number of routing rules must not exceed 50.' instead of '51 routing rules provided, the number of routing rules in a website configuration is limited to 50.' — i.e. AWS's fixed text with the request's rule count in the leading position.
4. GetBucketWebsite again and confirm the 50 stored rules are still in place (this part is already correct).
