Issue-Source: trace-sweep-divergence
Issue-Trace: elasticsearch-service-domain-validation-and-instance-type-limits-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticsearch-service-domain-validation-and-instance-type-limits-errors elasticsearch-service-package-lifecycle es-osdocstat-traj-enable-request-log-type-on-update
Issue-Shape-Id: com.amazonaws.elasticsearchservice#AmazonElasticsearchService2015
Issue-Key: es|CreateElasticsearchDomain|4xx->4xx|$.message
Issue-Key: es|DeleteElasticsearchDomain|4xx->4xx|status
Issue-Key: es|GetPackageVersionHistory|4xx->4xx|$.message
Issue-Key: es|GetPackageVersionHistory|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|GetPackageVersionHistory|4xx->4xx|status
Issue-Key: es|UpdateElasticsearchDomainConfig|4xx->4xx|$.message
Issue-Key: es|UpdateElasticsearchDomainConfig|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|UpdateElasticsearchDomainConfig|4xx->4xx|status
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# es legacy 2015-01-01 refusals diverge: anchored pattern text, 409 not-found status, and validation ordered after the resource lookup

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateElasticsearchDomain

## Description
4 net-new root causes on the `es` service token (the Amazon Elasticsearch Service / OpenSearch Service control plane served on endpoint prefix `es`, legacy 2015-01-01 surface plus the shared /2015-01-01/packages surface). They share one component: the refusal path of the legacy operations — how a constraint violation is rendered, when it is evaluated relative to the resource lookup, and what HTTP status a not-found refusal gets. Each cause below is its own replay unit and must be re-replayed separately before this issue is closed.

RELATED OPEN WORK (does NOT cover any of these replay keys — none of their Issue-Key / Issue-Trace-Ids entries name these fixtures/steps): 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md (same formatter/ordering family, but on DescribeDomain/ListDomainNames/UpdateDomainConfig/DescribeElasticsearchDomain steps of other fixtures) and 20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md (the ResourceNotFoundException->409 status mapping, keyed on DeleteDomain/DescribeDomain and 26 other operations, not on DeleteElasticsearchDomain). A fix to either may close causes here, but no existing pointer replays these steps, so they are filed with their own bindings.

Cause 1 of 4 — CreateElasticsearchDomain quotes the domainName @pattern WITH its model anchors, where AWS quotes it unanchored (fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors, step task#3).
AWS: Smithy (com.amazonaws.elasticsearchservice): CreateElasticsearchDomain POST /2015-01-01/es/domain, input member DomainName @required targeting DomainName @length(min 3, max 28) @pattern("^[a-z][a-z0-9\\-]+$"); errors include ValidationException (client, 400), BaseException (client, 400, defaulted), ResourceAlreadyExistsException (409), LimitExceededException (409), InvalidTypeException (409), DisabledOperationException (409), InternalException (500). The recording is 400 ValidationException with the standard aggregate template and the regex quoted WITHOUT `^`/`$`: `1 validation error detected: Value '<submitted domain name>' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+`. Fixed text: `N validation error(s) detected: `, `Value '<submitted value>' at '<member path>' failed to satisfy constraint: `, `Member must satisfy regular expression pattern: <expression>`. Variable parts: the error count, the echoed submitted value (from the request), the lower-camel member path (`domainName`) and the quoted expression — which this surface renders stripped of its trait anchors.
Emulator: 400 ValidationException, same status and error class, same clause, but the expression is emitted as the raw model trait `^[a-z][a-z0-9\\-]+$`. Replay reports $.message as the only diverging key.
Why: the refusal message is the only place the API states the accepted domain-name syntax; a client that compiles the quoted regex to pre-validate names derives a different accept set than AWS states, and any assertion or log comparison on the refusal text fails.
Fix: the constraint-violation message formatter reached from the legacy POST /2015-01-01/es/domain validation entry point — it must render this member's pattern the way the service does (unanchored). Listed separately from the DescribeElasticsearchDomain/DescribeDomain read paths because this is the create (body-member) entry point; a fix to the read paths alone leaves this step failing.
Traces: elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#3.

Cause 2 of 4 — DeleteElasticsearchDomain on an absent domain answers HTTP 409 instead of 404 (fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors, step task#4).
AWS: Smithy: DeleteElasticsearchDomain DELETE /2015-01-01/es/domain/{DomainName}; errors ResourceNotFoundException (client, modeled @httpError(409)), ValidationException (400), BaseException (400, defaulted), InternalException (500). The model's status disagrees with the wire and must not be used as the source: the recorded not-found refusal for this error shape is HTTP 404 with x-amzn-errortype: ResourceNotFoundException and a JSON body carrying the not-found message. Recorded behavior is the requirement; the error code and message are unchanged by this cause, only the status line is wrong.
Emulator: answers HTTP 409 for the same absent-domain delete. Replay detail: `status: expected 404, got 409` (divergence_key service=es|DeleteElasticsearchDomain|status=404->409|keys=status).
Why: an SDK or client that branches on the HTTP status (404 -> already gone, 409 -> conflict, retry or abort) takes the wrong branch; delete-if-exists and wait-until-gone loops on the legacy surface never terminate and conflict-retry loops retry a permanent condition.
Fix: the service's error-to-HTTP-status mapping for ResourceNotFoundException as reached from the legacy 2015-01-01 delete handler. The related open issue above owns the same mapping for 28 OTHER operations but not for DeleteElasticsearchDomain, so this step needs its own replay.
Traces: elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#4.

Cause 3 of 4 — GetPackageVersionHistory resolves the package before validating packageID, so a malformed id is reported as a not-found conflict instead of a 400 ValidationException (fixture elasticsearch-service-package-lifecycle, step task#7).
AWS: Smithy: GetPackageVersionHistory GET /2015-01-01/packages/{PackageID}/history, input PackageID @httpLabel @required; the id's syntax is stated by the shared package resource model, com.amazonaws.opensearch#PackageID @pattern("^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$"). Errors: ValidationException (client, 400), ResourceNotFoundException (client, 409 modeled / 404 on the wire), AccessDeniedException (403), BaseException (400, defaulted), InternalException (500). The recording shows AWS refusing on INPUT CONSTRAINTS FIRST: 400 with `1 validation error detected: Value '<submitted package id>' at 'packageID' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$` — here the expression IS quoted with its anchors — and x-amzn-errortype asserted as shaped. Fixed text as in cause 1; variable parts are the echoed submitted id, the member path `packageID` and the quoted pattern.
Emulator: answers 409 with x-amzn-errortype: ResourceNotFoundException and message `Package not found: <submitted package id>`: the package-existence lookup runs before any input validation, so the packageID pattern is never evaluated and no ValidationException is produced. Replay detail: `status: expected 400, got 409`, `$.message: value_mismatch`, `$headers.x-amzn-errortype: shape_mismatch`.
Why: a caller that passes a malformed package id is told the package does not exist, so it creates or re-imports a package instead of correcting the one malformed member AWS named; the member path that pinpoints the mistake is lost. Note that fixing the not-found status mapping (cause 2) does not close this step — the expected response is not a not-found at all.
Fix: the operation's handler ordering — input/label validation must run before the package is resolved from state — plus the packageID pattern in the legacy packages validation layer.
Traces: elasticsearch-service-package-lifecycle#task#7.

Cause 4 of 4 — UpdateElasticsearchDomainConfig resolves the domain before validating logPublishingOptions, so an out-of-enum log type is reported as a domain-not-found conflict (fixture es-osdocstat-traj-enable-request-log-type-on-update, step task#5).
AWS: Smithy: UpdateElasticsearchDomainConfig POST /2015-01-01/es/domain/{DomainName}/config, input member LogPublishingOptions targeting the map LogPublishingOptions whose KEY targets the enum LogType with values [INDEX_SLOW_LOGS, SEARCH_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS]; errors ValidationException (client, 400), ResourceNotFoundException (409 modeled / 404 on the wire), InvalidTypeException (409), LimitExceededException (409), BaseException (400, defaulted), InternalException (500). The recording refuses on input constraints first: 400 ValidationException with the map-key form of the template — `1 validation error detected: Value '<submitted logPublishingOptions map, rendered as the service's Java map/record toString>' at 'logPublishingOptions' failed to satisfy constraint: Map keys must satisfy constraint: [Member must satisfy enum value set: [INDEX_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS, SEARCH_SLOW_LOGS]]` — and x-amzn-errortype asserted as shaped. Fixed text: `N validation error(s) detected: `, `Value '<...>' at 'logPublishingOptions' failed to satisfy constraint: `, `Map keys must satisfy constraint: [Member must satisfy enum value set: [<set>]]`. Variable parts: the echoed submitted map (built from the request's own keys, log-group ARN and enabled flag) and the bracketed accepted-value set. Recorded behavior only for the ORDER of that set: the recording emits [INDEX_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS, SEARCH_SLOW_LOGS], which is neither alphabetical nor the on-disk declaration order, so the service's recorded ordering is the requirement; note the 2021-01-01 sibling path additionally lists TASK_DETAILS_LOGS, so the two surfaces have different accepted sets.
Emulator: answers 409 with x-amzn-errortype: ResourceNotFoundException and message `Domain not found: <domain name>` — the domain lookup preempts body validation. Replay detail: `status: expected 400, got 409`, `$.message: value_mismatch`, `$headers.x-amzn-errortype: shape_mismatch`.
Why: a client enabling a log type it spelled wrong (e.g. ACCESS_LOGS) is told its domain is missing, so it re-creates or aborts rather than correcting the key; it also never learns the accepted log-type set, which the message is the only statement of.
Fix: the legacy UpdateElasticsearchDomainConfig handler's ordering (validate the request body before resolving the domain) plus the map-key enum branch of the constraint-violation formatter and the LogType accepted-value ordering it reads for the 2015-01-01 surface. The 2021-01-01 UpdateDomainConfig equivalent is owned by the related issues above; this legacy operation key is not in either of them.
Traces: es-osdocstat-traj-enable-request-log-type-on-update#task#5.

The domain names, package ids, log-group ARNs and generated identifiers quoted above are this recording's inputs filling the templates; the requirement is each template's fixed text plus the model trait / accepted-value list and ordering it quotes, with the variable parts taken from the request and the emulator's own state.

## Evidence
Cause 1 — elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#3, CreateElasticsearchDomain, expected 400 / actual 400, diverging key $.message only (divergence_key service=es|CreateElasticsearchDomain|status=400->400|keys=$.message):
  expected "1 validation error detected: Value 'Bad_Name!' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+"
  got      "1 validation error detected: Value 'Bad_Name!' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z][a-z0-9\\-]+$"
  Smithy cited: com.amazonaws.elasticsearchservice#DomainName @length(3,28) @pattern("^[a-z][a-z0-9\\-]+$"); CreateElasticsearchDomain POST /2015-01-01/es/domain, errors include ValidationException (400). The echoed 'Bad_Name!' is this recording's input; the requirement is the unanchored rendering of the expression.

Cause 2 — elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#4, DeleteElasticsearchDomain, expected 404 / actual 409 (divergence_key service=es|DeleteElasticsearchDomain|status=404->409|keys=status):
  replay detail: "status: expected 404, got 409" — status is the only diverging key; the error code (ResourceNotFoundException) and message match.
  Smithy cited: com.amazonaws.elasticsearchservice#ResourceNotFoundException httpError=409 (model), contradicted by the recording's 404.

Cause 3 — elasticsearch-service-package-lifecycle#task#7, GetPackageVersionHistory, expected 400 / actual 409 (divergence_key service=es|GetPackageVersionHistory|status=400->409|keys=$.message,$headers.x-amzn-errortype,status):
  expected status 400, body message "1 validation error detected: Value 'not-a-valid-id' at 'packageID' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$", x-amzn-errortype asserted as shaped
  got      status 409, body message 'Package not found: not-a-valid-id', x-amzn-errortype 'ResourceNotFoundException'
  Smithy cited: GetPackageVersionHistory GET /2015-01-01/packages/{PackageID}/history, PackageID @httpLabel @required, errors ValidationException(400) + ResourceNotFoundException; com.amazonaws.opensearch#PackageID @pattern("^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$").

Cause 4 — es-osdocstat-traj-enable-request-log-type-on-update#task#5, UpdateElasticsearchDomainConfig, expected 400 / actual 409 (divergence_key service=es|UpdateElasticsearchDomainConfig|status=400->409|keys=$.message,$headers.x-amzn-errortype,status):
  expected status 400, body message "1 validation error detected: Value '{ACCESS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:logs:us-west-2:000000000000:log-group:/aws/opensearch/domains/<domain>/audit-logs, enabled=true)}' at 'logPublishingOptions' failed to satisfy constraint: Map keys must satisfy constraint: [Member must satisfy enum value set: [INDEX_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS, SEARCH_SLOW_LOGS]]", x-amzn-errortype asserted as shaped
  got      status 409, body message 'Domain not found: <domain name>', x-amzn-errortype 'ResourceNotFoundException'
  Smithy cited: com.amazonaws.elasticsearchservice#LogPublishingOptions map with key -> LogType enum values [INDEX_SLOW_LOGS, SEARCH_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS] (declaration order differs from the recorded order); UpdateElasticsearchDomainConfig errors include ValidationException (400). The ARN and generated domain suffix in the echoed map are per-run inputs, not requirements.

## Steps to Reproduce
1. Replay fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors step task#3 (POST /2015-01-01/es/domain with a domainName violating the @pattern) and diff the `message` body member: the quoted regex must read [a-z][a-z0-9\-]+ with no ^/$ anchors.
2. Replay the same fixture step task#4 (DELETE /2015-01-01/es/domain/<absent domain>) and observe HTTP 409 where the recording is HTTP 404 with x-amzn-errortype: ResourceNotFoundException.
3. Replay fixture elasticsearch-service-package-lifecycle step task#7 (GET /2015-01-01/packages/not-a-valid-id/history) and observe 409 ResourceNotFoundException 'Package not found: ...' where the recording is 400 ValidationException naming 'packageID' and quoting ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$.
4. Replay fixture es-osdocstat-traj-enable-request-log-type-on-update step task#5 (POST /2015-01-01/es/domain/{DomainName}/config with LogPublishingOptions keyed ACCESS_LOGS) and observe 409 'Domain not found' where the recording is 400 ValidationException naming 'logPublishingOptions' and listing the accepted log types in the recorded order.
5. All four steps must pass before this issue is closed.
