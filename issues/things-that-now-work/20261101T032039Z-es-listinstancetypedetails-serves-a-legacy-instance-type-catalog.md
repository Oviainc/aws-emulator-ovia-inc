Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog esdef-list-instance-type-details-max-results-ignored opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Commit: fecfddbbbf177cdbadf2635d78f2917398d0bf54

# es ListInstanceTypeDetails serves a legacy instance-type catalog and omits AvailabilityZones on every entry

> _Note: this issue was identified against the code base at commit `fecfddbbbf177cdbadf2635d78f2917398d0bf54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
3 net-new root causes on the `es` service token (the OpenSearch/Elasticsearch control plane reached on endpoint prefix `es`). All three are the same class of defect reached through three different fixtures: the reference instance-type catalog that backs ListInstanceTypeDetails is the emulator's own legacy table rather than the catalog AWS publishes for the requested engine version, and the entries it serves omit a modeled member. Each cause below is a separate replay unit and must be re-replayed separately.

NOTE ON RELATED WORK: the open issue 20261029T190009Z-opensearch-the-package-offering-version-and-instance-type.md ("opensearch: the package, offering, version and instance-type catalogs are synthetic") describes the same synthetic-catalog defect on the `opensearch` service token and is keyed to the opensearch-catalog-* fixtures. It does NOT carry the replay keys of the three steps below, so those three steps are not covered by it and are filed here.

Cause 1 of 3 - ListInstanceTypeDetails inside the engine-version/instance-catalog walk (fixture es-engine-version-and-instance-catalog, step task#3).
AWS: recorded behavior only - the Smithy model constrains the shape, not the catalog contents (Smithy: ListInstanceTypeDetails GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, output InstanceTypeDetails: InstanceTypeDetailsList; shape InstanceTypeDetails members InstanceType, EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole, AvailabilityZones). The recording shows AWS answering 200 with the current-generation search instance catalog for that engine version - the list is led by m7i.2xlarge.search and contains current families (c8g.*, r7i.*, oi2.*, ...) - and every entry carries the AvailabilityZones member (null when AZs were not requested via retrieveAZs).
Emulator: answers 200 but with a different, legacy catalog - the list is led by c5.large.search and is populated from the c5.* family (c5.large.search, c5.2xlarge.search, c5.4xlarge.search, c5.9xlarge.search, ...) - and every entry omits AvailabilityZones entirely. The divergence is reported on ~160 of the returned entries, i.e. essentially the whole list, not a tail difference.
Why: this list is what a client chooses an instance type from. A client that reads the emulator's answer picks a type AWS does not offer for that engine version (so a later CreateDomain/UpdateDomainConfig with it would be refused on real AWS), and never sees the types AWS does offer. Omitting AvailabilityZones means AZ-aware capacity planning has nothing to read, and a client that dereferences the modeled member sees it absent rather than null.
Fix: the static instance-type reference table behind this reader and its projection into InstanceTypeDetails (the catalog data plus the serializer that must emit AvailabilityZones).
Traces: es-engine-version-and-instance-catalog#task#3.

Cause 2 of 3 - ListInstanceTypeDetails with MaxResults narrowing the page (fixture esdef-list-instance-type-details-max-results-ignored, step task#1).
AWS: recorded behavior only; Smithy: ListInstanceTypeDetails.MaxResults @range(max 100), paginated(pageSize=MaxResults). The recording shows the page honoring the requested size and its first entry being AWS's first catalog entry for that engine version, m7i.2xlarge.search, with AvailabilityZones present.
Emulator: returns a page of the expected length - so the page size itself is honored - but its first (and only reported) entry is c5.large.search with no AvailabilityZones member. So the paging is right and the underlying catalog is wrong: the divergence is confined to $.InstanceTypeDetails[0]'s value.
Why: a client that takes the first page as a representative sample of available types (the common "what can I run?" probe) gets a type AWS does not list, from a single-entry response where there is no other entry to fall back on.
Fix: same component as cause 1 - the static instance-type catalog, not the pagination code, which behaves correctly here.
Traces: esdef-list-instance-type-details-max-results-ignored#task#1.

Cause 3 of 3 - ListInstanceTypeDetails reached from the domain version/capacity + software-update workflow (fixture opensearch-domain-version-capacity-and-software-update-management, step task#5).
AWS: recorded behavior only, as in cause 1: 200 with the engine version's current catalog, led by m7i.2xlarge.search, entries including c8g.12xlarge.search and r7i.12xlarge.search and each carrying AvailabilityZones.
Emulator: 200 with the same legacy c5.* table (c5.large.search, c5.2xlarge.search, c5.4xlarge.search, c5.9xlarge.search, ...) and no AvailabilityZones member; ~145 entries diverge.
Why: this step is the capacity-selection step of a resize/upgrade flow. Choosing a target instance type out of the emulator's catalog yields a resize AWS would refuse, so the whole scale-up path a client builds against the emulator is unusable against AWS.
Fix: same component as cause 1; recorded here as a third replay unit because it is reached through the domain-management fixture rather than the catalog fixtures.
Traces: opensearch-domain-version-capacity-and-software-update-management#task#5.

## Evidence
Cause 1 - es-engine-version-and-instance-catalog#task#3, expected 200 / actual 200: "$.InstanceTypeDetails[0]: missing (expected {'AdvancedSecurityEnabled': True, 'AppLogsEnabled': True, 'AvailabilityZones': None, 'CognitoEnabled': True, 'EncryptionEnabled': True, 'InstanceRole': ['data', 'master', 'coordinator'], 'InstanceType': 'm7i.2xlarge.search', 'WarmEnabled': True}, got {'InstanceType': 'c5.large.search', 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data', 'master', 'coordinator']})"; likewise "$.InstanceTypeDetails[2]: ... expected 'c8g.12xlarge.search', got 'c5.2xlarge.search'", "$.InstanceTypeDetails[3]: ... expected 'oi2.8xlarge.search', got 'c5.4xlarge.search'", "$.InstanceTypeDetails[4]: ... expected 'r7i.12xlarge.search', got 'c5.9xlarge.search'". The cluster's divergence key lists ~160 InstanceTypeDetails indices. Note the expected objects all carry the AvailabilityZones key and none of the actual objects do.
Cause 2 - esdef-list-instance-type-details-max-results-ignored#task#1, expected 200 / actual 200: "$.InstanceTypeDetails[0]: value_mismatch (expected {... 'AvailabilityZones': None, 'InstanceType': 'm7i.2xlarge.search' ...}, got {'InstanceType': 'c5.large.search', ... no AvailabilityZones})". Only index 0 is reported, so the page length matched the recording.
Cause 3 - opensearch-domain-version-capacity-and-software-update-management#task#5, expected 200 / actual 200: "$.InstanceTypeDetails[0]: missing (expected 'm7i.2xlarge.search' with AvailabilityZones, got 'c5.large.search' without)", "$.InstanceTypeDetails[2]: expected 'c8g.12xlarge.search', got 'c5.2xlarge.search'", "$.InstanceTypeDetails[3]: expected 'r7i.12xlarge.search', got 'c5.4xlarge.search'", "$.InstanceTypeDetails[4]: expected 'c8g.8xlarge.search', got 'c5.9xlarge.search'"; ~145 indices in the divergence key.
The specific type names, family mixes and index counts above are this recording's values; the requirement is that the catalog served for an engine version is AWS's catalog for that version and that each entry carries the modeled AvailabilityZones member.

## Steps to Reproduce
1. Replay fixture es-engine-version-and-instance-catalog step task#3 (GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}) and diff the InstanceTypeDetails list against the recording: the served families and the ordering differ and no entry has an AvailabilityZones member.
2. Replay fixture esdef-list-instance-type-details-max-results-ignored step task#1 and compare $.InstanceTypeDetails[0] - the page size matches but the entry is from the legacy catalog.
3. Replay fixture opensearch-domain-version-capacity-and-software-update-management step task#5 and diff InstanceTypeDetails the same way.
4. All three steps must pass before this issue is closed.

## Resolution

Fixed: the catalog DATA behind the reader. The listing for a release is the catalog the
service publishes for it, so it carries every current-generation family the service
accepts an instance type of — not only the families the checked-in
`OpenSearchPartitionInstanceType` snapshot enumerates. `INSTANCE_TYPE_CATALOG` in
`services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs` now
also publishes the families the service's own accepted-value list reports beyond that
enum: the seventh-generation Intel compute family (`c7i` beside `m7i`/`r7i`), the
Graviton4 families (`m8g`, `r8g`, `r8gd`, `i8ge` beside `c8g`), the Graviton
dense-storage families (`im4gn`, `r7gd`) and the second OpenSearch-optimized generation
(`om2`, `or2` beside `oi2`) — 73 instance types the listing previously omitted, each
with the node roles an instance of its family serves, the release its family is offered
from, its instance-storage kind and its size ladder. Every reader of an instance type
(the listing, the limit catalog, the cluster-configuration validation) reads that one
table, so none of them can disagree about which types exist. Held by
`tests/all/current_generation_instance_types.rs`; `tests/all/instance_type_catalog.rs`
gained the two size tokens the dense-storage ladder adds and the two
OpenSearch-optimized families, so its rules stay stated over the whole catalog.

Declined, as in 20261030T001859Z: the ORDER the catalog is published in, which is what
the replay key still reports. `com.amazonaws.opensearch#InstanceTypeDetailsList` is a
bare list with no ordering trait and no documented order, and the recorded orders differ
between recordings of the same operation — one leads with `m7i.2xlarge.search` and
places `c8g.12xlarge.search`, `oi2.8xlarge.search`, `r7i.12xlarge.search` at indices
2, 3, 4, another has `r7i.12xlarge.search` at 3 and `c8g.8xlarge.search` at 4, and a
third leads with `r5.large.search`. It is neither the modelled value set's order nor a
sort of family, generation, size or name, so it is the per-run iteration order of the
service's own collection rather than published content; writing those positions in would
be a memorised prefix, not a rule. The catalog is published in the generation grouping
AWS's supported-instance-types documentation lists (`state::catalog_rank`).

Declined: `AvailabilityZones` on an entry of a request that did not ask for zones. The
member is modelled optional, `restJson1` does not distinguish an absent member from an
explicit `null`, and the schema serializer omits a `None` member, so there is no value a
client reads differently. The zones are reported exactly when `retrieveAZs` asks for
them, which is also what the service's own refusal of an AZ read without that flag says.
