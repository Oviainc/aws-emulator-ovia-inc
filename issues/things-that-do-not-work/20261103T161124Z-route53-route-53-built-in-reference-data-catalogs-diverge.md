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
Issue-Key: route53|ListGeoLocations|2xx->2xx|$.ListGeoLocationsResponse.MaxItems
Issue-Commit: cdd15ea938f04f0ddc4e377abbe8b95b02ee9eac

# Route 53 built-in reference-data catalogs diverge from AWS: checker IP ranges and geo-location list

> _Note: this issue was identified against the code base at commit `cdd15ea938f04f0ddc4e377abbe8b95b02ee9eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCheckerIpRanges

## Description
The two Route 53 read-only catalog operations that serve fixed service reference data (GetCheckerIpRanges and ListGeoLocations) answer 200 with a catalog that is NOT the catalog AWS serves. The status, envelope and element shapes are right; the DATA inside them is wrong, so the operations are structurally implemented but materially unusable. 2 causes, both in route53's reference-data tables.

Cause 1 of 2: GetCheckerIpRanges returns a checker-IP-range set whose entries are not AWS's. Fixture route53-catalog-get-checker-ip-ranges, step task#1.
  AWS: Smithy: GetCheckerIpRanges GET /2013-04-01/checkeripranges, no input members, no modeled errors, output CheckerIpRanges (required, list of strings). The model does not enumerate the values, so: recorded behavior only — the recording shows AWS returning a fixed, service-published list of /32 CIDRs for the Route 53 health-checker fleet, e.g. members 19/20/22/23/24/26/27/28/30/31 are '54.241.32.64/32', '54.243.31.192/32', '54.245.168.0/32', '54.248.220.0/32', '54.250.253.192/32', '54.252.79.128/32', '54.252.254.243/32', '54.255.254.192/32', '176.34.159.192/32', '177.71.207.128/32'. This list is a published constant of the service, identical for every caller and every account, so it is a requirement and not a per-run value.
  Emulator: answers 200 with a list of the same length and the same ordering, but at least ten members hold different addresses: '54.241.32.96/32', '54.243.31.224/32', '54.245.168.32/32', '54.248.220.32/32', '54.250.253.224/32', '54.252.79.160/32', '54.252.254.224/32', '54.255.254.224/32', '176.34.159.224/32', '177.71.207.160/32' — each the AWS value with its last octet shifted, i.e. a synthesized/approximated table rather than the published one.
  Why: GetCheckerIpRanges exists so a client can program firewall or security-group allow-lists for the health-checker fleet. A client that builds its allow-list from the emulator's answer admits the wrong /32s and denies the real checkers, and any test that asserts a known checker address against the catalog fails for a reason that does not exist on AWS.
  Fix: the GetCheckerIpRanges handler's reference-data table in the route53 service crate — replace the synthesized CIDR list with the service's published checker-IP-range constant, preserving the recorded ordering.

Cause 2 of 2: ListGeoLocations returns a geo-location catalog with different country entries, and a MaxItems that does not match the recording. Fixture route53-catalog-list-geo-locations, step task#1.
  AWS: Smithy: ListGeoLocations GET /2013-04-01/geolocations; output GeoLocationDetailsList (required), IsTruncated (required, default false), MaxItems (required, Integer), optional NextContinentCode/NextCountryCode/NextSubdivisionCode; error InvalidInput (400). The model does not enumerate the catalog, so: recorded behavior only — the recording shows AWS returning GeoLocationDetails entries ordered by continent/country code, with AWS's own CountryName spellings and the full set of supported country codes, e.g. CD 'Congo', CG 'Republic of the Congo', CI 'Ivory Coast', CZ 'Czech Republic', FM 'Federated States of Micronesia', JO 'Hashemite Kingdom of Jordan', KR 'Republic of Korea', MD 'Republic of Moldova', MK 'Macedonia'. These names and codes are service-published constants, not per-run values. MaxItems echoes the effective page size of the request.
  Emulator: answers 200, but the list content is shifted/substituted from index 34 onward — the positions that must hold the entries above instead hold neighbouring codes ('CC' / 'Cocos [Keeling] Islands', 'CF' / 'Central African Republic', 'CH' / 'Switzerland', 'CX' / 'Christmas Island', 'FI' / 'Finland', 'IS' / 'Iceland', 'KI' / 'Kiribati', 'LV' / 'Latvia', 'ME' / 'Montenegro'), and the tail (indexes ~227-255, 319) diverges as well, so the emulator's catalog is missing AWS's country entries and carries a different membership and length. $.ListGeoLocationsResponse.MaxItems also diverges from the recorded value.
  Why: ListGeoLocations is how a client discovers which geo codes are valid before creating a geolocation record set. With a catalog whose membership differs, a client cannot find codes AWS supports (CD, CG, CI, CZ, FM, JO, KR, MD, MK ...), gets AWS-unsupported ones instead, and renders wrong country names; paging logic driven by the echoed MaxItems is also computed from the wrong page size.
  Hypothesis: the emulator derives the catalog from a generic ISO country table rather than Route 53's own geo-location table, which explains both the substituted codes and the AWS-specific long-form names ('Republic of Korea', 'Hashemite Kingdom of Jordan') being absent.
  Fix: the ListGeoLocations handler's reference-data table and its MaxItems echo in the route53 service crate — seed the geo-location catalog from Route 53's published continent/country/subdivision set with AWS's CountryName strings, and echo the effective page size the request asked for.

Related open issues (neither covers these steps): '20261029T220729Z-route53-route-53-response-envelopes-omit-hostedzone-features-and.md' documents the same class of MaxItems echo defect on ListHealthChecks (emulator emits 1000 where AWS emits 100), which is consistent with the MaxItems half of Cause 2 but is filed against a different operation and fixture.

## Evidence
Cause 1 — fixture route53-catalog-get-checker-ip-ranges, step task#1, GetCheckerIpRanges, expected 200 / actual 200:
  $.GetCheckerIpRangesResponse.CheckerIpRanges.member[19]: expected '54.241.32.64/32', got '54.241.32.96/32'
  member[20]: expected '54.243.31.192/32', got '54.243.31.224/32'
  member[22]: expected '54.245.168.0/32', got '54.245.168.32/32'
  member[23]: expected '54.248.220.0/32', got '54.248.220.32/32'
  member[24]: expected '54.250.253.192/32', got '54.250.253.224/32'
  member[26]: expected '54.252.79.128/32', got '54.252.79.160/32'
  member[27]: expected '54.252.254.243/32', got '54.252.254.224/32'
  member[28]: expected '54.255.254.192/32', got '54.255.254.224/32'
  member[30]: expected '176.34.159.192/32', got '176.34.159.224/32'
  member[31]: expected '177.71.207.128/32', got '177.71.207.160/32'

Cause 2 — fixture route53-catalog-list-geo-locations, step task#1, ListGeoLocations, expected 200 / actual 200:
  $.ListGeoLocationsResponse.GeoLocationDetailsList.GeoLocationDetails[39]: expected {'CountryCode': 'CD', 'CountryName': 'Congo'}, got {'CountryCode': 'CC', 'CountryName': 'Cocos [Keeling] Islands'}
  [41]: expected {'CountryCode': 'CG', 'CountryName': 'Republic of the Congo'}, got {'CountryCode': 'CF', 'CountryName': 'Central African Republic'}
  [43]: expected {'CountryCode': 'CI', 'CountryName': 'Ivory Coast'}, got {'CountryCode': 'CH', 'CountryName': 'Switzerland'}
  [54]: expected {'CountryCode': 'CZ', 'CountryName': 'Czech Republic'}, got {'CountryCode': 'CX', 'CountryName': 'Christmas Island'}
  [70]: expected {'CountryCode': 'FM', 'CountryName': 'Federated States of Micronesia'}, got {'CountryCode': 'FI', 'CountryName': 'Finland'}
  [109]: expected {'CountryCode': 'JO', 'CountryName': 'Hashemite Kingdom of Jordan'}, got {'CountryCode': 'IS', 'CountryName': 'Iceland'}
  [118]: expected {'CountryCode': 'KR', 'CountryName': 'Republic of Korea'}, got {'CountryCode': 'KI', 'CountryName': 'Kiribati'}
  [135]: expected {'CountryCode': 'MD', 'CountryName': 'Republic of Moldova'}, got {'CountryCode': 'LV', 'CountryName': 'Latvia'}
  [140]: expected {'CountryCode': 'MK', 'CountryName': 'Macedonia'}, got {'CountryCode': 'ME', 'CountryName': 'Montenegro'}
  plus further diverging indexes 34, 40, 42, 44, 56, 66, 73, 96, 113, 122, 139, 142, 144, 146, 177, 181, 209, 213, 217, 221, 227-253, 255, 319, and $.ListGeoLocationsResponse.MaxItems (value_mismatch).

Both clusters came back with empty stub_path and empty owned_by, i.e. no open issue carries their replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture route53-catalog-get-checker-ip-ranges step task#1 — GET /2013-04-01/checkeripranges. Compare every CheckerIpRanges member with the recorded response; members 19, 20, 22, 23, 24, 26, 27, 28, 30, 31 differ.
3. Cause 2: replay fixture route53-catalog-list-geo-locations step task#1 — GET /2013-04-01/geolocations. Compare GeoLocationDetails entry-by-entry with the recorded response; entries from index 34 onward diverge in CountryCode/CountryName and the response's MaxItems does not match the recorded value.
