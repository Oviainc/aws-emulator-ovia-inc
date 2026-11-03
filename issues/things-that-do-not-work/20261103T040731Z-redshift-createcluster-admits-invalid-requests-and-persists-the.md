Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-rshealth-create-cluster-credential-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-rshealth-create-cluster-credential-refusals redshift-rshealth-create-cluster-shape-refusals
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|CreateCluster|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateCluster|4xx->2xx|status
Issue-Key: redshift|CreateCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|CreateCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateCluster|4xx->4xx|status
Issue-Key: redshift|DescribeClusters|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeClusters|4xx->2xx|status
Issue-Key: redshift|DeleteCluster|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DeleteCluster|4xx->2xx|status
Issue-Commit: 5e177a4f4d28f6dc4714f734dc35ed5ad192c177

# Redshift CreateCluster admits invalid requests and persists the cluster, so shape/credential refusals and follow-up not-found reads all diverge

> _Note: this issue was identified against the code base at commit `5e177a4f4d28f6dc4714f734dc35ed5ad192c177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Six net-new root causes in the redshift service, all rooted in the CreateCluster write path: the emulator does not run Redshift's up-front request validation, so it ACCEPTS a CreateCluster that AWS refuses and persists a cluster; every subsequent refusal in the same fixture is then answered out of that phantom state (ClusterAlreadyExists instead of the real validation fault) and the follow-up reads/deletes answer 200 instead of ClusterNotFound. Each cause is a separate validation or lookup site and must be closed separately. Open issue 20261029T200555Z-redshift-request-validation-answers-a-generic-validationexception-or.md describes a neighbouring validation-taxonomy family and 20261102T105531Z-redshift-refusal-codes-and-messages-are-emulator-authored.md a neighbouring refusal-wording family; neither carries the replay keys below.

Cause 1 of 6: CreateCluster does not enforce the MasterUserPassword content rules. AWS: a MasterUserPassword that contains no decimal digit is refused before any cluster is created, HTTP 400, error code InvalidParameterValue, with the fixed message text "The parameter MasterUserPassword must contain at least 1 decimal digit." (no variable parts — the member name MasterUserPassword is the modeled member, and the whole sentence is fixed text). Smithy: CreateCluster.MasterUserPassword targets SensitiveString with no modeled pattern, and the modeled error set (ClusterAlreadyExistsFault, ClusterParameterGroupNotFoundFault, NumberOfNodesPerClusterLimitExceededFault, ...) contains no password fault, so the password-domain refusal is the service's own InvalidParameterValue — recorded behavior only for the exact wording. Emulator: HTTP 200, no ErrorResponse at all; the cluster is created. Why: a client that submits a non-compliant password is told the cluster exists and will start polling for an available cluster that AWS would never have provisioned, and the password policy is silently unenforced in every provisioning test run against the emulator. Fix: the CreateCluster handler's input validation, before any cluster is written to state. Traces: redshift-rshealth-create-cluster-credential-refusals#task#1.

Cause 2 of 6: the MasterUserPassword length rule is not enforced, and the phantom cluster from cause 1 hijacks the refusal. AWS: a MasterUserPassword shorter than 8 characters is refused HTTP 400 with code InvalidParameterValue and the fixed message "Invalid master password. Password should be atleast 8 characters long." (fixed text, no variable parts — note AWS's own spelling of "atleast"). Emulator: HTTP 400 but code ClusterAlreadyExists with the message "Cluster already exists: <ClusterIdentifier>" (fixed text "Cluster already exists: " plus the ClusterIdentifier taken from the request). Smithy: ClusterAlreadyExistsFault is modeled with awsQueryError code ClusterAlreadyExists / httpError 400, so the code itself exists but is being raised for the wrong reason: duplicate-identifier detection runs BEFORE parameter validation, and the identifier is only present because cause 1 persisted it. Why: the client is told to pick a different cluster name when the real defect is its password, so retry logic loops on a new name and fails identically forever. Fix: the CreateCluster handler's validation ordering — password/shape validation must precede the duplicate-identifier lookup. Traces: redshift-rshealth-create-cluster-credential-refusals#task#2 (cluster also affects task#3, task#4, task#5 and redshift-rshealth-create-cluster-shape-refusals#task#9, #task#10, #task#11).

Cause 3 of 6: the multi-node NumberOfNodes refusal message is emulator-authored. AWS: CreateCluster with ClusterType=multi-node and no (or a too-small) NumberOfNodes is refused HTTP 400, code InvalidParameterValue, with the fixed message "Number of nodes must be supplied for cluster type multi-node and must be greater than or equal to 2." — the whole sentence is fixed text; the cluster type is the only value echoed from the request. Emulator: the right status and code but the message "Number of nodes for cluster type multi-node must be greater than or equal to 2.", which drops AWS's "must be supplied for" clause, so the "you omitted the parameter" half of the diagnosis is lost. Why: AWS's single message covers both omission and an out-of-range value; clients and operators that match AWS's wording see text no AWS region emits, and an omitted NumberOfNodes is reported as if a value had been supplied. Fix: the CreateCluster handler's cluster-shape validation message. Traces: redshift-rshealth-create-cluster-shape-refusals#task#3.

Cause 4 of 6: a nonexistent ClusterParameterGroupName is not resolved at CreateCluster time. AWS: CreateCluster naming a ClusterParameterGroupName that does not exist is refused with ClusterParameterGroupNotFound at HTTP 404 and the message "ClusterParameterGroup not found: <ClusterParameterGroupName>" (fixed text "ClusterParameterGroup not found: " plus the group name from the request). Smithy: CreateCluster models ClusterParameterGroupNotFoundFault with awsQueryError code ClusterParameterGroupNotFound and httpResponseCode 404, so this is a modeled fault the emulator simply never raises. Emulator: HTTP 400 with code ClusterAlreadyExists and message "Cluster already exists: <other cluster identifier>" — again the duplicate check from the leaked state in cause 1/2 answering first. Why: the client is given a 400 about a name collision instead of the 404 that tells it to create the parameter group first, so dependency-ordering logic in provisioning tools never fires. Fix: the CreateCluster handler's referenced-resource resolution (parameter group, and the same lookup for subnet/security groups), run before the duplicate-identifier check. Traces: redshift-rshealth-create-cluster-shape-refusals#task#6.

Cause 5 of 6: DescribeClusters resolves a cluster whose creation AWS refused. AWS: after every CreateCluster in the fixture has been refused, DescribeClusters(ClusterIdentifier=<that identifier>) fails with ClusterNotFound at HTTP 404, message "Cluster <ClusterIdentifier> not found." (fixed text "Cluster " + identifier + " not found."). Smithy: DescribeClusters models ClusterNotFoundFault, awsQueryError code ClusterNotFound, httpResponseCode 404. Emulator: HTTP 200 with a Clusters payload for the phantom cluster. Why: the emulator reports a cluster that does not exist in AWS, so any test that asserts "the refused create left nothing behind" passes falsely and cleanup code operates on a resource AWS never charged for. Fix: the CreateCluster state write (do not persist on refusal); DescribeClusters' lookup is correct once state is. Traces: redshift-rshealth-create-cluster-credential-refusals#task#6.

Cause 6 of 6: DeleteCluster likewise succeeds against the phantom cluster. AWS: DeleteCluster on an identifier whose create was refused fails ClusterNotFound at HTTP 404 with the same "Cluster <ClusterIdentifier> not found." template. Smithy: DeleteCluster models ClusterNotFoundFault (awsQueryError ClusterNotFound, httpResponseCode 404). Emulator: HTTP 200 with a DeleteCluster result. Why: a teardown that deletes successfully hides the leak entirely — the emulator both created and destroyed a resource AWS refused, so the divergence is invisible to the caller. Fix: same CreateCluster state write; no separate DeleteCluster change is needed once nothing is persisted on refusal. Traces: redshift-rshealth-create-cluster-credential-refusals#teardown#7.

## Evidence
Cause 1 — redshift-rshealth-create-cluster-credential-refusals#task#1 (also redshift-rshealth-create-cluster-shape-refusals#task#5): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'The parameter MasterUserPassword must contain at least 1 decimal digit.'}}, got None.

Cause 2 — redshift-rshealth-create-cluster-credential-refusals#task#2 (cluster of 7 steps): $.ErrorResponse.Error.Code expected 'InvalidParameterValue', got 'ClusterAlreadyExists'; $.ErrorResponse.Error.Message expected 'Invalid master password. Password should be atleast 8 characters long.', got 'Cluster already exists: <cluster identifier>'.

Cause 3 — redshift-rshealth-create-cluster-shape-refusals#task#3 (cluster of 3 steps): $.ErrorResponse.Error.Message expected 'Number of nodes must be supplied for cluster type multi-node and must be greater than or equal to 2.', got 'Number of nodes for cluster type multi-node must be greater than or equal to 2.' (status and code matched).

Cause 4 — redshift-rshealth-create-cluster-shape-refusals#task#6: status expected 404, got 400; Code expected 'ClusterParameterGroupNotFound', got 'ClusterAlreadyExists'; Message expected 'ClusterParameterGroup not found: <group name>', got 'Cluster already exists: <other cluster identifier>'.

Cause 5 — redshift-rshealth-create-cluster-credential-refusals#task#6 (also redshift-rshealth-create-cluster-shape-refusals#task#12): status expected 404, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'ClusterNotFound', 'Message': 'Cluster <identifier> not found.'}}, got None.

Cause 6 — redshift-rshealth-create-cluster-credential-refusals#teardown#7 (also redshift-rshealth-create-cluster-shape-refusals#teardown#13): status expected 404, got 200; $.ErrorResponse missing — expected code 'ClusterNotFound', message 'Cluster <identifier> not found.', got None.

## Steps to Reproduce
1. Boot the emulator and point a redshift (query-protocol) client at it.
2. Cause 1: CreateCluster with a MasterUserPassword that has no decimal digit (otherwise valid request) -> expect HTTP 400 InvalidParameterValue 'The parameter MasterUserPassword must contain at least 1 decimal digit.'; today HTTP 200 and a cluster is created.
3. Cause 2: CreateCluster on the same ClusterIdentifier with a password shorter than 8 characters -> expect HTTP 400 InvalidParameterValue 'Invalid master password. Password should be atleast 8 characters long.'; today HTTP 400 ClusterAlreadyExists.
4. Cause 3: CreateCluster with ClusterType=multi-node and NumberOfNodes omitted -> the message must read 'Number of nodes must be supplied for cluster type multi-node and must be greater than or equal to 2.'
5. Cause 4: CreateCluster naming a ClusterParameterGroupName that does not exist -> expect HTTP 404 ClusterParameterGroupNotFound 'ClusterParameterGroup not found: <name>'; today HTTP 400 ClusterAlreadyExists.
6. Cause 5: after every create above has been refused, DescribeClusters(ClusterIdentifier=<that identifier>) -> expect HTTP 404 ClusterNotFound 'Cluster <identifier> not found.'; today HTTP 200 with a cluster.
7. Cause 6: DeleteCluster(ClusterIdentifier=<that identifier>) -> expect HTTP 404 ClusterNotFound; today HTTP 200.
8. Re-replay fixtures redshift-rshealth-create-cluster-credential-refusals and redshift-rshealth-create-cluster-shape-refusals to confirm.
