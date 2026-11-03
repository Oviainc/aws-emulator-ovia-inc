Issue-Source: trace-sweep-divergence
Issue-Trace: s3-wsh-index-and-error-document-refusals-then-recover
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-wsh-index-and-error-document-refusals-then-recover s3-wsh-redirect-all-exclusivity-refusals-then-recover s3-wsh-put-replaces-whole-config-and-refusal-keeps-prior s3-wsh-routing-rule-refusals-then-recover
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketWebsite|4xx->2xx|$
Issue-Key: s3|PutBucketWebsite|4xx->2xx|status
Issue-Key: s3|GetBucketWebsite|4xx->2xx|$.Error
Issue-Key: s3|GetBucketWebsite|4xx->2xx|status
Issue-Key: s3|PutBucketWebsite|2xx->4xx|status
Issue-Key: s3|GetBucketWebsite|2xx->2xx|$.WebsiteConfiguration.IndexDocument
Issue-Key: s3|GetBucketWebsite|2xx->2xx|$.WebsiteConfiguration.RedirectAllRequestsTo
Issue-Key: s3|GetBucketWebsite|2xx->2xx|$.WebsiteConfiguration.RoutingRules
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 bucket website refusals and stored configuration diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketWebsite

## Description
Cause 1 of 6: the missing-IndexDocument refusal is re-worded and loses its argument context. AWS: 400 InvalidArgument, Message 'A value for IndexDocument Suffix must be provided if RedirectAllRequestsTo is empty' (fixed text), ArgumentName 'IndexDocument', ArgumentValue 'null'. Recorded behavior only. Emulator: Message 'WebsiteConfiguration must provide an IndexDocument unless it specifies RedirectAllRequestsTo' and no ArgumentName/ArgumentValue. Why: text-matching clients and operators reading the refusal see wording AWS never produces, and lose the member name. Fix: the PutBucketWebsite validator's InvalidArgument construction. Traces: s3-wsh-index-and-error-document-refusals-then-recover#task#2 (cluster of 7 steps).

Cause 2 of 6: a malformed ErrorDocument key is reported as MalformedXML. AWS: 400 InvalidArgument 'The ErrorDocument Key is not well formed' with ArgumentName 'ErrorDocument' and ArgumentValue the value as sent. Recorded behavior only. Emulator: 400 MalformedXML with the generic schema sentence. Fix: validate ErrorDocument after parsing. Traces: s3-wsh-index-and-error-document-refusals-then-recover#task#6.

Cause 3 of 6: genuine schema faults are reported as InvalidArgument with the IndexDocument sentence. AWS: 400 MalformedXML 'The XML you provided was not well-formed or did not validate against our published schema'. Recorded behavior only. Emulator: 400 InvalidArgument 'WebsiteConfiguration must provide an IndexDocument unless it specifies RedirectAllRequestsTo' - the wrong class AND a sentence about a different member. Fix: the same validator's fault classification. Traces: s3-wsh-index-and-error-document-refusals-then-recover#task#7 (cluster of 6 steps).

Cause 4 of 6: a routing rule with HttpErrorCodeReturnedEquals 200 is accepted. AWS: 400 Code 'InvalidRequest', Message 'The provided HTTP error code (200) is not valid. Valid codes are 4XX or 5XX.' - fixed text whose parenthesised value is the code from the request. Recorded behavior only. Emulator: 200 with an empty body. Why: an unservable routing rule is stored. Fix: routing-rule validation in the Put handler. Traces: s3-wsh-routing-rule-refusals-then-recover#task#5.

Cause 5 of 6: a bucket with no website configuration reads as 200. AWS: 404 Code 'NoSuchWebsiteConfiguration', Message 'The specified bucket does not have a website configuration' with <BucketName> = the bucket addressed. Recorded behavior only. Emulator: 200 with no Error. Why: a client cannot tell whether website hosting is configured. Fix: the GetBucketWebsite absence branch. Traces: s3-wsh-routing-rule-refusals-then-recover#task#8.

Cause 6 of 6: a valid Put is refused and the prior configuration is served instead. AWS: the Put answers 200 and wholly replaces the configuration, so the following read returns IndexDocument Suffix 'index.html' and the RoutingRules, with no RedirectAllRequestsTo. Recorded behavior only. Emulator: the Put answers 400 and the read still returns the earlier RedirectAllRequestsTo (HostName) with IndexDocument and RoutingRules absent. Why: website configuration cannot be updated, and the client sees a configuration it replaced. Fix: the PutBucketWebsite validator (over-rejection) and its whole-document replace in state. Traces: s3-wsh-put-replaces-whole-config-and-refusal-keeps-prior#task#10, s3-wsh-put-replaces-whole-config-and-refusal-keeps-prior#task#11.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-wsh-index-and-error-document-refusals-then-recover#task#2 (400/400): Message expected 'A value for IndexDocument Suffix must be provided if RedirectAllRequestsTo is empty' got 'WebsiteConfiguration must provide an IndexDocument unless it specifies RedirectAllRequestsTo'; ArgumentName 'IndexDocument' and ArgumentValue 'null' missing (cluster of 7). #task#6: Code expected 'InvalidArgument' got 'MalformedXML', Message expected 'The ErrorDocument Key is not well formed'. #task#7: Code expected 'MalformedXML' got 'InvalidArgument' (cluster of 6). s3-wsh-routing-rule-refusals-then-recover#task#5: expected 400 InvalidRequest 'The provided HTTP error code (200) is not valid. Valid codes are 4XX or 5XX.', got 200. #task#8: expected 404 NoSuchWebsiteConfiguration, got 200. s3-wsh-put-replaces-whole-config-and-refusal-keeps-prior#task#10: expected 200 got 400; #task#11: IndexDocument and RoutingRules missing, RedirectAllRequestsTo extra.

## Steps to Reproduce
1. Replay s3-wsh-index-and-error-document-refusals-then-recover and compare steps task#2, task#6, task#7. 2. Replay s3-wsh-routing-rule-refusals-then-recover steps task#5 and task#8. 3. Replay s3-wsh-put-replaces-whole-config-and-refusal-keeps-prior steps task#10 and task#11.
