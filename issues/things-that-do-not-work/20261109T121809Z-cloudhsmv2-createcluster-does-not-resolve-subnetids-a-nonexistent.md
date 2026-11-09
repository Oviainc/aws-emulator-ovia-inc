Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudhsmv2.cluster-create-nonexistent-subnet-rejected-with-positive-control cloudhsmv2.cluster-duplicate-subnet-collapses-to-one-az-then-lifecycle
Issue-Shape-Id: com.amazonaws.cloudhsmv2#BaldrApiService
Issue-Key: cloudhsmv2.cluster-create-nonexistent-subnet-rejected-with-positive-control|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudhsm-v2|CreateCluster|CloudHsmInvalidRequestException
Issue-Commit: 9cecc173ac9c5f86564428d1030fb1c1f0593211

# cloudhsmv2 CreateCluster does not resolve SubnetIds: a nonexistent subnet is accepted, and a duplicate subnet is refused instead of collapsing to one AZ

> _Note: this issue was identified against the code base at commit `9cecc173ac9c5f86564428d1030fb1c1f0593211`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Both behavioral checks under the `cloudhsmv2` heading fail inside CreateCluster's SubnetIds handling, in two opposite directions: input AWS refuses is accepted, and input AWS accepts is refused. There are 2 causes in this issue and both must be closed.

Cause 1 of 2: CreateCluster never resolves the requested subnet ids, so a well-formed but nonexistent subnet is accepted and a cluster is minted.
AWS: Smithy (com-amazonaws-cloudhsmv2-baldrapiservice): CreateCluster.SubnetIds @required, @length(1,10), member SubnetId @pattern ^subnet-[0-9a-fA-F]{8,17}$; CreateCluster models CloudHsmResourceNotFoundException (errorKind client, httpError 400, member Message). A subnet id that matches the pattern but names no subnet in the account is refused HTTP 400 with __type/Error.Code CloudHsmResourceNotFoundException and Message whose fixed text is `The subnet ID '` ... `' does not exist` — the only variable part is the subnet id copied from the request — and no cluster is created (the check re-reads DescribeClusters(Filters={vpcIds:[<own VPC>]}) and requires it still empty). Against a real subnet the same call succeeds, and the resulting SubnetMapping is {<that subnet's actual AZ>: <that subnet id>} (Smithy: Cluster.SubnetMapping, "A map from availability zone to the cluster's subnet in that availability zone"), i.e. the AZ comes from the subnet, not from the region.
Emulator: answers HTTP 200 and returns a Cluster (State CREATE_IN_PROGRESS, a minted cluster-/vpc-/sg- id) for SubnetIds=['subnet-fffffffffffffffff'], and reports SubnetMapping {'us-west-2a': 'subnet-fffffffffffffffff'} — an AZ synthesized from the request region rather than looked up. The handler validates HsmType against its own catalog and rejects duplicates but performs no subnet lookup at all.
Why: a client relying on CreateCluster to reject a bad subnet gets a cluster id for a cluster that could never exist; provisioning code and any negative test asserting the not-found refusal silently pass, and the SubnetMapping the client records does not describe where the cluster actually is, so flows that pair the cluster with its network (and the CloudFormation CloudHSM resource, which reads SubnetMapping back) are wrong.
Fix: CreateCluster's validation (the handler's SubnetIds stage, services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/create_cluster.rs) must resolve each requested subnet id against the subnets the emulator's state knows and refuse an unknown one with CloudHsmResourceNotFoundException and AWS's message before any cluster is minted; the stored SubnetMapping must be keyed by each resolved subnet's own AZ instead of being synthesized from the region.
Already-filed note: open issue 20261109T115808Z-cloudhsmv2-createcluster-describeclusters-diverge-validation-refusals-and-cluster.md (wire corpus) describes the same missing subnet resolution as its Cause 2; this behavioral check is grounded in a different replay unit and is filed here so the behavioral key is carried. A fixer closing that issue's Cause 2 should re-replay this check too.
Traces: cloudhsmv2.cluster-create-nonexistent-subnet-rejected-with-positive-control.

Cause 2 of 2: CreateCluster refuses a request that names the same subnet twice, which AWS accepts and de-duplicates.
AWS: Smithy: SubnetIds is a plain `list` of SubnetId with length 1..10 and NO @uniqueItems, so repeating a member is a well-formed request. Recorded behavior: the same subnet given twice is accepted — HTTP 200, Cluster.State CREATE_IN_PROGRESS — and collapses to a single-AZ cluster whose SubnetMapping is {<that subnet's AZ>: <that subnet id>} (one entry). The cluster then settles to UNINITIALIZED with Hsms == [], BackupPolicy == DEFAULT, a provisioned SecurityGroup and Certificates == {}; tags round-trip through TagResource/ListTags/UntagResource; ModifyCluster(BackupRetentionPolicy={Type:DAYS,Value:'200'}) reports and persists the change; DeleteCluster reports DELETE_IN_PROGRESS and afterwards ListTags on the id is refused CloudHsmResourceNotFoundException with Message fixed text `The resource '` ... `' was not found.` (variable part: the cluster id from the request). None of that is reached today.
Emulator: refuses with CloudHsmInvalidRequestException and Message `SubnetIds names <subnetId> more than once; specify only one subnet per Availability Zone` — an invented refusal (create_cluster.rs lines 192-201 reject any repeated element), the subnet id being the only value taken from the request.
Why: a client that builds SubnetIds by concatenating per-AZ lists, or that passes a one-subnet list twice, has its create rejected where AWS would have succeeded; the de-duplication AWS performs is a documented tolerance, and a hard refusal makes the emulator strictly less permissive than AWS, which no retry can recover from.
Fix: the SubnetIds stage of the CreateCluster handler must de-duplicate instead of refusing: reject only genuinely conflicting input (two DIFFERENT subnets in the same Availability Zone, once subnets are resolved per cause 1) and build SubnetMapping with one entry per AZ. The duplicate-element loop in create_cluster.rs must go.
Traces: cloudhsmv2.cluster-duplicate-subnet-collapses-to-one-az-then-lifecycle.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/4 passed; per-service cloudhsmv2 0/2 (0%), bootstrap_signal false.

Cause 1 — [mismatch] cloudhsmv2.cluster-create-nonexistent-subnet-rejected-with-positive-control:
  Call: create_cluster(HsmType='hsm2m.medium', Mode='FIPS', SubnetIds=['subnet-fffffffffffffffff'], TagList=[{'Key':'sim2real',...}])
  Expected: ClientError, Error.Code == 'CloudHsmResourceNotFoundException', Error.Message == "The subnet ID 'subnet-fffffffffffffffff' does not exist", and DescribeClusters(Filters={vpcIds:[<run VPC>]}) still empty.
  Actual: HTTP 200 — AssertionError: expected the call to be refused, but it succeeded: {'Cluster': {'BackupPolicy': 'DEFAULT', 'BackupRetentionPolicy': {'Type': 'DAYS', 'Value': '90'}, 'ClusterId': 'cluster-...', 'Hsms': [], 'HsmType': 'hsm2m.medium', 'SecurityGroup': 'sg-...', 'State': 'CREATE_IN_PROGRESS', 'SubnetMapping': {'us-west-2a': 'subnet-fffffffffffffffff'}, 'VpcId': 'vpc-...', 'NetworkType': 'IPV4', 'Mode': 'FIPS', ...}} (ids/timestamps are per-run values, not requirements; the 200 and the AZ synthesized for an id that resolves to nothing are).

Cause 2 — [error] cloudhsmv2.cluster-duplicate-subnet-collapses-to-one-az-then-lifecycle:
  Call: create_cluster(HsmType='hsm2m.medium', Mode='FIPS', SubnetIds=[<subnet>, <same subnet>], TagList=[...])
  Expected: HTTP 200, Cluster.State == 'CREATE_IN_PROGRESS', Cluster.SubnetMapping == {<subnet AZ>: <subnet>} (a single entry), then the UNINITIALIZED / tag round-trip / ModifyCluster / DeleteCluster / ListTags-not-found sequence described above.
  Actual: botocore.errorfactory.CloudHsmInvalidRequestException: An error occurred (CloudHsmInvalidRequestException) when calling the CreateCluster operation: SubnetIds names subnet-00000000000000098 more than once; specify only one subnet per Availability Zone

Model: CreateCluster.SubnetIds -> com.amazonaws.cloudhsmv2#SubnetIds = list of SubnetId (pattern ^subnet-[0-9a-fA-F]{8,17}$), length 1..10, no uniqueItems; CreateCluster errors include CloudHsmResourceNotFoundException (client, 400) and CloudHsmInvalidRequestException (client, 400).
Source: services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/create_cluster.rs lines 192-201 (duplicate-element refusal), line 248 (subnet_mapping built from the request region, no subnet lookup), line 239 (store.mint_vpc_id()).

## Steps to Reproduce
1. git_clone, then run_behavioral_suite().
2. Cause 1: replay cloudhsmv2.cluster-create-nonexistent-subnet-rejected-with-positive-control. It first calls CreateCluster with HsmType='hsm2m.medium', Mode='FIPS' and SubnetIds naming one well-formed subnet id that exists nowhere ('subnet-' + 'f'*17). Observe HTTP 200 and a minted cluster instead of 400 CloudHsmResourceNotFoundException "The subnet ID '<subnetId>' does not exist"; note SubnetMapping keys an AZ for that unresolvable id. The positive-control leg (the same call against a real subnet of the run's own VPC, then DeleteCluster) must still succeed after the fix, with SubnetMapping == {<that subnet's AZ>: <that subnet>}.
3. Cause 2: replay cloudhsmv2.cluster-duplicate-subnet-collapses-to-one-az-then-lifecycle. It calls CreateCluster with SubnetIds=[subnet_a, subnet_a]. Observe the 400 CloudHsmInvalidRequestException 'SubnetIds names <subnetId> more than once; specify only one subnet per Availability Zone' instead of a 200 whose Cluster.SubnetMapping has the single entry {<AZ of subnet_a>: subnet_a}.
4. After the fix, re-replay both check ids in trace_ids; cause 2's check continues through UNINITIALIZED (Hsms [], BackupPolicy DEFAULT, SecurityGroup present, Certificates {}), TagResource/ListTags/UntagResource, ModifyCluster BackupRetentionPolicy DAYS/200 persistence, DeleteCluster DELETE_IN_PROGRESS and ListTags refused CloudHsmResourceNotFoundException "The resource '<clusterId>' was not found."
