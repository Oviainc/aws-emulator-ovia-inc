Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-mr-inv-describe-regions-rejects-unknown-region-name
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-mr-inv-describe-regions-rejects-unknown-region-name ec2-trajwire-lia-regions-name-mistakes-then-opt-in-route ec2-usg-traj-filter-name-and-page-size-validation
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[regionName="us-east-1"].geographySet
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[regionName="us-west-1"].geographySet
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[regionName="us-west-2"].geographySet
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[*].geographySet
Issue-Key: ec2|DescribeRegions|2xx->4xx|$.DescribeRegionsResponse
Issue-Key: ec2|DescribeRegions|2xx->4xx|status
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[*]
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 DescribeRegions serves the wrong region catalog and omits geographySet

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRegions

## Description
Four net-new causes in one operation's static catalog and paging surface.

Cause 1 of 4 — geographySet is missing from every Region item on the filtered-by-name read.
AWS: Smithy com.amazonaws.ec2#Region models Geography -> RegionGeographyList with xmlName 'geographySet' ('The geography information for the Region. The geography is returned as a list.'). Each region renders geographySet with one item whose name is the region's geography — e.g. 'United States of America' for the us-* regions, 'India' for ap-south-2.
Emulator: the member is absent from every item.
Why: a client grouping regions by geography (data-residency checks) gets nothing to group on.
Fix: the DescribeRegions projection and the region catalog's geography data.
Traces: ec2-mr-inv-describe-regions-rejects-unknown-region-name#task#1 (cluster also covers task#5 of the same fixture).

Cause 2 of 4 — the same omission on the unfiltered/paged read.
AWS: geographySet is rendered on every item of regionInfo, filtered or not.
Emulator: absent on items [0], [1] and [2].
Why: as above; the gap is in the catalog, not in one filter path.
Fix: same catalog/projection.
Traces: ec2-trajwire-lia-regions-name-mistakes-then-opt-in-route#task#4.

Cause 3 of 4 — DescribeRegions refuses a valid filtered/paged request with 400.
AWS: returns 200 with a regionInfo element (empty when the filter selects nothing).
Emulator: answers 400 with no body.
Why: a region-discovery step that AWS answers fails, breaking account-shape enumeration before it starts.
Fix: the DescribeRegions filter/MaxResults validation.
Traces: ec2-usg-traj-filter-name-and-page-size-validation#task#8.

Cause 4 of 4 — the paged region list is in the wrong order and the catalog's first page differs from AWS's.
AWS: the recorded first page starts with ap-south-2 (regionEndpoint ec2.ap-south-2.amazonaws.com, optInStatus 'not-opted-in', geographySet item name 'India').
Emulator: the first item is af-south-1 and carries no geographySet.
Why: a client that pages the region list and keys off position (or that stops after the first page) enumerates a different region set than AWS returns.
Fix: the region catalog's content/order plus the DescribeRegions pagination ordering.
Traces: ec2-usg-traj-filter-name-and-page-size-validation#task#9.

## Evidence
Cause 1: $.DescribeRegionsResponse.regionInfo.item[regionName="us-east-1"|"us-west-1"|"us-west-2"].geographySet missing (expected {'item': {'name': 'United States of America'}}, got None).
Cause 2: $.DescribeRegionsResponse.regionInfo.item[0..2].geographySet missing (same expectation).
Cause 3: status expected 200, got 400; $.DescribeRegionsResponse missing (expected requestId + regionInfo '').
Cause 4: $.DescribeRegionsResponse.regionInfo.item[0] value_mismatch — expected {'regionName': 'ap-south-2', 'regionEndpoint': 'ec2.ap-south-2.amazonaws.com', 'optInStatus': 'not-opted-in', 'geographySet': {'item': {'name': 'India'}}}, got {'optInStatus': 'not-opted-in', 'regionName': 'af-south-1', 'regionEndpoint': 'ec2.af-south-1.amazonaws.com'}.

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-mr-inv-describe-regions-rejects-unknown-region-name#task#1, ec2-trajwire-lia-regions-name-mistakes-then-opt-in-route#task#4, ec2-usg-traj-filter-name-and-page-size-validation#task#8 and #task#9.
