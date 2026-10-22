Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[*]
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member.LifecycleState
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Elastic Beanstalk ListPlatformBranches serves a placeholder branch catalog with lowercase lifecycle states

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformBranches

## Description
TEN causes, all of ListPlatformBranches, all rooted in the same defect: the emulator answers from a tiny hard-coded placeholder branch list instead of the AWS-managed platform-branch catalog, and it renders the catalog's enum-like fields in the wrong case and BranchOrder as a 0,1,2 counter.

Shared model grounding — Smithy: com.amazonaws.elasticbeanstalk#ListPlatformBranches returns ListPlatformBranchesResult{PlatformBranchSummaryList}; PlatformBranchSummary = {PlatformName, BranchName, LifecycleState (PlatformBranchLifecycleState), BranchOrder (integer, default 0), SupportedTierList}. Recorded behavior: AWS returns, per PlatformName filter, the full managed branch inventory for that platform, with LifecycleState capitalized ('Supported' / 'Deprecated' / 'Retired' / 'Beta' — the capitalized form is also what the model's PlatformDescription.PlatformBranchLifecycleState doc lists) and BranchOrder as large descending ordinals in hundreds (11300, 11200, 11100 ...), newest branch first. Hypothesis: the lowercase values and 0/1/2 ordinals come from the emulator's own seed table rather than from a transcription of AWS's catalog.

Cause 1 of 10: filter PlatformName='.NET Core on Linux'.
AWS: recorded behavior only — 200 with branches '.NET 10 running on 64bit Amazon Linux 2023' (LifecycleState 'Supported', BranchOrder 11300), '.NET 9 ...' (Supported, 11200), '.NET 8 ...' (Supported, 11100), '.NET 6 ...' (Retired, 11000) and further retired branches.
Emulator: 200 but a 3-entry list offset by one release ('.NET 9 ...' with LifecycleState 'beta', BranchOrder 2; '.NET 8 ...' 'supported', 1; '.NET Core running on 64bit Amazon Linux 2' 'deprecated', 0); the newest supported branch and all but one retired branch are absent.
Why: a client that picks a branch by LifecycleState=='Supported' finds none (case differs) and cannot see the current .NET branch at all, so platform selection and any UI ordered by BranchOrder are wrong.
Fix: the ListPlatformBranches handler and the static platform catalog it reads in the elasticbeanstalk service crate (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/list_platform_branches.rs and the catalog data it serves).
Traces: elasticbeanstalk-catalog-list-platform-branches#task#1.

Cause 2 of 10: filter PlatformName='.NET on Windows Server'.
AWS: recorded behavior only — 200 with 'IIS 10.0 running on 64bit Windows Server 2025' (Supported, 10500), '... Core 2025' (Supported, 10400), '... 2022' (Supported, 10200), '... Core 2022' (Supported, 10100) and more.
Emulator: 200 with only two entries ('... 2019' 'supported' order 1, '... Core 2016' 'retired' order 0); members [2..] are absent entirely.
Why: the Windows platform inventory a client enumerates is neither current nor complete.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#2.

Cause 3 of 10: filter PlatformName='Docker'.
AWS: recorded behavior only — 'Docker running on 64bit Amazon Linux 2023' (Supported, 12000), 'ECS running on 64bit Amazon Linux 2023' (Supported, 11500), plus retired Docker branches at 11000.
Emulator: 200 with 3 entries; the ECS branch is missing, LifecycleState lowercase, BranchOrder 0..2.
Why: the ECS-on-AL2023 branch cannot be discovered, so ECS-tier environment creation is unreachable by discovery.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#3.

Cause 4 of 10: filter PlatformName='GlassFish' (and the other filters that share this cluster).
AWS: recorded behavior only — 200 with four retired 'Preconfigured Docker - GlassFish ...' branches (LifecycleState 'Retired', BranchOrder 10000/9900/9800/9700).
Emulator: 200 with PlatformBranchSummaryList serialized as the empty string (no member at all) — the whole platform is missing from the catalog.
Why: a client enumerating a retired platform sees an empty result rather than its retired branches, so retirement-aware behavior cannot be exercised.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#4 (cluster also covers task#6, task#9, task#13 of the same fixture).

Cause 5 of 10: filter PlatformName='Go'.
AWS: recorded behavior only — 'Go 1 running on 64bit Amazon Linux 2023' (Supported, 12000), '(BETA) Go 1 ... Amazon Linux 2' (Retired, 11000), 'Go 1 ... Amazon Linux 2' (Retired, 11000), 'Go 1 ... Amazon Linux' (Retired, 10000) and the preconfigured-Docker Go branch.
Emulator: 200 with 2 entries, lowercase states, orders 0/1; members [2..] absent.
Why: same as above — truncated, mis-cased inventory.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#5.

Cause 6 of 10: filter PlatformName='Node.js'.
AWS: recorded behavior only — 'Node.js 24 ... 2023' (Supported, 12300), 'Node.js 22 ...' (Supported, 12200), 'Node.js 20 ...' (Retired, 12100), 'Node.js 18 ...' (Retired, 12000) and more (13 members).
Emulator: 200 with 3 entries, newest branch missing, LifecycleState lowercase ('supported'/'deprecated'), orders 1..3; AWS marks 20 and 18 'Retired' where the emulator marks 18 'deprecated'.
Why: branch support state drives upgrade decisions; a client reads a deprecated branch as still usable and cannot see Node.js 24.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#7.

Cause 7 of 10: filter PlatformName='PHP'.
AWS: recorded behavior only — 'PHP 8.5 ... 2023' (Supported, 12400) down through 'PHP 8.2 ...' (Supported, 12100) and 22 further members.
Emulator: 200 with 3 entries (8.4/8.3 'supported', 8.1 'retired'), orders 0..2; members [3..25] absent.
Why: the current PHP branch is undiscoverable and the list is 3 of 26 members.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#8 (cluster also covers task#12).

Cause 8 of 10: filter PlatformName='Python'.
AWS: recorded behavior only — 'Python 3.14 ... 2023' (Supported, 13500), '3.13' (Supported, 13400), '3.12' (Supported, 13300), '3.11' (Supported, 13200) and 12 more.
Emulator: 200 with 3 entries ('3.13' 'supported' 3, '3.11' 'supported' 2, '3.9' 'deprecated' 1); 3.14 and 3.12 missing.
Why: Python branch selection resolves to a non-current branch.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#10.

Cause 9 of 10: filter PlatformName='Ruby'.
AWS: recorded behavior only — 'Ruby 4.0 ... 2023' (Supported, 12400), '3.4' (Supported, 12300), '3.3' (Supported, 12200), '3.2' (Retired, 12100) and 25 more members.
Emulator: 200 with 2 entries ('3.4' 'supported' 1, '3.2' 'deprecated' 0); members [2..28] absent, and 3.2 is reported deprecated where AWS reports it retired.
Why: same as above.
Fix: same handler plus catalog data.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#11.

Cause 10 of 10: LifecycleState case alone, on a single-branch filtered result.
AWS: recorded behavior only — PlatformBranchSummaryList.member.LifecycleState is 'Supported'.
Emulator: 'supported'.
Why: any client or assertion comparing LifecycleState to AWS's value fails even when the branch itself is right; this is the case defect isolated from the inventory defect, so fixing the catalog contents without fixing the casing leaves it failing.
Fix: the serializer/catalog value for PlatformBranchSummary.LifecycleState in the same handler.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#2.

## Evidence
Cause 1 — expected member[0] {'BranchName': '.NET 10 running on 64bit Amazon Linux 2023', 'LifecycleState': 'Supported', 'BranchOrder': '11300'}; actual member[0] {'BranchName': '.NET 9 running on 64bit Amazon Linux 2023', 'LifecycleState': 'beta', 'BranchOrder': '2'} (status 200 both).
Cause 2 — expected member[0] 'IIS 10.0 running on 64bit Windows Server 2025' (Supported, 10500); actual 'IIS 10.0 running on 64bit Windows Server 2019' (supported, 1); expected members [2],[3],[4] present, actual None.
Cause 3 — expected member[1] 'ECS running on 64bit Amazon Linux 2023' (Supported, 11500); actual 'Docker running on 64bit Amazon Linux 2' (deprecated, 1).
Cause 4 — expected PlatformBranchSummaryList {'member': [4 x Preconfigured Docker - GlassFish ..., LifecycleState 'Retired']}; actual '' (type_mismatch, empty list).
Cause 5 — expected member[0] 'Go 1 running on 64bit Amazon Linux 2023' (Supported, 12000); actual same branch but LifecycleState 'supported', BranchOrder '1'; members [2..5] expected, actual None.
Cause 6 — expected member[0] 'Node.js 24 running on 64bit Amazon Linux 2023' (Supported, 12300); actual member[0] 'Node.js 22 ...' (supported, 3); 13 expected members vs 3 actual.
Cause 7 — expected member[0] 'PHP 8.5 running on 64bit Amazon Linux 2023' (Supported, 12400); actual 'PHP 8.4 ...' (supported, 2); expected members up to [25], actual None past [2].
Cause 8 — expected member[0] 'Python 3.14 running on 64bit Amazon Linux 2023' (Supported, 13500); actual 'Python 3.13 ...' (supported, 3).
Cause 9 — expected member[0] 'Ruby 4.0 running on 64bit Amazon Linux 2023' (Supported, 12400); actual 'Ruby 3.4 ...' (supported, 1); expected members up to [28], actual None past [1].
Cause 10 — $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member.LifecycleState: expected 'Supported', got 'supported'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture elasticbeanstalk-catalog-list-platform-branches, steps task#1, task#2, task#3, task#4, task#5, task#7, task#8, task#10, task#11 — each is a ListPlatformBranches call with a Filters[0] {Type:'PlatformName', Operator:'=', Values:[<platform>]}.
3. Compare PlatformBranchSummaryList to the recorded response: the emulator returns a 2-3 member placeholder list, lowercase LifecycleState and BranchOrder 0,1,2.
4. Replay fixture elasticbeanstalk-platform-catalog-and-account-quotas step task#2 and observe LifecycleState 'supported' where AWS returns 'Supported'.
