Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-price-list-download-workflow
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-price-list-download-workflow
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing|GetPriceListFileUrl|2xx->2xx|$.Url
Issue-Commit: 560e7694040bdb3912f85c22ba90afbac643f510

# pricing GetPriceListFileUrl returns a URL whose version segment is the price list's effective date, so it names a file set AWS never published

> _Note: this issue was identified against the code base at commit `560e7694040bdb3912f85c22ba90afbac643f510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPriceListFileUrl

## Description
One cause.

Cause 1 of 1: the `Url` GetPriceListFileUrl returns identifies a DIFFERENT price-list file set than the one the request's `PriceListArn` named on AWS, because the emulator's catalog derives a price list's version stamp from its effective date instead of from a publication instant.

AWS: Smithy — GetPriceListFileUrl takes `PriceListArn` (@required, @length 18..2048, @pattern ^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$) and `FileFormat` (@required, @length 1..255) and returns `Url` (String, "The URL to download your Price List file from"); the operation doc states the URL "is based on the PriceListArn and FileFormat that you retrieve from the ListPriceLists response". The model says nothing about the URL's layout, so for that it is recorded behavior only: the recording returns a path of the form /offers/v1.0/{partition}/{serviceCode}/{version}/{regionCode}/index.{fileFormat}, where {version} is a 14-digit yyyymmddHHMMSS PUBLICATION stamp carrying a real time of day (the recorded AmazonEC2 us-east-1 CSV URL is versioned 20240703191414 — 19:14:14). The same stamp is the version segment of the `PriceListArn` ListPriceLists reports, so on AWS the version is an attribute of the published file set and is independent of `ListPriceLists$EffectiveDate` ("The date that the Price List file prices are effective from"), which only SELECTS which published version is in effect.

Emulator: the catalog publishes exactly two synthetic price-list versions per service/Region and computes each one's version by stripping the non-digits out of its effective date (services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs: PRICE_LIST_EFFECTIVE_DATES, version_stamp(), price_list_record()), so every version it can ever publish is a midnight stamp (…000000) and the version is the effective date rather than a publication instant. GetPriceListFileUrl then returns the stored path verbatim, and the replayed step answers 200 with the version segment 20240601000000 where the recording has 20240703191414 — verdict `resource_not_identified` on $.Url. The price-list file the emulator renders repeats the same conflation: `version` is the digits of the effective date and `publicationDate` is the effective date itself (render_price_list_file in the same file).

Note on what is NOT claimed here: the host:port in the returned URL (127.0.0.1:<ephemeral port> vs pricing.us-east-1.amazonaws.com) is environment-specific — the emulator must announce an address it is actually reachable on — and is not part of this finding. The divergence is confined to the {version} path segment, which is resource identity, not a per-run value.

Why: a client that recorded a Price List download workflow against AWS, or that pins a price-list version from an ARN/URL it stored earlier, downloads a file set addressed by a version the emulator never publishes, and any version stamp it parses out of the URL or the file (yyyymmdd000000, equal to the effective date) is a shape AWS does not emit — so version-aware logic (comparing two published versions, asserting that a downloaded file's version matches the ARN it came from, or recording the publication time of a price list) is exercised against values that cannot occur on AWS. Because the emulator's ARNs and URLs are self-consistent, the breakage is silent: the workflow succeeds and only the identity of the file is wrong.

Fix: the seeded price-list catalog and its version stamping in the pricing service's state (price_list_record()/version_stamp()/PRICE_LIST_EFFECTIVE_DATES and PriceListRecord::version in services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs) — give each published price-list version its own publication instant with a time of day, distinct from the effective date it takes effect on, and have both the `PriceListArn`, the file path GetPriceListFileUrl hands back, and the rendered file's `version`/`publicationDate` fields derive from that publication instant. GetPriceListFileUrl's handler itself (ops/get_price_list_file_url.rs) only needs to keep echoing whatever the record stores.

Related open issue (does NOT cover this): 20261108T161655Z-pricing-catalog-metadata-diverges-from-the-published-price.md covers DescribeServices/GetAttributeValues/GetProducts catalog metadata and carries no replay key for this fixture step.

Traces: pricing-price-list-download-workflow#task#2 (GetPriceListFileUrl, 200 vs 200, $.Url).

## Evidence
Fixture pricing-price-list-download-workflow, step task#2, operation GetPriceListFileUrl.
Expected status 200, actual status 200 — body mismatch on $.Url (verdict: mismatch, detail classification `resource_not_identified`).

Expected: https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonEC2/20240703191414/us-east-1/index.csv
Actual:   https://127.0.0.1:42461/offers/v1.0/aws/AmazonEC2/20240601000000/us-east-1/index.csv

Fixed vs variable parts of the expected URL: the fixed template is `/offers/v1.0/{partition}/{serviceCode}/{version}/{regionCode}/index.{fileFormat}`; `aws`, `AmazonEC2`, `us-east-1` and `csv` come from the requested PriceListArn and FileFormat and match. `{version}` is the only diverging segment: the recording's 20240703191414 is a publication instant with a time of day (19:14:14), the emulator's 20240601000000 is the digits of a seeded effective date (2024-06-01T00:00:00Z) and so is always midnight. The host and ephemeral port differ by environment and are explicitly NOT part of this finding.

Emulator source backing the diagnosis:
- services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:1432 — `fn version_stamp(rfc3339: &str) -> String { rfc3339.chars().filter(char::is_ascii_digit).collect() }`
- services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:1375-1392 — the version used in both the ARN and the file path is `version_stamp(effective_date)`
- services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:1463 — `PRICE_LIST_EFFECTIVE_DATES = ["2023-01-01T00:00:00Z", "2024-06-01T00:00:00Z"]`
- services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs:606-607 — rendered file reports `"version": record.version()` and `"publicationDate": record.effective_date_text()`
- services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_price_list_file_url.rs:209-211 — `url: Some(format!("{base_url}{file_path}"))`, i.e. the stored path is echoed unchanged.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or reproduce by hand:
2. Call ListPriceLists with ServiceCode=AmazonEC2, CurrencyCode=USD, RegionCode=us-east-1 and an EffectiveDate in mid-2024, and take the returned PriceListArn.
3. Observe the ARN's version segment is 20240601000000 — a midnight stamp equal to the emulator's seeded effective date — where AWS publishes a version with a real publication time of day (e.g. 20240703191414).
4. Call GetPriceListFileUrl with that PriceListArn and FileFormat=csv.
5. The returned Url is <emulator-base>/offers/v1.0/aws/AmazonEC2/20240601000000/us-east-1/index.csv. Every path segment matches AWS's template except {version}, which names a file set version AWS never published.
6. Fetch the URL and read the file's header: `version` is again the effective-date digits and `publicationDate` is the effective date, instead of a publication instant distinct from the effective date.
