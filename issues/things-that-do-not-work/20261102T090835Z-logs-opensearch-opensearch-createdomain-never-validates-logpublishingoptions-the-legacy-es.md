Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.osdocstat-access-log-type-rejected logs__opensearch.osdocstat-audit-log-publishing-without-fgac-rejected logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected logs__opensearch.osdocstat-ungranted-group-rejected-until-account-policy-grants logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status logs__opensearch.osdocstat-legacy-es-api-reports-the-same-log-publishing logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle
Issue-Key: logs__opensearch.osdocstat-access-log-type-rejected|run#6|AssertionError|assert refused is not None, (
Issue-Key: logs__opensearch.osdocstat-audit-log-publishing-without-fgac-rejected|run#10|AssertionError|assert refused is not None, (
Issue-Key: logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group|run#18|AssertionError|assert refused is not None and refused[0] == "ValidationException", (
Issue-Key: logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected|run#13|AssertionError|assert refused is not None, (
Issue-Key: logs__opensearch.osdocstat-ungranted-group-rejected-until-account-policy-grants|run#13|AssertionError|assert refused is not None, (
Issue-Key: logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status|run#15|AssertionError|assert status == 200, f"expected the audit config PATCH to succeed, got {status}: {body!r}"
Issue-Key: elasticsearch-service|DescribeElasticsearchDomain|ResourceNotFoundException
Issue-Key: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# OpenSearch CreateDomain never validates LogPublishingOptions, the legacy ES describe cannot see an opensearch domain, and the domain's security-audit endpoint is unhandled (4 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Four net-new root causes. Each must be closed before submitting.

Cause 1 of 4 — CreateDomain accepts every invalid LogPublishingOptions configuration instead of refusing it with ValidationException.
AWS: Smithy (com.amazonaws.opensearch) CreateDomain, member LogPublishingOptions: map of LogType to LogPublishingOption; models ValidationException. The model carries no message text, so for the text this is recorded behavior only. At create, AWS validates the whole block and refuses the domain with ValidationException — creating nothing — in each of these cases: (a) a LogType the service does not vend for a domain (`ACCESS_LOGS`), (b) `AUDIT_LOGS` requested while the domain has no advanced security (fine-grained access control) enabled, (c) a CloudWatchLogsLogGroupArn naming a log group that does not exist, and (d) a log group that exists but has not granted the OpenSearch service principal permission to write to it (the create probes each named group, and the same create succeeds once an account-level resource policy grants it).
Emulator: accepts all four, returning a created domain, so nothing distinguishes a correctly-granted logging configuration from an impossible one.
Why: this validation at create is the only place a caller learns its logging wiring is wrong; accepted, the domain exists with log publishing that can never deliver, and the operator sees empty log groups with no error to explain them. The grant probe in particular is what makes a least-privilege setup verifiable — accepting an ungranted group makes every such test pass vacuously.
Fix: the LogPublishingOptions validation in CreateDomain's (and UpdateDomainConfig's) validation layer in the opensearch crate: reject unsupported log types, require advanced security for AUDIT_LOGS, resolve each CloudWatchLogsLogGroupArn, and evaluate the destination group's resource policy for the service principal. Note: open issues 20261029T182036Z-logs-opensearch-createdomain-accepts-logpublishingoptions-naming-a-log.md and 20261025T024958Z-logs-opensearch-opensearch-updatedomainconfig-accepts-logpublishingoptions-naming-a-cloudwatch-log.md record parts of this family and carry none of these five checks' keys.
Traces: logs__opensearch.osdocstat-access-log-type-rejected, logs__opensearch.osdocstat-audit-log-publishing-without-fgac-rejected, logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group, logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected, logs__opensearch.osdocstat-ungranted-group-rejected-until-account-policy-grants

Cause 2 of 4 — the domain endpoint has no handler for the security audit-configuration API, so audit logging cannot be configured or exercised.
AWS: recorded behavior only (a domain-hosted REST API, not in the Smithy model). On a domain with fine-grained access control, `PATCH /_opendistro/_security/api/audit` is served and applies the audit configuration, after which document requests are recorded in the AUDIT_LOGS group.
Emulator: answers HTTP 400 with `{'error': 'no handler found for uri [/_opendistro/_security/api/audit] and method [PATCH]'}`.
Why: without that endpoint the audit configuration cannot be set at all, so audit-log content (and the documented omission of a response status from an audit record) is unobservable, and a client configuring FGAC auditing gets a 400 that looks like a URL mistake.
Fix: the domain data-plane router in the opensearch crate — serve the _opendistro/_security/api/audit resource (GET/PATCH) and apply the configuration to audit-log emission.
Traces: logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status

Cause 3 of 4 — the legacy Elasticsearch describe cannot see a domain created through the OpenSearch API.
AWS: Smithy (com.amazonaws.elasticsearchservice) DescribeElasticsearchDomain; recorded behavior only for the equivalence. A domain is one resource exposed by two API surfaces: a domain created with opensearch CreateDomain is described by DescribeElasticsearchDomain, which reports the same LogPublishingOptions.
Emulator: answers ResourceNotFoundException `Domain not found: <domainName>` for a domain that opensearch DescribeDomain resolves — the two surfaces do not share the domain store.
Why: callers still on the legacy es client (older SDKs, Terraform's aws_elasticsearch_domain, existing automation) cannot see domains the OpenSearch API created, so the same resource appears to exist or not depending on which client asks.
Fix: back DescribeElasticsearchDomain (and the other es read operations) with the same domain state the opensearch crate writes, instead of a separate store.
Traces: logs__opensearch.osdocstat-legacy-es-api-reports-the-same-log-publishing

Cause 4 of 4 — the document-request status-log lifecycle never terminates.
AWS: recorded behavior only. A domain with index/search/application log publishing enabled writes records for document requests to the configured log groups within the check's polling window.
Emulator: the check's bounded poll never observes the expected records and the harness ends it with `Check timed out after 60.0s`.
Why: a client that enables OpenSearch log publishing and waits for records waits indefinitely, with no refusal or status to indicate that delivery is not happening.
Fix: the log-publishing delivery path from the domain data plane into the configured CloudWatch Logs groups in the opensearch crate.
Traces: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1:
  osdocstat-access-log-type-rejected: AssertionError: expected CreateDomain with a LogPublishingOptions key 'ACCESS_LOGS' to be REJECTED, but it was accepted
  osdocstat-audit-log-publishing-without-fgac-rejected: AssertionError: expected CreateDomain with AUDIT_LOGS and no advanced security to be REJECTED, but it was accepted
  osdocstat-log-publishing-to-nonexistent-group-at-create-rejected: AssertionError: expected CreateDomain publishing to the nonexistent group arn:aws:logs:<region>:<account>:log-group:<name> to be REJECTED, but it was accepted
  osdocstat-ungranted-group-rejected-until-account-policy-grants: AssertionError: expected CreateDomain publishing to the ungranted group <name> to be REJECTED, but it was accepted
  osdocstat-create-domain-probes-each-granted-log-group: AssertionError: expected CreateDomain to the ungranted group <name> to be refused with ValidationException, got None

Cause 2 — osdocstat-audit-log-records-document-get-without-response-status:
  AssertionError: expected the audit config PATCH to succeed, got 400: {'error': 'no handler found for uri [/_opendistro/_security/api/audit] and method [PATCH]'}

Cause 3 — osdocstat-legacy-es-api-reports-the-same-log-publishing (verdict: error):
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeElasticsearchDomain operation: Domain not found: <domainName>

Cause 4 — e2e-osdocstat-document-request-status-logs-lifecycle:
  Error: Check timed out after 60.0s

Domain names, group names and ARNs above are this run's values; the expected ValidationException refusals and the shared-domain equivalence are the requirements.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: aws opensearch create-domain --domain-name d --log-publishing-options 'ACCESS_LOGS={CloudWatchLogsLogGroupArn=<arn of an existing granted group>,Enabled=true}' -> accepted; AWS answers ValidationException. Repeat with AUDIT_LOGS and no --advanced-security-options, with an ARN naming a nonexistent group, and with an existing group that has no logs resource policy granting es.amazonaws.com/opensearchservice — all four are accepted here and all four are refused by AWS; granting the group by account policy must then let the create succeed.

Cause 2: on a domain with FGAC, PATCH https://<domain endpoint>/_opendistro/_security/api/audit with an audit configuration -> 400 'no handler found for uri ...'; AWS applies it.

Cause 3: aws opensearch create-domain --domain-name d ... ; aws es describe-elasticsearch-domain --domain-name d -> ResourceNotFoundException 'Domain not found: d'; AWS describes the same domain.

Cause 4: enable index/search/application log publishing on a domain, issue document GETs against it, and poll the configured log groups -> no records arrive.

Then re-replay the eight check ids in trace_ids.
