Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false
Issue-Key: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false|_bulk_load#10|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}, expected 200: {body!r}"
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# opensearch__sns__sqs: the alerting-monitor trigger check cannot load documents — POST /_bulk on the domain endpoint answers 405 with the index route's method list (1 cause)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
Cause 1 of 1 — POST /_bulk is not served on the domain REST data plane; the path is resolved as an INDEX name, so the alerting flow can never seed the documents its monitor counts.
AWS: Amazon OpenSearch Service serves the bulk API on the domain endpoint: `POST /_bulk` with a newline-delimited action/source body answers HTTP 200 `{"took":<n>,"errors":<bool>,"items":[...]}`. Only once documents exist can an alerting monitor's query match, render its message with the matched count, and publish that rendered body to the SNS topic its destination names (and withhold it when the trigger condition is false). Recorded behavior only for the emulator's wording.
Emulator: the signed `POST <endpoint>/_bulk` answers HTTP 405 `{'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}`. Fixed text: "Incorrect HTTP method for uri [", "] and method [", "], allowed: ["; variable parts: the URI, the method, and the allowed-method list — the list returned is the INDEX route's, the tell that `/_bulk` was matched as `/{index}`.
Why: a client wiring OpenSearch alerting to SNS/SQS cannot get any data into the domain, so neither the publish-on-true nor the withhold-on-false half of the documented alerting behaviour can be observed. The data plane now answers (it no longer returns 404 UnknownService 'es' as open issue 20261024T055421Z-opensearch-sns-sqs-opensearch-alerting-monitor-cannot-be-exercised-the-domain.md records), so this is a net-new refusal of the surface that answers.
Fix: the domain REST data-plane router in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_rest.rs — reserve `_`-prefixed first path segments from index-name resolution and register the bulk handler.
Traces: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false

## Evidence
Expected HTTP 200 from POST <domain endpoint>/_bulk; got HTTP 405 {'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}. The allowed-method list is the index route's, not the bulk endpoint's.

## Steps to Reproduce
1. CreateTopic in SNS, CreateQueue in SQS and subscribe the queue to the topic.
2. CreateDomain (OpenSearch_2.11, one t3.small.search node, gp3/10) with an AccessPolicies document allowing es:* for the caller; wait for it to be active and read DomainStatus.Endpoint.
3. SigV4-sign and send POST <endpoint>/_bulk with an NDJSON body of {"index":{"_index":"<idx>","_id":"<id>"}} / source pairs -> 405 instead of 200, so no documents are loaded.
4. (Blocked by step 3) create an alerting destination for the SNS topic, create a monitor whose query counts the seeded documents, execute it, and read the message delivered to the queue.
