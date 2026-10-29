Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected|run#20|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity|_poll#5|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation|_wait_master_auth#8|TimeoutError|raise TimeoutError(f"master user's basic-auth call to {base}: the run budget ended first; not graded")
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# opensearch: two net-new causes — UpdateVpcEndpoint accepts a subnet from another VPC, and the published domain REST data plane never answers, so the blue-green scale-out and cross-cluster-search scenarios never produce a response

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateVpcEndpoint

## Description
TWO distinct net-new root causes in the `opensearch` service, grounded in 3 failing behavioral checks that the replay report marks neither ALREADY FILED nor OWNED and whose check ids appear in no open issue's Issue-Trace-Ids. Both share the service token `opensearch` and one gate verdict, so they are bundled here. A fixer must close BOTH causes before resubmitting; each cause lists its own failing check ids.

=== Cause 1 of 2: UpdateVpcEndpoint accepts VpcOptions whose SubnetIds belong to a different VPC than the endpoint's, with no error at all ===
Failing check ids: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected.
AWS: Smithy com.amazonaws.opensearch#UpdateVpcEndpoint — POST /2021-01-01/opensearch/vpcEndpoints/update, success 200, output VpcEndpoint (required); input VpcEndpointId (@required, @length 5..256, pattern ^aos-[a-zA-Z0-9]*$) and VpcOptions (@required, VPCOptions). Modeled client errors: ValidationException (400), BaseException (400, defaulted), ConflictException (409), DisabledOperationException (409), ResourceNotFoundException (409). The model states no cross-VPC rule, so the rule itself is recorded behavior only: an endpoint's VPC is fixed at creation (DescribeVpcEndpoints reports the endpoint's VPCDerivedInfo/VPCId), so a VpcOptions whose SubnetIds resolve to a VPC other than the endpoint's is refused with ValidationException (HTTP 400) BEFORE any state is written — an independent DescribeVpcEndpoints still reports the ORIGINAL SubnetIds — while the same update with a subnet from the SAME VPC is accepted (200) and does persist. The refusal message's variable parts are the offending subnet id(s) and the endpoint's VPC id, taken from the request and from the endpoint's stored state; its fixed text is the surrounding validation sentence AWS words for this class. Nothing in the recorded refusal is a per-run value beyond those request/state values.
Emulator: the call raises nothing — the check's error-code probe returns None, i.e. HTTP 200 and the cross-VPC VpcOptions is accepted (`AssertionError: expected ValidationException, got None`). The endpoint is thereby reported as living in a VPC whose subnet its ENIs could never occupy.
Why: a client relying on AWS's behavior believes it can relocate an OpenSearch VPC endpoint between VPCs. The refusal that tells an operator the subnet is in the wrong VPC never arrives, and the emulator's DescribeVpcEndpoints then reports a topology AWS would never allow, so drift detection and least-privilege/network reviews get the opposite answer.
Fix: the VpcOptions validation in the UpdateVpcEndpoint handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_vpc_endpoint.rs) — resolve each requested subnet's VPC through the EC2 state and compare it with the endpoint's stored VPCId, raising the modeled ValidationException before the endpoint record is touched. Related open issue that does NOT carry this check's replay key: 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md describes the same rule as its cause 10 but is keyed only to opensearch.update-vpc-endpoint-invalid-subnet-rejected and opensearch.update-vpc-endpoint-out-of-range-config-rejected; this check (a cross-VPC subnet reached through a second VPC/subnet pair and an ACTIVE endpoint) is additionally failing and is not registered anywhere, so it needs this replay key.
Traces: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected.

=== Cause 2 of 2: the domain REST data plane published in DomainStatus.Endpoint never answers, so both deep data-plane scenarios exhaust their budget without any response ===
Failing check ids: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity, opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation.
AWS: Smithy com.amazonaws.opensearch#DescribeDomain/CreateDomain publish DomainStatus.Endpoint; what that host serves is recorded behavior only — the host the control plane advertises is an OpenSearch engine, which answers a SigV4('es')-signed GET /_cluster/health with HTTP 200 and a cluster-health body, and (on an FGAC domain) answers the configured master user's HTTP-basic request on the same path with 200 rather than refusing it. Everything the two checks grade hangs off that first 200: _bulk indexing, a match_all _search holding the same document set while an InstanceCount 2->3 change is in flight, _cat/nodes reporting the new node, _cat/shards placing a copy on it, and, for the second check, cross-cluster search through a CreateOutboundConnection peer.
Emulator: neither check ever obtains that 200. The blue-green check dies in its first data-plane poll (`_poll(lambda: _signed_request(ctx, "GET", f"{base}/_cluster/health"), ...)`) with `TimeoutError: HTTP 200 from signed _cluster/health: the run budget ended first; not graded`; the cross-cluster check dies in `_wait_master_auth` with `TimeoutError: master user's basic-auth call to <endpoint>: the run budget ended first; not graded`. The published endpoint is reachable but serves no `es` engine (the recorded answer for this emulator on that host is HTTP 404 with body {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}), so a poll for 200 can never succeed and the checks are killed by the budget instead of returning a verdict.
Why: a client that stores or queries documents against an emulated domain gets an answer that is not an OpenSearch answer at all, and no error class it can branch on; the blue-green data-continuity guarantee (search keeps answering 200 with the same documents across a topology change) and the cross-cluster search path are both unverifiable and unusable.
Fix: the emulator's request router and domain data plane for the OpenSearch service — serve the published domain endpoint host/`es` prefix with a domain-scoped index+search engine that answers _cluster/health, _bulk, _search, _cat/nodes and _cat/shards, and honours both SigV4('es') and FGAC basic-auth on it; the node/shard views must reflect the ClusterConfig the control plane records. Related open issues that describe this gap but carry NEITHER of these two check ids: 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md (its cause 1) and 20261029T065036Z-iam-opensearch-sts-a-published-fgac-opensearch-domain-endpoint-never-answers.md (the FGAC basic-auth variant, under a different service token). These two checks are the only grounding for the blue-green and cross-cluster scenarios and have no replay key anywhere.
Traces: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity (first probe: signed GET /_cluster/health never 200), opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation (master-user basic-auth call to the local domain endpoint never 200).

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1, service token `opensearch` (76/132 passed). The three checks below are not marked ALREADY FILED or OWNED and their ids appear in no open issue.

Cause 1 — [mismatch] opensearch.update-vpc-endpoint-out-of-range-subnet-rejected
  Setup: VPC A (10.90.0.0/16) with subnet A and security groups sg-a/sg-a2; VPC B (10.91.0.0/16) with subnet B; a VPC domain in VPC A (t3.small.search x1, gp3 10 GiB, VPCOptions {SubnetIds:[subnet A], SecurityGroupIds:[sg-a]}) waited to Processing=False; CreateVpcEndpoint(DomainArn=<domain arn>, VpcOptions={SubnetIds:[subnet A], SecurityGroupIds:[sg-a]}) waited ACTIVE.
  Request: UpdateVpcEndpoint(VpcEndpointId=<aos-...>, VpcOptions={SubnetIds:[<subnet in VPC B>], SecurityGroupIds:[sg-a]}).
  Expected (AWS): HTTP 400 ValidationException naming the out-of-VPC subnet and the endpoint's VPC; DescribeVpcEndpoints(<id>) still reporting VpcOptions.SubnetIds == [<subnet A>]; a following same-VPC update (SecurityGroupIds [sg-a, sg-a2]) accepted and persisted.
  Actual (emulator): no error raised at all — `AssertionError: expected ValidationException, got None` at check line 68/179 (`assert code == "ValidationException"`), i.e. the cross-VPC VpcOptions was accepted with HTTP 200.
  Smithy citation: UpdateVpcEndpoint — POST /2021-01-01/opensearch/vpcEndpoints/update, 200; input {VpcEndpointId @required pattern ^aos-[a-zA-Z0-9]*$, VpcOptions @required}; errors ValidationException {client,400}, BaseException {client,400,defaulted}, ConflictException/DisabledOperationException/ResourceNotFoundException {client,409}, InternalException {server,500}. DescribeVpcEndpoints — POST /2021-01-01/opensearch/vpcEndpoints/describe, output VpcEndpoints @required (the endpoint's VPCDerivedInfo is where its fixed VPCId is read back).

Cause 2 — [error] opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity
  Traceback: check line 230 -> `_poll(lambda: _signed_request(ctx, "GET", f"{base}/_cluster/health"), ...)` -> check line 194 -> `TimeoutError: HTTP 200 from signed _cluster/health: the run budget ended first; not graded`.
  Setup already passed: CreateDomain (OpenSearch_2.13, m5-class/t3 2 nodes per the check, gp3 10 GiB, account-root es:* AccessPolicies) and `_wait_settled` to Processing=False with a published Endpoint — so the control plane advertised an endpoint and the data plane behind it never answered 200.
  Expected (AWS): 200 from the signed GET /_cluster/health on DomainStatus.Endpoint, then _bulk of 24 docs (errors=false), a match_all _search returning (200, 24, <ids>), the same probe holding 200/24/<ids> at every poll while UpdateDomainConfig InstanceCount 2->3 is in flight, _cat/nodes reporting exactly 3 nodes with a newly joined name, and 6 shard copies STARTED across those 3 nodes.
  Actual (emulator): the first probe never returns 200; the recorded answer of this emulator on a published domain endpoint is HTTP 404 {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}, so no graded data-plane assertion is ever evaluated and the check is killed by the run budget.

Cause 2 — [error] opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation
  Traceback: check line 290 -> `_wait_master_auth(ctx, local_base, master_user, master_password)` -> check line 223 -> `TimeoutError: master user's basic-auth call to https://<host>:<port>: the run budget ended first; not graded` (the host:port is the harness's local rewrite of the published endpoint and is a per-run value, not part of the requirement).
  Expected (AWS): the FGAC master user's HTTP-basic request to the published endpoint answers 200, after which the check indexes distinct document sets into the two peered domains and verifies a cross-cluster search returns the REMOTE set.
  Actual (emulator): the master user's basic-auth call never succeeds against the published endpoint, so the cross-cluster legs are never reached.

## Steps to Reproduce
Boot the emulator and confirm opensearch, ec2 and sts answer.

Cause 1 (UpdateVpcEndpoint cross-VPC subnet):
1. ec2 CreateVpc 10.90.0.0/16 (VPC A); CreateSubnet 10.90.1.0/24 in an AZ; CreateSecurityGroup sg-a and sg-a2 in VPC A.
2. ec2 CreateVpc 10.91.0.0/16 (VPC B); CreateSubnet 10.91.1.0/24 in the same AZ.
3. opensearch CreateDomain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, VPCOptions={'SubnetIds':[<subnet A>],'SecurityGroupIds':[<sg-a>]}) and poll DescribeDomain until Processing=False.
4. opensearch CreateVpcEndpoint(DomainArn=<domain ARN>, VpcOptions={'SubnetIds':[<subnet A>],'SecurityGroupIds':[<sg-a>]}) and poll DescribeVpcEndpoints until Status=ACTIVE.
5. opensearch UpdateVpcEndpoint(VpcEndpointId=<aos-...>, VpcOptions={'SubnetIds':[<subnet B>],'SecurityGroupIds':[<sg-a>]}). AWS: HTTP 400 ValidationException naming the subnet that is not in the endpoint's VPC; emulator: HTTP 200, no error.
6. DescribeVpcEndpoints(VpcEndpointIds=[<id>]) — AWS still reports SubnetIds == [<subnet A>]; after the fix the refused update must leave the stored endpoint untouched.
7. Positive control that must keep working: UpdateVpcEndpoint with VpcOptions={'SubnetIds':[<subnet A>],'SecurityGroupIds':[<sg-a>, <sg-a2>]} is accepted and DescribeVpcEndpoints reports both security groups.

Cause 2 (domain data plane never answers):
8. opensearch CreateDomain(EngineVersion='OpenSearch_2.13', ClusterConfig InstanceCount=2, EBSOptions gp3/10, AccessPolicies allowing arn:aws:iam::<account>:root es:* on arn:aws:es:<region>:<account>:domain/<name>/*) and poll DescribeDomain until Processing=False with a published Endpoint.
9. Send a SigV4('es')-signed GET https://<DomainStatus.Endpoint>/_cluster/health. AWS: HTTP 200 with a cluster-health body; emulator: never 200 (404 {"__type":"UnknownService","message":"no emulated service is registered for 'es'"}).
10. Repeat for the FGAC variant: CreateDomain with AdvancedSecurityOptions.MasterUserOptions (internal master user/password), then call GET /_cluster/health on the published endpoint with HTTP basic auth as that master user. AWS: 200; emulator: never 200.
11. After the fix, re-replay opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity and opensearch.osx-deep-opensearch-3-cross-cluster-search-propagation so the graded legs (_bulk, match_all _search continuity across the InstanceCount 2->3 change, _cat/nodes, _cat/shards, and the cross-cluster search returning the remote document set) actually run.

Finally, re-replay the three check ids listed in trace_ids.
