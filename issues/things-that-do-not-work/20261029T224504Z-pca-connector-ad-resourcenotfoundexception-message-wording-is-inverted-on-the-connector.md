Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-delete-connector-idempotent-not-found
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-delete-connector-idempotent-not-found pca-connector-ad-template-lifecycle pca-connector-ad-template-validation-errors
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|GetConnector|4xx->4xx|$.Message
Issue-Key: pca-connector-ad|CreateTemplate|4xx->4xx|$.Message
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# ResourceNotFoundException message wording is inverted on the connector read and template-create paths

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnector

## Description
Two net-new wire causes with one root: the handlers build the ResourceNotFoundException message with their own wording instead of this service's `<ResourceType> was not found: <identifier>.` sentence.

Shared AWS requirement: a not-found refusal from pca-connector-ad answers HTTP 404, code ResourceNotFoundException (Smithy: ResourceNotFoundException @httpError 404 with Message, ResourceId and ResourceType all @required; ResourceType documented as one of Connector, Template, TemplateGroupAccessControlEntry, ServicePrincipalName, DirectoryRegistration), and its Message is `<ResourceType> was not found: <identifier exactly as supplied in the request>.` — fixed text `<Type> was not found: `, then the request's identifier, then a trailing period. For a missing connector that is `Connector was not found: <ConnectorArn>.`. The message text is recorded behavior only; the status, class and members are the model's. Status, error code, ResourceId and ResourceType are already correct in both causes — only Message diverges.

Shared emulator behavior: handlers format `Connector {arn} not found` — subject and predicate reversed, no colon, no trailing period. See services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/get_connector.rs:169 (`format!("Connector {} not found", input.connector_arn)`) and the equivalent construction in src/ops/create_template.rs.

Shared Why: every client that surfaces, logs or matches the not-found message — the single most common failure of these operations — sees text AWS never emits, so error-mapping tables and user-facing diagnostics keyed on the AWS sentence do not fire.

Shared Fix: the ResourceNotFoundException message construction in each affected handler, normalized to the service's `<Type> was not found: <id>.` wording (get_connector.rs, create_template.rs, and any other handler that builds this error).

Cause 1 of 2: GetConnector on a well-formed ConnectorArn that does not exist (here, after an idempotent double DeleteConnector) answers 404 ResourceNotFoundException with the inverted message. Traces: fixture pca-connector-ad-delete-connector-idempotent-not-found, step task#4.

Cause 2 of 2: CreateTemplate against a well-formed but nonexistent ConnectorArn answers 404 ResourceNotFoundException with the same inverted message, for the same resource type. Traces: fixture pca-connector-ad-template-lifecycle, step task#1 (cluster also covers pca-connector-ad-template-validation-errors#task#2).

Related open issue (does NOT cover these wire steps): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md, Cause 6, reports the same wording fault from the behavioral corpus; it carries no wire replay key, so neither wire step above is gated by it.

## Evidence
Both steps: expected status 404 == actual 404; code ResourceNotFoundException in both; ResourceId and ResourceType matched; only $.Message diverges.

Cause 1 — pca-connector-ad-delete-connector-idempotent-not-found#task#4 (GetConnector):
  expected $.Message: 'Connector was not found: <ConnectorArn>.' (recorded: 'Connector was not found: arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid>.')
  actual   $.Message: 'Connector <ConnectorArn> not found' (recorded: 'Connector arn:aws:pca-connector-ad:us-west-2:<account>:connector/<uuid> not found')
  Fixed text expected: 'Connector was not found: ' + trailing '.'; variable part: the ConnectorArn from the request.

Cause 2 — pca-connector-ad-template-lifecycle#task#1 (CreateTemplate; same cluster covers pca-connector-ad-template-validation-errors#task#2):
  expected $.Message: 'Connector was not found: <the request's ConnectorArn>.'
  actual   $.Message: 'Connector <the request's ConnectorArn> not found'

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1, or drive it directly (region us-west-2):
1. Pick a well-formed ConnectorArn for a connector that does not exist (or create one, DeleteConnector it twice — both tolerant 202s are correct). Call GetConnector(ConnectorArn=<that ARN>) -> 404 ResourceNotFoundException with Message 'Connector <arn> not found'; AWS answers 'Connector was not found: <arn>.'. (pca-connector-ad-delete-connector-idempotent-not-found#task#4)
2. Call CreateTemplate(ConnectorArn=<a well-formed nonexistent connector ARN>, Name=<a Name that satisfies the modeled pattern>, Definition={'TemplateV2': {...}}) -> same message divergence on the 404. (pca-connector-ad-template-lifecycle#task#1)
