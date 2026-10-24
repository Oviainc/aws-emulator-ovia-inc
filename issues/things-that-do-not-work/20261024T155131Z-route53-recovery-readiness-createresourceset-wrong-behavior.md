Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# CreateResourceSet never generates a ComponentId for a DNS target resource

_Filed: 2026-10-24T15:51:31Z_
_Operation: CreateResourceSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a resource described by `DnsTargetResource`, the service generates the resource's component identifier: `Resource$ComponentId` is documented as "The component identifier of the resource, generated when DNS target resource is used", and that generated id is the value `GetReadinessCheckResourceStatus`'s `ResourceIdentifier` accepts ("the Amazon Resource Name (ARN) or the identifier generated for the resource by Application Recovery Controller (for example, for a DNS target resource)").

Expected: a `CreateResourceSet` of type AWS::Route53RecoveryReadiness::DNSTargetResource whose `resources[*]` carry a `dnsTargetResource` and no `componentId` comes back (and is stored) with a service-generated `componentId` on each such entry, which later identifies the resource in `GetReadinessCheckResourceStatus`.

Actual: the handler stores and reports `input.resources` verbatim, so such an entry has no `componentId` at all in the create/`GetResourceSet`/`ListResourceSets` responses. A DNS-target resource therefore has no addressable identifier: `GET /readinesschecks/{check}/resource/{id}/status` can only be reached for resources the caller itself gave an `resourceArn` or a hand-written `componentId`, and a DNS target resource (which has no `resourceArn`) is unreachable — it answers 404 ResourceNotFoundException for every identifier.

## Evidence

Spec: `smithy_query(query="doc", shape="Resource", member="ComponentId")` = "The component identifier of the resource, generated when DNS target resource is used"; `smithy_query(query="doc", shape="GetReadinessCheckResourceStatusRequest", member="ResourceIdentifier")` = "...the ARN or the identifier generated for the resource by Application Recovery Controller (for example, for a DNS target resource)". Generation is therefore the service's job, not the caller's.

Handler: `src/ops/create_resource_set.rs` lines 240-265 — `resources: input.resources` is stored unchanged and echoed back; nothing inspects `dns_target_resource` or mints an id. `src/ops/get_readiness_check_resource_status.rs` lines 188-200 matches the requested identifier only against `resource.resource_arn` and `resource.component_id` as submitted, so a DNS target resource stored without a `componentId` can never be addressed.

Tests: `tests/all/readiness_check_lifecycle.rs:419-438` (case "a dns target resource pointing at a network load balancer") submits a `dnsTargetResource` with no `componentId`, and `assert_resource_set_reported` (:493-498) asserts the reported `resources` array equals the submitted one exactly — i.e. the current echo behaviour is pinned by a test and would need updating with the fix.

## Steps to Reproduce

1. With ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE=1, POST /resourcesets with {"resourceSetName":"dns_set","resourceSetType":"AWS::Route53RecoveryReadiness::DNSTargetResource","resources":[{"dnsTargetResource":{"domainName":"www.example.com","hostedZoneArn":"arn:aws:route53:::hostedzone/Z1111111111111","recordSetId":"record-1","recordType":"A"}}]}
2. Inspect the 200 response (and a follow-up GET /resourcesets/dns_set): the resources entry carries no componentId, where the service generates one
3. POST /readinesschecks {"readinessCheckName":"dns_check","resourceSetName":"dns_set"}
4. GET /readinesschecks/dns_check/resource/{componentId}/status — there is no component id to use, and any identifier answers 404 ResourceNotFoundException, so the per-resource status of a DNS target resource is unreachable
