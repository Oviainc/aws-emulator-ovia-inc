Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails
Issue-Key: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails|pull#19|Timeout|for msg in resp.get("Messages", []):
Issue-Commit: d737b8daf22219fa16f43304898be50ead051f23

# cloudformation__sns__sqs: a custom-resource provider response over 4,096 bytes is never rejected, so no CREATE_FAILED "Response object is too long." and no rollback Delete is ever published — the provider loop hangs

> _Note: this issue was identified against the code base at commit `d737b8daf22219fa16f43304898be50ead051f23`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in the single surviving failing check of this service that the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: an oversized custom-resource provider response is accepted silently, and the failure-path Delete it owes is never driven.

AWS: recorded behavior only, against the documented custom-resource response protocol (CloudFormation User Guide, crpg-ref). A provider answers a `Custom::*` request by PUTting a response document at the request's presigned `ResponseURL`. A response document whose serialized body is at most 4,096 bytes is read normally and completes the resource; a body of 4,097 bytes or more is one CloudFormation cannot read a `PhysicalResourceId` out of, so it FAILS that resource: the resource records `ResourceStatus` CREATE_FAILED (Smithy: com.amazonaws.cloudformation#ResourceStatus includes CREATE_FAILED; com.amazonaws.cloudformation#StackStatus includes ROLLBACK_IN_PROGRESS / ROLLBACK_COMPLETE) with `ResourceStatusReason` exactly `Response object is too long.` — entirely fixed text, no request- or state-derived parts — and the stack rolls back to ROLLBACK_COMPLETE. CloudFormation itself drives the rollback with no further client call: it still asks the provider to Delete the failed resource, and because it has no provider-supplied id for it, that Delete carries a CloudFormation-assigned physical id of the form `<StackName>-<LogicalId>-<SUFFIX>` (the stack name and logical id come from the request, the suffix is generated); that Delete is published BEFORE any Delete for a resource the failed one `DependsOn`.

Emulator: the oversized PUT is accepted at the ResponseURL and nothing further happens. After answering the 4,097-byte response the check polls only the provider's queue — it makes no intervening CloudFormation call — and no Delete request for the failed resource ever arrives, so the check consumed its whole window (`Error: Check timed out after 60.0s`). Because it blocked there, every downstream requirement it asserts is unverified by this replay: the `Response object is too long.` reason text, the CloudFormation-assigned id on the failure-path Delete, the failed-resource-before-its-dependency delete ordering, and the terminal ROLLBACK_COMPLETE. Hypothesis: either the 4,096-byte response-size check is not applied at all (so no failure, hence no Delete, is ever owed), or the work a landed response owes is only built while a CloudFormation API call is being served (the emulator's own note in `state::provisioning::advance_stabilisations`), so nothing publishes it between calls.

Why: a client whose provider returns a response larger than the protocol allows is told nothing — the stack reports progress or success instead of failing the resource — and the normal provider loop (answer a request, then wait for the next one CloudFormation owes) hangs indefinitely. The half-created external resource is never asked to be deleted, so it leaks, and the oversized response is never reported anywhere a client can see it.

Fix: the custom-resource response handling plus the provisioning driver — enforce the 4,096-byte bound on the response body received at the ResponseURL and route the violation through the resource-failure/rollback path that records `ResourceStatusReason`, and make the follow-up provider request (the failure-path Delete, in the dependency order the rollback requires) be driven by the emulator itself rather than by the client's next CloudFormation call.

Related open issues name the same seam but do not cover this check in this replay: `20261105T141713Z-cloudformation-sns-sqs-4-net-new-root-causes.md` lists this check id in its Issue-Trace-Ids (cause 4) and `20261105T040000Z-cloudformation-6-of-the-7-custom-resource-and-hook-keys-remain-open.md` describes the missing between-calls driver, but the replay report marked this check neither OWNED nor ALREADY FILED — its replay key no longer matches (the run now reports only the harness timeout, with no frame) — so it is re-grounded here. `20261101T212352Z-cloudformation-sns-sqs-a-custom-resource-whose-servicetoken.md` covers the SNS-backed provider never being invoked at all, which is no longer what this check shows.

Traces: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails.

## Evidence
Cause 1 — cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails [mismatch], detail: `Error: Check timed out after 60.0s`.

Expected: after a provider response body of exactly 4,096 bytes completes `Fits` (CREATE_COMPLETE) and a body of 4,097 bytes is PUT for `Overflows` (which `DependsOn: Fits`), (a) a Delete provider request for `Overflows` arrives on the subscribed queue whose `PhysicalResourceId` matches `<StackName>-Overflows-[A-Z0-9]+`, (b) no Delete for `Fits` is pending at that moment, (c) the stack settles at `StackStatus` ROLLBACK_COMPLETE, and (d) DescribeStackEvents shows exactly one `Overflows` CREATE_FAILED event with `ResourceStatusReason` == `Response object is too long.`

Actual: the oversized PUT returned 200 and no further provider request was ever delivered to the queue; the check ran out its 60 s window waiting on SQS after answering, making no intervening CloudFormation call. No CREATE_FAILED event and no `Response object is too long.` reason were observable. (The ids and stack names in any single run are per-run values; the requirement is the shape of the reason text and of the CloudFormation-assigned id, plus the delete ordering.)

## Steps to Reproduce
Setup (the provider): create an SNS topic; create an SQS queue; set the queue's `Policy` to allow `sns.amazonaws.com` `sqs:SendMessage` on the queue ARN with `Condition.ArnEquals.aws:SourceArn` = the topic ARN; `Subscribe` the queue ARN with Protocol=sqs and Attributes `{"RawMessageDelivery": "true"}`. Each custom-resource request then arrives as a raw queue message and is answered by PUTting a response document at the request's `ResponseURL`.

1. `CreateStack` with Resources: `Fits` = `Custom::Sized {ServiceToken: <topic arn>, ServiceTimeout: "120"}` and `Overflows` = `Custom::Sized {ServiceToken: <topic arn>, ServiceTimeout: "120"}` with `DependsOn: Fits`.
2. Read the `Fits` Create request off the queue and PUT a SUCCESS response whose serialized body is padded to exactly 4096 bytes. Poll `DescribeStackResource` until `Fits` is CREATE_COMPLETE.
3. Read the `Overflows` Create request and PUT a SUCCESS response padded to exactly 4097 bytes (the PUT is accepted, HTTP 200).
4. Poll ONLY the queue (make no further CloudFormation call): no Delete request for `Overflows` ever arrives. Expected: a Delete for `Overflows` with a `PhysicalResourceId` of `<StackName>-Overflows-<SUFFIX>`, and none for `Fits` yet.
5. Then `DescribeStacks` / `DescribeStackEvents`: the stack does not reach ROLLBACK_COMPLETE via an `Overflows` CREATE_FAILED whose `ResourceStatusReason` is `Response object is too long.`
