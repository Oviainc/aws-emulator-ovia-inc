Issue-Source: trace-sweep-divergence
Issue-Trace: route53-catalog-get-checker-ip-ranges
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-catalog-get-checker-ip-ranges route53-catalog-list-geo-locations
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|GetCheckerIpRanges|2xx->2xx|$.GetCheckerIpRangesResponse.CheckerIpRanges.member[*]
Issue-Key: route53|ListGeoLocations|2xx->2xx|$.ListGeoLocationsResponse.GeoLocationDetailsList.GeoLocationDetails[*]
Issue-Key: route53|ListGeoLocations|2xx->2xx|$.ListGeoLocationsResponse.IsTruncated
Issue-Key: route53|ListGeoLocations|2xx->2xx|$.ListGeoLocationsResponse.MaxItems
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 static reference catalogs diverge: checker IP ranges and geolocations

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCheckerIpRanges

## Description
Two Route 53 read-only reference catalogs are seeded with data that is not the catalog AWS serves. Both operations answer 200 with the right shape; the contents are wrong. 2 causes.

Cause 1 of 2 — GetCheckerIpRanges serves the retired health-checker address list.
AWS: Smithy: GetCheckerIpRanges takes no input, binds no errors, and its output requires CheckerIpRanges (a list of CIDR strings). AWS serves the current health-checker source ranges, which are the consolidated 15.177.0.0/18 sub-blocks (a run of /23 prefixes) plus a small number of single-address /32 entries, ordered ascending.
Emulator: serves the legacy per-region /26 blocks (13.x / 54.x / 107.x style entries) instead — every member of the list differs from the recorded response, element for element.
Why: this operation exists so that customers can allow-list Route 53's health checkers in firewalls and security groups. A client that programmes its allow-list from the emulator's answer allow-lists addresses Route 53 no longer probes from and omits the ones it does, so health checks would be blocked in reality.
Fix: the GetCheckerIpRanges handler's seeded catalog — replace the legacy regional /26 table with AWS's current checker range list.
Traces: route53-catalog-get-checker-ip-ranges#task#1.

Cause 2 of 2 — ListGeoLocations serves a different geolocation catalog.
AWS: Smithy: ListGeoLocations' output requires GeoLocationDetailsList, IsTruncated and MaxItems; GeoLocationDetails.CountryName is @length 1..64. AWS's catalog uses Route 53's own country naming — which keeps non-ASCII characters (e.g. 'Åland', 'Saint Barthélemy', 'Curaçao'), uses Route 53's short forms rather than ISO long forms ('Bonaire' not 'Bonaire, Sint Eustatius and Saba'; 'Cape Verde' not 'Cabo Verde'), uses square brackets in 'Cocos [Keeling] Islands', and includes entries the emulator's table lacks (the CD/CG/CI rows, which is why every later index shifts by one). The response also echoes the request's MaxItems and the matching IsTruncated for that page.
Emulator: serves an ISO-style, ASCII-folded country table with a different row set, so CountryName values diverge, the list positions shift, and IsTruncated/MaxItems diverge with them.
Why: geolocation routing records are keyed on these codes and displayed by these names. A client that validates a user-chosen location against the emulator's catalog rejects locations Route 53 accepts (and vice versa), and any UI built from the names shows names AWS does not use.
Fix: the ListGeoLocations handler's seeded catalog and its paging projection — load Route 53's geolocation table (codes, continent/country naming, row set) and echo the request's MaxItems with the matching IsTruncated.
Traces: route53-catalog-list-geo-locations#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 — both steps answered 200 with the right envelope and diverged only in catalog content:
- route53-catalog-get-checker-ip-ranges#task#1 GetCheckerIpRanges: all 32 CheckerIpRanges members mismatched, e.g. member[0] expected '15.177.2.0/23' got '13.57.226.0/26'; member[1] expected '15.177.6.0/23' got '13.124.199.0/26'; member[16] expected '54.183.255.160/32' got '107.23.255.0/26'.
- route53-catalog-list-geo-locations#task#1 ListGeoLocations: GeoLocationDetails[15] expected {CountryCode AX, CountryName 'Åland'} got {AX, 'Aland Islands'}; [26] expected {BL, 'Saint Barthélemy'} got {BL, 'Saint Barthelemy'}; [30] expected {BQ, 'Bonaire'} got {BQ, 'Bonaire, Sint Eustatius and Saba'}; [38] expected {CC, 'Cocos [Keeling] Islands'} got {CA, 'Canada'} — the row set differs so indices shift from [38] onward through [331]; $.ListGeoLocationsResponse.IsTruncated and .MaxItems also mismatched.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the two steps under Traces.
3. Individually: GET /2013-04-01/checkeripranges — the returned CIDRs are legacy regional /26 blocks, not the current 15.177.x.0/23 set.
4. GET /2013-04-01/geolocations — country names are ASCII-folded ISO long forms and the CD/CG/CI rows are absent, shifting every later entry.
