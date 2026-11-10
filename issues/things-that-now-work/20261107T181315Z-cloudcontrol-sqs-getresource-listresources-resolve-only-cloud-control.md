Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue
Issue-Key: cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue|_poll_until#8|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudcontrol__sqs: GetResource/ListResources resolve only Cloud Control's own private store — a queue created natively through SQS CreateQueue is never resolved, so read-through to the owning service does not exist

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
ONE net-new root cause spanning the Cloud Control API read path for AWS::SQS::Queue. A fixer must close it before submitting.

Cause 1 of 1: GetResource (and, behind the same store, ListResources) resolves an identifier only if Cloud Control itself provisioned that resource. A queue created directly through SQS CreateQueue — which SQS then reports normally through GetQueueAttributes — is refused by GetResource indefinitely, so Cloud Control cannot read, enumerate, patch or delete resources it did not create.

AWS: Smithy (com.amazonaws.cloudcontrol#GetResource) models GetResource with required TypeName (length 10-196, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and required Identifier (length 1-1024, pattern ^.+$), returning a ResourceDescription, with ResourceNotFoundException (errorKind client, httpError 404) as the absent-resource refusal; ListResources is paginated (NextToken/MaxResults/ResourceDescriptions) with the same error set. The GetResource documentation states it returns information about an existing resource in your account and Region "whether those resources were provisioned using Cloud Control API" — read-through to the owning service is the modeled contract, not an optimisation. So for a queue created by SQS CreateQueue with Attributes {"VisibilityTimeout": "<v>"}: GetResource(TypeName="AWS::SQS::Queue", Identifier=<queue URL>) returns 200 with ResourceDescription.Identifier equal to that queue URL and Properties carrying QueueName (the name in the URL), VisibilityTimeout (the value SQS reports, as a number) and Arn (the QueueArn SQS reports); ListResources(TypeName="AWS::SQS::Queue") enumerates that queue URL across its pages and does NOT enumerate a well-formed queue URL in the same account/region that names no queue; UpdateResource with PatchDocument [{"op":"replace","path":"/VisibilityTimeout","value":<w>}] reaches OperationStatus SUCCESS and SQS GetQueueAttributes then reports <w>; GetResource for the never-created URL is refused ResourceNotFoundException (404); and after DeleteResource reaches SUCCESS, SQS GetQueueUrl and SetQueueAttributes for that queue are refused with code "AWS.SimpleQueueService.NonExistentQueue" and message "The specified queue does not exist." (fixed text, no variable part).

Emulator: the check creates the queue through SQS (that call and the SQS GetQueueAttributes read of VisibilityTimeout/QueueArn both succeed, so the queue genuinely exists in the emulator's SQS state), then polls Cloud Control GetResource(TypeName="AWS::SQS::Queue", Identifier=<that queue URL>) every 3 s for 60 s and every attempt raises a ClientError — the probe never once returns a ResourceDescription. Replay output: AssertionError: waited 60.0s for Cloud Control GetResource to resolve '<queue name>', a queue it never created (bounded only for SQS's own eventual consistency) and it never happened; last observation None. Because the read never resolves, the later beats of the flow (ListResources enumerating the queue URL while excluding a phantom URL; UpdateResource writing /VisibilityTimeout through to SQS; GetResource answering ResourceNotFoundException for the phantom URL only; DeleteResource making SQS report the queue as NonExistentQueue) are never reached.

Why: Cloud Control is sold and modeled as a uniform read/write control plane over the resources in an account however they were provisioned. A client that inventories, tags, drift-checks or patches pre-existing infrastructure through Cloud Control sees an empty or partial account: a live queue is reported as not found and omitted from listings, so sweeps silently act on nothing and ResourceNotFoundException stops distinguishing "absent" from "not created by me".

Fix: the Cloud Control read path for AWS::SQS::Queue — the GetResource and ListResources handlers and the resource lookup in the cloudcontrol crate's state — must resolve and enumerate from SQS's own queue state (and the write path must patch through to it), instead of consulting a private index of self-created resources and otherwise returning ResourceNotFoundException.

Related but not covering: open issue 20261102T044041Z-cloudcontrol-sqs-2-net-new-causes-cloud-control.md was filed from a run of this same check that was killed at the harness's 60 s cap with no explicit assertion, and its replay key is that harness timeout; this run produces the check's own bounded-poll AssertionError at the GetResource read-through beat and the suite did not mark the check owned, so it is reported here. 20261102T095426Z-cloudcontrol-6-net-new-causes-cloud-control-s.md describes the same private-store/owning-service split for Logs, CloudWatch, SQS and SSM but not this read-through flow's assertions. If one shared join between the Cloud Control store and the owning services lands, this cause closes with it.

Traces: cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue.

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator. Generated queue names, queue URLs, ARNs and timings are per-run values, not requirements.

[mismatch] cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue —
  File "check_cloudcontrol__sqs_cloudcontrol_reads_and_writes_through_to_a_natively_created_queue.py", line 64, in run
    described = _poll_until(...)
  File "...", line 204, in _poll_until
    raise AssertionError(...)
  AssertionError: waited 60.0s for Cloud Control GetResource to resolve 'sim2real-behav-ccthru-<suffix>', a queue it never created (bounded only for SQS's own eventual consistency) and it never happened; last observation None

The probe is cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<queue URL>)["ResourceDescription"], returning None on ClientError; "last observation None" means every attempt over 60 s was refused.

Prerequisites that already hold in this run: sqs.create_queue(QueueName=<n>, Attributes={"VisibilityTimeout": "45"}) returns a QueueUrl, and sqs.get_queue_attributes(QueueUrl=<that URL>, AttributeNames=["VisibilityTimeout","QueueArn"]) returns Attributes with VisibilityTimeout "45" and a QueueArn ending in ":" + the queue name. The queue exists in SQS.

Expected, from the point of divergence:
1. cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<queue URL>) returns 200; ResourceDescription.Identifier == the queue URL; json.loads(Properties) has QueueName == the queue's name, VisibilityTimeout == the number SQS reports, Arn == SQS's QueueArn.
2. cc.list_resources(TypeName="AWS::SQS::Queue") enumerates that queue URL across its pages, and does not enumerate a well-formed-but-never-created queue URL in the same account/region.
3. cc.update_resource(TypeName="AWS::SQS::Queue", Identifier=<queue URL>, PatchDocument=[{"op":"replace","path":"/VisibilityTimeout","value":75}]) reaches OperationStatus "SUCCESS" and sqs.get_queue_attributes then reports "75".
4. cc.get_resource for the never-created queue URL is refused HTTP 404 ResourceNotFoundException.
5. cc.delete_resource for the queue reaches "SUCCESS", after which sqs.get_queue_url and sqs.set_queue_attributes for it are refused with code "AWS.SimpleQueueService.NonExistentQueue" and message "The specified queue does not exist."

Actual: step 1 never succeeds — GetResource refuses the identifier of a live SQS queue for the whole 60 s window — so steps 2-5 are never exercised.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol"), sqs = client("sqs").

1. url = sqs.create_queue(QueueName=n, Attributes={"VisibilityTimeout": "45"})["QueueUrl"]; confirm sqs.get_queue_attributes(QueueUrl=url, AttributeNames=["VisibilityTimeout", "QueueArn"]) reports VisibilityTimeout "45" and a QueueArn. (Both already hold.)
2. THE DIVERGENCE: cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=url) must return a ResourceDescription whose Identifier is url and whose Properties carry QueueName=n, VisibilityTimeout=45 and Arn=<the QueueArn from step 1>. Today every attempt is refused for at least 60 s.
3. cc.list_resources(TypeName="AWS::SQS::Queue"), paging NextToken to exhaustion, must contain url and must NOT contain <url's prefix>/<a name nothing ever created>.
4. cc.update_resource(TypeName="AWS::SQS::Queue", Identifier=url, PatchDocument=json([{"op":"replace","path":"/VisibilityTimeout","value":75}])); poll get_resource_request_status to a terminal state — it must be SUCCESS — then sqs.get_queue_attributes(QueueUrl=url, AttributeNames=["VisibilityTimeout"]) must report "75".
5. cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<the never-created URL>) must be refused HTTP 404 ResourceNotFoundException, and sqs.get_queue_url for that name must be refused ("AWS.SimpleQueueService.NonExistentQueue", "The specified queue does not exist.").
6. cc.delete_resource(TypeName="AWS::SQS::Queue", Identifier=url) must reach SUCCESS, after which sqs.get_queue_url(QueueName=n) and sqs.set_queue_attributes(QueueUrl=url, ...) must both be refused with ("AWS.SimpleQueueService.NonExistentQueue", "The specified queue does not exist.").
