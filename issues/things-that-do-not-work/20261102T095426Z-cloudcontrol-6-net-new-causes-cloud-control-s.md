Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cloudcontrol-list-resources-paginated-agrees-with-native-logs-inventory cloudcontrol.numeric-precision-preserved cloudcontrol.unicode-property-round-trip cloudcontrol.e2e-create-use-observe-delete cloudcontrol.e2e-update-changes-runtime-behavior cloudcontrol.cloudcontrol-concurrent-patches-native-version-count-equals-successes cloudcontrol.deploy-cfn-resource-behaves cloudcontrol.deploy-cfn-resource-behaves-post-create cloudcontrol.deploy-cfn-stack-deletion-removes-resource cloudcontrol.deploy-cfn-update-reflected-in-getresource cloudcontrol.deploy-cloudformation-resource-behaves cloudcontrol.deploy-cloudformation-stack-deletion-removes-resource cloudcontrol.deploy-cfn-immutable-property-update-rejected cloudcontrol.cloudcontrol-create-over-preexisting-native-resource-rejected-and-native-value-untouched cloudcontrol.cloudcontrol-desired-state-tags-are-applied-to-the-native-resource cloudcontrol.patch-create-only-non-identifier-property-refused-while-updatable-sibling-applies cloudcontrol.list-resources-resource-model-scopes-compound-identifier cloudcontrol.server-side-handler-failure-propagates-exact-exception cloudcontrol.server-side-computed-readonly-property-rejected-on-write cloudcontrol.server-side-enforced-policy-applied-without-client-input cloudcontrol.primary-identifier-accepted-as-json-derived-from-the-registry-schema
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.cloudcontrol-list-resources-paginated-agrees-with-native-logs-inventory|run#16|AssertionError|assert mine_cc == set(native), (
Issue-Key: cloudcontrol.numeric-precision-preserved|run#16|AssertionError|assert len(alarms) == 1, (
Issue-Key: sqs|ListQueueTags|AWS.SimpleQueueService.NonExistentQueue
Issue-Key: cloudcontrol|SendMessage|AWS.SimpleQueueService.NonExistentQueue
Issue-Key: ssm|GetParameterHistory|ParameterNotFound
Issue-Key: cloudcontrol.deploy-cfn-immutable-property-update-rejected|run#15|AssertionError|assert code == "NotUpdatableException", (
Issue-Key: cloudcontrol.cloudcontrol-create-over-preexisting-native-resource-rejected-and-native-value-untouched|run#8|AssertionError|assert terminal["OperationStatus"] == "FAILED", (
Issue-Key: cloudcontrol|ListTagsForResource|NotImplemented
Issue-Key: cloudcontrol.patch-create-only-non-identifier-property-refused-while-updatable-sibling-applies|run#9|AssertionError|assert queue_url.endswith("/" + queue_name), (
Issue-Key: cloudcontrol.list-resources-resource-model-scopes-compound-identifier|run#8|AssertionError|assert event.get("Identifier") == expected, (
Issue-Key: cloudcontrol.server-side-handler-failure-propagates-exact-exception|run#19|AssertionError|assert missing_identifier != identifier, f"ResourceId {resource_id!r} not found in {identifier!r}"
Issue-Key: cloudcontrol.server-side-computed-readonly-property-rejected-on-write|run#11|KeyError|real_arn = original_props["Arn"]
Issue-Key: cloudcontrol.server-side-enforced-policy-applied-without-client-input|run#12|AssertionError|assert pab is not None, (
Issue-Key: cloudcontrol.primary-identifier-accepted-as-json-derived-from-the-registry-schema|run#5|KeyError|pointers = schema["primaryIdentifier"]
Issue-Related: 20261102T095426Z-cloudcontrol-10-net-new-causes-cloud-control-refusal.md
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudcontrol: 6 net-new causes — Cloud Control's private store is not joined to the owning services (Logs, CloudWatch, SQS, SSM), resources created elsewhere are invisible, and Identifier/read-only properties are wrong

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
SIX net-new root causes, all in the cloudcontrol (Cloud Control API) service. A fixer must close all six. Two open issues already describe this split-store family for two type families only — 20261102T041105Z-cloudcontrol-dynamodb-... (AWS::DynamoDB::Table) and 20261102T044041Z-cloudcontrol-sqs-... (AWS::SQS::Queue provisioning + read-through of natively created queues) — and the three checks they own are skipped here. The causes below are the ones those two do not carry: other type families, resources created by CloudFormation, an unimplemented native tagging operation, the Identifier value, read-only properties, and the registry schema.

Cause 1 of 6: CreateResource/UpdateResource do not provision or mutate the resource in the owning service for type families beyond DynamoDB and SQS control-plane.
AWS: Smithy models CreateResource with required TypeName/DesiredState answering a ProgressEvent whose OperationStatus SUCCESS is documented as "The resource operation has successfully completed"; Cloud Control is a facade over the type's handler, so a SUCCESS means the resource exists in the owning service. Concretely: AWS::Logs::LogGroup creates appear in CloudWatch Logs DescribeLogGroups and accept CreateLogStream; AWS::CloudWatch::Alarm creates appear in DescribeAlarms; an AWS::SQS::Queue whose create SUCCEEDED accepts SendMessage and ListQueueTags on the returned queue URL, and a patched VisibilityTimeout changes the queue's runtime behavior; successive patches of an AWS::SSM::Parameter bump its native parameter version once per successful request (visible in GetParameterHistory).
Emulator: the effect is recorded only in Cloud Control's own store. After a SUCCESS, CloudWatch Logs reports no log groups (ListResources lists six that DescribeLogGroups does not have), DescribeAlarms finds 0 alarms for the alarm just created, SQS answers AWS.SimpleQueueService.NonExistentQueue ("The specified queue does not exist.") to SendMessage/ListQueueTags/GetQueueUrl on the identifier the SUCCESS event returned, and SSM answers ParameterNotFound to GetParameter/GetParameterHistory.
Why: Cloud Control exists so clients can provision without knowing the owning API, then USE the resource through that API. Reporting SUCCESS for a resource no service has makes every Cloud Control provisioning flow a silent no-op, and the FAILED/NotStabilizedException path that should report the failure never fires.
Fix: the request-effect materialisation in the cloudcontrol crate must apply each ResourceEffect to the state of the service that owns TypeName (CloudWatch Logs, CloudWatch, SQS, SSM, SNS, ...) before the request may reach SUCCESS, and terminate FAILED when it cannot.
Failing checks: cloudcontrol.cloudcontrol-list-resources-paginated-agrees-with-native-logs-inventory, cloudcontrol.numeric-precision-preserved, cloudcontrol.unicode-property-round-trip, cloudcontrol.e2e-create-use-observe-delete, cloudcontrol.e2e-update-changes-runtime-behavior, cloudcontrol.cloudcontrol-concurrent-patches-native-version-count-equals-successes

Cause 2 of 6: GetResource/ListResources do not resolve resources that Cloud Control did not itself create — including everything CloudFormation provisioned.
AWS: the GetResource doc states it returns information about an existing resource in your account and Region "whether those resources were provisioned using Cloud Control API". So a queue created by a CloudFormation stack resolves through GetResource under its primary identifier (the queue URL CloudFormation reports as PhysicalResourceId), reflects later stack updates, disappears after the stack is deleted, and a create over a pre-existing native resource is refused as already existing (handler error code AlreadyExists).
Emulator: GetResource answers ResourceNotFoundException (404) for every CloudFormation-created queue — "Resource of type 'AWS::SQS::Queue' with identifier '<queue url>' was not found." — because the lookup only consults Cloud Control's private store; and a CreateResource over a pre-existing native SSM parameter reaches SUCCESS instead of failing, overwriting nothing but reporting success. A create-only patch against a CFN-created queue is likewise refused ResourceNotFoundException where NotUpdatableException is required.
Why: Cloud Control is the uniform read/write API over existing infrastructure; an account's real resources are invisible to it, so inventory, drift detection, tagging sweeps and patch workflows act on an empty account while reporting no error, and a create cannot detect a clash with a resource that already exists.
Fix: the GetResource/ListResources lookup (and the already-exists check in CreateResource) must resolve and enumerate from the owning service's state rather than from Cloud Control's private index. The sibling cause for natively created SQS queues is already filed as Cause 2 of 20261102T044041Z-cloudcontrol-sqs-...; this cause is the CloudFormation-provisioned and pre-existing-resource half, which that issue does not carry.
Failing checks: cloudcontrol.deploy-cfn-resource-behaves, cloudcontrol.deploy-cfn-resource-behaves-post-create, cloudcontrol.deploy-cfn-stack-deletion-removes-resource, cloudcontrol.deploy-cfn-update-reflected-in-getresource, cloudcontrol.deploy-cloudformation-resource-behaves, cloudcontrol.deploy-cloudformation-stack-deletion-removes-resource, cloudcontrol.deploy-cfn-immutable-property-update-rejected, cloudcontrol.cloudcontrol-create-over-preexisting-native-resource-rejected-and-native-value-untouched

Cause 3 of 6: the native ListTagsForResource the tag assertion needs is an unimplemented scaffold stub.
AWS: tags in DesiredState are applied to the real resource, so the owning service's tag-listing operation returns them. recorded behavior only for this check's path: the replay never gets an AWS-shaped answer because the emulator refuses the call outright.
Emulator: the call fails with "An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator" — a 'NotImplemented' error code that AWS never returns for a valid request.
Why: tags set through Cloud Control cannot be observed at all, so no client or audit tool can verify them; and the NotImplemented code is not in any modeled error set, so SDK error handling cannot classify it.
Fix: implement the owning service's ListTagsForResource handler (the scaffold stub reached by this check) and have the Cloud Control create path apply DesiredState Tags to the native resource.
Failing check: cloudcontrol.cloudcontrol-desired-state-tags-are-applied-to-the-native-resource

Cause 4 of 6: ProgressEvent.Identifier / ResourceDescription.Identifier is not the type's primary identifier.
AWS: ProgressEvent.Identifier is documented as "The primary identifier for the resource", taken from the type's registry schema primaryIdentifier. For AWS::SQS::Queue that is the queue URL (ending "/<QueueName>"); for a type with a compound primary identifier (AWS::Logs::LogStream) the segments are joined with the '|' delimiter ("<LogGroupName>|<LogStreamName>"); for AWS::ApplicationAutoScaling::ScalableTarget it embeds the ResourceId ("table/<TableName>").
Emulator: the Identifier is the resource NAME for queues (not a URL), only the last segment for a compound identifier, and a freshly minted uuid for the ScalableTarget — e.g. Identifier 'd67f8ea6-044e-4043-aa52-85ec9c5fe499' for ResourceId 'table/<table>'.
Why: the Identifier is what a client feeds back into GetResource/UpdateResource/DeleteResource and into the owning service's API; a name where a URL/ARN/compound key is required makes the returned handle unusable and cannot be matched against resources listed elsewhere.
Fix: derive the Identifier from the type's registry schema primaryIdentifier (joining multiple pointers with '|') in the create/update/list/get paths. Related: the open issue 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent... covers ARN-identifier types (IAM ManagedPolicy, StepFunctions StateMachine) only; the queue-URL, compound and ScalableTarget forms here are not in it.
Failing checks: cloudcontrol.patch-create-only-non-identifier-property-refused-while-updatable-sibling-applies, cloudcontrol.list-resources-resource-model-scopes-compound-identifier, cloudcontrol.server-side-handler-failure-propagates-exact-exception

Cause 5 of 6: GetResource Properties omit read-only and server-applied properties.
AWS: ResourceDescription.Properties is the resource's full current model, which includes the schema's readOnlyProperties computed by the owning service (AWS::SQS::Queue /properties/Arn) and properties the service applies without client input (AWS::S3::Bucket PublicAccessBlockConfiguration, from the account-wide Block Public Access default that AWS enables for new buckets).
Emulator: Properties carries back only what DesiredState set — the queue's Arn key is absent (the check fails with KeyError: 'Arn') and the bucket reports no PublicAccessBlockConfiguration.
Why: clients read the Arn (and other read-only values) from GetResource to wire the resource into policies and references; an absent key breaks them, and a server-enforced security default that is invisible makes the API's view of the resource materially wrong. It also makes the read-only-write refusal unreachable: a patch of /Arn must be refused with ValidationException "Invalid patch update: readOnlyProperties [/properties/Arn] cannot be updated".
Fix: the GetResource/ListResources serializer must populate the full model from the owning service, including readOnlyProperties and service-applied defaults.
Failing checks: cloudcontrol.server-side-computed-readonly-property-rejected-on-write, cloudcontrol.server-side-enforced-policy-applied-without-client-input

Cause 6 of 6: the registry type schema served for a resource type has no primaryIdentifier.
AWS: CloudFormation DescribeType(Type="RESOURCE", TypeName="AWS::SQS::Queue") returns the type's resource provider schema, which always declares "primaryIdentifier" (a list of JSON pointers into "properties") — that is what tells a client which property to pass as a Cloud Control Identifier.
Emulator: the returned Schema has no "primaryIdentifier" key at all (the check dies with KeyError: 'primaryIdentifier'), so the identifier of a type cannot be discovered from the registry.
Why: any generic client (and Cloud Control's own JSON-identifier form of GetResource) derives the identifier property from this schema; without it, type-agnostic tooling cannot address resources at all. It is also the schema Cause 4 must read to fix the Identifier.
Fix: the registry schema served by the CloudFormation DescribeType handler must include primaryIdentifier (and keep it consistent with the declared properties) for every supported resource type.
Failing check: cloudcontrol.primary-identifier-accepted-as-json-derived-from-the-registry-schema

Traces: failing check ids per cause above, collected in Issue-Trace-Ids. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs, update_resource.rs, delete_resource.rs, get_resource.rs, list_resources.rs and the store/effect code in src/state.rs; plus the CloudFormation DescribeType handler for Cause 6.

## Evidence
Quotes from the replayed checks (suite simparity-behavioral-aws-v1). Hosts, generated names, uuids and tokens are per-run values.

Cause 1 — "cloudcontrol reports ['<g>-0', ..., '<g>-5'] but CloudWatch Logs reports []"; "expected exactly one native CloudWatch alarm named '<alarm>', found 0"; "botocore.errorfactory.QueueDoesNotExist: An error occurred (AWS.SimpleQueueService.NonExistentQueue) when calling the SendMessage operation: The specified queue does not exist." (also ListQueueTags, GetQueueUrl); "botocore.errorfactory.ParameterNotFound: An error occurred (ParameterNotFound) when calling the GetParameterHistory operation:" — each immediately after a Cloud Control request reported OperationStatus=SUCCESS.

Cause 2 — "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResource operation: Resource of type 'AWS::SQS::Queue' with identifier 'https://sqs.<region>.amazonaws.com/<account>/<stack>-Queue-<suffix>' was not found." for a queue a CloudFormation stack had just created (6 checks, create/update/delete variants); "expected NotUpdatableException mutating create-only FifoQueue, got 'ResourceNotFoundException'"; "CreateResource over a pre-existing native parameter reached {... 'OperationStatus': 'SUCCESS', 'ResourceModel': '{\"Name\":\"...\",\"Type\":\"String\",\"Value\":\"via-cloudcontrol\"}'}" where a terminal FAILED (AlreadyExists) is required.

Cause 3 — "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator".

Cause 4 — "the SUCCESS event reports Identifier '<queue-name>', which is not a URL for the queue named '<queue-name>'"; "a compound primary identifier must be reported delimited, expected '<log-group>|<log-stream>', got '<log-stream>'"; "ResourceId 'table/<table>' not found in 'd67f8ea6-044e-4043-aa52-85ec9c5fe499'".

Cause 5 — "KeyError: 'Arn'" reading json.loads(GetResource.ResourceDescription.Properties)["Arn"] for an AWS::SQS::Queue whose create SUCCEEDED; "expected the account-wide S3 Block Public Access default to be reflected on GetResource even though DesiredState never set PublicAccessBlockConfiguration".

Cause 6 — "KeyError: 'primaryIdentifier'" on json.loads(cloudformation.describe_type(Type='RESOURCE', TypeName='AWS::SQS::Queue')['Schema']).

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol").

Cause 1: cc.create_resource(TypeName="AWS::Logs::LogGroup", DesiredState=json({"LogGroupName": n})) and poll to SUCCESS, then logs.describe_log_groups(logGroupNamePrefix=n) — must list it. Same pattern for AWS::CloudWatch::Alarm (cloudwatch.describe_alarms), AWS::SQS::Queue (sqs.send_message / sqs.list_queue_tags on the returned Identifier) and AWS::SSM::Parameter (ssm.get_parameter_history after successive patches: one new version per successful request).
Cause 2: deploy a CloudFormation stack containing an AWS::SQS::Queue, take the queue URL CloudFormation reports, then cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<that url>) — must return a ResourceDescription, reflect a stack update, and answer ResourceNotFoundException only after the stack is deleted. Separately: create an SSM parameter natively, then cc.create_resource for the same Name — must fail as already existing and leave the native value untouched.
Cause 3: cc.create_resource with Tags in DesiredState, poll to SUCCESS, then call the owning service's list_tags_for_resource — must return the tags, not NotImplemented.
Cause 4: cc.create_resource(TypeName="AWS::SQS::Queue", ...) — ProgressEvent.Identifier must be the queue URL ending "/<QueueName>"; for AWS::Logs::LogStream it must be "<LogGroupName>|<LogStreamName>"; for AWS::ApplicationAutoScaling::ScalableTarget it must embed ResourceId "table/<TableName>".
Cause 5: create an AWS::SQS::Queue through cc, then json.loads(cc.get_resource(...)["ResourceDescription"]["Properties"]) — must contain "Arn"; then cc.update_resource(PatchDocument=[{"op":"replace","path":"/Arn","value":"arn:aws:sqs:...:bogus"}]) must be ValidationException "Invalid patch update: readOnlyProperties [/properties/Arn] cannot be updated". For AWS::S3::Bucket, GetResource Properties must report PublicAccessBlockConfiguration even though DesiredState omitted it.
Cause 6: json.loads(cloudformation.describe_type(Type="RESOURCE", TypeName="AWS::SQS::Queue")["Schema"]) — must contain "primaryIdentifier" whose pointers all resolve into "properties".
