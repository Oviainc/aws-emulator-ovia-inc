Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected
Issue-Key: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected|run#15|KeyError|epo = ds["DomainEndpointOptions"]
Issue-Commit: bfd8b7223bf6a1ab9b658956ab58262126f30b11

# OpenSearch DomainStatus omits DomainEndpointOptions entirely when CreateDomain did not supply it, instead of reporting the defaulted endpoint options with CustomEndpointEnabled=false

> _Note: this issue was identified against the code base at commit `bfd8b7223bf6a1ab9b658956ab58262126f30b11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
ONE root cause, in the OpenSearch domain record's DomainStatus projection. Service token taken from the failing check's id (cognito-identity__cognito-idp__iam__opensearch — a cross-service check that builds a Cognito user pool / identity pool / IAM role estate and then creates an OpenSearch domain with CognitoOptions); the defect itself lives in the opensearch crate.

Cause 1 of 1: a domain created WITHOUT DomainEndpointOptions in the request reports no DomainEndpointOptions at all in DomainStatus.

AWS: Smithy com.amazonaws.opensearch#DomainStatus carries member DomainEndpointOptions (target com.amazonaws.opensearch#DomainEndpointOptions: EnforceHTTPS, TLSSecurityPolicy, CustomEndpointEnabled, CustomEndpoint, CustomEndpointCertificateArn) — "Additional options for the domain endpoint, such as whether to require HTTPS for all traffic." DomainEndpointOptions is an optional member of the CreateDomain *input*, but OpenSearch materializes the domain's effective endpoint options on the server side, so every CreateDomain / DescribeDomain / DescribeDomains response for a live domain carries the DomainEndpointOptions structure whether or not the request supplied one. Recorded behavior for a domain created with no DomainEndpointOptions: the structure is present and reports the server-side default for a domain that has no custom endpoint — CustomEndpointEnabled is the boolean false (not an absent member), and CustomEndpoint and CustomEndpointCertificateArn are absent because no custom endpoint exists. (The recording does not pin EnforceHTTPS or TLSSecurityPolicy for the omitted-input case, so nothing is claimed about their values here beyond their being server-chosen; the requirement grounded in the recording is that the DomainEndpointOptions member itself is always populated and that CustomEndpointEnabled is explicitly false with no CustomEndpoint.)

Emulator: DescribeDomain's DomainStatus has NO DomainEndpointOptions key at all, so the SDK's response dict does not contain it and `DomainStatus["DomainEndpointOptions"]` raises KeyError: 'DomainEndpointOptions'. The cause is visible in the handler: the stored domain record keeps the request's value verbatim (`domain_endpoint_options: input.domain_endpoint_options`), so it is None whenever the request omitted the member, and the DomainStatus projection then serializes nothing — there is no server-side defaulting step. Correspondingly, when the request DOES supply DomainEndpointOptions the member echoes back fine (see the acm__opensearch replays), which is why this only shows up on the no-endpoint-options path.

Why: a client cannot tell "custom endpoint disabled" from "the service did not tell me". Any caller that reads DomainStatus.DomainEndpointOptions unconditionally — which is safe against AWS, since AWS always returns it — crashes (KeyError in botocore/boto3, a null-deref or missing-field error in typed SDKs) rather than reading CustomEndpointEnabled=false. Infrastructure reconcilers that diff desired against actual endpoint options see an absent structure and either report a spurious drift on every poll or skip the comparison entirely, leaving a domain's HTTPS/TLS/custom-endpoint posture unverifiable.

Fix: the DomainStatus projection of the stored domain record for the opensearch domain operations — services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs (the stored-record construction around the `domain_endpoint_options: input.domain_endpoint_options` assignment and the DomainStatus projection it feeds) and the shared projection used by DescribeDomain/DescribeDomains. CreateDomain must materialize the effective endpoint options onto the stored domain when the request omits them (CustomEndpointEnabled false, CustomEndpoint/CustomEndpointCertificateArn unset) so that every DomainStatus read carries the structure.

Traces: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected.

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1), cognito-identity__cognito-idp__iam__opensearch 2/3 passed.

[error] cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected:
  File "check_cognito_identity__cognito_idp__iam__opensearch_cognito_options_for_a_user_pool_without_a_hosted_domain_rejected.py", line 42, in run
    epo = ds["DomainEndpointOptions"]
  KeyError: 'DomainEndpointOptions'

The check had already gotten PAST its earlier legs: CreateDomain naming a user pool with no hosted-UI domain was correctly refused with ValidationException 'Domain needs to be specified for user pool', DescribeDomain then answered ResourceNotFoundException, the hosted domain was added, the identical CreateDomain was accepted, and DomainStatus.CognitoOptions round-tripped exactly. The failure is purely the next read: the accepted CreateDomain request contained DomainName, EngineVersion, ClusterConfig, EBSOptions, NodeToNodeEncryptionOptions, EncryptionAtRestOptions, TagList and CognitoOptions — and NO DomainEndpointOptions — so the emulator's DomainStatus came back with the DomainEndpointOptions member missing.

Expected (what the check asserts of AWS): `DomainStatus["DomainEndpointOptions"]` resolves, `epo.get("CustomEndpointEnabled") is False`, and `"CustomEndpoint" not in epo`.
Actual: the key is absent from DomainStatus altogether.

Contrast establishing the same cause is a defaulting gap and not a serializer-wide drop: in the acm__opensearch replays, where the CreateDomain request DID carry DomainEndpointOptions, DomainStatus echoed {'EnforceHTTPS': True, 'CustomEndpointEnabled': True, 'CustomEndpoint': ..., 'CustomEndpointCertificateArn': ...} intact.

Handler evidence: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs stores `domain_endpoint_options: input.domain_endpoint_options` and elsewhere tests `domain.domain_endpoint_options.is_some()` — the stored value is the raw Option from the request, with no server-side default materialized.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2).
2. opensearch.create_domain(DomainName='<token>', EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, NodeToNodeEncryptionOptions={'Enabled':True}, EncryptionAtRestOptions={'Enabled':True}) — i.e. with NO DomainEndpointOptions member at all. (The failing check reaches this point with CognitoOptions also set, after giving the user pool a hosted-UI domain; CognitoOptions is not needed to reproduce.)
3. ds = opensearch.describe_domain(DomainName='<token>')['DomainStatus'].
4. Observe 'DomainEndpointOptions' not in ds — so ds['DomainEndpointOptions'] raises KeyError. Expected: the member is present, ds['DomainEndpointOptions'].get('CustomEndpointEnabled') is False, and 'CustomEndpoint' not in it.
5. For the contrast, repeat step 2 with DomainEndpointOptions={'EnforceHTTPS':True} supplied and observe the member does come back — the member is dropped only when the request omitted it.
6. Re-replay the check id in trace_ids to confirm the fix.
