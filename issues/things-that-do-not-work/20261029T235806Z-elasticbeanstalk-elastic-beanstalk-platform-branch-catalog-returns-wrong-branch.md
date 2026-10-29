Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[*]
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# Elastic Beanstalk platform-branch catalog returns wrong branch names, lifecycle states and orders (10 causes)

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformBranches

## Description
TEN distinct root causes in the elasticbeanstalk emulator, all in the same service and all on ListPlatformBranches. The emulator ships a hand-written platform-branch catalog that disagrees with the catalog AWS publishes: per PlatformName filter it serves different BranchName strings, different LifecycleState values, different BranchOrder ordinals, and a different set/ordering of PlatformBranchSummary members. TEN causes must be closed before submitting. One issue per platform is intentional: each cause is a separate slice of the catalog and is replayed by its own fixture step.

Common AWS reference for all ten: Smithy ListPlatformBranches(Filters: SearchFilters, MaxRecords >=1, NextToken) -> ListPlatformBranchesResult{PlatformBranchSummaryList, NextToken}; PlatformBranchSummary{PlatformName, BranchName, LifecycleState (PlatformBranchLifecycleState: beta | supported | deprecated | retired), BranchOrder (integer, default 0 — "A larger BranchOrder value designates a newer platform branch within the platform"), SupportedTierList (WebServer/Standard | Worker/SQS/HTTP)}. The branch inventory itself is service data, so the requirement for each cause below is the RECORDED catalog for that PlatformName filter — the set of branch names, their lifecycle states, their relative BranchOrder ordinals and their supported tiers — not any single run's values. Emulator behaviour for all ten is HTTP 200 with a well-formed but wrong list, so nothing signals the divergence to a client.

Why (shared): clients and IaC tools enumerate this catalog to choose a platform branch before creating an environment. A branch AWS reports as Supported but the emulator reports Retired (or omits entirely) makes a valid deployment look impossible; a branch the emulator invents makes an impossible deployment look valid; and because BranchOrder is documented as the newest-first ordinal, wrong ordinals make 'pick the newest branch' select the wrong platform.

Fix (shared): the platform-branch catalog data the ListPlatformBranches handler serves and the filter/ordering it applies to it — services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/list_platform_branches.rs and the platform catalog held in crate::state.

Cause 1 of 10: PlatformName '.NET Core on Linux' branch set is wrong.
AWS: recorded behavior only — the branch list contains '.NET Core running on 64bit Amazon Linux 2' (LifecycleState Retired, BranchOrder 11000, tiers WebServer/Standard + Worker/SQS/HTTP) at the position the emulator fills with something else.
Emulator: that member is absent; index 4 instead holds '.NET 6 running on 64bit Amazon Linux 2' with BranchOrder 10100, and an extra member is appended at index 5.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#1 (same cluster also fails at task#5).

Cause 2 of 10: PlatformName '.NET on Windows Server' lifecycle states are shifted one level toward end-of-life.
AWS: recorded behavior only — 'IIS 10.0 running on 64bit Windows Server 2019' and 'IIS 10.0 running on 64bit Windows Server Core 2019' are LifecycleState 'Supported'; the 2016 pair are 'Deprecated'.
Emulator: the 2019 pair are reported 'Deprecated' and the 2016 pair 'Retired' — every branch is aged by one lifecycle step while BranchName/BranchOrder/SupportedTierList match.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#2.

Cause 3 of 10: PlatformName 'Docker' omits the BETA branch and misorders ECS.
AWS: recorded behavior only — the list includes '(BETA) Docker running on 64bit Amazon Linux 2' (BranchOrder 11000), 'ECS running on 64bit Amazon Linux 2' (BranchOrder 10500) and 'Docker running on 64bit Amazon Linux' (BranchOrder 9000).
Emulator: the '(BETA)' branch is missing entirely, ECS is emitted at BranchOrder 11000 in the BETA branch's slot, and the BranchOrder-9000 Docker-on-Amazon-Linux member is absent.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#3.

Cause 4 of 10: PlatformName 'GlassFish' branch names and supported tiers are wrong.
AWS: recorded behavior only — branch names carry the Java qualifier and the Linux generation AWS published, e.g. 'Preconfigured Docker - GlassFish 5.0 with Java 8 running on 64bit Amazon Linux', 'Preconfigured Docker - GlassFish 4.1 with Java 8 running on 64bit Debian', each with SupportedTierList {WebServer/Standard, Worker/SQS/HTTP}.
Emulator: the 'with Java 8' qualifier is dropped and the OS generation changed ('Preconfigured Docker - GlassFish 5.0 running on 64bit Amazon Linux 2', 'Preconfigured Docker - GlassFish 4.1 running on 64bit Debian jessie'), and SupportedTierList carries only WebServer/Standard.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#4.

Cause 5 of 10: PlatformName 'Java' has no current Corretto branches at all.
AWS: recorded behavior only — the list leads with the Supported Corretto branches on Amazon Linux 2023 (Corretto 25/21/17/11, BranchOrder descending from 11400).
Emulator: those members are absent; the list begins with Retired 'Java 8 running on 64bit Amazon Linux' / 'Java 7 running on 64bit Amazon Linux' (BranchOrder 10100/10000) and runs short, so every Supported Java branch is missing.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#6.

Cause 6 of 10: PlatformName 'Node.js' omits the BETA branches and the 32-bit branch, and BranchOrder is off by one step.
AWS: recorded behavior only — the list includes '(BETA) Node.js 12 running on 64bit Amazon Linux 2' (BranchOrder 11100), '(BETA) Node.js 10 running on 64bit Amazon Linux 2' (BranchOrder 11000), 'Node.js 10 running on 64bit Amazon Linux 2' at BranchOrder 11100 and 'Node.js running on 32bit Amazon Linux' (BranchOrder 9900).
Emulator: both '(BETA)' members and the 32-bit member are absent, and 'Node.js 10 running on 64bit Amazon Linux 2' is emitted with BranchOrder 11000 instead of 11100.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#7.

Cause 7 of 10: PlatformName 'PHP' omits the BETA branches and the PHP 7.3 Amazon Linux branch.
AWS: recorded behavior only — the list includes '(BETA) PHP 7.4 / 7.3 / 7.2 running on 64bit Amazon Linux 2' (BranchOrder 11200/11100/11000) and 'PHP 7.3 running on 64bit Amazon Linux' (BranchOrder 10000).
Emulator: none of those are present; the non-BETA PHP 7.4/7.2/7.1 members slide into their slots with the BETA ordinals, so 18 list positions differ from the recording.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#8.

Cause 8 of 10: PlatformName 'Python' BranchOrder ordinals are wrong and the BETA 3.7 branch is missing.
AWS: recorded behavior only — 'Python 3.9 running on 64bit Amazon Linux 2023' is BranchOrder 13000, 'Python 3.8 running on 64bit Amazon Linux 2' is 12000, '(BETA) Python 3.7 running on 64bit Amazon Linux 2' is 11000 and 'Python 3.7 running on 64bit Amazon Linux 2' is 11000.
Emulator: the same branch names are served with BranchOrder 13100 / 12200 / 12100, the '(BETA)' member is absent, and 12 list positions differ.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#10.

Cause 9 of 10: PlatformName 'Ruby' omits the BETA branches and renames the Puma branches.
AWS: recorded behavior only — the list includes '(BETA) Ruby 2.7 / 2.6 / 2.5 running on 64bit Amazon Linux 2' (BranchOrder 11200/11100/11000) and 'Puma with Ruby 2.6 running on 64bit Amazon Linux' (BranchOrder 10000).
Emulator: the '(BETA)' members are absent; 'Ruby 3.0 running on 64bit Amazon Linux 2' (BranchOrder 11300) and 'Ruby 2.6 with Puma running on 64bit Amazon Linux' (BranchOrder 10400) occupy their slots — i.e. the Puma branch name word order differs from AWS's — and 23 positions diverge.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#11.

Cause 10 of 10: PlatformName 'Tomcat' has no Tomcat 11 branches, so the whole list is shifted.
AWS: recorded behavior only — the list leads with Supported 'Tomcat 11 with Corretto 25 / 21 / 17 running on 64bit Amazon Linux 2023' (BranchOrder descending from 12800).
Emulator: the newest branch it knows is 'Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023' (BranchOrder 12300); every one of the 26 reported members is therefore at the wrong position with the wrong ordinal.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#12.

## Evidence
All ten clusters: expected HTTP 200, actual HTTP 200 — the status matches and only the body diverges, so nothing surfaces to a client. Representative diffs from the replay (fixture elasticbeanstalk-catalog-list-platform-branches):
- task#1: $...PlatformBranchSummaryList.member[4] expected BranchName '.NET Core running on 64bit Amazon Linux 2' / BranchOrder '11000', got BranchName '.NET 6 running on 64bit Amazon Linux 2' / BranchOrder '10100'; member[5] extra.
- task#2: member[4] LifecycleState expected 'Supported', got 'Deprecated'; member[6] expected 'Deprecated', got 'Retired' (BranchName and BranchOrder identical).
- task#3: member[2] expected '(BETA) Docker running on 64bit Amazon Linux 2', got 'Docker running on 64bit Amazon Linux 2'; member[6] expected 'Docker running on 64bit Amazon Linux'/BranchOrder 9000, got None; member[3] extra 'ECS running on 64bit Amazon Linux 2'.
- task#4: member[0] expected 'Preconfigured Docker - GlassFish 5.0 with Java 8 running on 64bit Amazon Linux' with tiers [WebServer/Standard, Worker/SQS/HTTP], got 'Preconfigured Docker - GlassFish 5.0 running on 64bit Amazon Linux 2' with tier 'WebServer/Standard'.
- task#6: member[0] expected 'Corretto 25 running on 64bit Amazon Linux 2023'/Supported/11400, got 'Java 8 running on 64bit Amazon Linux'/Retired/10100; member[2] and member[3] (Corretto 17/11) got None.
- task#7: member[7] expected '(BETA) Node.js 12 running on 64bit Amazon Linux 2', got 'Node.js 12 running on 64bit Amazon Linux 2'; member[8] BranchOrder expected 11100 got 11000; member[10], member[12] got None.
- task#8: member[7] expected '(BETA) PHP 7.4 running on 64bit Amazon Linux 2', got 'PHP 7.4 running on 64bit Amazon Linux 2'; 18 member keys diverge.
- task#10: member[4] BranchOrder expected 13000 got 13100; member[5] expected 12000 got 12200; member[6] expected '(BETA) Python 3.7 ...'/11000 got 'Python 3.7 ...'/12100.
- task#11: member[5] expected '(BETA) Ruby 2.7 running on 64bit Amazon Linux 2'/11200, got 'Ruby 3.0 running on 64bit Amazon Linux 2'/11300; member[9] expected '(BETA) Ruby 2.5 ...' got 'Ruby 2.6 with Puma running on 64bit Amazon Linux'/10400.
- task#12: member[0] expected 'Tomcat 11 with Corretto 25 running on 64bit Amazon Linux 2023'/12800, got 'Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023'/12300; all 26 members diverge.
The branch names, lifecycle states, tiers and the relative BranchOrder ordinals are the requirement; the account id, region and any per-run token in the fixture are not.

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. For each cause, call eb.list_platform_branches(Filters=[{'Attribute':'PlatformName','Operator':'=','Values':[<PlatformName>]}]) with <PlatformName> in {'.NET Core on Linux' (cause 1), '.NET on Windows Server' (2), 'Docker' (3), 'GlassFish' (4), 'Java' (5), 'Node.js' (6), 'PHP' (7), 'Python' (8), 'Ruby' (9), 'Tomcat' (10)}.
3. Compare PlatformBranchSummaryList member-for-member against the recorded response: BranchName, LifecycleState, BranchOrder and SupportedTierList, in order.
4. Re-replay fixture elasticbeanstalk-catalog-list-platform-branches steps task#1, task#2, task#3, task#4, task#6, task#7, task#8, task#10, task#11, task#12 (step task#5 is in the same cluster as task#1).
