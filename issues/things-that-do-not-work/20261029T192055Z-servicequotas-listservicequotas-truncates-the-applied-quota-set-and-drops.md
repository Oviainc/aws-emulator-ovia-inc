Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-error-paths service-quotas-tag-applied-quota
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.NextToken
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"]
Issue-Commit: 5bf04d9c01ed3990fbed03ee0a0bc59641f482e6

# ListServiceQuotas truncates the applied-quota set and drops its NextToken

> _Note: this issue was identified against the code base at commit `5bf04d9c01ed3990fbed03ee0a0bc59641f482e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceQuotas

## Description
Cause 1 of 2 - the applied-quota set a ListServiceQuotas page carries is short, and the Descriptions it does carry are paraphrased.
AWS: ListServiceQuotas returns the applied quota values for the requested ServiceCode, each as a ServiceQuota carrying the service's own QuotaName/Description and QuotaAppliedAtLevel (Smithy: ListServiceQuotas input {ServiceCode required, NextToken, MaxResults 1..100, QuotaCode, QuotaAppliedAtLevel}, output {NextToken, Quotas: ServiceQuotaListDefinition}; ServiceQuota.Description is QuotaDescription @length 0..350).
Emulator: for ServiceCode `lambda` the page omits applied quotas the recording carries - L-B430C318, L-4273958C, L-25EEC0A4, L-14320ECE and L-BA29C22B are all reported `missing ... got None` - and rewrites the Description of the one it does carry: L-B82A30EA is 'The maximum rate of SendDurableExecutionCallbackSuccess API requests.' where AWS reports 'The maximum number of SendDurableExecutionCallbackSuccess API requests per second.' The Description string is fixed text per quota code with no variable parts.
Why: applied quotas are what a client reads to learn the value actually in force for its account; an absent row makes the quota invisible, and a rewritten Description loses the per-second-rate wording that distinguishes a rate quota from a count.
Fix: the applied-quota catalog data behind the ListServiceQuotas handler (the same static quota table ListAWSDefaultServiceQuotas reads), including the Description values - not the handler's filtering.
Traces: service-quotas-error-paths#task#4, service-quotas-tag-applied-quota#task#1.

Cause 2 of 2 - the response omits NextToken, so a truncated page is reported as the whole set.
AWS: ListServiceQuotas is paginated (Smithy: paginated {inputToken NextToken, outputToken NextToken, items Quotas, pageSize MaxResults}; output NextToken is NextToken @length 0..2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$). When more applied quotas remain, the response carries an opaque NextToken; the recording for this step carries one.
Emulator: $.NextToken is absent from the body (`missing (expected '<opaque-pagination-token>', got None)`), while the Quotas list it returned is not the complete set either.
Why: a paginating client stops after the first page because no continuation token came back, so it silently never sees the remaining applied quotas - a wrong answer rather than a failure. The token value itself is opaque and per-run, so the requirement is that a token is PRESENT whenever the page does not exhaust the set, not that it equals any recorded string.
Fix: the ListServiceQuotas handler's pagination - emit the continuation token whenever the page it serves does not exhaust the matching applied-quota set.
Traces: service-quotas-error-paths#task#4.

Note on existing issues: 20261023T182924Z-servicequotas-applied-and-default-quota-lookups-return-records.md and 20261024T212954Z-service-quotas-listservicequotas-pagination.md both touch this area but are RESOLVED, so neither covers these failures. The replay reports owned_by [] and stub_path "" for both clusters, so no open issue and no unimplemented-operation issue covers them.

## Evidence
Cluster A - `service=servicequotas|ListServiceQuotas|status=200->200|keys=$.NextToken,$.Quotas[QuotaCode="L-14320ECE"],$.Quotas[QuotaCode="L-25EEC0A4"],$.Quotas[QuotaCode="L-4273958C"],$.Quotas[QuotaCode="L-B430C318"],$.Quotas[QuotaCode="L-B82A30EA"].Description,$.Quotas[QuotaCode="L-BA29C22B"]`, fixture_id `service-quotas-error-paths`, step_key `task#4`, verdict mismatch, expected_status 200, actual_status 200, affected_count 1, owned_by [], stub_path "". Detail:
- `$.NextToken: missing (expected '<opaque-pagination-token>', got None)`
- `$.Quotas[QuotaCode="L-B82A30EA"].Description: value_mismatch (expected 'The maximum number of SendDurableExecutionCallbackSuccess API requests per second.', got 'The maximum rate of SendDurableExecutionCallbackSuccess API requests.')`
- `$.Quotas[QuotaCode="L-B430C318"]: missing (expected {'Adjustable': False, 'Description': 'The maximum execution duration of a MicroVM (in Hours).', 'GlobalQuota': False, 'QuotaAppliedAtLevel': 'ACCOUNT', 'QuotaArn': 'arn:aws:servicequotas:us-west-2:000000000000:lambda/L-B430C318', 'QuotaCode': 'L-B430C318', 'QuotaName': 'Max Execution Duration of a MicroVM (in Hours)', 'ServiceCode': 'lambda', 'ServiceName': 'AWS Lambda', 'Unit': 'None', 'Value': 8.0}, got None)`
- `$.Quotas[QuotaCode="L-4273958C"]: missing (expected {... 'QuotaName': 'Rate of GetPolicy API requests', ... 'Value': 15.0}, got None)`
- `$.Quotas[QuotaCode="L-25EEC0A4"]: missing (expected {... 'QuotaName': 'Burst rate of ResumeMicrovm API requests', ... 'Value': 5.0}, got None)`
- `$.Quotas[QuotaCode="L-14320ECE"]: missing (expected {... sustained network bandwidth for a non-VPC execution environment ...}, got None)`
- `$.Quotas[QuotaCode="L-BA29C22B"]: missing`

Cluster B - `service=servicequotas|ListServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-14320ECE"],$.Quotas[QuotaCode="L-25EEC0A4"],$.Quotas[QuotaCode="L-4273958C"],$.Quotas[QuotaCode="L-B430C318"],$.Quotas[QuotaCode="L-B82A30EA"].Description,$.Quotas[QuotaCode="L-BA29C22B"]`, fixture_id `service-quotas-tag-applied-quota`, step_key `task#1`, verdict mismatch, expected_status 200, actual_status 200, owned_by [], stub_path "". Same five missing applied quotas and the same paraphrased L-B82A30EA Description, reached through a different fixture - so the gap is in the catalog data, not in one fixture's setup. This cluster shows no $.NextToken diff, which is why the dropped token is a separate cause.

The recorded QuotaArn values and the account id in them are per-environment and are not part of the requirement; the requirement is that the quota rows exist with the service's own QuotaName/Description/Value/Unit/Adjustable/QuotaAppliedAtLevel, and that a continuation token is present when the page does not exhaust the set.

Smithy evidence (services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json): ListServiceQuotas input {ServiceCode required @length 1..63, NextToken @length 0..2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$, MaxResults @range 1..100, QuotaCode, QuotaAppliedAtLevel: AppliedLevelEnum}, output {NextToken, Quotas: ServiceQuotaListDefinition}, paginated {inputToken NextToken, outputToken NextToken, items Quotas, pageSize MaxResults}, errors AccessDeniedException(403)/IllegalArgumentException(400)/InvalidPaginationTokenException(400)/NoSuchResourceException(404)/ServiceException(500)/TooManyRequestsException(429).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire fixture corpus.
3. Step `task#4` of fixture `service-quotas-error-paths` calls ListServiceQuotas for ServiceCode=lambda. The response is HTTP 200 but $.NextToken is absent where the recording carries an opaque continuation token, and $.Quotas omits L-B430C318, L-4273958C, L-25EEC0A4, L-14320ECE and L-BA29C22B.
4. In the same response, read $.Quotas[QuotaCode="L-B82A30EA"].Description: it is the emulator's rewrite ('The maximum rate of ... API requests.') rather than the service's text ('The maximum number of ... API requests per second.').
5. Step `task#1` of fixture `service-quotas-tag-applied-quota` calls ListServiceQuotas for the same ServiceCode from a different fixture and shows the same five missing applied quotas and the same paraphrased Description, confirming the catalog - not the fixture - is the source.
