Issue-Source: trace-sweep-divergence
Issue-Trace: eks-capability-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-capability-error-paths eks-eks-catalog-read-only-discovery eks-eks-tag-operations-error-paths eks-tag-operations-missing-cluster eks-register-cluster-invalid-connector-role-error eks-cluster-create-invalid-subnet-error
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCapability|4xx->4xx|$.message
Issue-Key: eks|DescribeAddonConfiguration|4xx->4xx|$.message
Issue-Key: eks|ListTagsForResource|4xx->4xx|$.message
Issue-Key: eks|TagResource|4xx->4xx|$.message
Issue-Key: eks|UntagResource|4xx->4xx|$.message
Issue-Key: eks|RegisterCluster|4xx->4xx|$.message
Issue-Key: eks|CreateCluster|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# eks refusal messages diverge from AWS wording across seven operations

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapability

## Description
SEVEN net-new root causes, all in EKS refusal payloads. In every one the HTTP status matches the recording and only the `message` member diverges, so each is a message-composition defect in the operation that raises it, not a routing or classification defect. All seven must be closed before this issue is resubmitted.

Cause 1 of 7: CreateCapability rejects an out-of-enum `type` with the generic framework constraint sentence instead of EKS's own text.
AWS: Smithy: CreateCapability POST /clusters/{clusterName}/capabilities; input member `type` targets CapabilityType, an enum whose values are exactly [ACK, KRO, ARGOCD], and the operation models InvalidParameterException (httpError 400). The recorded refusal is 400 with the message 'Invalid capability type' — that is fixed text in its entirety; it has no variable part and does not echo the rejected value.
Emulator: answers 400 but with the shared request-validation sentence "1 validation error detected: Value '<supplied type>' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [ACK, KRO, ARGOCD]" — i.e. the generic enum-constraint renderer rather than the capability handler's own refusal.
Why: a client that branches on the refusal text (or asserts it, as the recorded workflow does) cannot tell an invalid capability type from any other enum violation, and the leaked framework phrasing is text AWS never emits for this API.
Fix: CreateCapability's input validation in the eks service handler — the capability-type check must reject before/instead of the generic enum validator and emit EKS's fixed sentence.
Traces: eks-capability-error-paths#task#4.

Cause 2 of 7: DescribeAddonConfiguration's not-found refusal does not name the add-on or version it could not find.
AWS: Smithy: DescribeAddonConfiguration GET /addons/configuration-schemas with required httpQuery members addonName and addonVersion; models ResourceNotFoundException (httpError 404). The recorded refusal is 404 with 'Schema not found for requested addonName=[<addonName>] and addonVersion=[<addonVersion>]'. Fixed text: 'Schema not found for requested addonName=[', '] and addonVersion=[', ']'. Variable parts: the two values are echoed verbatim from the request's query string.
Emulator: answers 404 with the service-wide generic 'The requested resource does not exist.', naming neither member.
Why: the caller loses the only diagnostic the API gives it — which add-on/version pair has no schema — and any assertion on AWS's wording fails.
Fix: DescribeAddonConfiguration's handler in the eks service — raise the operation-specific not-found message instead of falling through to the generic ResourceNotFoundException text.
Traces: eks-eks-catalog-read-only-discovery#task#7.

Cause 3 of 7: ListTagsForResource on a nonexistent cluster ARN composes its own prose and interpolates the ARN.
AWS: Smithy: ListTagsForResource GET /tags/{resourceArn} models NotFoundException (httpError 404), documented as 'A service resource associated with the request could not be found.' The recorded refusal is 404 with the message 'Resource was not found' — entirely fixed text, with NO variable part: the requested ARN is deliberately not interpolated.
Emulator: answers 404 with 'The resource <resourceArn> could not be found', i.e. different fixed wording plus the request's ARN spliced in.
Why: clients and recorded expectations keyed to AWS's fixed sentence do not match, and the emulator advertises a per-request detail the real tag plane never discloses.
Fix: the eks tag-plane handler's not-found path (shared by the three tag operations) — emit NotFoundException with the fixed sentence and no ARN interpolation.
Traces: eks-eks-tag-operations-error-paths#task#1 (cluster-ARN form also reached from eks-tag-operations-missing-cluster#task#2).

Cause 4 of 7: TagResource on a nonexistent cluster ARN, same defect on the write path.
AWS: Smithy: TagResource POST /tags/{resourceArn} models NotFoundException (httpError 404). Recorded refusal: 404, message 'Resource was not found' (fixed text, no variable part).
Emulator: 404 with 'The resource <resourceArn> could not be found'.
Why: as cause 3 — a tagging client cannot match AWS's refusal text.
Fix: TagResource's resource-resolution step in the eks tag plane.
Traces: eks-eks-tag-operations-error-paths#task#2 (also eks-tag-operations-missing-cluster#task#1).

Cause 5 of 7: UntagResource on a nonexistent cluster ARN, same defect on the delete path.
AWS: Smithy: UntagResource DELETE /tags/{resourceArn} models NotFoundException (httpError 404). Recorded refusal: 404, message 'Resource was not found' (fixed text, no variable part).
Emulator: 404 with 'The resource <resourceArn> could not be found'.
Why: as cause 3.
Fix: UntagResource's resource-resolution step in the eks tag plane.
Traces: eks-eks-tag-operations-error-paths#task#3 (also eks-tag-operations-missing-cluster#task#3).

Cause 6 of 7: RegisterCluster's connector-role refusal drops the wrapped-service tail AWS appends.
AWS: Smithy: RegisterCluster POST /cluster-registrations, required member connectorConfig; models InvalidParameterException (httpError 400). The recorded refusal is 400 and the message carries two parts: the sentence 'Nonexistent role or missing ssm service principal in trust policy: <roleArn>' — fixed text with the role ARN taken from connectorConfig.roleArn — followed by the originating-service tail ' (Service: EksExternalClusterManager, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: 1)', whose fixed parts are the 'Service: EksExternalClusterManager', 'Status Code: 400' and 'SDK Attempt Count: 1' fields; the request id inside it is per-run and is NOT part of the requirement.
Emulator: emits only the leading sentence and omits the whole parenthesised tail.
Why: callers that parse the originating service or status out of the wrapped message see a differently shaped error; the requirement is that the tail is present and carries those fields, not that it carries the recorded id.
Fix: RegisterCluster's connector-role validation in the eks handler — append the wrapped-service tail when composing the refusal.
Traces: eks-register-cluster-invalid-connector-role-error#task#1.

Cause 7 of 7: CreateCluster's wrapped EC2 subnet-not-found refusal names a different subnet of the request than AWS names (LOW).
AWS: Smithy: CreateCluster POST /clusters, required resourcesVpcConfig (VpcConfigRequest.subnetIds); models InvalidParameterException (httpError 400). Recorded behavior only: the refusal is 400 with the wrapped EC2 message "The subnet ID '<subnetId>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <request id>; Proxy: null)". Fixed text: everything but the subnet id and the request id; the subnet id is a value taken from the request's subnetIds list, and AWS reports a specific member of that list.
Emulator: returns the same status, error code and template but names a different member of the request's subnetIds list than the recording does (the recording names the later subnet of the pair, the emulator the first). The Request ID field also differs, which is per-run and expressly not part of this finding.
Why: a client reading the offending subnet out of the message is pointed at the wrong element of its own request, so remediation targets a subnet AWS did not complain about.
Fix: CreateCluster's subnet-resolution step in the eks handler — select the same member of subnetIds AWS reports when more than one is unresolvable. (The tail-omission half of this template was previously filed and closed as 'EKS CreateCluster never mints the cluster security group in EC2, and its wrapped EC2 subnet-not-found message omits the request-id/proxy tail'; this is the residual which-subnet divergence, which that issue does not cover.)
Traces: eks-cluster-create-invalid-subnet-error#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (40 fixtures, 232 steps, score 48.3%).

1. CreateCapability, eks-capability-error-paths#task#4: expected 400 / $.message 'Invalid capability type'; actual 400 / "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [ACK, KRO, ARGOCD]".
2. DescribeAddonConfiguration, eks-eks-catalog-read-only-discovery#task#7: expected 404 / 'Schema not found for requested addonName=[<addonName>] and addonVersion=[<addonVersion>]'; actual 404 / 'The requested resource does not exist.'
3. ListTagsForResource, eks-eks-tag-operations-error-paths#task#1 (+1 affected step): expected 404 / 'Resource was not found'; actual 404 / 'The resource <cluster ARN> could not be found'.
4. TagResource, eks-eks-tag-operations-error-paths#task#2 (+1 affected step): expected 404 / 'Resource was not found'; actual 404 / 'The resource <cluster ARN> could not be found'.
5. UntagResource, eks-eks-tag-operations-error-paths#task#3 (+1 affected step): expected 404 / 'Resource was not found'; actual 404 / 'The resource <cluster ARN> could not be found'.
6. RegisterCluster, eks-register-cluster-invalid-connector-role-error#task#1: expected 400 / 'Nonexistent role or missing ssm service principal in trust policy: <roleArn> (Service: EksExternalClusterManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)'; actual 400 / 'Nonexistent role or missing ssm service principal in trust policy: <roleArn>' with the tail absent.
7. CreateCluster, eks-cluster-create-invalid-subnet-error#task#1: expected 400 naming subnet '...deadbeef1'; actual 400 naming subnet '...deadbeef0' from the same request's subnetIds list (request id difference ignored).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /clusters/<cluster>/capabilities with type set to a value outside [ACK, KRO, ARGOCD] (fixture eks-capability-error-paths, step task#4) and read $.message.
3. Cause 2: GET /addons/configuration-schemas?addonName=<unknown>&addonVersion=<unknown> (eks-eks-catalog-read-only-discovery#task#7).
4. Causes 3-5: GET, POST and DELETE /tags/<ARN of a cluster that does not exist> (eks-eks-tag-operations-error-paths#task#1..#3; also eks-tag-operations-missing-cluster#task#1..#3).
5. Cause 6: POST /cluster-registrations with connectorConfig.roleArn naming a role with no ssm trust relationship (eks-register-cluster-invalid-connector-role-error#task#1).
6. Cause 7: POST /clusters with resourcesVpcConfig.subnetIds listing more than one nonexistent subnet (eks-cluster-create-invalid-subnet-error#task#1) and compare which subnet id the message names.
