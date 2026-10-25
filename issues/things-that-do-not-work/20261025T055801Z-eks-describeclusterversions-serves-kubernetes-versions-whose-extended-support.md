Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.ekscc-cluster-versions-support-status-agrees-with-support-dates
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.ekscc-cluster-versions-support-status-agrees-with-support-dates|run#8|AssertionError|assert now < ext, f"{v['clusterVersion']} is past extended support but still listed"
Issue-Commit: f701461943b9c8eea3c6b27fb58efe804c9a3fc9

# EKS DescribeClusterVersions serves Kubernetes versions whose extended support has already ended on an unfiltered call (includeAll is accepted and ignored)

> _Note: this issue was identified against the code base at commit `f701461943b9c8eea3c6b27fb58efe804c9a3fc9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterVersions

## Description
Replaying the behavioral suite against the EKS emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend) leaves ONE net-new failing check that the suite did not attribute to a scaffold stub or to an open issue's replay key. It is a single root cause.

Cause 1 of 1 — the Kubernetes version catalogue reports rows whose end-of-extended-support day has passed, on a call that asked for no such thing.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) — DescribeClusterVersions is GET /cluster-versions, paginated on clusterVersions with nextToken; its input carries includeAll (BoxedBoolean, documented "Include all available versions in the response"), versionStatus (VersionStatus: EXTENDED_SUPPORT | STANDARD_SUPPORT | UNSUPPORTED) and the deprecated status (ClusterVersionStatus: extended-support | standard-support | unsupported); each reported ClusterVersionInformation carries endOfStandardSupportDate, endOfExtendedSupportDate, versionStatus, defaultVersion and kubernetesPatchVersion. Recorded behavior: an UNFILTERED listing (no includeAll, no status/versionStatus) offers only versions still inside standard or extended support — every entry satisfies now < endOfExtendedSupportDate, and its versionStatus agrees with its own two dates (STANDARD_SUPPORT while now < endOfStandardSupportDate, EXTENDED_SUPPORT between the two end dates). A version whose extended support has lapsed is reachable only when the caller asks for it — includeAll=true, or a status/versionStatus filter naming the unsupported state. The concrete version numbers and dates are catalogue values and are examples; the requirement is the default filter, and that each row's reported status is a function of its own dates and the clock.

Emulator: the calendar is served filtered ONLY by release date, never by support window, and includeAll is deliberately ignored. src/ops/describe_cluster_versions.rs documents includeAll as "accepted and ignored, exactly as ListClusters accepts and ignores include" and selects rows with state::released_cluster_versions(now) (src/state.rs), whose only predicate is is_released (now >= release_date). The static table VERSION_CALENDAR in src/state.rs holds a row with end-of-extended-support 2026-07-23 for Kubernetes 1.30, and the unfiltered paginated listing still returns that row now that the clock is past that day — while the crate's own ClusterVersionRecord::support_status() classifies the very same row UNSUPPORTED. The served entry therefore contradicts its own dates, and no value of includeAll can change the answer.

Why: version discovery is the first call of a provisioning flow — a client reads the unfiltered catalogue and hands a clusterVersion to CreateCluster. Against this emulator it can pick a version real AWS no longer offers, so code that is correct against AWS either fails later or silently pins an out-of-support version; upgrade planners and compliance checks that read versionStatus see UNSUPPORTED rows in a listing AWS would never put them in, and cannot use includeAll to distinguish "everything" from "what is still offered" because the flag has no effect.

Why this is filed: the open issue 20261024T081355Z-eks-connected-clusters-are-never-gated-status-error.md discusses this same support-window filter as its Cause 2, but the suite did not mark this check as owned by it (its recorded replay key does not match this failure), so it is reported here as surviving. A fixer closing that issue's Cause 2 should close this one in the same change; the open issue 20261023T152714Z-eks-read-only-catalog-operations-serve-an-incomplete-static-catalog covers the catalogue being structurally incomplete and says nothing about the support window.

Fix: the DescribeClusterVersions handler's row selection in services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_cluster_versions.rs — after the release-date filter, drop rows whose ClusterVersionRecord::support_status(now) is UNSUPPORTED unless the request asks for them (includeAll true, or an explicit status/versionStatus filter naming the unsupported state), so every row in a default listing agrees with its own end-of-support dates; the calendar rows themselves live in src/state.rs (VERSION_CALENDAR) and must keep enough in-support minors that the default listing still offers several distinct versions and exactly one defaultVersion.

Traces: eks.ekscc-cluster-versions-support-status-agrees-with-support-dates.

## Evidence
Suite: simparity-behavioral-ga-prod-v1 — 15/111 passed; eks 14/90 (76 failed; all other eks failures are already-filed scaffold-stub clusters or carry an open issue's replay key).

[mismatch] eks.ekscc-cluster-versions-support-status-agrees-with-support-dates
  File "check_eks_ekscc_cluster_versions_support_status_agrees_with_support_dates.py", line 51, in run
    assert now < ext, f"{v['clusterVersion']} is past extended support but still listed"
  AssertionError: 1.30 is past extended support but still listed

The assertion is raised while walking EVERY page of an unfiltered eks.describe_cluster_versions() (no includeAll, no status/versionStatus), after the check has already confirmed the listing holds several distinct versions. Expected for every returned entry: now < endOfExtendedSupportDate, versionStatus == (STANDARD_SUPPORT if now < endOfStandardSupportDate else EXTENDED_SUPPORT), and kubernetesPatchVersion of that minor. Actual: the entry for Kubernetes 1.30 is still returned although its endOfExtendedSupportDate has passed.

Source confirmation in the checkout:
- services/service-com-amazonaws-eks-awswesleyfrontend/src/state.rs, VERSION_CALENDAR: ("1.30", "2024-05-23", "2025-07-23", "2026-07-23") — release, end of standard support, end of extended support.
- same file, ClusterVersionRecord::support_status(now): returns SupportStatus::Unsupported once now >= end_of_extended_support.
- same file, ComAmazonawsEksAwswesleyfrontendState::released_cluster_versions(now): filters on entry.is_released(now) only — no support-window predicate.
- services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_cluster_versions.rs, handler: builds `matching` from released_cluster_versions(now) with filters for clusterVersions / clusterType / defaultOnly / status+versionStatus only; its module doc states includeAll "is accepted and ignored".

Model evidence used: smithy_query query="doc" shape="DescribeClusterVersionsRequest" member="includeAll" against services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json -> "Include all available versions in the response."; the operation's modelled input/output members (includeAll, versionStatus, status, clusterVersions, nextToken, maxResults 1..100; ClusterVersionInformation.endOfStandardSupportDate / endOfExtendedSupportDate / versionStatus) and its errors InvalidParameterException (400), InvalidRequestException (400), ServerException (500) as recorded in the handler's modelled-contract header.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an EKS client at its data listener.
2. Walk every page of an unfiltered listing: kwargs={}; loop page = eks.describe_cluster_versions(**kwargs), collect page["clusterVersions"], continue while page.get("nextToken").
3. For each returned entry assert now < entry["endOfExtendedSupportDate"], and entry["versionStatus"] == "STANDARD_SUPPORT" when now < entry["endOfStandardSupportDate"] else "EXTENDED_SUPPORT". Observe an entry (today, Kubernetes 1.30, extended support ended 2026-07-23) whose extended support has already lapsed is still listed, so the first assertion fails.
4. Confirm the flag is inert: compare describe_cluster_versions() with describe_cluster_versions(includeAll=True) — the two listings are identical, whereas on AWS only the includeAll call may contain an out-of-support version.
5. Confirm the catalogue contradicts itself: the same row the default listing returns is classified UNSUPPORTED by its own dates, and describe_cluster_versions(versionStatus="UNSUPPORTED") returns it too.
6. Controls that must keep working after the fix: the default listing still reports >= 2 distinct versions, exactly one with defaultVersion true, each kubernetesPatchVersion prefixed by its minor, and describe_cluster_versions(defaultOnly=True) returns exactly that default.
7. Re-replay eks.ekscc-cluster-versions-support-status-agrees-with-support-dates to confirm.
