Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: es.associate-package-rejected-on-processing-domain-leaves-association-unset es.domain-tag-lifecycle-add-list-remove es.update-package-creates-new-version-visible-in-history
Issue-Shape-Id: com.amazonaws.elasticsearchservice#AmazonElasticsearchService2015
Issue-Key: es.associate-package-rejected-on-processing-domain-leaves-association-unset|run#16|AssertionError|assert code == "ValidationException", (
Issue-Key: es|ListTags|NotImplemented
Issue-Key: es|GetPackageVersionHistory|NotImplemented
Issue-Commit: db763654427c553d8b06d764576848aa87eee804

# es: 3 net-new root causes — AssociatePackage, the domain tag surface (AddTags/ListTags/RemoveTags) and the package-version surface (UpdatePackage/GetPackageVersionHistory) all answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `db763654427c553d8b06d764576848aa87eee804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociatePackage

## Description
Three distinct, independently-fixable root causes in the Amazon Elasticsearch Service (es) emulator. The service IS bootstrapped and answers domain-lifecycle and package-creation calls, but three whole sub-surfaces are still scaffold stubs that answer HTTP 501 with error code `NotImplemented` and the message `operation <Name> is scaffolded but not implemented in this emulator`. That code/status pair is not in any of these operations' modeled error sets, so no client can interpret it. Each cause below must be closed before this issue is submitted.

=== Cause 1 of 3: AssociatePackage never enforces (or performs) the association, so its documented refusal arm is unreachable ===
AWS: Smithy `AssociatePackage` — POST /2015-01-01/packages/associate/{PackageID}/{DomainName}, input `PackageID` (required, httpLabel) and `DomainName` (required, httpLabel, @length 3..28, @pattern ^[a-z][a-z0-9\-]+$); output `DomainPackageDetails`; modeled errors AccessDeniedException (403), BaseException (400), ConflictException (409), InternalException (500), ResourceNotFoundException (409), ValidationException (400). Recorded behavior: a domain that is still Processing after CreateElasticsearchDomain (i.e. not yet on the latest service software) refuses AssociatePackage with HTTP 400, Error.Code `ValidationException` and Error.Message `The domain is not on the latest software version. Upgrade the domain to start using this feature. ` — that whole sentence pair is FIXED text (note the trailing space AWS emits; no request or state value is interpolated into it). The refused call must be a no-op on both sides: ListPackagesForDomain(DomainName) and ListDomainsForPackage(PackageID) must both still report an empty DomainPackageDetailsList.
Emulator: the handler is a scaffold stub and answers 501 `NotImplemented`: `operation AssociatePackage is scaffolded but not implemented in this emulator`. Neither the software-version gate nor the association itself exists.
Why: a client that associates a dictionary/synonym package with a domain gets an unmodeled 501 instead of either the DomainPackageDetails success or the documented ValidationException, so it can neither associate a package nor distinguish "domain not upgraded yet" from a server fault; retry logic keyed on 5xx will loop.
Fix: the AssociatePackage handler, its validation (the latest-service-software gate, which must run before any state mutation) and the shared package/domain association state it writes, which ListPackagesForDomain and ListDomainsForPackage read.
Traces: es.associate-package-rejected-on-processing-domain-leaves-association-unset.

=== Cause 2 of 3: the whole domain tagging surface is unimplemented — AddTags, ListTags and RemoveTags all answer 501 ===
AWS: Smithy `ListTags` — GET /2015-01-01/tags with required httpQuery `arn`, output `TagList`; errors BaseException (400), InternalException (500), ResourceNotFoundException (409), ValidationException (400). `AddTags` — POST /2015-01-01/tags, required `ARN` + `TagList`, empty output; errors include LimitExceededException (409). `RemoveTags` — POST /2015-01-01/tags-removal, required `ARN` + `TagKeys`, empty output. Recorded behavior: a freshly created domain's ListTags returns `TagList: []`; AddTags is additive (two keys added in one call are both visible on the next ListTags with their submitted values); RemoveTags deletes only the named keys and leaves every other key/value intact. All three answer HTTP 200.
Emulator: all three handlers are scaffold stubs. The check's first tagging call fails with 501 `NotImplemented`: `operation ListTags is scaffolded but not implemented in this emulator`; AddTags and RemoveTags carry the same stub marker, so the whole tag lifecycle is dead, not just the read.
Why: tagging is how callers attribute cost and drive tag-based access control and resource discovery; a client that tags a domain and then reads its tags back gets a 5xx on every leg and cannot model domain tags at all.
Fix: the AddTags / ListTags / RemoveTags handlers plus the per-domain tag store they share (keyed by domain ARN, so ListTags resolves the ARN that CreateElasticsearchDomain returned in DomainStatus.ARN) and the ARN-resolution validation that produces ResourceNotFoundException / ValidationException for an unknown or malformed ARN.
Traces: es.domain-tag-lifecycle-add-list-remove.

=== Cause 3 of 3: the package-version surface is unimplemented — UpdatePackage and GetPackageVersionHistory both answer 501, so a package has no version history ===
AWS: Smithy `GetPackageVersionHistory` — GET /2015-01-01/packages/{PackageID}/history, required httpLabel `PackageID`, optional httpQuery `maxResults` (@range max 100) and `nextToken`, paginated (inputToken/outputToken NextToken, pageSize MaxResults); output `PackageID`, `PackageVersionHistoryList`, `NextToken`; errors AccessDeniedException (403), BaseException (400), InternalException (500), ResourceNotFoundException (409), ValidationException (400). `UpdatePackage` — POST /2015-01-01/packages/update, required `PackageID` + `PackageSource`, optional `PackageDescription` (@length 0..1024) and `CommitMessage` (@length 0..160); output `PackageDetails`. Recorded behavior: a created package that has reached PackageStatus AVAILABLE has exactly one history entry, PackageVersion `v1`; UpdatePackage with a new S3 source moves the package back through COPYING to AVAILABLE and mints the next version (`v2`), and GetPackageVersionHistory then lists the versions NEWEST FIRST (`v2`, `v1`) with each entry's `CreatedAt` strictly increasing from oldest to newest. The version labels are a generated sequence (v1, v2, ...), not literal values from any one run.
Emulator: both handlers are scaffold stubs. The check fails on the first history read with 501 `NotImplemented`: `operation GetPackageVersionHistory is scaffolded but not implemented in this emulator`, and UpdatePackage would answer the same, so no second version can ever exist.
Why: a client that publishes a dictionary and then revises it cannot update the package or enumerate its versions, and cannot tell which version a domain is using; it sees a 5xx where AWS returns a paginated history.
Fix: the UpdatePackage handler (version minting + the COPYING→AVAILABLE transition on the package record) and the GetPackageVersionHistory handler/serializer (newest-first ordering, per-version CreatedAt, NextToken paging), over the package state DescribePackages already serves.
Traces: es.update-package-creates-new-version-visible-in-history.

Note: two further es failures in this run (DescribeElasticsearchDomains and GetCompatibleElasticsearchVersions) are already covered by their own auto-filed scaffold-stub issues and are deliberately NOT bundled here. The open es issues `20261030T090206Z-es-describedryrunprogress-unknown-dryrunid-refusal-diverges-http-409.md` and `20261030T090206Z-es-reference-catalogs-diverge-from-aws-upgrade-matrix.md` concern different operations and do not cover any cause above.

## Evidence
Cause 1 — es.associate-package-rejected-on-processing-domain-leaves-association-unset [mismatch]
  Expected (AWS): HTTP 400, Error.Code='ValidationException', Error.Message='The domain is not on the latest software version. Upgrade the domain to start using this feature. ' (fixed text, trailing space included; nothing interpolated), then ListPackagesForDomain -> DomainPackageDetailsList == [] and ListDomainsForPackage -> DomainPackageDetailsList == [].
  Actual (emulator): AssertionError: "AssociatePackage on a Processing domain should be refused ValidationException, got 'NotImplemented': 'operation AssociatePackage is scaffolded but not implemented in this emulator'" (HTTP 501).
  Stub marker: services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/src/ops/associate_package.rs:4 (`//! SCAFFOLDED, NOT IMPLEMENTED`), 501 returned at line 168.

Cause 2 — es.domain-tag-lifecycle-add-list-remove [error]
  Expected (AWS): ListTags(ARN=<domain ARN>) -> 200 {"TagList": []} on a new domain; after AddTags with two entries, ListTags reports both keys with their submitted values; after RemoveTags of one key, ListTags reports only the remaining key.
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTags operation: operation ListTags is scaffolded but not implemented in this emulator (HTTP 501).
  Stub markers: src/ops/list_tags.rs:4 (501 at line 160), src/ops/add_tags.rs:4, src/ops/remove_tags.rs:4 — all three in services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/.

Cause 3 — es.update-package-creates-new-version-visible-in-history [error]
  Expected (AWS): GetPackageVersionHistory on an AVAILABLE, never-updated package -> PackageVersionHistoryList with PackageVersion ['v1']; after UpdatePackage with a new PackageSource and a return to AVAILABLE -> ['v2','v1'] newest-first with history[0].CreatedAt > history[1].CreatedAt.
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetPackageVersionHistory operation: operation GetPackageVersionHistory is scaffolded but not implemented in this emulator (HTTP 501).
  Stub markers: services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/src/ops/get_package_version_history.rs:4 and .../src/ops/update_package.rs:4.

In every case `NotImplemented` / HTTP 501 is absent from the operation's modeled error set (AssociatePackage, ListTags and GetPackageVersionHistory each model only AccessDenied/Base/Conflict/Internal/ResourceNotFound/ValidationException as applicable), so the response is uninterpretable by an SDK client.

## Steps to Reproduce
Replay the behavioral suite against the es emulator; all three checks are in the failing list.

Cause 1 (es.associate-package-rejected-on-processing-domain-leaves-association-unset):
  1. s3.create_bucket + put_object to hold a TXT-DICTIONARY package body.
  2. es.create_package(PackageName=<28-char name>, PackageType='TXT-DICTIONARY', PackageSource={S3BucketName,S3Key}) -> note PackageID.
  3. es.create_elasticsearch_domain(DomainName=<=28 chars, ElasticsearchVersion='7.10', ElasticsearchClusterConfig={InstanceType:'t3.small.elasticsearch',InstanceCount:1}, EBSOptions={EBSEnabled:true,VolumeType:'gp2',VolumeSize:10}).
  4. Immediately (domain still Processing) call es.associate_package(PackageID=<id>, DomainName=<name>). Expect 400 ValidationException with the latest-software-version sentence; observe 501 NotImplemented.
  5. Then es.list_packages_for_domain(DomainName=<name>) and es.list_domains_for_package(PackageID=<id>) must both be empty.

Cause 2 (es.domain-tag-lifecycle-add-list-remove):
  1. es.create_elasticsearch_domain(...) as above; take DomainStatus.ARN.
  2. es.list_tags(ARN=<arn>) — expect 200 with TagList []; observe 501 NotImplemented.
  3. es.add_tags(ARN=<arn>, TagList=[{Key:k1,Value:v1},{Key:k2,Value:v2}]) then es.list_tags(ARN=<arn>) — expect both keys.
  4. es.remove_tags(ARN=<arn>, TagKeys=[k2]) then es.list_tags(ARN=<arn>) — expect only k1.

Cause 3 (es.update-package-creates-new-version-visible-in-history):
  1. Create a bucket with two keys (v1 body, v2 body) and es.create_package against the first.
  2. Poll es.describe_packages(Filters=[{Name:'PackageID',Value:[<id>]}]) until PackageStatus leaves COPYING and is AVAILABLE.
  3. es.get_package_version_history(PackageID=<id>) — expect PackageVersion ['v1']; observe 501 NotImplemented.
  4. es.update_package(PackageID=<id>, PackageSource={S3BucketName,S3Key:'v2.txt'}), poll back to AVAILABLE, then es.get_package_version_history(PackageID=<id>) — expect ['v2','v1'] newest-first with strictly increasing CreatedAt.
