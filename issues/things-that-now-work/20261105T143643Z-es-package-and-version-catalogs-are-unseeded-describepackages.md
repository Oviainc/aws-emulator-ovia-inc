Issue-Source: trace-sweep-divergence
Issue-Trace: es-catalog-get-package-version-history
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-catalog-get-package-version-history es-catalog-describe-packages es-catalog-get-compatible-elasticsearch-versions
Issue-Shape-Id: com.amazonaws.elasticsearchservice#AmazonElasticsearchService2015
Issue-Key: es|GetPackageVersionHistory|2xx->4xx|$.PackageID
Issue-Key: es|GetPackageVersionHistory|2xx->4xx|$.PackageVersionHistoryList
Issue-Key: es|GetPackageVersionHistory|2xx->4xx|status
Issue-Key: es|DescribePackages|2xx->2xx|$.PackageDetailsList[*]
Issue-Key: es|GetCompatibleElasticsearchVersions|2xx->2xx|$.CompatibleElasticsearchVersions[*]
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# es package and version catalogs are unseeded: DescribePackages returns nothing, package-version history 409s, and the compatible-version matrix is wrong

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPackageVersionHistory

## Description
3 net-new root causes on the `es` service token (Amazon Elasticsearch Service / OpenSearch Service control plane, endpoint prefix `es`). They share one component: the service's read-only reference data — the account's visible plugin/package catalog and the engine upgrade-compatibility matrix — which the emulator either does not populate at all or populates with a different data set than the service serves. Each cause below is its own replay unit and must be re-replayed separately before this issue is closed.

No existing open issue carries these replay keys: the es issues already open (20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md, 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md, 20261029T190009Z-es-ten-requests-real-aws-refuses-are-accepted.md, 20261102T091103Z-es-legacy-2015-01-01-elasticsearch-domain-reads.md) are about refusal status, refusal wording and domain reads, not about the catalog content these steps read.

Cause 1 of 3 — GetPackageVersionHistory on a package the service's catalog exposes answers a not-found refusal instead of that package's version history (fixture es-catalog-get-package-version-history, step task#1; the cluster covers 91 steps across that fixture, one per catalogued package).
AWS: Smithy (com.amazonaws.elasticsearchservice): GetPackageVersionHistory GET /2015-01-01/packages/{PackageID}/history, output PackageID plus PackageVersionHistoryList (list of PackageVersionHistory{PackageVersion, CommitMessage @length(0,1024-bound 160), CreatedAt timestamp}) and NextToken; errors ResourceNotFoundException, ValidationException, AccessDeniedException, InternalException. Recorded behavior only for the catalog's CONTENT: every package the service lists through DescribePackages is readable here, and the recording answers HTTP 200 with PackageID echoed and one PackageVersionHistoryList entry per published version, newest first, each carrying its PackageVersion label, its CreatedAt epoch-seconds-with-millis timestamp and a CommitMessage (null for the AWS-published plugins). The requirement is that a package present in the catalog is resolvable by its id and reports its version history, not the specific ids/timestamps of one recording.
Emulator: answers HTTP 409 with x-amzn-errortype ResourceNotFoundException for every one of those 91 package ids, and the response carries neither PackageID nor PackageVersionHistoryList. Replay detail: `status: expected 200, got 409`, `$.PackageID: missing`, `$.PackageVersionHistoryList: missing`. (The 409-vs-404 status mapping is a separate known fault; it is irrelevant here because the expected response is a successful 200, not a refusal.)
Why: a client enumerating the account's available plugins and then reading each one's versions — the normal way to pick a version to associate with a domain — gets a hard not-found for every package the same emulator would be expected to list, so plugin-association workflows cannot proceed at all.
Fix: the package catalog in this service's state — the seed/fixture data that backs the packages resource — must register the service-visible packages under their ids so the history read resolves; and the GetPackageVersionHistory handler must project PackageID and the per-version history list from it.
Traces: es-catalog-get-package-version-history#task#1 (the cluster covers that fixture's task#2..task#91 as well).

Cause 2 of 3 — DescribePackages returns an empty PackageDetailsList where the service lists its visible package catalog (fixture es-catalog-describe-packages, step task#1).
AWS: Smithy: DescribePackages POST /2015-01-01/packages/describe, output PackageDetailsList (list of PackageDetails{PackageID, PackageName @length(3,28) @pattern("^[a-z][a-z0-9\\-]+$"), PackageType enum, PackageDescription @length(0,1024), PackageStatus enum, CreatedAt, LastUpdatedAt, AvailablePackageVersion, ErrorDetails}) and NextToken; paginated inputToken/outputToken NextToken, pageSize MaxResults @range(max 100). Recorded behavior only for the content: the recording answers 200 with a populated PackageDetailsList — ~92 entries — each a fully projected PackageDetails: a pkg-<hex> PackageID, a PackageName, PackageType ZIP-PLUGIN, PackageStatus AVAILABLE, an AvailablePackageVersion label, CreatedAt and LastUpdatedAt timestamps, a PackageDescription and ErrorDetails null. The requirement is a non-empty catalog whose entries carry every modelled member, not the exact package set of one recording.
Emulator: answers 200 but with no list members at all — the replay reports every expected index of $.PackageDetailsList as `missing (expected {...}, got None)`, i.e. the response's PackageDetailsList is empty/absent.
Why: a client discovers installable plugins only through this call. An empty list makes the service look like an account with no packages, so AssociatePackage/version-selection flows have nothing to choose from and any ‘list then use’ automation silently does nothing instead of failing loudly.
Fix: the same package catalog state as cause 1, plus the DescribePackages handler's projection of PackageDetails members (and its filter/pagination over that catalog). Listed separately because the list read and the per-id history read are different handlers: seeding alone without the projection, or the projection alone without the seed, leaves one of the two steps failing.
Traces: es-catalog-describe-packages#task#1.

Cause 3 of 3 — GetCompatibleElasticsearchVersions reports a legacy Elasticsearch-only upgrade matrix instead of the matrix the service serves (fixture es-catalog-get-compatible-elasticsearch-versions, step task#1).
AWS: Smithy: GetCompatibleElasticsearchVersions GET /2015-01-01/es/compatibleVersions, input DomainName @httpQuery("domainName") @length(3,28) @pattern("^[a-z][a-z0-9\\-]+$"), output CompatibleElasticsearchVersions: list of CompatibleVersionsMap{SourceVersion -> ElasticsearchVersionString @pattern("^[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}$"), TargetVersions list}. Recorded behavior only for the content: the account-wide recording (no domainName) answers 200 with a matrix whose source versions are the currently upgradable engines — the 6.8/7.x Elasticsearch line and the OpenSearch_1.x/2.x/3.x line — and whose TargetVersions for each source are every newer supported engine, including the OpenSearch_* targets reachable from an Elasticsearch source (e.g. source 7.7 -> 7.8, 7.9, 7.10, OpenSearch_1.0..1.3). The requirement is that the matrix covers the engine versions the service actually offers and that an Elasticsearch source lists its OpenSearch upgrade targets.
Emulator: answers 200 with a different, hard-coded matrix: sources are retired Elasticsearch/legacy versions (1.5, 2.3, 5.1, 5.3, 5.5, 5.6, 6.0 ...) and the TargetVersions are only the next legacy versions, with no OpenSearch_* source or target anywhere. Every index of $.CompatibleElasticsearchVersions diverges in the replay.
Why: this call is how a client decides whether and to what version a domain can be upgraded. With this matrix, an UpgradeDomain planner concludes that no OpenSearch target exists (so a current domain appears un-upgradable) and offers legacy targets the service no longer supports, inverting the upgrade decision.
Fix: the compatible-version reference data this operation reads, and the handler that filters it by the queried domain's current engine version.
Traces: es-catalog-get-compatible-elasticsearch-versions#task#1.

The package ids, package names, descriptions, version labels and timestamps quoted from the recordings are that recording's catalog instance; the requirement is a populated catalog with every modelled member projected, resolvable by id, and an upgrade matrix covering the engines the service offers.

## Evidence
Cause 1 — es-catalog-get-package-version-history#task#1, GetPackageVersionHistory, expected 200 / actual 409, 91 affected steps (divergence_key service=es|GetPackageVersionHistory|status=200->409|keys=$.PackageID,$.PackageVersionHistoryList,status):
  replay detail: "status: expected 200, got 409"; "$.PackageID: missing (expected 'pkg-10b3c5c4ab016794431c100a4963d8d1e07e3673', got None)"; "$.PackageVersionHistoryList: missing (expected [{'CommitMessage': None, 'CreatedAt': 1753272650.518, 'PackageVersion': 'v2'}, {'CommitMessage': None, 'CreatedAt': 1745506273.179, 'PackageVersion': 'v1'}], got None)"
  The cluster's affected list spans es-catalog-get-package-version-history#task#1 .. #task#91 — one step per catalogued package id, all answering 409.
  Smithy cited: GetPackageVersionHistory GET /2015-01-01/packages/{PackageID}/history, output PackageID + PackageVersionHistoryList(PackageVersionHistory{PackageVersion, CommitMessage, CreatedAt}).

Cause 2 — es-catalog-describe-packages#task#1, DescribePackages, expected 200 / actual 200 with an empty list (divergence_key service=es|DescribePackages|status=200->200|keys=$.PackageDetailsList[0] .. [91]):
  replay detail, e.g.: "$.PackageDetailsList[0]: missing (expected {'AvailablePackageVersion': 'v2', 'CreatedAt': 1745506273.179, 'ErrorDetails': None, 'LastUpdatedAt': 1753272650.518, 'PackageDescription': '...', 'PackageID': 'pkg-10b3c5c4ab016794431c100a4963d8d1e07e3673', 'PackageName': 'babel-street-match', 'PackageStatus': 'AVAILABLE', 'PackageType': 'ZIP-PLUGIN'}, got None)" — and the same `missing ... got None` for indices 1 through 91.
  Smithy cited: com.amazonaws.elasticsearchservice#PackageDetails members PackageID, PackageName, PackageType, PackageDescription, PackageStatus, CreatedAt, LastUpdatedAt, AvailablePackageVersion, ErrorDetails; DescribePackages paginated on NextToken/MaxResults @range(max 100).

Cause 3 — es-catalog-get-compatible-elasticsearch-versions#task#1, GetCompatibleElasticsearchVersions, expected 200 / actual 200 with the wrong matrix (divergence_key service=es|GetCompatibleElasticsearchVersions|status=200->200|keys=$.CompatibleElasticsearchVersions[0..31]):
  expected [0] {'SourceVersion': '7.7', 'TargetVersions': ['7.8', '7.9', '7.10', 'OpenSearch_1.0', 'OpenSearch_1.1', 'OpenSearch_1.2', 'OpenSearch_1.3']}; got {'SourceVersion': '1.5', 'TargetVersions': ['2.3']}
  expected [1] {'SourceVersion': '6.8', 'TargetVersions': ['7.1', '7.4', '7.7', '7.8', '7.9', '7.10', 'OpenSearch_1.0'..'OpenSearch_1.3']}; got {'SourceVersion': '2.3', 'TargetVersions': ['5.1', '5.3', '5.5', '5.6']}
  expected [4] {'SourceVersion': 'OpenSearch_2.3', 'TargetVersions': ['OpenSearch_2.5' .. 'OpenSearch_2.19']}; got {'SourceVersion': '5.5', 'TargetVersions': ['5.6']}
  No OpenSearch_* source or target appears anywhere in the emulator's matrix.
  Smithy cited: CompatibleVersionsMap{SourceVersion -> ElasticsearchVersionString @pattern("^[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}$"), TargetVersions}. The specific version numbers are the recording's engine inventory; the requirement is coverage of the offered engines and the Elasticsearch->OpenSearch targets.

## Steps to Reproduce
1. Replay fixture es-catalog-describe-packages step task#1 (POST /2015-01-01/packages/describe) and check PackageDetailsList is non-empty and every entry projects PackageID, PackageName, PackageType, PackageStatus, PackageDescription, AvailablePackageVersion, CreatedAt, LastUpdatedAt and ErrorDetails.
2. Replay fixture es-catalog-get-package-version-history step task#1 (GET /2015-01-01/packages/<a catalogued pkg- id>/history) and observe HTTP 409 ResourceNotFoundException where the recording is 200 with PackageID echoed and a newest-first PackageVersionHistoryList; repeat for any of task#2..task#91 in that fixture.
3. Replay fixture es-catalog-get-compatible-elasticsearch-versions step task#1 (GET /2015-01-01/es/compatibleVersions) and diff CompatibleElasticsearchVersions: the emulator returns a legacy 1.5/2.3/5.x matrix with no OpenSearch_* source or target.
4. All three steps must pass before this issue is closed.
