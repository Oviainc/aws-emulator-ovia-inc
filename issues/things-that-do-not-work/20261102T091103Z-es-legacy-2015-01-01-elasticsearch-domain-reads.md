Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-osc-legacy-es-api-domain-lookups
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-osc-legacy-es-api-domain-lookups
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeElasticsearchDomain|4xx->4xx|status
Issue-Key: es|DescribeElasticsearchDomainConfig|4xx->4xx|status
Issue-Commit: 486151182441fa9779198fc02e1d6d3c56236874

# es legacy 2015-01-01 Elasticsearch domain reads answer HTTP 409 for an absent domain instead of 404

> _Note: this issue was identified against the code base at commit `486151182441fa9779198fc02e1d6d3c56236874`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeElasticsearchDomain

## Description
2 net-new root causes on the `es` service token, one shared fault: on the legacy 2015-01-01 Elasticsearch Service domain readers, a not-found refusal is serialized with HTTP 409 instead of the recorded 404. Each cause is its own replay unit and must be re-replayed separately before this issue is closed.

RELATED OPEN WORK (does NOT cover these replay keys): 20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md reports the same status defect across 28 operations on the 2021-01-01 OpenSearch surface, but its Issue-Key list contains no `es|DescribeElasticsearchDomain` or `es|DescribeElasticsearchDomainConfig` entry and its Issue-Trace-Ids do not include opensearch-osc-legacy-es-api-domain-lookups, so neither step below is gated by it. If that issue's fix is a single shared error-to-status mapping it may close these too, but they must be re-replayed to establish that; if the legacy 2015-01-01 handlers serialize errors through their own path, they need a separate change.

Cause 1 of 2 — DescribeElasticsearchDomain on a domain that does not exist (fixture opensearch-osc-legacy-es-api-domain-lookups, step task#4).
AWS: Smithy (com.amazonaws.elasticsearchservice): DescribeElasticsearchDomain GET /2015-01-01/es/domain/{DomainName}; errors BaseException (client, 400, defaulted), ResourceNotFoundException, ValidationException (client, 400), InternalException (500). The checked-in model must NOT be used as the source of the status here: it declares ResourceNotFoundException with @httpError(409) (and its own doc string says 400), while the recording answers HTTP 404. Recorded behavior is the requirement: HTTP 404 carrying the ResourceNotFoundException error class; the error code and the message text are not in dispute in this cluster — the replay reports `status` as the only diverging key.
Emulator: serializes the same refusal with HTTP 409 (the modeled httpError). Replay detail: `status: expected 404, got 409`.
Why: every SDK and hand-rolled client branches on the HTTP status before it inspects the body — 404 means 'absent, treat as gone', 409 means 'conflict, retry or abort'. A describe-then-create or wait-until-deleted loop against the legacy ES API therefore takes the conflict branch forever for a domain that simply does not exist.
Fix: the error-to-HTTP-status mapping used when serializing ResourceNotFoundException on the 2015-01-01 Elasticsearch Service routes (not the handler, which already raises the right error shape).
Traces: opensearch-osc-legacy-es-api-domain-lookups#task#4.

Cause 2 of 2 — DescribeElasticsearchDomainConfig on a domain that does not exist (fixture opensearch-osc-legacy-es-api-domain-lookups, step task#5).
AWS: Smithy: DescribeElasticsearchDomainConfig GET /2015-01-01/es/domain/{DomainName}/config; same error set, ResourceNotFoundException again modeled @httpError(409) and again recorded as HTTP 404. Recorded behavior is the requirement.
Emulator: HTTP 409. Replay detail: `status: expected 404, got 409`; `status` is the only diverging key.
Why: same as cause 1, on the config reader that clients poll while a domain-configuration change settles.
Fix: same status mapping as cause 1. Listed as its own cause because it is a distinct route and a distinct replay key — a fix verified only on the domain reader leaves this step unexercised.
Traces: opensearch-osc-legacy-es-api-domain-lookups#task#5.

## Evidence
Cause 1 — opensearch-osc-legacy-es-api-domain-lookups#task#4, DescribeElasticsearchDomain (divergence_key service=es|DescribeElasticsearchDomain|status=404->409|keys=status):
  expected: HTTP 404 with the ResourceNotFoundException error class
  actual:   HTTP 409
  replay detail: "status: expected 404, got 409" — status is the only diverging key, so the error code and message already match.
  Smithy cited: com.amazonaws.elasticsearchservice#ResourceNotFoundException errorKind=client, httpError=409 (model), contradicted by the recording's 404; DescribeElasticsearchDomain GET /2015-01-01/es/domain/{DomainName} lists ResourceNotFoundException among its errors.

Cause 2 — opensearch-osc-legacy-es-api-domain-lookups#task#5, DescribeElasticsearchDomainConfig (divergence_key service=es|DescribeElasticsearchDomainConfig|status=404->409|keys=status):
  expected: HTTP 404; actual: HTTP 409; replay detail "status: expected 404, got 409", status the only diverging key.
  Smithy cited: DescribeElasticsearchDomainConfig GET /2015-01-01/es/domain/{DomainName}/config, ResourceNotFoundException httpError=409 in the model vs 404 recorded.

The absent domain name used by the fixture is a per-run probe value and is not part of the requirement; the requirement is the 404 status line for this error class on these routes.

## Steps to Reproduce
1. Replay fixture opensearch-osc-legacy-es-api-domain-lookups step task#4: GET /2015-01-01/es/domain/<a domain name that does not exist>.
2. Observe HTTP 409 where the recording is HTTP 404 (body error class ResourceNotFoundException and message already agree).
3. Replay the same fixture's step task#5: GET /2015-01-01/es/domain/<absent domain>/config, and observe HTTP 409 where the recording is HTTP 404.
4. Both steps must pass before this issue is closed.
