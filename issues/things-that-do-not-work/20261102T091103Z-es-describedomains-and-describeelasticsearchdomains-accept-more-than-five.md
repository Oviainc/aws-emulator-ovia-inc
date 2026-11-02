Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-albos-domain-lookup-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-albos-domain-lookup-mistakes opensearch-osc-legacy-es-api-domain-lookups
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeDomains|4xx->2xx|$.message
Issue-Key: es|DescribeDomains|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeDomains|4xx->2xx|status
Issue-Key: es|DescribeElasticsearchDomains|4xx->2xx|$.message
Issue-Key: es|DescribeElasticsearchDomains|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeElasticsearchDomains|4xx->2xx|status
Issue-Commit: 486151182441fa9779198fc02e1d6d3c56236874

# es DescribeDomains and DescribeElasticsearchDomains accept more than five domain names instead of refusing the batch

> _Note: this issue was identified against the code base at commit `486151182441fa9779198fc02e1d6d3c56236874`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomains

## Description
2 net-new root causes on the `es` service token, one shared fault: the per-request cap of five domain names on the batch domain-describe operations is not enforced, so a request AWS refuses with HTTP 400 is served with HTTP 200. Each cause is its own replay unit and must be re-replayed separately before this issue is closed.

RELATED OPEN WORK (does NOT cover these replay keys): 20261029T190009Z-es-ten-requests-real-aws-refuses-are-accepted.md collects other `es` requests accepted where AWS refuses (CreateDomain, CreatePackage, CreateOutboundConnection); neither DescribeDomains nor DescribeElasticsearchDomains appears among its keys.

Cause 1 of 2 — DescribeDomains with more than five DomainNames (fixture opensearch-albos-domain-lookup-mistakes, step task#4).
AWS: Smithy (com.amazonaws.opensearch): DescribeDomains POST /2021-01-01/opensearch/domain-info, input DomainNames: DomainNameList @required (member @length(3,28) @pattern("^[a-z][a-z0-9\\-]+$")); errors BaseException (client, 400, defaulted), ValidationException (client, 400), InternalException (500). The model states NO maximum on the list, so this cap is recorded behavior only: the recording shows AWS answering HTTP 400 with the body member `message` set to exactly `Please provide a maximum of 5 domain names to describe.` (the errortype header is asserted only as shaped). The whole sentence is fixed text — the only variable in the rule is the limit, 5, and the limit applies to the number of entries in DomainNames.
Emulator: answers HTTP 200 with a DescribeDomains response body — the list length is never checked, so no refusal is produced at all. Replay detail: `status: expected 400, got 200`, `$.message: missing`, `$headers.x-amzn-errortype: missing`.
Why: a client that batches domain lookups is silently allowed to exceed the service's batch limit. Code written and tested against the emulator ships a page size AWS rejects outright, and the failure appears only in production; conversely a client testing its own error handling for the oversized batch can never exercise that branch.
Fix: this operation's input validation — a pre-handler check on the DomainNames list length that raises the service's 400 refusal with that message.
Traces: opensearch-albos-domain-lookup-mistakes#task#4.

Cause 2 of 2 — DescribeElasticsearchDomains with more than five DomainNames on the legacy 2015-01-01 surface (fixture opensearch-osc-legacy-es-api-domain-lookups, step task#8).
AWS: Smithy (com.amazonaws.elasticsearchservice): DescribeElasticsearchDomains POST /2015-01-01/es/domain-info, input DomainNames: DomainNameList @required; errors BaseException (client, 400, defaulted), ValidationException (client, 400), InternalException (500). Again no modeled list maximum, so recorded behavior only: HTTP 400 with `message` exactly `Please provide a maximum of 5 Elasticsearch domain names to describe.` — note the wording differs from cause 1 by the word `Elasticsearch`, so the two surfaces do not share one message string. Errortype asserted as shaped.
Emulator: answers HTTP 200 with a DescribeElasticsearchDomains response body; the cap is not enforced here either. Replay detail: `status: expected 400, got 200`, `$.message: missing`, `$headers.x-amzn-errortype: missing`.
Why: same as cause 1, on the legacy API surface still used by older SDKs and the `aws es` CLI.
Fix: this operation's input validation. Listed as its own cause because the refusal text differs from cause 1's and it is a separate handler, so a fix applied only to the 2021-01-01 operation leaves this step failing.
Traces: opensearch-osc-legacy-es-api-domain-lookups#task#8.

## Evidence
Cause 1 — opensearch-albos-domain-lookup-mistakes#task#4, DescribeDomains (divergence_key service=es|DescribeDomains|status=400->200|keys=$.message,$headers.x-amzn-errortype,status):
  expected: HTTP 400, $.message = "Please provide a maximum of 5 domain names to describe.", $headers.x-amzn-errortype present (shaped, not value-asserted)
  actual:   HTTP 200, no $.message, no x-amzn-errortype header
  replay detail: "status: expected 400, got 200 / $.message: missing (expected 'Please provide a maximum of 5 domain names to describe.', got None) / $headers.x-amzn-errortype: missing"
  Smithy cited: com.amazonaws.opensearch DescribeDomains POST /2021-01-01/opensearch/domain-info, DomainNames: DomainNameList @required with NO modeled list length — the cap of 5 is established by the recording only.

Cause 2 — opensearch-osc-legacy-es-api-domain-lookups#task#8, DescribeElasticsearchDomains (divergence_key service=es|DescribeElasticsearchDomains|status=400->200|keys=$.message,$headers.x-amzn-errortype,status):
  expected: HTTP 400, $.message = "Please provide a maximum of 5 Elasticsearch domain names to describe.", errortype header present
  actual:   HTTP 200, no $.message, no x-amzn-errortype header
  Smithy cited: com.amazonaws.elasticsearchservice DescribeElasticsearchDomains POST /2015-01-01/es/domain-info, DomainNames: DomainNameList @required with no modeled list length.

Both messages are entirely fixed text; the only value in the rule is the limit 5 applied to the submitted DomainNames list length.

## Steps to Reproduce
1. Replay fixture opensearch-albos-domain-lookup-mistakes step task#4: POST /2021-01-01/opensearch/domain-info with a DomainNames list of more than five names.
2. Observe HTTP 200 with a DomainStatusList body where the recording is HTTP 400 with message "Please provide a maximum of 5 domain names to describe.".
3. Replay fixture opensearch-osc-legacy-es-api-domain-lookups step task#8: POST /2015-01-01/es/domain-info with more than five names.
4. Observe HTTP 200 where the recording is HTTP 400 with message "Please provide a maximum of 5 Elasticsearch domain names to describe.".
5. Both steps must pass before this issue is closed.
