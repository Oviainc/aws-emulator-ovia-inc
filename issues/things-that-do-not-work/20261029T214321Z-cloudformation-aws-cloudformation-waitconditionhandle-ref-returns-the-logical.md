Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.create-stack-reaches-complete cloudformation.ref-intrinsic-resolves cloudformation.update-stack-add-resource cloudformation.signal-resource-completes-wait-condition cloudformation.wait-condition-count-and-data-signal-accounting
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.create-stack-reaches-complete|run#10|AssertionError|assert handle_url and handle_url.startswith("https://") and "s3" in handle_url, (
Issue-Key: cloudformation.ref-intrinsic-resolves|run#11|AssertionError|assert parsed.scheme in ("http", "https"), f"expected an http(s) presigned URL, got scheme {parsed.scheme!r} in {handle_url!r}"
Issue-Key: cloudformation.update-stack-add-resource|run#12|AssertionError|assert physical_id.startswith("https://"), (
Issue-Key: cloudformation.signal-resource-completes-wait-condition|_signal_handle_with_retry#9|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: cloudformation.wait-condition-count-and-data-signal-accounting|_signal#14|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: AWS::CloudFormation::WaitConditionHandle Ref returns the logical resource name, not a presigned S3 URL, so every signal path is unusable (1 cause)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new behavioral root cause of service `cloudformation`, grounded in five failing behavioral checks. A fixer must close it before submitting.

Cause 1 of 1: the physical resource id the emulator assigns to an AWS::CloudFormation::WaitConditionHandle is a generated stack-scoped NAME, so Ref on the handle yields that name instead of the presigned S3 URL AWS returns.

AWS: the Smithy model (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) does not model CloudFormation resource types, so this is recorded behavior only. For AWS::CloudFormation::WaitConditionHandle, the resource's physical id — and therefore `Ref` on it, the `PhysicalResourceId` in DescribeStackResource/DescribeStackResources/ListStackResources, and any stack Output built from it — is a presigned HTTPS S3 URL (scheme `https`, an S3 host, and the query string carrying the presigned parameters). A client is expected to be able to HTTP PUT a signal document to that URL. The fixed part of the contract is the SHAPE: an absolute http(s) URL whose host is an S3 endpoint of the stack's region; the bucket, key and signature parameters are per-run values.

Emulator: the handle's physical id is the logical-name-derived string `<stack-name>-<LogicalId>-<suffix>` (e.g. a value of the form `sim2real-behav-stk-41302eaa2491-Handle-6AD8973F783A`); the exact token differs every run and is not the requirement — the requirement is that it be a presigned URL and it is not one. `urlparse()` of it yields an EMPTY scheme. Two checks that then treat the Ref value as a URL and PUT to it are refused by the harness's egress guard with `simparity.egress.EgressRefused: egress refused: '<handle value>' is not the endpoint under test (127.0.0.1)` — the emulator announced an address other than the endpoint it is reached on, which is this same defect observed one hop later.

Why: the WaitConditionHandle contract IS the URL. A client relying on AWS's behavior reads the Ref/Output and PUTs `{"Status":"SUCCESS","UniqueId":...,"Data":...}` to it to release an AWS::CloudFormation::WaitCondition. Against the emulator there is nothing to PUT to, so CreationPolicy/WaitCondition-gated stacks — the standard pattern for EC2 user-data and bootstrap signalling, and for holding a stack in *_IN_PROGRESS — cannot be driven at all, and the counted/Data-carrying signal accounting a WaitCondition reports after release is unreachable.

Fix: the provisioner/physical-id assignment for AWS::CloudFormation::WaitConditionHandle in the cloudformation service's resource-provider layer (it must mint and store a presigned S3 PUT URL as the handle's physical id, and the serializers for Ref/GetAtt, DescribeStackResource(s), ListStackResources and stack Outputs must surface that stored value), plus the handle-URL ingest path that accepts the PUT and feeds the WaitCondition's signal accounting. No existing open issue covers this: 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md is about resource types that do not deploy at all, and 20261029T065537Z-cloudformation-an-unsignalled-aws-cloudformation-waitcondition-vanishes-from.md is about an unsignalled WaitCondition disappearing from the resource list, not about the handle's physical id.

Traces: cloudformation.create-stack-reaches-complete, cloudformation.ref-intrinsic-resolves and cloudformation.update-stack-add-resource assert the URL shape directly; cloudformation.signal-resource-completes-wait-condition and cloudformation.wait-condition-count-and-data-signal-accounting fail one hop later when the non-URL value is used as a host.

## Evidence
cloudformation.create-stack-reaches-complete [mismatch]
  AssertionError: Expected Handle PhysicalResourceId to be a presigned S3 URL, got 'sim2real-behav-stk-41302eaa2491-Handle-6AD8973F783A'

cloudformation.ref-intrinsic-resolves [mismatch]
  AssertionError: expected an http(s) presigned URL, got scheme '' in 'sim2real-behav-stk-1a98bbacf960-Handle-20DA8A7C17B3'

cloudformation.update-stack-add-resource [mismatch]
  AssertionError: Expected Handle2 PhysicalResourceId to be a presigned URL, got 'sim2real-behav-stk-abdeaedf826d-Handle2-8A95DE51AB1B'

cloudformation.signal-resource-completes-wait-condition [error]
  simparity.egress.EgressRefused: egress refused: 'sim2real-signalres-ea0a2a977760-Handle-3ADDB1864EAD' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

cloudformation.wait-condition-count-and-data-signal-accounting [error]
  simparity.egress.EgressRefused: egress refused: 'sim2real-wc-count2-68f016cd08f8-Handle-1D26E134D1B7' is not the endpoint under test (127.0.0.1); ...

The per-run token (`sim2real-...-Handle-<hex>`) fills no template in the requirement; the requirement is scheme `https` + an S3 host + presigned query parameters. Smithy consulted: no CloudFormation resource type is modelled, so the URL contract is recorded behavior only; the affected response members are PhysicalResourceId (StackResource/StackResourceDetail/StackResourceSummary) and Output.OutputValue, none of which constrain the value in the model.

## Steps to Reproduce
1. CreateStack with TemplateBody {"Resources": {"Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"}}, "Outputs": {"HandleUrl": {"Value": {"Ref": "Handle"}}}} and wait for CREATE_COMPLETE.
2. DescribeStackResource(StackName=<stack>, LogicalResourceId='Handle') and read PhysicalResourceId; DescribeStacks and read Outputs[HandleUrl].OutputValue.
3. Observe a name of the form '<stack>-Handle-<hex>' with an empty URL scheme. AWS returns a presigned https S3 URL that can be PUT to.
4. Add a second handle via UpdateStack and repeat step 2 — same divergence (cloudformation.update-stack-add-resource).
5. Drive the full pattern: a stack with an AWS::CloudFormation::WaitCondition whose Handle is the Ref above, then HTTP PUT {"Status":"SUCCESS","UniqueId":"1","Data":"ok"} to the Ref value. Against AWS the WaitCondition releases and GetAtt Data reports the signal; against the emulator there is no URL to PUT to.
6. Re-replay: cloudformation.create-stack-reaches-complete, cloudformation.ref-intrinsic-resolves, cloudformation.update-stack-add-resource, cloudformation.signal-resource-completes-wait-condition, cloudformation.wait-condition-count-and-data-signal-accounting.
