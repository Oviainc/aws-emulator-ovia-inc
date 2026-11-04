Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.access-policy-denies-data-plane-index-path opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password opensearch.overly-permissive-vpc-endpoint-update-invalid-security-group-rejected opensearch.overly-permissive-vpc-endpoint-update-malformed-sg-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.access-policy-denies-data-plane-index-path|_cat_index_names#20|AssertionError|assert status == 200, f"_cat/indices failed with HTTP {status}: {listing}"
Issue-Key: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password|run#11|AssertionError|assert ear.get("Enabled") is True and ear.get("KmsKeyId", "").startswith("arn:"), (
Issue-Key: opensearch.overly-permissive-vpc-endpoint-update-invalid-security-group-rejected|_error_code#21|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: opensearch.overly-permissive-vpc-endpoint-update-malformed-sg-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 71ad45cb8263c75348f691f106f1f184e4e6c4d9

# opensearch: 4 net-new causes — the domain data plane has no _cat/indices handler, DescribeDomain omits the default KMS key ARN for encryption at rest, and UpdateVpcEndpoint accepts nonexistent and malformed security-group ids

> _Note: this issue was identified against the code base at commit `71ad45cb8263c75348f691f106f1f184e4e6c4d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateVpcEndpoint

## Description
FOUR distinct net-new root causes in the `opensearch` service, grounded in 4 failing behavioral checks that the replay report marks neither ALREADY FILED nor OWNED. All four share the service token `opensearch` and one gate verdict, so they are bundled here. A fixer must close ALL FOUR causes before resubmitting; each cause lists its own failing check ids.

=== Cause 1 of 4: the domain REST data plane has no `_cat/indices` handler — it answers HTTP 400 "no handler found" instead of 200 with the index listing ===
Failing check ids: opensearch.access-policy-denies-data-plane-index-path.
AWS: recorded behavior only (the Smithy model covers only the control plane; DomainStatus.Endpoint publishes an OpenSearch engine host). The engine behind a domain endpoint serves the _cat APIs: a SigV4('es')-signed `GET <endpoint>/_cat/indices?format=json` answers HTTP 200 with a JSON array, one object per existing index, each carrying at least an `index` member naming it. The listing reflects actual state: an index created by a successful write appears, an index whose creating write was refused by the domain access policy does NOT appear, and an index removed by `DELETE <endpoint>/<index>` disappears from a following call. The surrounding legs the check already got right (policy-scoped 403 on the denied index path, 200 on the allowed one) prove the endpoint and signing work; only this URI is unrouted.
Emulator: `GET <endpoint>/_cat/indices?format=json` answers HTTP 400 with the body {"error": "no handler found for uri [/_cat/indices] and method [GET]"}. Replay: `AssertionError: _cat/indices failed with HTTP 400: {'error': 'no handler found for uri [/_cat/indices] and method [GET]'}`. The status is wrong (400 for an unrouted engine URI; AWS answers 200 here, and for a genuinely unknown URI an OpenSearch engine answers 400 with an `illegal_argument_exception`-shaped error body, not a bare `error` string), so no "effect absent"/"effect present" assertion on index existence can be made at all.
Why: a client cannot enumerate a domain's indices. Every operational flow built on the _cat APIs (verifying that a refused write created nothing, confirming a delete, listing indices before a reindex) gets a 400 it cannot distinguish from a bad request, so index-level state is unobservable even where the rest of the data plane answers correctly.
Fix: the domain data plane's REST router in the OpenSearch service crate — route the `_cat/*` family (starting with `GET /_cat/indices`, honouring `format=json`) to the domain's index catalog and answer 200 with one entry per existing index; unknown engine URIs should keep the engine's own error shape rather than a bare `error` string. Related open issue that describes the same data plane but does NOT carry this check's replay key: 20261101T140814Z-opensearch-the-domain-rest-data-plane-serves-only.md.
Traces: opensearch.access-policy-denies-data-plane-index-path (the 403 legs already pass; the check dies on the first _cat/indices call).

=== Cause 2 of 4: EncryptionAtRestOptions read back from DescribeDomain omits KmsKeyId when the create request enabled encryption without naming a key ===
Failing check ids: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password.
AWS: Smithy com.amazonaws.opensearch#EncryptionAtRestOptions has members Enabled, KmsKeyId (@length 1..500) and EncryptionMode; CreateDomain accepts EncryptionAtRestOptions and DescribeDomain returns DomainStatus.EncryptionAtRestOptions. Recorded behavior for the resolution rule: when CreateDomain is given EncryptionAtRestOptions={"Enabled": true} with no KmsKeyId, the service resolves the account's service-default OpenSearch key and DescribeDomain reports BOTH Enabled=true AND KmsKeyId as a full KMS key ARN (an `arn:`-prefixed value — the account id, region and key id in it are per-account/per-run values, the requirement is that a resolved key ARN is present, not any particular one).
Emulator: DescribeDomain returns `{'Enabled': True}` — KmsKeyId is absent entirely. Replay: `AssertionError: at-rest encryption should be on with a default KMS key ARN filled in, got {'Enabled': True}`.
Why: a client that reads back which key encrypts a domain (compliance evidence, a key-rotation or key-policy audit, or an IaC provider computing drift against a declared KmsKeyId) finds the field missing and concludes no key is attached, even though encryption reports as enabled. Code that unconditionally dereferences the resolved ARN breaks.
Fix: the CreateDomain handler's EncryptionAtRestOptions normalization and the DomainStatus projection in the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs plus the stored domain state the describe path serializes) — on Enabled=true with no KmsKeyId, resolve and store the account's default OpenSearch KMS key ARN, and emit it in DomainStatus/DomainConfig.EncryptionAtRestOptions.KmsKeyId.
Traces: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password (the check's later legs — gp3 3000 IOPS/125 MiB/s defaults, AdvancedSecurityOptions round-trip, the write-only master password, the access-policy round-trip — are not reached).

=== Cause 3 of 4: UpdateVpcEndpoint accepts VpcOptions.SecurityGroupIds naming a security group that does not exist ===
Failing check ids: opensearch.overly-permissive-vpc-endpoint-update-invalid-security-group-rejected.
AWS: Smithy com.amazonaws.opensearch#UpdateVpcEndpoint — POST /2021-01-01/opensearch/vpcEndpoints/update, success 200, output VpcEndpoint (@required); input VpcEndpointId (@required, @length 5..256, pattern ^aos-[a-zA-Z0-9]*$) and VpcOptions (@required). Modeled client errors: ValidationException (400), BaseException (400, defaulted), ConflictException (409), DisabledOperationException (409), ResourceNotFoundException (409). The existence rule is recorded behavior only: a SecurityGroupIds entry that is well-formed (`sg-<hex>`) but resolves to no security group in the account/region is refused SYNCHRONOUSLY with HTTP 400 ValidationException whose message is `Invalid id: "<id>"` — fixed text `Invalid id: ` plus the offending id, quoted, taken verbatim from the request. Nothing is written: DescribeVpcEndpoints still reports Status ACTIVE and the ORIGINAL SecurityGroupIds. The same update naming a real security group in the endpoint's VPC is accepted (200).
Emulator: the call returns HTTP 200 and the bogus group is stored. Replay: `AssertionError: expected the call to be rejected, but it succeeded and returned {... 'VpcEndpoint': {'VpcEndpointId': 'aos-...', 'VpcOptions': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['sg-00000000000000000']}, 'Status': 'UPDATING', ...}}` — the endpoint is moved to UPDATING with a security group that cannot exist.
Why: a client gets no refusal for a typo'd or deleted security group and is told the endpoint is reconfiguring toward it. The endpoint then reports a network posture no ENI could ever have, so security review, drift detection and negative-path tests that assert the ValidationException all get the opposite answer.
Fix: the VpcOptions validation in the UpdateVpcEndpoint handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_vpc_endpoint.rs) — resolve every requested SecurityGroupIds entry against the EC2 state and raise the modeled ValidationException with AWS's `Invalid id: "<id>"` wording BEFORE the endpoint record is touched or its Status changed. Related open issue that validates the SUBNET side of the same input but carries neither of these two check ids: 20261029T072536Z-opensearch-two-net-new-causes-updatevpcendpoint-accepts-a.md.
Traces: opensearch.overly-permissive-vpc-endpoint-update-invalid-security-group-rejected.

=== Cause 4 of 4: UpdateVpcEndpoint accepts a SecurityGroupIds entry that is not even of the `sg-...` shape ===
Failing check ids: opensearch.overly-permissive-vpc-endpoint-update-malformed-sg-rejected.
AWS: same operation and modeled errors as cause 3. The SYNTAX rule is a distinct refusal class with its own wording, recorded behavior only: an identifier that does not match the `sg-<hex>` shape EC2 requires is refused synchronously with HTTP 400 ValidationException and the message `Invalid id: "<id>" (expecting "sg-...")` — fixed text `Invalid id: ` ... ` (expecting "sg-...")` around the offending id taken verbatim from the request. Nothing is written: DescribeVpcEndpoints still reports the original SecurityGroupIds, and the identical update with the real group is accepted (200) and settles back to ACTIVE with that group.
Emulator: the call returns HTTP 200 and stores the malformed string. Replay: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'VpcEndpoint': {'VpcEndpointId': 'aos-...', 'VpcOptions': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['not-a-security-group!!']}, 'Status': 'UPDATING', ...}}`.
Why: a client receives a success for input AWS rejects outright, and the endpoint is left advertising an identifier that is not an identifier. The failure surfaces, if at all, only as an asynchronous UPDATING transition instead of the synchronous, actionable 400 AWS returns, so a caller cannot tell a bad request from a transient one.
Fix: the same VpcOptions validation in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_vpc_endpoint.rs — apply the id-shape check before the existence check and emit the distinct `(expecting "sg-...")` wording for the malformed class, leaving the stored endpoint and its Status untouched.
Traces: opensearch.overly-permissive-vpc-endpoint-update-malformed-sg-rejected.

## Evidence
Replay report, suite simparity-behavioral-aws-v1, service heading `### opensearch` (137/256 passed, 119 failed). The four checks below are the only ones under that heading marked neither ALREADY FILED nor OWNED.

Cause 1 — [mismatch] opensearch.access-policy-denies-data-plane-index-path
  Setup already passing: CreateDomain (OpenSearch_2.11, t3.small.search x1, DedicatedMasterEnabled=false, ZoneAwarenessEnabled=false, gp3 10 GiB), waited to Processing=False with a published Endpoint, UpdateDomainConfig AccessPolicies to a policy that Allows es:* on the allowed index path and Denies it on the denied one, waited Active. The policy legs then PASSED: signed PUT to the allowed index -> 200/201, POST _refresh -> 200, POST _search -> 200 with exactly one hit; signed PUT and _search on the Deny-covered index path -> HTTP 403 with a "not authorized" body.
  Request: SigV4('es')-signed `GET <endpoint>/_cat/indices?format=json`.
  Expected (AWS): HTTP 200 and a JSON array of index objects; the allowed index present, the Deny-refused index absent; after `DELETE <endpoint>/<allowed index>` the same call no longer lists it.
  Actual (emulator): HTTP 400, body {'error': 'no handler found for uri [/_cat/indices] and method [GET]'}. Traceback: check line 313 -> `names = _cat_index_names(ctx, base)` -> check line 260 -> `assert status == 200` -> `AssertionError: _cat/indices failed with HTTP 400: {'error': 'no handler found for uri [/_cat/indices] and method [GET]'}`.

Cause 2 — [mismatch] opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password
  Request: CreateDomain(DomainName=<unique>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'ZoneAwarenessEnabled':False,'MultiAZWithStandbyEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True}, NodeToNodeEncryptionOptions={'Enabled':True}, DomainEndpointOptions={'EnforceHTTPS':True,'TLSSecurityPolicy':'Policy-Min-TLS-1-2-2019-07'}, AdvancedSecurityOptions={'Enabled':True,'InternalUserDatabaseEnabled':True,'MasterUserOptions':{...}}, AccessPolicies=<account-scoped es:* policy>), then DescribeDomain(DomainName=<same>).
  Expected (AWS): DomainStatus.EncryptionAtRestOptions carries Enabled=true AND KmsKeyId set to the resolved default OpenSearch key ARN (an `arn:`-prefixed value).
  Actual (emulator): DomainStatus.EncryptionAtRestOptions == {'Enabled': True}; KmsKeyId absent. Traceback: check line 38 -> `assert ear.get("Enabled") is True and ear.get("KmsKeyId", "").startswith("arn:")` -> `AssertionError: at-rest encryption should be on with a default KMS key ARN filled in, got {'Enabled': True}`.
  Smithy citation: EncryptionAtRestOptions{Enabled: Boolean, KmsKeyId: @length 1..500, EncryptionMode: enum}; DescribeDomain returns DomainStatus.EncryptionAtRestOptions.

Cause 3 — [mismatch] opensearch.overly-permissive-vpc-endpoint-update-invalid-security-group-rejected
  Setup already passing: a VPC domain (t3.small.search x1, gp3 10 GiB, VPCOptions {SubnetIds:[<subnet>], SecurityGroupIds:[<default sg>]}) waited Active, then CreateVpcEndpoint(DomainArn=<arn>, VpcOptions same) waited ACTIVE.
  Request: UpdateVpcEndpoint(VpcEndpointId='aos-...', VpcOptions={'SubnetIds':['<subnet>'],'SecurityGroupIds':['sg-00000000000000000']}).
  Expected (AWS): HTTP 400 ValidationException, message `Invalid id: "sg-00000000000000000"`; then DescribeVpcEndpoints(VpcEndpointIds=['aos-...']) still Status ACTIVE with SecurityGroupIds == [<default sg>].
  Actual (emulator): HTTP 200. `AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'VpcEndpoint': {'VpcEndpointId': 'aos-...', 'VpcEndpointOwner': '...', 'DomainArn': 'arn:aws:es:...:domain/...', 'VpcOptions': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['sg-00000000000000000']}, 'Status': 'UPDATING', 'Endpoint': 'vpc-....es.amazonaws.com'}}` at check line 65/138. (The endpoint id, domain name, subnet id and host in that payload are per-run values.)
  Smithy citation: UpdateVpcEndpoint — POST /2021-01-01/opensearch/vpcEndpoints/update, 200; input {VpcEndpointId @required @length 5..256 pattern ^aos-[a-zA-Z0-9]*$, VpcOptions @required}; errors ValidationException {client,400}, BaseException {client,400,defaulted}, ConflictException/DisabledOperationException/ResourceNotFoundException {client,409}, InternalException {server,500}.

Cause 4 — [mismatch] opensearch.overly-permissive-vpc-endpoint-update-malformed-sg-rejected
  Request: UpdateVpcEndpoint(VpcEndpointId='aos-...', VpcOptions={'SubnetIds':['<subnet>'],'SecurityGroupIds':['not-a-security-group!!']}) against an ACTIVE endpoint of a VPC domain.
  Expected (AWS): HTTP 400 ValidationException, message `Invalid id: "not-a-security-group!!" (expecting "sg-...")`; DescribeVpcEndpoints still reports SecurityGroupIds == [<real sg>]; the identical update with the real sg is then accepted and polls to Status ACTIVE with that group.
  Actual (emulator): HTTP 200. `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'VpcEndpoint': {'VpcEndpointId': 'aos-...', 'VpcOptions': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['not-a-security-group!!']}, 'Status': 'UPDATING', ...}}` at check line 35/82.

## Steps to Reproduce
Boot the emulator and confirm opensearch, ec2, iam and sts answer.

Cause 1 (_cat/indices unrouted):
1. opensearch CreateDomain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}); poll DescribeDomain until Processing=False and Endpoint is published.
2. SigV4('es')-signed PUT https://<Endpoint>/<index>/_doc/1 {"title":"allowed"} (expect 200/201) then POST .../<index>/_refresh (expect 200).
3. SigV4('es')-signed GET https://<Endpoint>/_cat/indices?format=json. AWS: HTTP 200 with a JSON array containing an object whose `index` is <index>. Emulator: HTTP 400 {'error': 'no handler found for uri [/_cat/indices] and method [GET]'}.
4. DELETE https://<Endpoint>/<index> (expect 200), then repeat step 3: AWS no longer lists <index>.

Cause 2 (default KMS key ARN missing):
5. opensearch CreateDomain(... EncryptionAtRestOptions={'Enabled':True} with NO KmsKeyId, NodeToNodeEncryptionOptions={'Enabled':True}, DomainEndpointOptions={'EnforceHTTPS':True,'TLSSecurityPolicy':'Policy-Min-TLS-1-2-2019-07'}, AdvancedSecurityOptions={'Enabled':True,'InternalUserDatabaseEnabled':True,'MasterUserOptions':{'MasterUserName':'admin','MasterUserPassword':<complex>}}).
6. opensearch DescribeDomain(DomainName=<name>) and read DomainStatus.EncryptionAtRestOptions. AWS: {'Enabled': True, 'KmsKeyId': 'arn:aws:kms:<region>:<account>:key/<id>'}. Emulator: {'Enabled': True}.

Causes 3 and 4 (UpdateVpcEndpoint security-group validation):
7. ec2: pick a VPC, one of its subnets and its `default` security group; iam CreateServiceLinkedRole(AWSServiceName='opensearchservice.amazonaws.com').
8. opensearch CreateDomain(DomainName=<name2>, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, VPCOptions={'SubnetIds':[<subnet>],'SecurityGroupIds':[<default sg>]}); poll DescribeDomain to Active and take ARN.
9. opensearch CreateVpcEndpoint(DomainArn=<arn>, VpcOptions={'SubnetIds':[<subnet>],'SecurityGroupIds':[<default sg>]}); poll DescribeVpcEndpoints until Status=ACTIVE.
10. opensearch UpdateVpcEndpoint(VpcEndpointId=<aos-...>, VpcOptions={'SubnetIds':[<subnet>],'SecurityGroupIds':['sg-00000000000000000']}). AWS: HTTP 400 ValidationException `Invalid id: "sg-00000000000000000"`. Emulator: HTTP 200, Status UPDATING with that group stored.
11. opensearch DescribeVpcEndpoints(VpcEndpointIds=[<aos-...>]). AWS: Status ACTIVE, SecurityGroupIds == [<default sg>].
12. opensearch UpdateVpcEndpoint(VpcEndpointId=<aos-...>, VpcOptions={'SubnetIds':[<subnet>],'SecurityGroupIds':['not-a-security-group!!']}). AWS: HTTP 400 ValidationException `Invalid id: "not-a-security-group!!" (expecting "sg-...")`. Emulator: HTTP 200, Status UPDATING with that string stored.
13. opensearch DescribeVpcEndpoints again: AWS still reports [<default sg>].
14. Positive controls that must keep working: UpdateVpcEndpoint with a real security group created in the same VPC is accepted (200) and the endpoint polls back to ACTIVE with that group.

Finally, re-replay the four check ids listed in trace_ids.
