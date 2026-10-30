Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog opensearch-describe-reserved-instance-offerings-paginated opensearch-list-and-paginate-reserved-instance-offerings opensearch-paginate-reserved-instance-offerings
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|GetCompatibleVersions|2xx->2xx|$.CompatibleVersions[*]
Issue-Key: es|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Commit: 96efc7e677bf514eee6c6e14a4fa1b11294b220d

# es: the engine-version compatibility matrix and reserved-instance offering catalog diverge from AWS

> _Note: this issue was identified against the code base at commit `96efc7e677bf514eee6c6e14a4fa1b11294b220d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCompatibleVersions

## Description
2 net-new root causes on the `es` service token, sharing one fault: the static reference catalogs this service reads from are emulator-synthesized rather than AWS's published catalog, so a 200 with the right shape carries the wrong rows. Both operations are catalog readers that take no stateful input, so the content is the whole answer.

Cause 1 of 2 — GetCompatibleVersions: the upgrade matrix is missing AWS's Elasticsearch-line rows.
AWS: GET /2021-01-01/opensearch/compatibleVersions with no domainName returns 200 with CompatibleVersions, one CompatibleVersionsMap per upgradable source engine version carrying SourceVersion and the TargetVersions that source may actually upgrade to. The recording's matrix contains the Elasticsearch minor/major upgrade rows: Elasticsearch_6.0 -> [Elasticsearch_6.3, 6.4, 6.5, 6.7, 6.8]; Elasticsearch_5.1 -> [Elasticsearch_5.6]; Elasticsearch_5.3 -> [Elasticsearch_5.6]; Elasticsearch_5.6 -> [Elasticsearch_6.3, 6.4, 6.5, 6.7, 6.8]. Smithy: GetCompatibleVersions output member CompatibleVersions -> CompatibleVersionsList (http GET /2021-01-01/opensearch/compatibleVersions, code 200); the model constrains the shape only, so the row set is recorded behavior only.
Emulator: 200 with a CompatibleVersions list whose rows at those four positions are OpenSearch-line rows instead — OpenSearch_2.3 -> [OpenSearch_2.5 ... 2.19], OpenSearch_2.5 -> [OpenSearch_2.7 ... 2.19], OpenSearch_2.11 -> [OpenSearch_2.13 ... 2.19], OpenSearch_3.3 -> [OpenSearch_3.5, OpenSearch_3.7]. The Elasticsearch upgrade paths AWS publishes are absent from the matrix and extra OpenSearch rows stand in their place.
Why: this matrix is what a client consults before calling UpgradeDomain. A client holding an Elasticsearch 5.1/5.3/5.6/6.0 domain is told by the emulator that no upgrade target exists for it, so a legitimate upgrade is never attempted; conversely the fabricated OpenSearch rows advertise upgrade pairs the client cannot rely on.
Fix: the static engine-version upgrade-compatibility table behind the GetCompatibleVersions handler on the es service.
Traces: es-engine-version-and-instance-catalog#task#2 (expected vs actual at $.CompatibleVersions[21], [22], [25], [31]).

Cause 2 of 2 — DescribeReservedInstanceOfferings: the offering catalog's recurring charge is not AWS's pricing.
AWS: GET /2021-01-01/opensearch/reservedInstanceOfferings returns 200 with ReservedInstanceOfferings, each offering carrying ReservedInstanceOfferingId, InstanceType, Duration, FixedPrice, UsagePrice, CurrencyCode, PaymentOption and RecurringCharges (RecurringChargeAmount + RecurringChargeFrequency). In the recording the first offering of the first page is a NO_UPFRONT, USD offering whose RecurringCharges is a single Hourly charge of 2.61. Smithy: DescribeReservedInstanceOfferings output member ReservedInstanceOfferings -> ReservedInstanceOfferingList, paginated (NextToken/MaxResults range max 100); the model does not constrain prices, so the catalog contents are recorded behavior only.
Emulator: 200, same shape and the same CurrencyCode USD / PaymentOption NO_UPFRONT, but the leading offering's RecurringCharges is a single Hourly charge of 0.42432 — a different offering row (served as c4.2xlarge.search, Duration 31536000, FixedPrice 0.0, UsagePrice 0.0) occupies position 0, so the asserted recurring charge does not match. The offering id itself is a per-run generated UUID and is not asserted; the divergence is the catalog row and its pricing, not the id.
Why: the offering catalog is the price list a client purchases from. A client that reads an hourly recurring charge out of the emulator's catalog, or picks the leading offering as its cheapest NO_UPFRONT option, is costing and selecting against instance types and prices AWS does not offer, and AWS's actual offerings never appear. It also affects both the paginated and single-page listings, so no page of the catalog is trustworthy.
Fix: the static reserved-instance offering table (instance types, durations, fixed/usage prices and recurring charges, and the order they are served in) behind the DescribeReservedInstanceOfferings reader on the es service.
Traces: opensearch-describe-reserved-instance-offerings-paginated#task#2 (representative), plus opensearch-list-and-paginate-reserved-instance-offerings#task#1, opensearch-list-and-paginate-reserved-instance-offerings#task#2 and opensearch-paginate-reserved-instance-offerings#task#2 in the same cluster.

Related but NOT covering these: 20261029T190009Z-opensearch-the-package-offering-version-and-instance-type.md files the same family of synthetic catalogs under the `opensearch` service token (its causes 2 and 3 are the offering table and the upgrade matrix there). These two causes replay on the `es` token and are reported as net-new by the corpus, so they need their own fix verification on this service's readers.

## Evidence
Cause 1 (GetCompatibleVersions, expected 200 / actual 200, keys $.CompatibleVersions[21],[22],[25],[31]):
  "$.CompatibleVersions[21]: missing (expected {'SourceVersion': 'Elasticsearch_6.0', 'TargetVersions': ['Elasticsearch_6.3', 'Elasticsearch_6.4', 'Elasticsearch_6.5', 'Elasticsearch_6.7', 'Elasticsearch_6.8']}, got {'SourceVersion': 'OpenSearch_2.3', 'TargetVersions': ['OpenSearch_2.5', ... 'OpenSearch_2.19']})"
  "$.CompatibleVersions[22]: missing (expected {'SourceVersion': 'Elasticsearch_5.1', 'TargetVersions': ['Elasticsearch_5.6']}, got {'SourceVersion': 'OpenSearch_2.5', ...})"
  "$.CompatibleVersions[25]: missing (expected {'SourceVersion': 'Elasticsearch_5.3', 'TargetVersions': ['Elasticsearch_5.6']}, got {'SourceVersion': 'OpenSearch_2.11', ...})"
  "$.CompatibleVersions[31]: missing (expected {'SourceVersion': 'Elasticsearch_5.6', 'TargetVersions': ['Elasticsearch_6.3', 'Elasticsearch_6.4', 'Elasticsearch_6.5', 'Elasticsearch_6.7', 'Elasticsearch_6.8']}, got {'SourceVersion': 'OpenSearch_3.3', 'TargetVersions': ['OpenSearch_3.5', 'OpenSearch_3.7']})"
  divergence_key: service=es|GetCompatibleVersions|status=200->200|keys=$.CompatibleVersions[21],$.CompatibleVersions[22],$.CompatibleVersions[25],$.CompatibleVersions[31]

Cause 2 (DescribeReservedInstanceOfferings, expected 200 / actual 200, key $.ReservedInstanceOfferings[0], affected_count 4):
  "$.ReservedInstanceOfferings[0]: missing (expected {'CurrencyCode': 'USD', 'PaymentOption': 'NO_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 2.61, 'RecurringChargeFrequency': 'Hourly'}], 'Duration': '<shaped-not-asserted>', 'FixedPrice': '<shaped-not-asserted>', 'InstanceType': '<shaped-not-asserted>', 'ReservedInstanceOfferingId': '<shaped-not-asserted>', 'UsagePrice': '<shaped-not-asserted>'}, got {'ReservedInstanceOfferingId': <generated uuid>, 'InstanceType': 'c4.2xlarge.search', 'Duration': 31536000, 'FixedPrice': 0.0, 'UsagePrice': 0.0, 'CurrencyCode': 'USD', 'PaymentOption': 'NO_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.42432, 'RecurringChargeFrequency': 'Hourly'}]})"
  divergence_key: service=es|DescribeReservedInstanceOfferings|status=200->200|keys=$.ReservedInstanceOfferings[0]
  Only the recorded-and-asserted members diverge: RecurringCharges[0].RecurringChargeAmount (2.61 vs 0.42432). CurrencyCode and PaymentOption match; the offering id is a per-run value and is not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay fixture es-engine-version-and-instance-catalog step task#2 (GetCompatibleVersions, no domainName) and diff $.CompatibleVersions against the recording — indices 21, 22, 25 and 31 carry OpenSearch rows where AWS carries the Elasticsearch_6.0 / 5.1 / 5.3 / 5.6 upgrade rows.
3. Cause 2: replay fixture opensearch-describe-reserved-instance-offerings-paginated step task#2 (DescribeReservedInstanceOfferings) and diff $.ReservedInstanceOfferings[0].RecurringCharges — expected a single Hourly charge of 2.61 for the leading NO_UPFRONT/USD offering, got 0.42432. The same cluster reproduces on opensearch-list-and-paginate-reserved-instance-offerings#task#1, opensearch-list-and-paginate-reserved-instance-offerings#task#2 and opensearch-paginate-reserved-instance-offerings#task#2.
4. Both steps return HTTP 200 with the modeled shape, so the divergence is only visible by comparing the catalog rows, not the status.
