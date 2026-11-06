Issue-Source: trace-sweep-divergence
Issue-Trace: eks-catalog-list-access-policies
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-catalog-list-access-policies
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodSystemNamespacePolicy"]
Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodTrainingPolicy"]
Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodUserClusterPolicy"]
Issue-Commit: e0e096f7847ddbab99498abf6c574ac0da8fbc09

# eks ListAccessPolicies omits the SageMaker HyperPod managed cluster-access policies

> _Note: this issue was identified against the code base at commit `e0e096f7847ddbab99498abf6c574ac0da8fbc09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccessPolicies

## Description
ONE net-new root cause in the EKS managed access-policy catalog. The reader answers 200 with the right envelope, but the published policy set it projects is missing a whole family of managed cluster-access policies, so the divergence lives in the static catalog the operation serves rather than in its serializer.

Cause 1 of 1: ListAccessPolicies does not serve the Amazon SageMaker HyperPod managed cluster-access policies.
AWS: Smithy: ListAccessPolicies GET /access-policies -> 200 with accessPolicies (AccessPoliciesList of AccessPolicy{name, arn}), paginated (inputToken/outputToken nextToken, items accessPolicies, pageSize maxResults 1..100); the only modeled error is ServerException (500). AccessPolicy's doc states the policies are managed by Amazon EKS, so the set is reference data the service publishes, not account state. AWS names each managed policy as arn:aws:eks::aws:cluster-access-policy/<PolicyName> with the AccessPolicy.name member equal to <PolicyName>; the HyperPod family in the recording is AmazonSagemakerHyperpodSystemNamespacePolicy, AmazonSagemakerHyperpodTrainingPolicy and AmazonSagemakerHyperpodUserClusterPolicy (recorded behavior only for which names are in the published set — the model enumerates no values).
Emulator: answers 200 with an accessPolicies list from which all three HyperPod entries are absent — the replay reports $.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodSystemNamespacePolicy"], ...HyperpodTrainingPolicy and ...HyperpodUserClusterPolicy each as missing (expected {name, arn}, got None). Status and envelope match; only the catalog contents diverge.
Why: AssociateAccessPolicy takes a policyArn drawn from this catalog, so a client that discovers or validates an access policy against the published set treats the HyperPod policies as nonexistent and cannot build a HyperPod-style access entry at all; a client that enumerates the managed policies to render a chooser silently offers an incomplete set. Because this reader is also the paginated source of truth for the policy namespace, a caller cannot distinguish "not published" from "not seeded".
Fix: the managed access-policy catalog seed behind the ListAccessPolicies handler in the eks service (the reference data and its pagination over the full set), not the response serializer.
Traces: eks-catalog-list-access-policies#task#1.

Related open issue, which does NOT cover this: 'eks read-only catalogs serve incomplete static data across five readers' (20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md) describes the same HyperPod gap as its Cause 3 of 5, but its stored replay key for this reader is recorded truncated (Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn), so the replay gate does not bind this cluster to it and the step came back unowned on this sweep. This issue carries the exact fixture/step binding so the fix is gated on a replay that actually exercises it.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 55 fixtures replayed, 393 steps, 227 passed, 164 mismatched, 2 errored, score 57.8%, 39 divergence clusters (all service=eks). This cluster is the only one reported with no owning open issue and no stub.

ListAccessPolicies, fixture eks-catalog-list-access-policies, step task#1 (affected_count 1):
  divergence_key: service=eks|ListAccessPolicies|status=200->200|keys=$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodSystemNamespacePolicy"],$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodTrainingPolicy"],$.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodUserClusterPolicy"]
  expected status 200, actual status 200 (status is NOT the divergence)
  Expected: accessPolicies contains {'name': 'AmazonSagemakerHyperpodSystemNamespacePolicy', 'arn': 'arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodSystemNamespacePolicy'}; got None
  Expected: accessPolicies contains {'name': 'AmazonSagemakerHyperpodTrainingPolicy', 'arn': 'arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodTrainingPolicy'}; got None
  Expected: accessPolicies contains {'name': 'AmazonSagemakerHyperpodUserClusterPolicy', 'arn': 'arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodUserClusterPolicy'}; got None

The requirement is structural, not literal: for every managed cluster-access policy EKS publishes, accessPolicies must contain one AccessPolicy whose arn is arn:aws:eks::aws:cluster-access-policy/<PolicyName> and whose name is that same <PolicyName>. The fixed text of each ARN is 'arn:aws:eks::aws:cluster-access-policy/' (note the empty region and account segments, which are part of the fixed text for these service-managed policies); the variable part is the policy name. The three names above are the entries this fixture shows missing, not an exhaustive list of the catalog.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Locate the divergence cluster for fixture eks-catalog-list-access-policies, step task#1, operation ListAccessPolicies.
3. Equivalently by hand: issue GET /access-policies against the eks endpoint with no maxResults/nextToken and read $.accessPolicies from the 200 response.
4. Search the returned list for arn:aws:eks::aws:cluster-access-policy/AmazonSagemakerHyperpodSystemNamespacePolicy, .../AmazonSagemakerHyperpodTrainingPolicy and .../AmazonSagemakerHyperpodUserClusterPolicy. All three are absent from the emulator's response and present in the recording.
5. Page the reader with maxResults to confirm the entries are missing from the catalog itself rather than cut off by pagination.
6. After fixing, re-replay eks-catalog-list-access-policies#task#1 and confirm each HyperPod policy appears as an AccessPolicy whose name equals the trailing segment of its arn:aws:eks::aws:cluster-access-policy/<PolicyName> ARN.
