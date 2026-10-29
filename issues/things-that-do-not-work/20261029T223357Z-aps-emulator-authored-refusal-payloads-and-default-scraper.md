Issue-Source: trace-sweep-divergence
Issue-Trace: amp-scraper-error-paths-and-defaults
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amp-scraper-error-paths-and-defaults amp-anomaly-detector-workflow amp-workspace-lifecycle-and-configuration
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: aps|CreateScraper|4xx->4xx|$.message
Issue-Key: aps|CreateScraper|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: aps|CreateScraper|4xx->4xx|status
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|$.message
Issue-Key: aps|GetDefaultScraperConfiguration|2xx->2xx|$.configuration
Issue-Key: aps|DescribeScraperLoggingConfiguration|4xx->4xx|$.message
Issue-Key: aps|DescribeScraperLoggingConfiguration|4xx->4xx|$.resourceType
Issue-Key: aps|DeleteScraperLoggingConfiguration|4xx->4xx|$.message
Issue-Key: aps|DeleteScraperLoggingConfiguration|4xx->4xx|$.resourceId
Issue-Key: aps|DeleteScraperLoggingConfiguration|4xx->4xx|$.resourceType
Issue-Key: aps|DescribeWorkspace|4xx->4xx|$.message
Issue-Commit: 900a7beaab356ed52889492ca7befa08b17bdc0c

# aps: emulator-authored refusal payloads and default scraper document diverge from the service's own text (6 causes)

> _Note: this issue was identified against the code base at commit `900a7beaab356ed52889492ca7befa08b17bdc0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScraper (also GetDefaultScraperConfiguration, DescribeScraperLoggingConfiguration, DeleteScraperLoggingConfiguration, CreateAnomalyDetector, DescribeWorkspace)

## Description
SIX net-new wire root causes in the aps (Amazon Managed Service for Prometheus) emulator, found by replaying the wire fixture corpus simparity-wire-aws-v1. aps IS served (its bootstrap issue is resolved; 35/41 replayed steps pass), so these are fidelity defects, not bootstrap gaps. They share one fault: where the service answers with ITS OWN fixed payload — a fixed refusal sentence, the resource family a not-found names, the error class a refusal belongs to, or the published default scraper document — the emulator answers with text and classification it authored itself. SIX causes must be closed before submitting; each cause names its own failing fixture and step.

Related open issue 20261029T190216Z-amp-6-refusal-message-divergences-every-rejection-text.md was filed from the BEHAVIORAL corpus and describes the same emulator wording for two of the surfaces below (its cause 1 = CreateAnomalyDetector's RCF message, its cause 5 = DescribeWorkspace's trailing period). It carries no wire replay key, so causes 5 and 6 here are the wire-corpus bindings of those two surfaces and must be re-replayed against these fixtures; causes 1-4 are not described there at all. Related open issue 20261029T150024Z-amp-createanomalydetector-maintainability.md notes that the workspace/scraper not-found refusal is composed inline in 28 handlers, which is why the wrong family leaks into several of these paths.

Cause 1 of 6: CreateScraper refuses a request naming a nonexistent subnet as a 404 ResourceNotFoundException on the workspace instead of the service's 400 ValidationException about the subnet.
  AWS: Smithy: CreateScraper (POST /scrapers, success 202) takes required scrapeConfiguration (ScrapeConfiguration union, configurationBlob), source (Source union: eksConfiguration | vpcConfiguration, each carrying subnetIds) and destination, and models BOTH ValidationException @httpError 400 and ResourceNotFoundException @httpError 404. Recorded behavior: when the source's subnet does not exist, the call is refused 400 with x-amzn-errortype: ValidationException, and the message is the subnet refusal the service surfaces from its EC2 hop — fixed text "The subnet ID '<subnetId>' does not exist" with the subnet id taken from the request, followed by the service's cross-service attribution suffix. The subnet check is reached regardless of whether the destination workspace resolves, i.e. this input validation is not gated behind the workspace lookup. (The "(Service: Ec2, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: 1)" tail of the recorded message carries a per-run request id; the request id value itself is not a requirement, the ValidationException class, the 400 status and the "The subnet ID '<id>' does not exist" sentence are.)
  Emulator: answers 404 with x-amzn-errortype: ResourceNotFoundException and message 'Workspace not found.' — a different HTTP status, a different modeled error shape and a refusal about a different resource than the one the request got wrong.
  Why: a client branches on the error class: ValidationException/400 is a non-retryable bad-input condition the caller fixes by correcting subnetIds, while ResourceNotFoundException/404 tells it to create or wait for a workspace that is in fact fine. Any caller catching ResourceNotFoundException separately (or retrying on it) takes the wrong path, and the actual bad member is never reported.
  Fix: CreateScraper's request validation and refusal ordering in the aps service — the source/subnet validation must run and refuse as ValidationException before (and independently of) the destination workspace lookup that currently short-circuits it.
  Traces: amp-scraper-error-paths-and-defaults#task#4

Cause 2 of 6: GetDefaultScraperConfiguration serves a default scrape-configuration document that is not the published one — the in-cluster authentication members are missing.
  AWS: Smithy: GetDefaultScraperConfiguration (GET /scraperconfiguration, success 200, @readonly, no input) returns required configuration (Blob), documented as "The configuration file. Base 64 encoded" and linked to the "Scraper configuration" section of the Amazon Managed Service for Prometheus User Guide. Recorded behavior: the blob is a fixed, service-wide document with no variable parts — global.scrape_interval 30s, an inert commented external_labels/clusterArn placeholder, and the scrape_configs jobs pod_exporter, cadvisor, kubernetes-apiservers and kube-proxy. The recorded document gives cadvisor an authorization.credentials_file pointing at the in-cluster service-account token mount and gives kubernetes-apiservers a bearer_token_file pointing at the same mount; the blob is compared byte-for-byte because callers feed it straight to Prometheus.
  Emulator: returns a document that drops exactly those authentication members (the handler's own doc comment records the omission as deliberate), so $.configuration does not equal the served document.
  Why: a caller that takes this blob as the starting point for its own scraper configuration — which is the only purpose of the operation — gets a Prometheus config whose cadvisor and apiserver jobs carry no credentials and therefore cannot scrape; and no client can round-trip the blob it would get from the service.
  Fix: the default-document constant that GetDefaultScraperConfiguration's handler serves in the aps service must carry the published document's authentication members. If the merge gate's secret-path scan is what forced the omission, the paths must be composed at runtime rather than dropped, since the served bytes are the contract.
  Traces: amp-scraper-error-paths-and-defaults#task#1

Cause 3 of 6: DescribeScraperLoggingConfiguration's not-found names the scraper instead of the scraper logging configuration.
  AWS: Smithy: DescribeScraperLoggingConfiguration (GET /scrapers/{scraperId}/logging-configuration, 200) models ResourceNotFoundException @httpError 404, whose members message, resourceId and resourceType are all required. Recorded behavior: when no scraper logging configuration resolves for the requested scraperId, the refusal is 404 ResourceNotFoundException with message "Scraper logging configuration not found." — fixed text, no variable parts, the scraperId is not interpolated — and resourceType "AWS::APS::ScraperLoggingConfiguration", the CloudFormation type name of the resource the URI addresses.
  Emulator: same status and error code, but message 'Scraper not found.' and resourceType 'AWS::APS::Scraper' — the parent resource's sentence and type rather than the logging configuration's.
  Why: resourceType is the machine-readable half of this error; a caller that reads it to decide which resource to create (a scraper, or a logging configuration on an existing scraper) is told the wrong family, and message-keyed assertions and log scrapers never match.
  Fix: the not-found refusal in DescribeScraperLoggingConfiguration's handler in the aps service must be composed for the logging-configuration family — its own sentence and AWS::APS::ScraperLoggingConfiguration — instead of reusing the scraper-lookup refusal.
  Traces: amp-scraper-error-paths-and-defaults#task#8

Cause 4 of 6: DeleteScraperLoggingConfiguration's not-found likewise reports the scraper, and its resourceId is the bare id instead of the request-scoped identifier the service reports.
  AWS: Smithy: DeleteScraperLoggingConfiguration (DELETE /scrapers/{scraperId}/logging-configuration, 202, @idempotent, clientToken bound to the clientToken query string) models ResourceNotFoundException @httpError 404 with required message, resourceId and resourceType. Recorded behavior: when the delete cannot resolve a scraper logging configuration, the refusal is 404 ResourceNotFoundException with message "Failed to delete the scraper logging configuration." — fixed text with no variable parts — resourceType "AWS::APS::ScraperLoggingConfiguration", and a resourceId that identifies the failing REQUEST rather than the bare id: the fixed form "DeleteScraperLoggingConfigurationRequest(scraperId=<scraperId>, clientToken=<clientToken>).scraperId/scraperlogging", whose two variable parts are the scraperId and clientToken taken from the request (the recorded id and token values fill that template, they are not part of it).
  Emulator: same status and code, but message 'Scraper not found.', resourceType 'AWS::APS::Scraper', and resourceId the bare scraperId.
  Why: the delete path is the one a caller drives idempotently, so it is the one whose refusal is most often parsed; reporting the parent family and a bare id makes it indistinguishable from a genuinely missing scraper and loses the clientToken correlation the service echoes back.
  Fix: the not-found refusal in DeleteScraperLoggingConfiguration's handler in the aps service must compose the delete-path sentence, the ScraperLoggingConfiguration resourceType, and the request-scoped resourceId template with scraperId and clientToken substituted.
  Traces: amp-scraper-error-paths-and-defaults#task#10

Cause 5 of 6: CreateAnomalyDetector's RCF-expression refusal uses an emulator-authored member-constraint sentence.
  AWS: Smithy: CreateAnomalyDetector (POST /workspaces/{workspaceId}/anomalydetectors, 202) takes required configuration (AnomalyDetectorConfiguration, member randomCutForest.query) and models ValidationException @httpError 400. Recorded behavior: a query whose root is not a supported aggregation operator is refused 400 ValidationException with the message "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator" — the whole sentence is fixed text with NO variable parts: it does not echo the rejected query, the member path, or the list of operators.
  Emulator: same status and code, but message 'Invalid configuration.randomCutForest.query: Member must be wrapped by one of the supported PromQL aggregation operators (avg, count, group, max, min, quantile, stddev, stdvar, sum)' — a generic 'Invalid <member path>: Member must ...' constraint template that enumerates operators the service never names in this message.
  Why: the refusal is presented as a client-side member-constraint failure rather than the service's semantic rejection, so callers mis-attribute it to SDK input validation, and any assertion on the service's documented sentence fails.
  Fix: CreateAnomalyDetector's configuration validation in the aps service must emit the service's fixed sentence for this class instead of routing through the shared 'Invalid <member>: Member must ...' message builder. Also described, from the behavioral corpus, as cause 1 of open issue 20261029T190216Z-amp-6-refusal-message-divergences-every-rejection-text.md; this is its wire binding.
  Traces: amp-anomaly-detector-workflow#task#6

Cause 6 of 6: DescribeWorkspace's not-found sentence carries a trailing period the read path does not use.
  AWS: Smithy: DescribeWorkspace (GET /workspaces/{workspaceId}, 200, @readonly) models ResourceNotFoundException @httpError 404. Recorded behavior: for a workspaceId that does not resolve, the read is refused 404 ResourceNotFoundException with the message "Workspace not found" — fixed text with NO trailing period and no interpolated workspaceId. The write path (DeleteWorkspace) uses the spelling WITH the period, so the two are distinct texts in the service.
  Emulator: answers 'Workspace not found.' on the read path too — the write path's spelling — so the read-path text diverges by its trailing period.
  Why: the two spellings are how a caller tells which path refused; collapsing them to one makes that distinction unreproducible and breaks exact-match assertions on the read path.
  Fix: the workspace not-found refusal used by DescribeWorkspace in the aps service must use the read-path spelling, keeping the delete path's. The inline duplication of this refusal across handlers (open issue 20261029T150024Z-amp-createanomalydetector-maintainability.md) is why one sentence is serving both paths. Also described, from the behavioral corpus, as cause 5 of open issue 20261029T190216Z-amp-6-refusal-message-divergences-every-rejection-text.md; this is its wire binding.
  Traces: amp-workspace-lifecycle-and-configuration#task#2

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 41 steps, 35 passed, 6 mismatched, score 85.4%; emulator_up=true, bootstrap_signal=false). All six clusters carry empty stub_path and empty owned_by.

Cause 1 — fixture amp-scraper-error-paths-and-defaults, step task#4, CreateScraper [mismatch]
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.message: expected "The subnet ID 'subnet-<id>' does not exist (Service: Ec2, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: 1)", got 'Workspace not found.'
  (The <uuid> in the recorded tail is a per-run request id and is not asserted as a requirement here; the 400 status, the ValidationException code and the "The subnet ID '<id>' does not exist" sentence are.)

Cause 2 — fixture amp-scraper-error-paths-and-defaults, step task#1, GetDefaultScraperConfiguration [mismatch]
  status 200 matched; $.configuration: value_mismatch. The expected base64 blob decodes to the published default scrape configuration, whose cadvisor job carries
    authorization:
      credentials_file: /var/run/secrets/kubernetes.io/serviceaccount/token
  and whose kubernetes-apiservers job carries
    bearer_token_file: /var/run/secrets/kubernetes.io/serviceaccount/token
  Neither member is present in the constant the emulator serves (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/get_default_scraper_configuration.rs:92-145; the doc comment at lines 82-88 records the omission as deliberate, and the handler returns that constant at lines 223-225).

Cause 3 — fixture amp-scraper-error-paths-and-defaults, step task#8, DescribeScraperLoggingConfiguration [mismatch]
  status 404 and code ResourceNotFoundException matched.
  $.message: expected 'Scraper logging configuration not found.', got 'Scraper not found.'
  $.resourceType: expected 'AWS::APS::ScraperLoggingConfiguration', got 'AWS::APS::Scraper'

Cause 4 — fixture amp-scraper-error-paths-and-defaults, step task#10, DeleteScraperLoggingConfiguration [mismatch]
  status 404 and code ResourceNotFoundException matched.
  $.message: expected 'Failed to delete the scraper logging configuration.', got 'Scraper not found.'
  $.resourceId: expected 'DeleteScraperLoggingConfigurationRequest(scraperId=<scraperId>, clientToken=<clientToken>).scraperId/scraperlogging' (recorded as scraperId=s-00000000-0000-0000-0000-000000000000, clientToken=1e951cce-bf10-4459-9696-4c091d5c4fed — those are the request's values filling the template), got the bare scraperId
  $.resourceType: expected 'AWS::APS::ScraperLoggingConfiguration', got 'AWS::APS::Scraper'

Cause 5 — fixture amp-anomaly-detector-workflow, step task#6, CreateAnomalyDetector [mismatch]
  status 400 and code matched.
  $.message: expected "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator", got 'Invalid configuration.randomCutForest.query: Member must be wrapped by one of the supported PromQL aggregation operators (avg, count, group, max, min, quantile, stddev, stdvar, sum)'

Cause 6 — fixture amp-workspace-lifecycle-and-configuration, step task#2, DescribeWorkspace [mismatch]
  status 404 and code ResourceNotFoundException matched.
  $.message: expected 'Workspace not found', got 'Workspace not found.'

## Steps to Reproduce
Replay the whole corpus against the emulator to see all six at once; each cause is reproducible on its own as follows.

Cause 1 (amp-scraper-error-paths-and-defaults#task#4): POST /scrapers with a valid base64 scrapeConfiguration.configurationBlob, a source whose subnetIds name a subnet that does not exist, and a destination workspace. Read the status and x-amzn-errortype: the emulator answers 404/ResourceNotFoundException with 'Workspace not found.' where the service answers 400/ValidationException naming the missing subnet id.

Cause 2 (amp-scraper-error-paths-and-defaults#task#1): GET /scraperconfiguration, base64-decode $.configuration and diff it against the default scrape configuration published under "Scraper configuration" in the Amazon Managed Service for Prometheus User Guide — the cadvisor job's authorization.credentials_file and the kubernetes-apiservers job's bearer_token_file are absent.

Cause 3 (amp-scraper-error-paths-and-defaults#task#8): GET /scrapers/{scraperId}/logging-configuration for a scraperId that resolves to no logging configuration — read $.message and $.resourceType: 'Scraper not found.' / 'AWS::APS::Scraper' instead of 'Scraper logging configuration not found.' / 'AWS::APS::ScraperLoggingConfiguration'.

Cause 4 (amp-scraper-error-paths-and-defaults#task#10): DELETE /scrapers/{scraperId}/logging-configuration?clientToken=<token> for a scraperId that resolves to no logging configuration — read $.message, $.resourceId and $.resourceType: 'Scraper not found.' / bare scraperId / 'AWS::APS::Scraper' instead of 'Failed to delete the scraper logging configuration.' / the DeleteScraperLoggingConfigurationRequest(...).scraperId/scraperlogging identifier / 'AWS::APS::ScraperLoggingConfiguration'.

Cause 5 (amp-anomaly-detector-workflow#task#6): create a workspace and wait until ACTIVE, then POST /workspaces/{workspaceId}/anomalydetectors with configuration.randomCutForest.query set to an expression whose root is not an aggregation operator (e.g. a bare selector). The status/code are right; $.message is the 'Invalid configuration.randomCutForest.query: Member must ...' template instead of the service's "Unsupported PromQL expression for RandomCutForest, ..." sentence.

Cause 6 (amp-workspace-lifecycle-and-configuration#task#2): GET /workspaces/{workspaceId} for a workspaceId that does not exist — $.message is 'Workspace not found.' where the read path answers 'Workspace not found' with no trailing period.
