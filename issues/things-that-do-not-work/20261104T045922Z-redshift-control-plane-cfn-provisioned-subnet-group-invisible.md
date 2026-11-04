Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.deploy-cluster-cfn-live-connectivity redshift.deploy-redshift-cluster-cfn-behavioral-connectivity redshift.subnet-group-invalid-subnet-fails redshift.subnet-group-duplicate-fails redshift.failover-primary-compute-transition redshift.failover-primary-compute-non-multiaz-rejected redshift.reserved-node-exchange-configuration-options-scope redshift.enable-logging-bucket-policy-validation redshift.enable-logging-precondition-errors redshift.get-cluster-credentials-duration-seconds-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.deploy-cluster-cfn-live-connectivity|run#14|AssertionError|assert final_status == "CREATE_COMPLETE", (
Issue-Key: redshift.deploy-redshift-cluster-cfn-behavioral-connectivity|run#16|AssertionError|raise AssertionError(
Issue-Key: redshift.subnet-group-invalid-subnet-fails|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: redshift.subnet-group-duplicate-fails|run#9|AssertionError|assert message == f"The Cluster subnet group '{group_name}' already exists.", message
Issue-Key: redshift.failover-primary-compute-transition|run#7|KeyError|secondary_az_before = before["MultiAZSecondary"]["AvailabilityZone"]
Issue-Key: redshift.failover-primary-compute-non-multiaz-rejected|run#10|AssertionError|assert code == "InvalidClusterState", f"expected InvalidClusterState, got {code!r}"
Issue-Key: redshift.reserved-node-exchange-configuration-options-scope|run#16|AssertionError|assert resize_code == "ResizeNotFound", (
Issue-Key: redshift.enable-logging-bucket-policy-validation|run#10|AssertionError|assert code == "BucketNotFoundFault", f"expected BucketNotFoundFault, got {code!r}"
Issue-Key: redshift.enable-logging-precondition-errors|run#13|AssertionError|assert not_found_code == "BucketNotFoundFault", f"expected BucketNotFoundFault, got {not_found_code}"
Issue-Key: redshift.get-cluster-credentials-duration-seconds-out-of-range-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Redshift control plane: CFN-provisioned subnet group invisible to CreateCluster, no subnet validation, Multi-AZ secondary omitted, and DescribeResize/EnableLogging/FailoverPrimaryCompute answer NotImplemented (8 causes)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Eight net-new root causes on the Redshift control plane, all grounded in the behavioral replay.

Cause 1 of 8 — a CloudFormation-provisioned AWS::Redshift::ClusterSubnetGroup is not registered under its declared ClusterSubnetGroupName, so a dependent AWS::Redshift::Cluster in the same stack cannot resolve it. AWS: within one stack, the Cluster resource's ClusterSubnetGroupName (a Ref to the subnet-group resource) resolves to the group CloudFormation just created and the stack reaches CREATE_COMPLETE. Emulator: the Cluster resource fails with 'Resource handler returned message: "Cluster subnet group <stack>-ClusterSubnetGroup-<suffix> does not exist." (HandlerErrorCode: NotFound)' and the stack rolls back to ROLLBACK_COMPLETE/DELETE_COMPLETE. Why: no stack can provision a Redshift cluster inside a VPC — the single most common deployment shape fails, and the failure names a resource the same stack reported created. Fix: the AWS::Redshift::ClusterSubnetGroup resource provider must register the created group in Redshift state under its declared/physical name so CreateCluster resolves it. Traces: redshift.deploy-cluster-cfn-live-connectivity, redshift.deploy-redshift-cluster-cfn-behavioral-connectivity. This is the subnet-group analogue of the open 20261028T085300Z-cloudwatch-redshift-cloudformation-provisioning-of-aws-redshift-clusterparametergroup-does-not.md (parameter group), which carries neither of these checks.

Cause 2 of 8 — CreateClusterSubnetGroup accepts a SubnetIds entry that is not a real subnet. AWS: each supplied subnet id is resolved before the group is created; a well-formed but nonexistent id is refused 400 with Error.Code 'InvalidParameterValue' and "Some input subnets in :[<subnet ids>] are invalid." — fixed text "Some input subnets in :[" + the comma-joined offending ids from the request + "] are invalid." (Smithy: CreateClusterSubnetGroup models InvalidSubnet, httpError 400), and no group is created. Emulator: HTTP 200, the group is created and even reports a VpcId and an availability zone for the nonexistent subnet. Why: a client relying on the service to validate subnets creates a group that can never host a cluster and only discovers it much later. Fix: subnet resolution in the CreateClusterSubnetGroup handler's validation, before the state write. Traces: redshift.subnet-group-invalid-subnet-fails. Cause 3 of the open 20261029T192501Z-redshift-cluster-subnet-group-operations-answer-the-wrong.md describes the same validation gap from the wire corpus but does not carry this check.

Cause 3 of 8 — the ClusterSubnetGroupAlreadyExists message is emulator-authored. AWS: re-creating a subnet-group name in use is refused 400 ClusterSubnetGroupAlreadyExists with "The Cluster subnet group '<ClusterSubnetGroupName>' already exists." — fixed text "The Cluster subnet group '" + the requested name + "' already exists." (Smithy: ClusterSubnetGroupAlreadyExistsFault, httpError 400, awsQueryError ClusterSubnetGroupAlreadyExists). Emulator: correct code, message "Cluster subnet group '<name>' already exists." — the leading "The " is missing. Fix: the CreateClusterSubnetGroup duplicate-name refusal message. Traces: redshift.subnet-group-duplicate-fails.

Cause 4 of 8 — a Multi-AZ cluster reports no secondary compute unit. AWS: DescribeClusters for a cluster created with MultiAZ enabled returns MultiAZ 'Enabled' AND the MultiAZSecondary structure (Smithy: SecondaryClusterInfo with AvailabilityZone and ClusterNodes, documented "The AvailabilityZone and ClusterNodes information of the secondary compute unit"), whose AvailabilityZone differs from the cluster's primary AvailabilityZone. Emulator: MultiAZ is reported 'Enabled' but the Cluster payload has no MultiAZSecondary member at all (KeyError on the member). Why: a client cannot see which AZ holds the secondary, cannot verify Multi-AZ placement, and any failover flow that reads the pre/post-failover secondary AZ breaks. Fix: the Cluster state/serializer must populate MultiAZSecondary for Multi-AZ clusters. Traces: redshift.failover-primary-compute-transition.

Cause 5 of 8 — FailoverPrimaryCompute answers NotImplemented instead of its modeled refusal. AWS: FailoverPrimaryCompute on a cluster that is not Multi-AZ is refused with 'InvalidClusterState' (Smithy: FailoverPrimaryCompute errors ClusterNotFoundFault 404, InvalidClusterStateFault 400 awsQueryError InvalidClusterState, UnauthorizedOperation, UnsupportedOperationFault). Emulator: 'NotImplemented' — "operation ... is scaffolded but not implemented in this emulator". Why: the client sees a protocol-level 501-style failure instead of a modeled, handleable business refusal, so negative paths around Multi-AZ cannot be exercised. Fix: implement the FailoverPrimaryCompute handler's state precondition and modeled refusals. Traces: redshift.failover-primary-compute-non-multiaz-rejected.

Cause 6 of 8 — DescribeResize answers NotImplemented instead of ResizeNotFound. AWS: DescribeResize for a cluster with no resize in flight is refused 404 'ResizeNotFound' with "No Resize found for Cluster <ClusterIdentifier>" — fixed text "No Resize found for Cluster " + the requested cluster identifier (Smithy: ResizeNotFoundFault, httpError 404, awsQueryError ResizeNotFound, doc "A resize operation for the specified cluster is not found"). Emulator: 'NotImplemented'. Why: the standard way to prove no resize is in flight (and to poll one that is) is unusable, so a client cannot tell "no resize" from "service broken". Fix: implement DescribeResize's lookup and its ResizeNotFound refusal. Traces: redshift.reserved-node-exchange-configuration-options-scope.

Cause 7 of 8 — EnableLogging answers NotImplemented instead of its modeled bucket refusals. AWS: EnableLogging with a BucketName that does not exist is refused 400 'BucketNotFoundFault', and its other preconditions surface as InsufficientS3BucketPolicyFault / InvalidS3BucketNameFault / InvalidS3KeyPrefixFault / InvalidClusterState / ClusterNotFound (Smithy: EnableLogging errors BucketNotFoundFault 400, ClusterNotFoundFault 404, InsufficientS3BucketPolicyFault 400, InvalidClusterStateFault 400, InvalidS3BucketNameFault 400, InvalidS3KeyPrefixFault 400, UnsupportedOperationFault 400). Emulator: 'NotImplemented' for every one of them. Why: audit-logging configuration cannot be driven at all, and the precondition errors a client is written to handle never appear. Fix: implement the EnableLogging handler's bucket existence/policy preconditions and refusals. Traces: redshift.enable-logging-bucket-policy-validation, redshift.enable-logging-precondition-errors. Related: the auto-filed EnableLogging scaffold-stub issue covers the blocked check redshift.enable-logging-server-side-delivery-enforced, and 20261029T200555Z-redshift-control-operations-answer-notimplemented-instead-of-their.md names EnableLogging for other checks; neither carries these two.

Cause 8 of 8 — the GetClusterCredentials DurationSeconds range refusal is emulator-authored. AWS: a DurationSeconds outside 900–3600 is refused with the message "Token duration must be between 900 and 3600 seconds" — fixed text, with the two bounds as the only (constant) values; it does not echo the supplied value (Smithy: GetClusterCredentials.DurationSeconds is IntegerOptional with no modeled range, errors ClusterNotFoundFault and UnsupportedOperationFault, so the range check and its wording are recorded behavior). Emulator: "Invalid value for DurationSeconds: 100. DurationSeconds must be between 900 and 3600." — different wording and it echoes the request value. Why: callers that assert on or log the documented refusal text do not recognise it. Fix: the GetClusterCredentials DurationSeconds validation message. Traces: redshift.get-cluster-credentials-duration-seconds-out-of-range-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator.

Cause 1: redshift.deploy-cluster-cfn-live-connectivity — "stack <stack> did not reach CREATE_COMPLETE (got ROLLBACK_COMPLETE): ['<stack>: The following resource(s) failed to create: [RedshiftCluster]. ', 'RedshiftCluster: Resource handler returned message: \"Cluster subnet group <stack>-ClusterSubnetGroup-<suffix> does not exist.\" (HandlerErrorCode: NotFound)']"; redshift.deploy-redshift-cluster-cfn-behavioral-connectivity — same message, stack ends DELETE_COMPLETE.

Cause 2: redshift.subnet-group-invalid-subnet-fails — "expected a rejection, but the call SUCCEEDED and returned {'ClusterSubnetGroup': {... 'Subnets': [{'SubnetIdentifier': 'subnet-0123456789abcdef0', ...}], 'SubnetGroupStatus': 'Complete'}, 'HTTPStatusCode': 200}"; expected code 'InvalidParameterValue' and message "Some input subnets in :[subnet-0123456789abcdef0] are invalid."

Cause 3: redshift.subnet-group-duplicate-fails — code was ClusterSubnetGroupAlreadyExists as expected; message was "Cluster subnet group '<name>' already exists." against the asserted "The Cluster subnet group '<name>' already exists."

Cause 4: redshift.failover-primary-compute-transition — KeyError: 'MultiAZSecondary' at `before["MultiAZSecondary"]["AvailabilityZone"]`, after the same payload satisfied the MultiAZ == 'Enabled' assertion.

Cause 5: redshift.failover-primary-compute-non-multiaz-rejected — "expected InvalidClusterState, got 'NotImplemented'".

Cause 6: redshift.reserved-node-exchange-configuration-options-scope — "cluster <id> must have no resize in flight after a rejected exchange, got 'NotImplemented'" at `describe_resize`, expecting code ResizeNotFound and message "No Resize found for Cluster <id>".

Cause 7: redshift.enable-logging-bucket-policy-validation — "expected BucketNotFoundFault, got 'NotImplemented'"; redshift.enable-logging-precondition-errors — "expected BucketNotFoundFault, got NotImplemented".

Cause 8: redshift.get-cluster-credentials-duration-seconds-out-of-range-rejected — "expected message 'Token duration must be between 900 and 3600 seconds', got 'Invalid value for DurationSeconds: 100. DurationSeconds must be between 900 and 3600.'"

Stack names, generated resource suffixes, cluster ids and the sample subnet id are this run's environment; the requirements are the statuses, codes, message templates and payload members described per cause.

## Steps to Reproduce
1. Deploy a stack containing an AWS::Redshift::ClusterSubnetGroup and an AWS::Redshift::Cluster whose ClusterSubnetGroupName references it: the cluster resource fails NotFound "Cluster subnet group ... does not exist." and the stack rolls back.
2. CreateClusterSubnetGroup with SubnetIds=['subnet-0123456789abcdef0'] (well-formed, nonexistent): the call returns 200 instead of 400 InvalidParameterValue "Some input subnets in :[subnet-0123456789abcdef0] are invalid."
3. CreateClusterSubnetGroup twice with the same name and compare the ClusterSubnetGroupAlreadyExists message against "The Cluster subnet group '<name>' already exists."
4. CreateCluster with MultiAZ enabled, wait for 'available', and read DescribeClusters: MultiAZ is 'Enabled' but MultiAZSecondary is absent.
5. Call FailoverPrimaryCompute on a non-Multi-AZ cluster (expect InvalidClusterState), DescribeResize on a cluster with no resize in flight (expect ResizeNotFound "No Resize found for Cluster <id>") and EnableLogging with a nonexistent BucketName (expect BucketNotFoundFault): all three answer NotImplemented.
6. GetClusterCredentials with DurationSeconds=100 and compare the message against "Token duration must be between 900 and 3600 seconds".
7. Re-replay the behavioral checks listed in trace_ids.
