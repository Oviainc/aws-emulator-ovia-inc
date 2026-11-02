Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.intrinsic-cidr-ipv6-and-parameter-block-outputs-exact cloudformation.intrinsic-foreach-expands-resources-and-outputs cloudformation.intrinsic-foreach-invalid-collection-fails-transform cloudformation.intrinsic-length-and-to-json-string-resolve-nested-intrinsics cloudformation.template-init-metadata-round-trips-describe-stack-resource cloudformation.template-interface-metadata-in-summary cloudformation.template-resource-count-limit cloudformation.stack-events-golden-create-update-delete cloudformation.template-wait-condition-failure-signal-fails-create
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.intrinsic-cidr-ipv6-and-parameter-block-outputs-exact|run#9|AssertionError|assert outputs == expected, f"Fn::Cidr outputs differ: expected {expected!r}, got {outputs!r}"
Issue-Key: cloudformation.intrinsic-foreach-expands-resources-and-outputs|run#6|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation.intrinsic-foreach-invalid-collection-fails-transform|run#9|AssertionError|assert failed["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.intrinsic-length-and-to-json-string-resolve-nested-intrinsics|run#10|AssertionError|assert outputs == expected, f"outputs differ: expected {expected!r}, got {outputs!r}"
Issue-Key: cloudformation.template-init-metadata-round-trips-describe-stack-resource|run#8|AssertionError|assert json.loads(before["Metadata"]) == _expected("v1", name), f"Metadata after create: {before['Metadata']!r}"
Issue-Key: cloudformation.template-interface-metadata-in-summary|run#7|AssertionError|assert json.loads(deployed["Metadata"]) == _EXPECTED_METADATA, (
Issue-Key: cloudformation.template-resource-count-limit|run#6|AssertionError|assert summary["ResourceTypes"] == expected_types, (
Issue-Key: cloudformation.stack-events-golden-create-update-delete|run#17|AssertionError|assert _rows(got) == want, f"{label} events: expected {want!r}, got {_rows(got)!r}"
Issue-Key: cloudformation.template-wait-condition-failure-signal-fails-create|run#5|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation: 9 net-new root causes — intrinsics (Fn::Cidr, Fn::ForEach, Fn::ToJsonString), Metadata omitted from resource/summary views, deduplicated ResourceTypes, and spurious update events

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplateSummary

## Description
Nine distinct net-new root causes in template processing and the template/resource read surface. Open issue 20261029T214321Z-cloudformation-aws-cloudformation-waitconditionhandle-ref-returns-the-logical.md covers the WaitConditionHandle Ref symptom for other replay keys but not the one in Cause 8.

Cause 1 of 9: Fn::Cidr compresses IPv6 blocks.
AWS: recorded behavior only — Fn::Cidr over an IPv6 block returns each /64 with every group written out, e.g. '2001:db8:1234:5600:0:0:0:0/64'.
Emulator: returns the RFC-5952-compressed form '2001:db8:1234:5600::/64'. IPv4 output matches.
Why: a client that string-compares or splits the output against AWS's form (subnet properties, outputs) mismatches.
Fix: the Fn::Cidr IPv6 formatter in the intrinsic evaluator.
Traces: cloudformation.intrinsic-cidr-ipv6-and-parameter-block-outputs-exact.

Cause 2 of 9: the `&{Identifier}` loop-reference form of Fn::ForEach is unsupported.
AWS: recorded behavior only — inside Fn::ForEach, `&{Slug}` substitutes the loop value in logical ids and the stack reaches CREATE_COMPLETE with one resource and one output per collection member.
Emulator: the stack ends CREATE_FAILED with reason 'Fn::ForEach::BySlug: the &{Slug} form of the loop reference is not one this service can apply'.
Why: any Language-Extensions template using the documented loop-reference syntax cannot deploy.
Fix: Fn::ForEach expansion in the template transform stage.
Traces: cloudformation.intrinsic-foreach-expands-resources-and-outputs.

Cause 3 of 9: an invalid Fn::ForEach collection fails on the wrong path.
AWS: recorded behavior only — a collection that is a plain string (not a list) fails the transform and the stack rolls back.
Emulator: the stack ends CREATE_FAILED with 'Fn::ForEach::Handles: the collection is not a list this service can resolve from the template alone' and no rollback.
Why: the caller sees a non-rolled-back stack and an emulator-specific reason rather than a transform failure.
Fix: Fn::ForEach collection validation in the transform stage + rollback on transform failure.
Traces: cloudformation.intrinsic-foreach-invalid-collection-fails-transform.

Cause 4 of 9: Fn::ToJsonString reorders object keys.
AWS: recorded behavior only — Fn::ToJsonString serialises the object in template order, e.g. {"stack":...,"port":...,"hosts":[...],"first":...,"nested":{...}}.
Emulator: emits keys sorted alphabetically ({"first":...,"hosts":...,"nested":...,"port":...,"stack":...}).
Why: any consumer that hashes or byte-compares the JSON (config payloads, signatures, CDK asset hashes) diverges.
Fix: the Fn::ToJsonString serializer — preserve insertion order.
Traces: cloudformation.intrinsic-length-and-to-json-string-resolve-nested-intrinsics.

Cause 5 of 9: DescribeStackResource omits Metadata.
AWS: Smithy: StackResourceDetail has a Metadata member — 'The content of the Metadata attribute declared for the resource'. AWS sends exactly [DriftInformation, LastUpdatedTimestamp, LogicalResourceId, Metadata, PhysicalResourceId, ResourceStatus, ResourceType, StackId, StackName].
Emulator: the same set minus Metadata, so a resource's AWS::CloudFormation::Init block never round-trips.
Why: cfn-init consumers read their configuration from this field and get nothing.
Fix: the DescribeStackResource serializer / resource state.
Traces: cloudformation.template-init-metadata-round-trips-describe-stack-resource, cloudformation.template-interface-metadata-in-summary (GetTemplateSummary's Metadata member, modelled as GetTemplateSummaryOutput.Metadata, is likewise absent for the template's AWS::CloudFormation::Interface block).

Cause 6 of 9: GetTemplateSummary deduplicates ResourceTypes.
AWS: Smithy: GetTemplateSummaryOutput.ResourceTypes; AWS reports one entry per resource in the template, so two AWS::SSM::Parameter resources yield two entries (['AWS::SNS::Topic','AWS::SQS::Queue','AWS::SSM::Parameter','AWS::SSM::Parameter']).
Emulator: returns the distinct set (one entry per type), reporting 1 entry where a 500-resource template must report 500.
Why: callers that count resources from the summary (quota pre-checks) under-count.
Fix: the GetTemplateSummary handler's ResourceTypes projection.
Traces: cloudformation.template-resource-count-limit.

Cause 7 of 9: the update event stream re-creates resources the update did not change and omits the cleanup phase.
AWS: recorded behavior only — an update that touches one resource emits [stack UPDATE_IN_PROGRESS 'User Initiated', ('Param','UPDATE_IN_PROGRESS'), ('Param','UPDATE_COMPLETE'), stack UPDATE_COMPLETE_CLEANUP_IN_PROGRESS, stack UPDATE_COMPLETE].
Emulator: inserts ('Handle','CREATE_IN_PROGRESS'), ('Handle','CREATE_IN_PROGRESS','Resource creation Initiated'), ('Handle','CREATE_COMPLETE') for an untouched resource and never emits UPDATE_COMPLETE_CLEANUP_IN_PROGRESS.
Why: event-driven consumers see phantom resource churn and miss the cleanup phase.
Fix: the update planner (no-change resources) and event emission.
Traces: cloudformation.stack-events-golden-create-update-delete.

Cause 8 of 9: AWS::CloudFormation::WaitConditionHandle's physical id is not a presigned URL.
AWS: recorded behavior only — the handle's PhysicalResourceId is an https presigned S3 URL that signallers POST to.
Emulator: PhysicalResourceId is the generated logical name '<stack>-Handle-<suffix>' (scheme ''), so there is no data-plane endpoint to signal.
Why: cfn-signal and any wait-condition workflow cannot report success, so the create can only time out.
Fix: WaitConditionHandle provisioning (mint and publish a signalling URL).
Traces: cloudformation.template-wait-condition-failure-signal-fails-create.

Cause 9 of 9: see Cause 5 — GetTemplateSummary's Metadata omission is filed there with its own replay key (cloudformation.template-interface-metadata-in-summary) and must be fixed in the GetTemplateSummary serializer as well as DescribeStackResource.

## Evidence
C1: expected V6 '2001:db8:1234:5600:0:0:0:0/64,...', got '2001:db8:1234:5600::/64,...'.
C2: stack CREATE_FAILED, reason 'Fn::ForEach::BySlug: the &{Slug} form of the loop reference is not one this service can apply'.
C3: stack CREATE_FAILED, reason 'Fn::ForEach::Handles: the collection is not a list this service can resolve from the template alone'; expected a rollback.
C4: expected Config '{"stack":...,"port":...,"hosts":[...],"first":...,"nested":{...}}', got '{"first":...,"hosts":[...],"nested":{...},"port":...,"stack":...}'.
C5: 'StackResourceDetail for \'Bare\' carries keys [DriftInformation, LastUpdatedTimestamp, LogicalResourceId, PhysicalResourceId, ResourceStatus, ResourceType, StackId, StackName], real AWS sends exactly [... Metadata ...]'; and 'Metadata from the stack: <the AWS::CloudFormation::Interface block>' for GetTemplateSummary.
C6: 'GetTemplateSummary reports ResourceTypes [AWS::SNS::Topic, AWS::SQS::Queue, AWS::SSM::Parameter]; real AWS reports one entry per resource'; and 'GetTemplateSummary ResourceTypes: one entry per resource, not deduplicated; got 1 entries'.
C7: got [(stack,'UPDATE_IN_PROGRESS','User Initiated'),('Handle','CREATE_IN_PROGRESS',None),('Handle','CREATE_IN_PROGRESS','Resource creation Initiated'),('Handle','CREATE_COMPLETE',None),('Param','UPDATE_IN_PROGRESS',None),('Param','UPDATE_COMPLETE',None),(stack,'UPDATE_COMPLETE',None)].
C8: 'the published data-plane endpoint is not a URL with a host: \'<stack>-Handle-<suffix>\''.
Names/suffixes/uuids are per-run fillers.

## Steps to Reproduce
Replay the checks in trace_ids: 1) deploy a template with Fn::Cidr over an IPv6 block and read its outputs; 2) deploy a Language-Extensions template using Fn::ForEach with &{Identifier}; 3) deploy one whose Fn::ForEach collection is a string; 4) deploy one using Fn::ToJsonString over an object; 5) DescribeStackResource on a resource declaring Metadata / AWS::CloudFormation::Init and GetTemplateSummary on a template with AWS::CloudFormation::Interface; 6) GetTemplateSummary on a template with repeated resource types; 7) update one resource of a two-resource stack and read DescribeStackEvents; 8) create a stack with AWS::CloudFormation::WaitConditionHandle and read its PhysicalResourceId.
