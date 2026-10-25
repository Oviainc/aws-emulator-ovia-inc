Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.tag-resource-invalid-tag-key-format-rejected ecs.tag-resource-reserved-aws-prefix-key-rejected ecs.ecracc-service-on-a-deregistered-revision-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.tag-resource-invalid-tag-key-format-rejected|_error#5|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.tag-resource-reserved-aws-prefix-key-rejected|_error#5|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.ecracc-service-on-a-deregistered-revision-rejected|run#15|AssertionError|assert by_arn == "InvalidParameterException", f"service on an INACTIVE ARN: {by_arn!r}"
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# ecs: 2 net-new behavioral root causes — TagResource accepts reserved aws: tag keys, and CreateService on an INACTIVE revision uses the wrong error code

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new root causes in the ECS service, numbered below. BOTH must be closed before this issue is done.

Cause 1 of 2: TagResource accepts a tag key carrying the reserved `aws:` prefix.
AWS: Smithy: TagResource.tags is required and targets Tags -> Tag; Tag.key targets TagKey with @length min 1 max 128 and @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$ — the pattern admits ':' so the MODEL does not reject the key; the restriction is enforced by the service and is documented on the Tag shape itself: "Do not use aws:, AWS:, or any upper or lowercase combination of such as a prefix for either keys or values as it is reserved for Amazon Web Services use. You cannot edit or delete tag keys or values with this prefix." The request is refused with HTTP 400 InvalidParameterException (Smithy: TagResource errors InvalidParameterException, httpError 400). The recorded refusal message is "Tag key must not start with aws:" — quoted as an EXAMPLE; the requirement is the 400 status and the InvalidParameterException code.
Emulator: answers HTTP 200 with an empty JSON body ({}) for TagResource with tags=[{key: "aws:reserved", value: "x"}] and with tags=[{key: "aws:sim2real-test", value: "x"}] on a freshly created cluster ARN — the reserved-prefix key is accepted instead of refused.
Why: a client that relies on ECS to police the reserved namespace can write `aws:`-prefixed tags that real AWS would never let it create, and that it could then never edit or delete; cost-allocation and tag-policy tooling keyed on AWS-reserved tags reads attacker/caller-written values, and every negative test of the reserved prefix passes wrongly.
Fix: TagResource handler validation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/tag_resource.rs), ideally in the shared tag-validation helper so the same gate applies to the `tags` member of CreateCluster / CreateService / RegisterTaskDefinition.
Traces: ecs.tag-resource-invalid-tag-key-format-rejected, ecs.tag-resource-reserved-aws-prefix-key-rejected.

Cause 2 of 2: CreateService naming the exact ARN of a deregistered (INACTIVE) task-definition revision is refused with ClientException instead of InvalidParameterException.
AWS: Smithy: CreateService models ClientException (400) and InvalidParameterException (400) as two DISTINCT client errors, and real ECS uses them to distinguish two different conditions on the taskDefinition member. Recorded behavior (audited check): CreateService on the exact ARN of a revision that was deregistered — the revision still describes with status INACTIVE — is refused with InvalidParameterException (recorded message, as an example: "TaskDefinition is inactive"), while CreateService on a bare FAMILY whose only revision is INACTIVE is refused with ClientException (recorded message example: "TaskDefinition not found."). Both legs are part of the same recorded contract; the emulator gets the second right and the first wrong.
Emulator: the INACTIVE-ARN leg is refused with error code 'ClientException' — i.e. the same code the "family not found" leg uses — so the two conditions are indistinguishable to a client.
Why: callers branch on the error code to decide what to do next — InvalidParameterException on an INACTIVE ARN means "this revision exists but was retired, point the service at a live revision", ClientException means "no such task definition, register one first". Collapsing both onto ClientException makes a deploy pipeline take the wrong recovery branch (re-register a family that already exists, or treat a retired-revision mistake as a missing family) and makes the negative contract untestable.
Fix: CreateService's task-definition resolution/validation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs): when the taskDefinition argument RESOLVES to a stored revision whose status is INACTIVE, answer InvalidParameterException; reserve ClientException for the case where nothing resolves at all.
Traces: ecs.ecracc-service-on-a-deregistered-revision-rejected.

Related open issues, not duplicates: 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md bundles ten other missing ECS validations (none of them the reserved tag-key prefix); 20261022T152302Z-ecs-accepts-invalid-requests-with-200-and-misreports.md covers a different set of accepted-invalid requests. Both causes above were replayed as failing after those were filed.

## Evidence
Replay output, verdict `mismatch` in all three checks:
- ecs.tag-resource-invalid-tag-key-format-rejected — `ecs.tag_resource(resourceArn=<cluster arn>, tags=[{"key": "aws:reserved", "value": "x"}])` -> AssertionError: "expected rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}". Expected: HTTP 400, Error.Code InvalidParameterException (recorded message example "Tag key must not start with aws:"), and the tag absent from ListTagsForResource.
- ecs.tag-resource-reserved-aws-prefix-key-rejected — `ecs.tag_resource(resourceArn=<cluster arn>, tags=[{"key": "aws:sim2real-test", "value": "x"}])` -> AssertionError: "real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}". Same expectation.
- ecs.ecracc-service-on-a-deregistered-revision-rejected — after `deregister_task_definition(taskDefinition=<arn>)` (DescribeTaskDefinition then correctly reports status INACTIVE), `ecs.create_service(cluster=..., serviceName=..., taskDefinition=<the INACTIVE arn>, desiredCount=0, launchType="EC2")` -> AssertionError: "service on an INACTIVE ARN: 'ClientException'". Expected Error.Code InvalidParameterException (HTTP 400); actual Error.Code ClientException.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the three checks named in trace_ids (all under the `ecs` service token).
3. Cause 1 by hand: CreateCluster, then TagResource(resourceArn=<clusterArn>, tags=[{key:"aws:anything", value:"x"}]) — observe HTTP 200 with an empty body instead of HTTP 400 InvalidParameterException, then ListTagsForResource and observe whether the reserved key landed. A normally-formed key (e.g. key="owner") must still be accepted.
4. Cause 2 by hand: RegisterTaskDefinition (family F, networkMode bridge, one container), DeregisterTaskDefinition on the returned ARN, confirm DescribeTaskDefinition reports status INACTIVE, then CreateService with taskDefinition=<that exact ARN> — observe Error.Code ClientException where AWS answers InvalidParameterException. Repeat with taskDefinition=F (the bare family, all revisions INACTIVE) — that leg correctly answers ClientException, which is what makes the first leg's code wrong.
