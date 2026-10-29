Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place
Issue-Key: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place|_create_stack_tolerating_role_propagation#4|AssertionError|raise AssertionError(f"stack {stack_name} failed to create ({status}): {reasons!r}")
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# CloudFormation AWS::OpenSearchService::Domain provisioner forwards AccessPolicies as a JSON object, so CreateDomain answers HTTP 400 "expected string, got object start" and the stack rolls back

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
ONE root cause, grounded in 1 failing behavioral check (service token = the composite leading token of the check id, 'cloudformation__iam__opensearch__sts').

Already-open issues that describe the SAME root cause but whose replay keys the report did NOT match for this run, so this check is not marked OWNED or ALREADY FILED: 20261028T035419Z-cloudformation-aws-opensearchservice-domain-provisioner-sends-the-accesspolicies.md (carries this check id in Issue-Trace-Ids, but its Issue-Key fingerprint names `_create_stack_tolerating_role_propagation#4|AssertionError|raise AssertionError(f"stack {stack_name} failed to create: {reasons!r}") from err`, while the current failure raises from a different line with the message form `stack {stack_name} failed to create ({status}): {reasons!r}`), 20261028T032345Z-ec2-elbv2-opensearch-cloudformation-provisioned-aws-opensearchservice-domain-is-refused-by.md and 20261028T053726Z-opensearch-three-net-new-causes-the-cfn-aws.md. A fixer should verify all of them together; this issue exists so the check id below is re-replayed.

Cause 1 of 1: a CloudFormation stack containing an AWS::OpenSearchService::Domain whose AccessPolicies property is written in the normal CloudFormation form — a nested policy-document OBJECT containing intrinsics (Principal.AWS: !GetAtt RoleA.Arn / !GetAtt RoleB.Arn, Resource: !Sub ...) — cannot be created. The CreateDomain request the resource provisioner emits carries AccessPolicies as a JSON object instead of the JSON string the model requires; the OpenSearch request deserializer refuses the body with HTTP 400; the Domain resource fails with HandlerErrorCode InvalidRequest and the stack ends ROLLBACK_COMPLETE. Failing check: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place.

AWS: Smithy com.amazonaws.opensearch#CreateDomain — member AccessPolicies targets com.amazonaws.opensearch#PolicyDocument, a STRING with @length {min 0, max 102400} and @pattern '.*' ("Access policy rules for an Amazon OpenSearch Service domain endpoint"); modeled client errors include ValidationException (400) and BaseException (400). On AWS the AWS::OpenSearchService::Domain resource handler accepts an object-valued AccessPolicies in the template, resolves the intrinsics inside it, serializes it to a JSON string and calls CreateDomain, so such a stack reaches CREATE_COMPLETE, publishes its Outputs, and DescribeDomain returns AccessPolicies as a JSON string naming the resolved role ARNs. For a request that genuinely is ill-typed, AWS answers a modeled client error whose message names the offending member and the expected type; it never surfaces a deserializer's internal parser text as the service message. CloudFormation's wrapper frame is recorded behavior only: `Resource handler returned message: "<service message> (Service: <svc>, Status Code: <status>, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: <code>)` — the frame is fixed text, the angle-bracketed parts are per-request values.

Emulator: the Domain resource fails at create time. Stack reason `The following resource(s) failed to create: [Domain]. `; resource reason `Resource handler returned message: "Invalid request provided: expected string, got object start (Service: OpenSearch, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. In that message `Invalid request provided: expected string, got object start` is entirely FIXED text — it names no member, carries no value from the request; only the request id and request token are per-run. So two things diverge: (a) a CreateDomain call AWS accepts is refused 400, and (b) the refusal text is the deserializer's internal message rather than a modeled ValidationException/BaseException naming the member and expected type.

Why: a client relying on AWS's behavior cannot deploy an OpenSearch domain through CloudFormation whenever the domain carries a resource policy — the normal case, since AccessPolicies is how a domain is made reachable. The stack rolls back, no Outputs (DomainArn, DomainEndpoint, RoleAArn, RoleBArn) are published, and the whole change-set/in-place-update flow that swaps Allow/Deny between two !GetAtt-resolved role ARNs is never exercised. Because it surfaces as a handler InvalidRequest with raw parser text, an IaC user reads it as a fault in their own template and cannot tell which property is wrong.

Fix: the CloudFormation AWS::OpenSearchService::Domain resource provisioner's property-to-request mapping — after intrinsic resolution, a policy-document property must be serialized to a JSON string before being placed in the CreateDomain body so forwarded members match the modeled member types. Secondarily, the OpenSearch CreateDomain input deserializer/handler must report a genuine type mismatch through a modeled client error whose message names the member and the expected type.

Traces: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place (fails in _create_stack_tolerating_role_propagation, before any change set is created).

## Evidence
Replay report, service cloudformation__iam__opensearch__sts:

  [mismatch] cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place
  File "check_...deploy_opensearch_1_changeset_swaps_allowed_and_denied_roles_in_place.py", line 143, in run
    stack_id, domain_name = _create_stack_tolerating_role_propagation(ctx, cfn, osc)
  File "...", line 402, in _create_stack_tolerating_role_propagation
    raise AssertionError(f"stack {stack_name} failed to create ({status}): {reasons!r}")
  AssertionError: stack sim2real-xsvc-os1-<id> failed to create (ROLLBACK_COMPLETE):
    ['sim2real-xsvc-os1-<id>: CREATE_FAILED: The following resource(s) failed to create: [Domain]. ',
     'Domain: CREATE_FAILED: Resource handler returned message: "Invalid request provided: expected string, got object start (Service: OpenSearch, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)']

Template under test (one stack): RoleA and RoleB (AWS::IAM::Role, each with an inline Allow on es:ESHttpGet/Head/Post/Put for the domain ARN) and Domain (AWS::OpenSearchService::Domain, DependsOn both roles) with DomainName !Ref, EngineVersion OpenSearch_2.11, ClusterConfig {InstanceType t3.small.search, InstanceCount 1, DedicatedMasterEnabled false, ZoneAwarenessEnabled false}, EBSOptions {EBSEnabled true, VolumeType gp3, VolumeSize 10}, DomainEndpointOptions {EnforceHTTPS true, TLSSecurityPolicy Policy-Min-TLS-1-2-2019-07}, Tags, and AccessPolicies written as a policy-document OBJECT whose two statements have Principal.AWS = !GetAtt RoleA.Arn / !GetAtt RoleB.Arn and Resource = !Sub 'arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/${DomainName}/*'. Outputs: DomainArn, DomainEndpoint, RoleAArn, RoleBArn.

Expected (AWS): stack reaches CREATE_COMPLETE; all four Outputs carry resolved OutputValues; DescribeDomain returns AccessPolicies as a JSON STRING whose statements resolve to {RoleAArn: Allow, RoleBArn: Deny} with ARN/Endpoint matching the Outputs. The check then creates an UPDATE change set swapping the two Effects and asserts it touches only the Domain.

Actual (emulator): CreateDomain for that resource answers HTTP 400 with service message 'Invalid request provided: expected string, got object start'; the Domain resource fails InvalidRequest; the stack ends ROLLBACK_COMPLETE with no Outputs and no domain, so nothing downstream — change set, in-place update identity, data-plane verdict swap — is evaluated.

Message detail: fixed text = 'Invalid request provided: expected string, got object start' (emitted unchanged for any object-where-string body); variable parts of the CloudFormation frame = Request ID and RequestToken. AWS's equivalent refusal is a modeled ValidationException/BaseException (HTTP 400) whose message names the member ('AccessPolicies') and the expected type.

Smithy citation: com.amazonaws.opensearch#CreateDomain, member AccessPolicies -> PolicyDocument (string, @length 0..102400, @pattern '.*'); errors ValidationException {client, 400}, BaseException {client, 400}.

## Steps to Reproduce
1. Boot the emulator; confirm CloudFormation, IAM, OpenSearch and STS answer.
2. CreateStack (Capabilities=['CAPABILITY_IAM'], Parameters=[{DomainName: <3..28 char lowercase name>}]) with a template containing two AWS::IAM::Role resources and one AWS::OpenSearchService::Domain that DependsOn both, where the Domain's Properties include ClusterConfig {InstanceType t3.small.search, InstanceCount 1, DedicatedMasterEnabled false, ZoneAwarenessEnabled false}, EBSOptions {EBSEnabled true, VolumeType gp3, VolumeSize 10}, DomainEndpointOptions {EnforceHTTPS true, TLSSecurityPolicy Policy-Min-TLS-1-2-2019-07}, and AccessPolicies declared as a nested policy-document OBJECT with two statements whose Principal.AWS are !GetAtt RoleA.Arn and !GetAtt RoleB.Arn (one Allow, one Deny) and whose Resource is !Sub 'arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/${DomainName}/*'. Declare Outputs DomainArn, DomainEndpoint, RoleAArn, RoleBArn.
3. Poll DescribeStacks / DescribeStackEvents to a terminal status. Observe the Domain resource CREATE_FAILED with ResourceStatusReason containing 'Invalid request provided: expected string, got object start (Service: OpenSearch, Status Code: 400, ...)' and the stack reason 'The following resource(s) failed to create: [Domain]. '; the stack ends ROLLBACK_COMPLETE. On AWS it reaches CREATE_COMPLETE.
4. Narrow it to the request shape: call opensearch.create_domain directly with the same values but AccessPolicies=json.dumps(<policy>) — that must be accepted with HTTP 200 and a DomainStatus. Then inspect the CreateDomain body the CloudFormation provisioner emitted; if AccessPolicies is an object there, that is the defect.
5. Confirm the message separately: send CreateDomain with AccessPolicies as a JSON object. AWS refuses under a modeled client error (ValidationException/BaseException, 400) naming the member and the expected type; the emulator answers 400 'Invalid request provided: expected string, got object start'.
6. After the fix: the stack must reach CREATE_COMPLETE, all four Outputs must carry resolved OutputValues, DescribeDomain must return AccessPolicies as a JSON string resolving to {RoleAArn: Allow, RoleBArn: Deny}, and an UPDATE change set that only swaps the two Effects must report a change touching only the Domain.
7. Re-replay cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place.
