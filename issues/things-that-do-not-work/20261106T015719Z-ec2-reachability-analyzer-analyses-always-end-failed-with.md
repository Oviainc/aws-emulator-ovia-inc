Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgssh-reachability-allow-all-ordered-before-deny-finds-path ec2.asgssh-reachability-deny-on-neighbouring-ports-does-not-block-22 ec2.asgssh-reachability-path-blocked-by-nacl-deny-22-until-replaced ec2.op-contract-modify-ipam-prefix-list-resolver-target
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asgssh-reachability-allow-all-ordered-before-deny-finds-path|run#15|AssertionError|assert (result["Status"], result.get("NetworkPathFound")) == ("succeeded", True), (
Issue-Key: ec2.asgssh-reachability-deny-on-neighbouring-ports-does-not-block-22|run#16|AssertionError|assert (result["Status"], result.get("NetworkPathFound")) == ("succeeded", True), (
Issue-Key: ec2.asgssh-reachability-path-blocked-by-nacl-deny-22-until-replaced|run#13|AssertionError|assert (blocked["Status"], blocked.get("NetworkPathFound")) == ("succeeded", False), (
Issue-Key: ec2.op-contract-modify-ipam-prefix-list-resolver-target|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# EC2: Reachability Analyzer analyses always end `failed` with an emulator-authored StatusMessage and no path result, and the IPAM prefix-list-resolver chain never converges so ModifyIpamPrefixListResolverTarget is unreachable (2 causes)

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNetworkInsightsAnalysis

## Description
Two net-new root causes in this service, grounded in four failing behavioral checks. Both must be closed before those checks pass.

Cause 1 of 2: a network-insights analysis never produces a reachability verdict - it terminates with Status `failed` and an emulator-authored StatusMessage instead of Status `succeeded` with NetworkPathFound, ForwardPathComponents and Explanations.
AWS: Smithy: StartNetworkInsightsAnalysis returns a NetworkInsightsAnalysis; com.amazonaws.ec2#AnalysisStatus = running|succeeded|failed; NetworkInsightsAnalysis members NetworkPathFound ("Indicates whether the destination is reachable from the source"), ForwardPathComponents / ReturnPathComponents (PathComponentList, whose AclRule carries RuleNumber and RuleAction), Explanations (ExplanationList, "see Reachability Analyzer explanation codes") and StatusMessage, documented as "The status message, if the status is failed". Recorded behavior: a completed analysis of a well-formed path reaches Status `succeeded` and answers NetworkPathFound true or false - true with the ACL hop at the FIRST matching rule number (lowest rule number wins, e.g. allow at 100 ahead of a deny at 150, and a deny covering 23-1024 not blocking tcp/22), false with an Explanations entry naming the ACL that blocked it. `failed` is reserved for an analysis AWS could not run at all.
Emulator: every analysis terminates with Status `failed`, no NetworkPathFound, no ForwardPathComponents and no Explanations; on one check StatusMessage reads, verbatim, "This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path." - text AWS never emits.
Why: Reachability Analyzer is the API clients use to decide whether a path is open and, when it is not, which NACL rule or security group closed it. A client gets no verdict at all, and the `failed` status reads as an AWS-side analysis failure rather than an unimplemented feature, so retry/diagnosis logic loops or reports the wrong cause. The NACL ordering semantics the three checks probe (lowest rule number wins; a port range that excludes 22 does not block 22; replacing a deny with an allow re-opens the path) are unobservable.
Fix: the StartNetworkInsightsAnalysis / DescribeNetworkInsightsAnalyses handlers and the path-evaluation they need over the stored VPC state (route tables, NACL entries evaluated in ascending rule-number order, stateful security groups), plus the serializer members above; a `failed` status must stop being the universal answer. The open issue 20261104T142958Z-ec2-accepts-ten-requests-aws-refuses-or-answers.md owns one sibling check (ec2.asgssh-reachability-path-blocked-by-security-group-until-authorized) and 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md mentions reachability-analyzer paths answering 501; neither carries the three checks listed here.
Traces: ec2.asgssh-reachability-allow-all-ordered-before-deny-finds-path, ec2.asgssh-reachability-deny-on-neighbouring-ports-does-not-block-22, ec2.asgssh-reachability-path-blocked-by-nacl-deny-22-until-replaced.

Cause 2 of 2: ModifyIpamPrefixListResolverTarget cannot be exercised because the resource chain it needs never converges - a created IPAM / managed prefix list / IPAM prefix list resolver / resolver target is polled for `create-complete` and the check burns its entire budget without the state ever advancing.
AWS: Smithy: ModifyIpamPrefixListResolverTarget(IpamPrefixListResolverTargetId, DesiredVersion, TrackLatestVersion) -> IpamPrefixListResolverTarget, whose State is documented as create-in-progress | create-complete | create-failed | modify-in-progress | modify-complete | modify-failed | delete-in-progress | delete-complete | delete-failed, with LastSyncedVersion "the version of the prefix list that was last successfully synchronized by this target". Recorded behavior: each of these resources leaves its *-in-progress state and reaches create-complete shortly after creation (the recorded flow waits a few seconds), a tracking target then reports LastSyncedVersion, and pinning DesiredVersion with TrackLatestVersion false moves the target through sync-in-progress to sync-complete.
Emulator: the check dies with `Error: Check timed out after 60.0s` inside the first of these polls - one of DescribeIpams / DescribeManagedPrefixLists / DescribeIpamPrefixListResolvers / DescribeIpamPrefixListResolverTargets keeps reporting its *-in-progress state, so the modify under test is never reached and no DesiredVersion/LastSyncedVersion behavior is observable. No call is refused; the state simply never advances.
Why: any client that creates one of these resources and waits for create-complete before using it (the documented contract, and what every SDK waiter and CloudFormation-style provisioner does) hangs forever, and the whole prefix-list-resolver feature is unusable even though its operations answer 200.
Fix: the create handlers for these resources and the clock-owed state transitions they record (the same mechanism that converges other EC2 resources), so a created IPAM, managed prefix list, resolver and resolver target reach create-complete, a tracking target publishes LastSyncedVersion, and a version pin converges through sync-in-progress to sync-complete. The open issue 20261104T142958Z-ec2-state-never-converges-for-six-waited-on.md records the same class for other waited-on EC2 resources but does not carry this check.
Traces: ec2.op-contract-modify-ipam-prefix-list-resolver-target.

## Evidence
Check [mismatch] ec2.asgssh-reachability-allow-all-ordered-before-deny-finds-path
  Expected (AWS): with ingress ACL order allow@100, deny@150, *@32767, the analysis of src->dst tcp/22 reads (Status, NetworkPathFound) == ("succeeded", True) and ForwardPathComponents crosses that ACL at (RuleNumber 100, RuleAction allow).
  Actual (emulator): (Status, NetworkPathFound, Explanations) == ('failed', None, None).

Check [mismatch] ec2.asgssh-reachability-deny-on-neighbouring-ports-does-not-block-22
  Expected (AWS): a deny covering 23-1024 does not match tcp/22, so (Status, NetworkPathFound) == ("succeeded", True).
  Actual (emulator): ('failed', None, None).

Check [mismatch] ec2.asgssh-reachability-path-blocked-by-nacl-deny-22-until-replaced
  Expected (AWS): with a deny-SSH entry the analysis reads ("succeeded", False) plus an Explanations entry naming the ACL; after the entry is replaced with an allow the same path reads ("succeeded", True).
  Actual (emulator): ('failed', None, 'This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path.') - the fixed text of that StatusMessage is emulator-authored; AWS emits a StatusMessage only for a genuine analysis failure.

Check [mismatch] ec2.op-contract-modify-ipam-prefix-list-resolver-target
  Expected (AWS): CreateIpam -> IPAM create-complete; CreateManagedPrefixList -> create-complete; CreateIpamPrefixListResolver -> create-complete; CreateIpamPrefixListResolverTarget(TrackLatestVersion=true) -> create-complete and a non-null LastSyncedVersion; ModifyIpamPrefixListResolverTarget(DesiredVersion=<that version>, TrackLatestVersion=false) echoes TrackLatestVersion false / DesiredVersion and an independent read converges to State sync-complete.
  Actual (emulator): `Error: Check timed out after 60.0s` while polling for create-complete; the modify is never reached.

## Steps to Reproduce
Cause 1:
1. Create a VPC with two subnets in one AZ, a network ACL associated with the destination subnet holding ingress allow-all at rule 100 and a deny tcp/22 at rule 150, and a security group permitting tcp/22 from the VPC CIDR.
2. Create one ENI in each subnet, then ec2 CreateNetworkInsightsPath(Source=<src eni>, Destination=<dst eni>, Protocol=tcp, DestinationPort=22) and ec2 StartNetworkInsightsAnalysis on it.
3. Poll ec2 DescribeNetworkInsightsAnalyses until the status is terminal.
   Observed: Status `failed`, no NetworkPathFound, no ForwardPathComponents, no Explanations, and StatusMessage "This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path.". AWS answers Status `succeeded` with NetworkPathFound true and the ACL hop at rule 100 allow.
4. Repeat with the deny numbered 50 but covering ports 23-1024 (AWS: reachable), and with a deny on tcp/22 later replaced by an allow (AWS: unreachable, then reachable).

Cause 2:
1. ec2 CreateIpam(OperatingRegions=[<region>]) and poll ec2 DescribeIpams for State create-complete.
2. ec2 ModifyIpam(Tier=advanced); ec2 CreateManagedPrefixList(MaxEntries=1, AddressFamily=IPv4) and poll DescribeManagedPrefixLists for create-complete.
3. ec2 CreateIpamPrefixListResolver(IpamId=<ipam>, AddressFamily=ipv4) and poll DescribeIpamPrefixListResolvers for create-complete; then CreateIpamPrefixListResolverTarget(PrefixListId=<pl>, PrefixListRegion=<region>, TrackLatestVersion=true) and poll DescribeIpamPrefixListResolverTargets for create-complete and a LastSyncedVersion.
   Observed: one of these polls never leaves its *-in-progress state, so the run exhausts its time budget and ModifyIpamPrefixListResolverTarget is never called.
