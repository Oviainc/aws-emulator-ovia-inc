Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.register-taskdef-missing-container-name-rejected ecs.register-taskdef-malformed-container-definition-json-rejected ecs.register-taskdef-requires-container-defs ecs.register-taskdef-invalid-resource-limits ecs.tag-resource-invalid-character-rejected ecs.put-cluster-capacity-providers-invalid-base-weight-rejected ecs.concurrent-cluster-settings-update ecs.put-cluster-capacity-providers-unknown-provider-rejected ecs.update-service-invalid-deployment-percent-bounds-rejected ecs.create-service-mismatched-launch-type-capacity-provider-rejected ecs.delete-active-task-definition-rejected ecs.op-contract-deleteTaskDefinitions ecs.task-definition-delete-active-rejected ecs.taskdef-delete-while-active-fails
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.register-taskdef-missing-container-name-rejected|run#5|AssertionError|assert message == "Container.name should not be null or empty.", f"refusal message {message!r}"
Issue-Key: ecs.register-taskdef-malformed-container-definition-json-rejected|run#5|AssertionError|assert message == "Container.image should not be null or empty.", f"refusal message {message!r}"
Issue-Key: ecs.register-taskdef-requires-container-defs|run#5|AssertionError|assert err.get("Message") == "Container list cannot be empty.", err
Issue-Key: ecs.register-taskdef-invalid-resource-limits|_refusal#3|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.tag-resource-invalid-character-rejected|run#12|AssertionError|assert message == (
Issue-Key: ecs.put-cluster-capacity-providers-invalid-base-weight-rejected|run#8|AssertionError|assert message == (
Issue-Key: ecs.concurrent-cluster-settings-update|run#8|AssertionError|assert message == "Invalid setting 'value'", f"unexpected message: {message!r}"
Issue-Key: ecs.put-cluster-capacity-providers-unknown-provider-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.update-service-invalid-deployment-percent-bounds-rejected|_error#11|AssertionError|raise AssertionError(
Issue-Key: ecs.create-service-mismatched-launch-type-capacity-provider-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.delete-active-task-definition-rejected|run#9|AssertionError|assert failures == [{"arn": arn, "reason": _ACTIVE_REASON}], (
Issue-Key: ecs.op-contract-deleteTaskDefinitions|run#23|AssertionError|assert reason == _ACTIVE_REASON, (
Issue-Key: ecs.task-definition-delete-active-rejected|run#10|AssertionError|assert resp["failures"] == expected_failures, (
Issue-Key: ecs.taskdef-delete-while-active-fails|run#13|AssertionError|assert reason == expected_reason, (
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# ecs: 7 net-new root causes in request validation — four invalid requests are accepted with 200 and three refusals carry generic or invented message text

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
Seven independent ECS validation defects. Each cause below must be closed before this issue is submitted. The already-open issue 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md covers a different set of ten validations and carries none of the replay keys below.

Cause 1 of 7: RegisterTaskDefinition's container-definition refusals use emulator wording.
AWS: RegisterTaskDefinition refuses incomplete container definitions with ClientException (Smithy: RegisterTaskDefinition models ClientException, httpError 400, and `containerDefinitions` is a required member), each with fixed text and no interpolation: a container with no `name` is "Container.name should not be null or empty."; a container with no `image` is "Container.image should not be null or empty."; an empty containerDefinitions list is "Container list cannot be empty." No revision is created for the family in any of the three cases.
Emulator: the code is ClientException and no revision leaks, but the messages are emulator-internal prose: "Invalid setting for containerDefinitions[0]: 'name' is required", "Invalid setting for container 'app': 'image' is required" (which additionally echoes the container name from the request), and "containerDefinitions must describe at least one container".
Why: the message is part of the error contract; a client or IaC provider matching AWS's sentence does not recognise the refusal, and the echoed container name leaks request input AWS keeps out of this message.
Fix: the refusal texts in src/ops/register_task_definition.rs' container-definition validation.
Traces: ecs.register-taskdef-missing-container-name-rejected, ecs.register-taskdef-malformed-container-definition-json-rejected, ecs.register-taskdef-requires-container-defs.

Cause 2 of 7: RegisterTaskDefinition accepts memoryReservation greater than memory.
AWS: a container whose soft limit exceeds its hard limit is internally inconsistent and is refused with ClientException "Invalid setting for container '<container name>'. 'memory' must be greater than or equal to 'memoryReservation'." — fixed text with the offending container's name from the request. No revision is created.
Emulator: memory=128 with memoryReservation=256 is ACCEPTED: HTTP 200, revision 1 registered ACTIVE with both values persisted.
Why: a nonsensical resource spec AWS blocks at registration is stored here, so a client validating its task definitions against the emulator ships a definition real ECS refuses.
Fix: a cross-field memory/memoryReservation check in src/ops/register_task_definition.rs.
Traces: ecs.register-taskdef-invalid-resource-limits.

Cause 3 of 7: three refusals emit a generic constraint-validation message instead of ECS's own sentence.
AWS: ECS answers these constraint violations with InvalidParameterException and its own domain wording. A tag value containing a disallowed character (e.g. an emoji, Unicode category So) is "Some tags contain invalid characters. Valid characters: UTF-8 letters, spaces, numbers and _ . / = + - : @." — entirely fixed text, naming no member and echoing no value. A negative `base` in defaultCapacityProviderStrategy is "The specified base value '<base>' is invalid. Specify an integer between 0 and 100000 and try again." — fixed text with the offending value from the request and the modelled 0..100000 bound. An out-of-enum containerInsights value on UpdateClusterSettings is "Invalid setting 'value'" — fixed text naming the member, not the value.
Emulator: the code is right but each message is the generic constraint rendering: "1 validation error detected: Value '<value>' at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$"; "1 validation error detected: Value '-1' at 'defaultCapacityProviderStrategy.1.member.base' failed to satisfy constraint: Member must have value greater than or equal to 0"; and 'settings value must be one of enhanced, enabled, disabled; got "<value>"'.
Why: the emulator leaks its model-validation layer into responses where AWS speaks in domain terms. Clients matching the real sentence miss the refusal, and the generic form exposes internal member paths AWS never names for these errors.
Fix: domain-specific refusal text in src/ops/tag_resource.rs, src/ops/put_cluster_capacity_providers.rs and src/ops/update_cluster_settings.rs, ahead of the generic constraint validator.
Traces: ecs.tag-resource-invalid-character-rejected, ecs.put-cluster-capacity-providers-invalid-base-weight-rejected, ecs.concurrent-cluster-settings-update.

Cause 4 of 7: PutClusterCapacityProviders accepts a capacity provider that does not exist.
AWS: every name in `capacityProviders` and in `defaultCapacityProviderStrategy` must resolve to an existing capacity provider; a name that was never created is refused InvalidParameterException "The specified capacity provider was not found. Specify a valid capacity provider and try again." (fixed text) and the cluster's association is left untouched. (Smithy: PutClusterCapacityProviders models InvalidParameterException, httpError 400; both list members are required.)
Emulator: the call returns HTTP 200 and persists the bogus association — DescribeClusters then reports capacityProviders ["<nonexistent name>"] and a defaultCapacityProviderStrategy naming it, with attachmentsStatus UPDATE_IN_PROGRESS.
Why: the cluster is left permanently referencing a provider that cannot exist, so every later placement decision that consults the default strategy runs on a dangling reference and a client validating its wiring gets a false pass.
Fix: resolve each named provider against state before persisting the association in src/ops/put_cluster_capacity_providers.rs.
Traces: ecs.put-cluster-capacity-providers-unknown-provider-rejected.

Cause 5 of 7: UpdateService accepts a deploymentConfiguration whose minimumHealthyPercent exceeds 100.
AWS: minimumHealthyPercent has a 100 ceiling; UpdateService with minimumHealthyPercent 150 (and maximumPercent 100) is refused InvalidParameterException "minimumHealthyPercent must be at most 100" — fixed text naming the ceiling — and nothing is persisted on the service. (Smithy: UpdateService models InvalidParameterException, httpError 400.)
Emulator: the call returns HTTP 200 and the service's deploymentConfiguration reads back minimumHealthyPercent 150 beside maximumPercent 100.
Why: an impossible deployment configuration is stored, so emulated deployment arithmetic runs on bounds real ECS never accepts and a client's bad configuration passes validation.
Fix: the deploymentConfiguration bounds check in src/ops/update_service.rs (the same check CreateService needs).
Related: the already-open 20261024T081305Z issue owns the sibling CreateService-side percent checks (ecs.escc-*); this cause is the UpdateService path, which that issue does not name.
Traces: ecs.update-service-invalid-deployment-percent-bounds-rejected.

Cause 6 of 7: CreateService's launchType + capacityProviderStrategy conflict message uses emulator wording.
AWS: supplying both `launchType` and `capacityProviderStrategy` is refused InvalidParameterException "Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again." — entirely fixed text. (Smithy: CreateService models InvalidParameterException, httpError 400, and the two members are optional and mutually exclusive.)
Emulator: the refusal lands but the message is 'launchType must be omitted when capacityProviderStrategy is specified'.
Why: the emulator's wording names the members in its own convention, so no client matching AWS's sentence recognises it.
Fix: the mutual-exclusion refusal text in src/ops/create_service.rs.
Traces: ecs.create-service-mismatched-launch-type-capacity-provider-rejected.

Cause 7 of 7: DeleteTaskDefinitions' ACTIVE-revision failure entry has the wrong reason and an extra member.
AWS: DeleteTaskDefinitions on a revision still in ACTIVE status reports it through `failures[]` (not an exception) as exactly {arn: <the revision ARN from the request>, reason: "The specified task definition is still in ACTIVE status. Please deregister the target and try again."} — the reason is fixed text naming both the ACTIVE-status block and the deregister remedy, and the entry carries no other member. The same call succeeds once the revision is deregistered. (Smithy: DeleteTaskDefinitions returns `failures` of shape Failures beside `taskDefinitions`.)
Emulator: the failure entry is {arn: <ARN>, reason: "Task definition must be in INACTIVE status to be deleted.", detail: "The task definition is in ACTIVE status."} — different reason wording, plus a `detail` member AWS does not populate for this failure.
Why: `failures[].reason` is the only per-entry signal a batch caller gets; a client branching on the recorded reason (or relaying the remedy to a user) neither matches nor conveys it, and the surplus `detail` is a field AWS leaves absent.
Fix: the ACTIVE-status failure construction in src/ops/delete_task_definitions.rs.
Traces: ecs.delete-active-task-definition-rejected, ecs.op-contract-deleteTaskDefinitions, ecs.task-definition-delete-active-rejected, ecs.taskdef-delete-while-active-fails.

## Evidence
Cause 1 — missing name: expected ClientException "Container.name should not be null or empty.", got "Invalid setting for containerDefinitions[0]: 'name' is required". Missing image: expected "Container.image should not be null or empty.", got "Invalid setting for container 'app': 'image' is required". Empty list: expected {'Code': 'ClientException', 'Message': 'Container list cannot be empty.'}, got {'Code': 'ClientException', 'Message': 'containerDefinitions must describe at least one container'}.
Cause 2 — memory=128, memoryReservation=256: expected ClientException "Invalid setting for container 'app'. 'memory' must be greater than or equal to 'memoryReservation'."; actual HTTP 200 with taskDefinition revision 1 ACTIVE and containerDefinitions[0] carrying memory 128 / memoryReservation 256.
Cause 3 — tag value with an emoji: expected InvalidParameterException "Some tags contain invalid characters. Valid characters: UTF-8 letters, spaces, numbers and _ . / = + - : @.", got "1 validation error detected: Value 'prod-🎉' at 'tags.1.member.value' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$". base=-1: expected "The specified base value '-1' is invalid. Specify an integer between 0 and 100000 and try again.", got "1 validation error detected: Value '-1' at 'defaultCapacityProviderStrategy.1.member.base' failed to satisfy constraint: Member must have value greater than or equal to 0". Invalid containerInsights value: expected "Invalid setting 'value'", got 'settings value must be one of enhanced, enabled, disabled; got "not-a-real-value"'.
Cause 4 — expected InvalidParameterException "The specified capacity provider was not found. Specify a valid capacity provider and try again." and an untouched cluster; actual HTTP 200 and DescribeClusters reporting capacityProviders ['<bogus name>'], defaultCapacityProviderStrategy [{'capacityProvider': '<bogus name>', 'weight': 1, 'base': 0}], attachmentsStatus 'UPDATE_IN_PROGRESS'.
Cause 5 — expected InvalidParameterException "minimumHealthyPercent must be at most 100"; actual HTTP 200 and the service's deploymentConfiguration reading back {'maximumPercent': 100, 'minimumHealthyPercent': 150, ...}.
Cause 6 — expected 'Specifying both a launch type and capacity provider strategy is not supported. Remove one and try again.', got 'launchType must be omitted when capacityProviderStrategy is specified'.
Cause 7 — expected failures=[{'arn': <revision ARN>, 'reason': 'The specified task definition is still in ACTIVE status. Please deregister the target and try again.'}]; actual failures=[{'arn': <revision ARN>, 'reason': 'Task definition must be in INACTIVE status to be deleted.', 'detail': 'The task definition is in ACTIVE status.'}].
Generated family/cluster/service names and the 000000000000 account in the recordings are per-run values, not requirements.

## Steps to Reproduce
Cause 1: RegisterTaskDefinition(family=<f>, containerDefinitions=[{image, memory, essential}]) — no name; then [{name, essential}] — no image; then containerDefinitions=[] (client-side validation off). Read each message and confirm ListTaskDefinitions(familyPrefix=<f>) stays empty.
Cause 2: RegisterTaskDefinition with containerDefinitions=[{name:'app', image:..., essential:true, memory:128, memoryReservation:256}].
Cause 3: CreateCluster, then TagResource(resourceArn=<cluster arn>, tags=[{key:'k', value:'prod-🎉'}]); PutClusterCapacityProviders(cluster=<name>, capacityProviders=['FARGATE'], defaultCapacityProviderStrategy=[{capacityProvider:'FARGATE', weight:1, base:-1}]) with botocore parameter_validation disabled; UpdateClusterSettings(cluster=<name>, settings=[{name:'containerInsights', value:'not-a-real-value'}]).
Cause 4: CreateCluster, then PutClusterCapacityProviders naming a capacity provider that was never created in both capacityProviders and defaultCapacityProviderStrategy; then DescribeClusters.
Cause 5: CreateCluster + RegisterTaskDefinition + CreateService(desiredCount=0, launchType=EC2); UpdateService(deploymentConfiguration={minimumHealthyPercent:150, maximumPercent:100}); then DescribeServices.
Cause 6: CreateService passing BOTH launchType and capacityProviderStrategy.
Cause 7: RegisterTaskDefinition (revision stays ACTIVE), then DeleteTaskDefinitions([that ARN]) and inspect failures[]; deregister and repeat to see it succeed.
Replay the check ids in trace_ids to confirm.
