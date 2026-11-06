Issue-Source: trace-sweep-divergence
Issue-Trace: iam-ekscc-traj-attach-cluster-policy-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-ekscc-traj-attach-cluster-policy-mistakes iam-emr-roles-policy-path-and-rerun-mistakes iam-gpn-traj-attach-glue-service-role-to-the-worker
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|AttachRolePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|AttachRolePolicy|4xx->2xx|status
Issue-Key: iam|ListAttachedRolePolicies|2xx->2xx|$.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies.member
Issue-Key: iam|ListAttachedRolePolicies|2xx->2xx|$.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies.member[*]
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM accepts AWS-managed policy ARNs with a nonexistent path, creating phantom attachments

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachRolePolicy

## Description
The emulator does not resolve an AWS-managed policy ARN against the managed-policy catalog: any syntactically valid arn:aws:iam::aws:policy/... ARN is attached, including one whose path does not exist. The bogus attachment then shows up in the role's attached-policy list. 3 causes.

Cause 1 of 3: AttachRolePolicy succeeds for an AWS-managed policy ARN whose path is wrong.
AWS: Smithy AttachRolePolicy models NoSuchEntityException with awsQueryError code NoSuchEntity and httpError 404. Recording: 404 with Error.Type Sender, Error.Code NoSuchEntity and message "Policy <policyArn> does not exist or is not attachable." — fixed text "Policy " + the requested PolicyArn + " does not exist or is not attachable." AmazonEKSClusterPolicy exists at arn:aws:iam::aws:policy/AmazonEKSClusterPolicy, so the /service-role/ form names no policy.
Emulator: 200, and the attachment is recorded.
Why: a caller that guessed the wrong path is told the attachment succeeded. The role then appears to carry a permission that does not exist anywhere in AWS, and the caller never corrects the ARN.
Fix: AttachRolePolicy's handler must look the PolicyArn up in the managed-policy catalog (path included) before attaching, and refuse with NoSuchEntity otherwise.
Traces: iam-ekscc-traj-attach-cluster-policy-mistakes#task#2.

Cause 2 of 3: ListAttachedRolePolicies then reports the phantom attachment alongside the real one.
AWS: the recording shows a single AttachedPolicies entry, {PolicyName: AmazonEKSClusterPolicy, PolicyArn: arn:aws:iam::aws:policy/AmazonEKSClusterPolicy}.
Emulator: two entries with the same PolicyName and two different ARNs (the real one and the /service-role/ one), because the refused attachment of Cause 1 persisted.
Why: a client that keys attached policies by PolicyName sees a duplicate and either errors or detaches the wrong ARN; an idempotency check ("is this policy already attached?") answers yes for an ARN AWS never accepted.
Fix: state — the attached-policy set must only contain catalog-resolvable policies; keyed on the resolved policy identity, not the raw ARN string.
Traces: iam-ekscc-traj-attach-cluster-policy-mistakes#task#7.

Cause 3 of 3: the same phantom entry appears in the EMR role's attached-policy list, so the list contents and ordering diverge.
AWS: the recording shows exactly the two attachments the fixture made, with AmazonElasticMapReducePlacementGroupPolicy at its real ARN.
Emulator: returns three entries — AmazonElasticMapReduceRole, and AmazonElasticMapReducePlacementGroupPolicy twice, once under the nonexistent /service-role/ path.
Why: as Cause 2, and the extra member shifts every index a caller reads positionally.
Fix: same component as Cause 2.
Traces: iam-emr-roles-policy-path-and-rerun-mistakes#task#9.

The open issue 20261030T014928Z-iam-built-in-aws-reference-catalogs-diverge-from.md covers managed-policy metadata fidelity but carries no replay key for the accept/reject behaviour above; 20261029T184359Z-iam-nosuchentity-messages-use-emulator-prose-instead-of.md covers NoSuchEntity wording on other operations.

## Evidence
Replay of trace set simparity-wire-aws-v1.

Cause 1 — iam-ekscc-traj-attach-cluster-policy-mistakes#task#2, AttachRolePolicy (cluster of 4, also iam-emr-roles-policy-path-and-rerun-mistakes#task#4/#task#6, iam-gpn-traj-attach-glue-service-role-to-the-worker#task#5): status expected 404, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchEntity', 'Message': 'Policy arn:aws:iam::aws:policy/service-role/AmazonEKSClusterPolicy does not exist or is not attachable.'}, 'RequestId': '<shaped-not-asserted>'}, got None.

Cause 2 — iam-ekscc-traj-attach-cluster-policy-mistakes#task#7, ListAttachedRolePolicies (also iam-gpn-traj-attach-glue-service-role-to-the-worker#task#7), 200 vs 200: $.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies.member expected {'PolicyArn': 'arn:aws:iam::aws:policy/AmazonEKSClusterPolicy', 'PolicyName': 'AmazonEKSClusterPolicy'}, got [{'PolicyName': 'AmazonEKSClusterPolicy', 'PolicyArn': 'arn:aws:iam::aws:policy/AmazonEKSClusterPolicy'}, {'PolicyName': 'AmazonEKSClusterPolicy', 'PolicyArn': 'arn:aws:iam::aws:policy/service-role/AmazonEKSClusterPolicy'}].

Cause 3 — iam-emr-roles-policy-path-and-rerun-mistakes#task#9, ListAttachedRolePolicies, 200 vs 200: AttachedPolicies.member[1] expected {'PolicyArn': 'arn:aws:iam::aws:policy/AmazonElasticMapReducePlacementGroupPolicy', 'PolicyName': 'AmazonElasticMapReducePlacementGroupPolicy'}, got {'PolicyName': 'AmazonElasticMapReduceRole', ...}; member[2] expected None, got {'PolicyName': 'AmazonElasticMapReducePlacementGroupPolicy', 'PolicyArn': 'arn:aws:iam::aws:policy/service-role/AmazonElasticMapReducePlacementGroupPolicy'}.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay iam-ekscc-traj-attach-cluster-policy-mistakes: at step task#2 AttachRolePolicy is called with arn:aws:iam::aws:policy/service-role/AmazonEKSClusterPolicy — observe 200 instead of 404 NoSuchEntity.
3. Continue to step task#7 and observe two AttachedPolicies members instead of one.
4. Replay iam-emr-roles-policy-path-and-rerun-mistakes#task#9 and diff the AttachedPolicies list.
