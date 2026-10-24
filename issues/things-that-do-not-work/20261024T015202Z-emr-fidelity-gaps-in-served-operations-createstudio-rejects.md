Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.list-studio-session-mappings-invalid-identity-type-filter-rejected emr.list-studio-session-mappings-invalid-identity-type-rejected emr.security-configuration-identical-create-is-idempotent emr.concurrent-create-same-security-configuration-name emr.block-public-access-empty-vs-absent-exceptions-preserved emr.supported-instance-types-report-accurate-hardware-facts emr.op-cluster-id-mutations-reject-unknown-cluster emr.get-cluster-session-credentials-without-execution-role-denied emr.op-describe-job-flows-is-deprecated emr.op-studio-unknown-id-operations-fail emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|CreateStudio|InvalidRequestException
Issue-Key: emr.list-studio-session-mappings-invalid-identity-type-rejected|_retry_create_studio#2|Timeout|time.sleep(5)
Issue-Key: emr|CreateSecurityConfiguration|InvalidRequestException
Issue-Key: emr.concurrent-create-same-security-configuration-name|run#9|AssertionError|assert not unexpected, (
Issue-Key: emr.block-public-access-empty-vs-absent-exceptions-preserved|run#17|AssertionError|assert absent_ranges == [], (
Issue-Key: emr.supported-instance-types-report-accurate-hardware-facts|run#11|AssertionError|assert entry["InstanceFamilyId"] == family_id, (
Issue-Key: emr.op-cluster-id-mutations-reject-unknown-cluster|_assert_codes#3|AssertionError|assert not mismatched, f"unexpected rejection message(s): {mismatched}"
Issue-Key: emr.get-cluster-session-credentials-without-execution-role-denied|run#3|AssertionError|assert message == (
Issue-Key: emr.op-describe-job-flows-is-deprecated|run#6|AssertionError|assert "deprecated" in message.lower(), (
Issue-Key: emr.op-studio-unknown-id-operations-fail|run#4|AssertionError|assert message == _studio_missing_message(message), f"{op}: unexpected message {message!r}"
Issue-Key: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected|run#21|AssertionError|assert message == (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# EMR fidelity gaps in served operations: CreateStudio rejects a valid IAM-auth Studio, CreateSecurityConfiguration is not idempotent, BlockPublicAccess Put is not a full replace, instance-family data is synthesized, and six refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStudio

## Description
Nine root causes in EMR operations the emulator DOES serve (every one of them is reached after a successful call, so these are fidelity divergences, not scaffold gaps). Two are over-rejections of requests real AWS accepts, one is a representation gap, one is synthesized reference data, and five are refusal messages the emulator authors itself where AWS's text is a fixed, known string (the error CODE and HTTP status are correct in all five — only the message diverges).

Cause 1 of 9: CreateStudio refuses AuthMode=IAM unless UserRole is supplied.
AWS: Smithy com.amazonaws.emr#CreateStudio input marks Name, AuthMode, VpcId, SubnetIds, ServiceRole, WorkspaceSecurityGroupId, EngineSecurityGroupId and DefaultS3Location required; UserRole is NOT required, and its documentation states "Only specify a UserRole when you use IAM Identity Center authentication." So an AuthMode=IAM CreateStudio with no UserRole is a well-formed request that must succeed and return a StudioId; CreateStudio models only InvalidRequestException (400) and InternalServerException (500), and neither applies here.
Emulator: returns 400 InvalidRequestException with message "AuthMode IAM requires a UserRole for users logged in to the Studio" for exactly that request, so no IAM-auth Studio can be created at all.
Why: every client (and every downstream check) that creates an IAM-mode Studio — the normal, non-Identity-Center case — is blocked; the Studio and Studio-session-mapping APIs become unreachable. One dependent check additionally wedges: its CreateStudio retry loop treats InvalidRequestException as IAM propagation lag and retries 12 x 5s, so the refusal surfaces as a 60s wall-clock hang rather than an error.
Fix: the input validation in the CreateStudio handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/create_studio.rs) — require UserRole only for the IAM Identity Center auth mode, never for AuthMode=IAM.
Traces: emr.list-studio-session-mappings-invalid-identity-type-filter-rejected, emr.list-studio-session-mappings-invalid-identity-type-rejected.

Cause 2 of 9: CreateSecurityConfiguration refuses a byte-identical re-create instead of being idempotent.
AWS: recorded behavior only (Smithy models only InvalidRequestException/InternalServerException on CreateSecurityConfiguration and says nothing about name collisions). Recorded: re-creating a security configuration with the SAME Name and byte-identical SecurityConfiguration JSON is accepted, leaves the stored content unchanged, and leaves exactly one entry under that Name in ListSecurityConfigurations; four concurrent identical creates therefore all succeed (only throttling is tolerated). A create with the same Name but DIFFERENT content is the case that is refused.
Emulator: any second create of an existing Name is refused with 400 InvalidRequestException "Security configuration with name '<name>' already exists.", including the byte-identical case, and 3 of 4 concurrent identical creates fail that way.
Why: the idempotent re-create is how clients make security-configuration provisioning safely re-runnable (CFN/Terraform-style converge loops, and any retry after an ambiguous timeout). Against the emulator the second converge run fails hard, and a concurrent provisioning path loses all but one racer.
Fix: the name-collision branch in the CreateSecurityConfiguration handler and its state lookup (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/create_security_configuration.rs) — compare stored content and return success when it is identical, refusing only on a content mismatch.
Traces: emr.security-configuration-identical-create-is-idempotent, emr.concurrent-create-same-security-configuration-name.

Cause 3 of 9: PutBlockPublicAccessConfiguration does not canonicalize an omitted PermittedPublicSecurityGroupRuleRanges to an empty list (the Put is not a full replace).
AWS: Smithy BlockPublicAccessConfiguration requires BlockPublicSecurityGroupRules and makes PermittedPublicSecurityGroupRuleRanges an optional list; Put takes the whole configuration structure. Recorded behavior: the Put is a full replace — once any Put has been made, a subsequent Put that omits PermittedPublicSecurityGroupRuleRanges clears the previously set ranges and Get then reports the SAME canonical representation as an explicit empty list (the documented port-22 default only ever applies to an account/region that has never called Put).
Emulator: an explicit [] does round-trip as [], but after a Put that omits the member, GetBlockPublicAccessConfiguration omits PermittedPublicSecurityGroupRuleRanges from the response entirely (botocore sees None) instead of the empty list, so the omitted-field and explicit-empty cases do not collapse to one canonical representation.
Why: a client reading the configuration back cannot tell "no public-port exceptions" from "field unknown"; code that iterates response['BlockPublicAccessConfiguration']['PermittedPublicSecurityGroupRuleRanges'] raises KeyError against the emulator where it works against AWS, and any merge-vs-replace assumption about the Put is unverifiable.
Fix: the state write/serializer for the block-public-access singleton (the PutBlockPublicAccessConfiguration handler and the Get serializer in services/service-com-amazonaws-emr-elasticmapreduce/src/ops/) — store the full replacement and emit the list member as an empty list, not as absent.
Traces: emr.block-public-access-empty-vs-absent-exceptions-preserved.

Cause 4 of 9: ListSupportedInstanceTypes reports InstanceFamilyId as the instance-type prefix instead of EMR's family-and-generation class.
AWS: Smithy com.amazonaws.emr#SupportedInstanceType.InstanceFamilyId documents it as "The Amazon EC2 family and generation for the SupportedInstanceType". Recorded behavior: real EMR reports a workload-class x generation category — GENERAL_CURRENT_GEN for m5.xlarge and m5.2xlarge, COMPUTE_CURRENT_GEN for c5.xlarge — drawn from the closed set {GENERAL,COMPUTE,HI_MEM,STORAGE,GPU} x {CURRENT,PREVIOUS} + "_GEN".
Emulator: reports the bare instance-type prefix, e.g. 'm5' for m5.xlarge, which is outside that closed set.
Why: the catalogue is reference data clients use to group/choose instance types by family and generation; a value that is just the type prefix makes any grouping or generation test wrong, and callers validating against the known enum-like set reject every entry.
Fix: the catalogue data/projection behind the ListSupportedInstanceTypes handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_supported_instance_types.rs) — derive InstanceFamilyId as the workload-class-plus-generation class rather than slicing the type name.
Traces: emr.supported-instance-types-report-accurate-hardware-facts.

Cause 5 of 9: the ClusterId-shaped mutations all share one unknown-cluster message instead of EMR's per-operation wording.
AWS: recorded behavior only (these operations model InvalidRequestException (400) and an internal error; the text is not in the model). Recorded, and confirmed per-operation against real AWS: ModifyCluster and CancelSteps answer "Cluster id '<id>' is not valid." (request-derived — they echo the supplied id), AddInstanceFleet answers "Specified job flow ID not valid." and GetClusterSessionCredentials answers "Cluster id is not valid." (neither echoes the id). The code (InvalidRequestException) and status (400) are already correct in the emulator.
Emulator: all four answer one uniform string, "Cluster id '<id>' is not valid or does not exist.".
Why: the uniform message is indistinguishable from a handler that blanket-refuses every call to these operations rather than one that actually resolves the cluster id, and clients/log scrapers keyed on EMR's per-operation wording (notably AddInstanceFleet's "Specified job flow ID not valid.") do not match.
Fix: the unknown-cluster refusal path in each operation's handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/modify_cluster.rs, cancel_steps.rs, add_instance_fleet.rs, get_cluster_session_credentials.rs) — emit each operation's own wording, echoing the id only where AWS does.
Traces: emr.op-cluster-id-mutations-reject-unknown-cluster.

Cause 6 of 9: GetClusterSessionCredentials on a RUNNING cluster with no runtime-role/FGAC association returns the wrong refusal text.
AWS: recorded behavior only for the text; Smithy models InvalidRequestException (400) for this operation. Recorded: the refusal is 400 InvalidRequestException with the fixed sentence "You haven\u2019t enabled runtime roles or IAM Identity Center for your cluster in its security configuration." (note the typographic apostrophe), i.e. the refusal names the missing security-configuration feature.
Emulator: same code and status, but the message is "Cluster '<cluster-id>' has no endpoint with fine-grained access control activated, so no session credentials can be issued for it." — an emulator-authored sentence that attributes the refusal to a missing endpoint instead of the cluster's security configuration.
Why: the message is the only part of this response that tells an operator what to change; pointing at an "endpoint" instead of runtime roles / IAM Identity Center in the security configuration sends a client to the wrong remedy.
Fix: the refusal path in the GetClusterSessionCredentials handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_cluster_session_credentials.rs).
Traces: emr.get-cluster-session-credentials-without-execution-role-denied.

Cause 7 of 9: the DescribeJobFlows retirement refusal never says the API is deprecated.
AWS: Smithy marks com.amazonaws.emr#DescribeJobFlows @deprecated; the refusal identity itself is recorded behavior (the operation models only InternalServerError). Recorded: every DescribeJobFlows call — with JobFlowIds or with no arguments at all — is refused with error code ValidationException whose message states that the API is deprecated.
Emulator: the code (ValidationException) is correct, but the message is "DescribeJobFlows is no longer supported. Use ListClusters, DescribeCluster, ListSteps, ListInstanceGroups and ListBootstrapActions instead." and contains no "deprecated" wording.
Why: clients and operators grep this refusal for the deprecation signal to distinguish a retired API from an argument-validation failure of the same code; the emulator's wording reads as an ordinary ValidationException.
Fix: the refusal message in the DescribeJobFlows handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/describe_job_flows.rs) — keep the ValidationException code and state the deprecation.
Traces: emr.op-describe-job-flows-is-deprecated.

Cause 8 of 9: the unknown-StudioId refusal does not use EMR Studio's wrapped-message form.
AWS: recorded behavior only for the text; DescribeStudio/UpdateStudio/DeleteStudio model InvalidRequestException (400). Recorded: all three answer 400 InvalidRequestException with the backend-wrapped form "Studio does not exist. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <per-call request id>; Proxy: null)" — the only per-run part is the request id; the rest, including the AWSEditors service name, is fixed and does NOT echo the StudioId.
Emulator: same code, but the message is "Studio id '<studio-id>' is not valid or does not exist.".
Why: a client that parses this refusal for the wrapped Error Code / Service fields (the form every EMR Studio error carries) gets nothing to parse, and the emulator echoes an id AWS does not.
Fix: the unknown-id refusal path shared by the Studio read/mutate handlers (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/describe_studio.rs, update_studio.rs, delete_studio.rs).
Traces: emr.op-studio-unknown-id-operations-fail.

Cause 9 of 9: EMR Serverless StartJobRun on a non-STARTED, auto-start-disabled application returns the wrong refusal text. (This check carries the emr service token but drives the separate emr-serverless model, com.amazonaws.emrserverless#AwsToledoWebService, via ctx.client('emr-serverless').)
AWS: recorded behavior only for the text. Recorded: the refusal is ValidationException with the fixed sentence "Job run cannot be started as Application is not in STARTED state and auto-start is disabled".
Emulator: the code (ValidationException) is correct, but the message is "Application is in CREATED state and does not start automatically on job submission; call StartApplication first".
Why: the refusal is the documented auto-start gate; clients keyed on AWS's fixed sentence (the only stable discriminator, since the code ValidationException also covers ordinary input errors on this operation) do not match the emulator's.
Fix: the state-gate refusal in the EMR Serverless StartJobRun handler (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_job_run.rs).
Traces: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected.

Related already-open issues that do NOT cover any of the above: 20261022T154846Z-emr-enum-filter-refusals-use-unmodeled-validationexception-runjobflow.md (ListClusters/ListNotebookExecutions enum refusals, RunJobFlow instance-profile validation, EMR Serverless runs never executing) and 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md (emr-containers).

## Evidence
Cause 1 — expected: CreateStudio(Name=..., AuthMode='IAM', VpcId=..., SubnetIds=[...], ServiceRole=..., WorkspaceSecurityGroupId=..., EngineSecurityGroupId=..., DefaultS3Location='s3://.../studio/') with no UserRole returns a StudioId. Actual: botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CreateStudio operation: AuthMode IAM requires a UserRole for users logged in to the Studio. The sibling check that retries InvalidRequestException for IAM propagation instead reports 'Check timed out after 60.0s' (12 x 5s of retries).
Cause 2 — expected: a second create with the same Name and byte-identical SecurityConfiguration succeeds. Actual: botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CreateSecurityConfiguration operation: Security configuration with name 'sim2real-behav-emr-secconf-idem-8d46cd86' already exists. Concurrent case: AssertionError: identical concurrent creates must all succeed (idempotent), got unexpected failure codes ['InvalidRequestException', 'InvalidRequestException', 'InvalidRequestException'].
Cause 3 — expected: after Put{BlockPublicSecurityGroupRules:true} (no PermittedPublicSecurityGroupRuleRanges) following a Put with ranges [{22,22},{8000,8010}], Get reports PermittedPublicSecurityGroupRuleRanges == []. Actual: AssertionError: omitting PermittedPublicSecurityGroupRuleRanges after a prior non-empty Put was expected to fully replace (clear) the ranges via a full-replace Put, got None -- looks like a merge/partial-update instead.
Cause 4 — expected InstanceFamilyId 'GENERAL_CURRENT_GEN' for m5.xlarge (and 'COMPUTE_CURRENT_GEN' for c5.xlarge). Actual: AssertionError: m5.xlarge belongs to EMR instance family 'GENERAL_CURRENT_GEN'; catalogue reports 'm5'.
Cause 5 — expected messages per op: ModifyCluster/CancelSteps "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.", AddInstanceFleet "Specified job flow ID not valid.", GetClusterSessionCredentials "Cluster id is not valid.". Actual: AssertionError: unexpected rejection message(s): {'ModifyCluster': "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid or does not exist.", 'CancelSteps': "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid or does not exist.", 'AddInstanceFleet': "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid or does not exist.", 'GetClusterSessionCredentials': "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid or does not exist."} — the code InvalidRequestException matched for all four.
Cause 6 — expected message "You haven\u2019t enabled runtime roles or IAM Identity Center for your cluster in its security configuration." with code InvalidRequestException. Actual (code matched): "Cluster 'j-0000000000001' has no endpoint with fine-grained access control activated, so no session credentials can be issued for it."
Cause 7 — expected: code ValidationException and 'deprecated' in the message. Actual (code matched): AssertionError: rejection should explain the API is deprecated, got 'DescribeJobFlows is no longer supported. Use ListClusters, DescribeCluster, ListSteps, ListInstanceGroups and ListBootstrapActions instead.'
Cause 8 — expected "Studio does not exist. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)". Actual (code matched): AssertionError: DescribeStudio: unexpected message "Studio id 'es-1234567890ABCDEFGHIJKLMNO' is not valid or does not exist."
Cause 9 — expected "Job run cannot be started as Application is not in STARTED state and auto-start is disabled" with code ValidationException. Actual (code matched): "Application is in CREATED state and does not start automatically on job submission; call StartApplication first".

## Steps to Reproduce
1. Boot the emulator and point botocore at it; emr = client('emr').
2. Cause 1: create a VPC/subnet, two security groups, a bucket and an elasticmapreduce.amazonaws.com-trusted role, then emr.create_studio(Name=..., AuthMode='IAM', VpcId=..., SubnetIds=[...], ServiceRole=..., WorkspaceSecurityGroupId=..., EngineSecurityGroupId=..., DefaultS3Location='s3://<bucket>/studio/') with NO UserRole -> expect a StudioId; observe 400 InvalidRequestException 'AuthMode IAM requires a UserRole ...'.
3. Cause 2: payload='{"EncryptionConfiguration":{"EnableInTransitEncryption":false,"EnableAtRestEncryption":false}}'; emr.create_security_configuration(Name=n, SecurityConfiguration=payload) twice with the same n and payload -> expect both to succeed with one listing entry; observe 400 InvalidRequestException on the second. Repeat with 4 concurrent identical creates.
4. Cause 3: put_block_public_access_configuration({'BlockPublicSecurityGroupRules':True,'PermittedPublicSecurityGroupRuleRanges':[{'MinRange':22,'MaxRange':22},{'MinRange':8000,'MaxRange':8010}]}), then put_block_public_access_configuration({'BlockPublicSecurityGroupRules':True}), then get_block_public_access_configuration() -> expect PermittedPublicSecurityGroupRuleRanges == []; observe the member absent.
5. Cause 4: emr.list_supported_instance_types(ReleaseLabel='emr-6.15.0'), following Marker; find Type=='m5.xlarge' -> expect InstanceFamilyId 'GENERAL_CURRENT_GEN'; observe 'm5'.
6. Cause 5: with j='j-ZZZZZZZZZZZZZ', call modify_cluster(ClusterId=j, StepConcurrencyLevel=2), cancel_steps(ClusterId=j, StepIds=['s-ZZZZZZZZZZZZZ']), add_instance_fleet(ClusterId=j, InstanceFleet={...TASK...}) and get_cluster_session_credentials(ClusterId=j, ExecutionRoleArn=...) -> expect the four distinct AWS messages; observe one uniform message.
7. Cause 6: launch a cluster with no security configuration, wait for RUNNING/WAITING, call get_cluster_session_credentials(ClusterId=<id>) -> expect the runtime-roles/IAM Identity Center sentence; observe the endpoint/FGAC sentence.
8. Cause 7: emr.describe_job_flows(JobFlowIds=['j-ZZZZZZZZZZZZZ']) and emr.describe_job_flows() -> expect ValidationException whose message says the API is deprecated; observe the 'no longer supported' wording.
9. Cause 8: describe_studio / update_studio / delete_studio with StudioId='es-1234567890ABCDEFGHIJKLMNO' -> expect the wrapped 'Studio does not exist. (Service: AWSEditors; ...)' message; observe 'Studio id ... is not valid or does not exist.'.
10. Cause 9: ems = client('emr-serverless'); create an application with autoStart disabled, leave it in CREATED/STOPPED, ems.start_job_run(applicationId=..., executionRoleArn=..., jobDriver={...}) -> expect ValidationException 'Job run cannot be started as Application is not in STARTED state and auto-start is disabled'; observe the emulator's CREATED-state sentence.
11. Re-replay the check ids in trace_ids.
