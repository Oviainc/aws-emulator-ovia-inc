Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status
Issue-Key: logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# opensearch: the audit-log delivery scenario never converges — an FGAC domain created with AUDIT_LOGS LogPublishingOptions never becomes usable within the check's deadline

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: a CreateDomain that is ACCEPTED with FGAC and AUDIT_LOGS LogPublishingOptions never reaches a state where the scenario can proceed, so no audit record is ever delivered to the granted CloudWatch Logs group.

AWS: Smithy (opensearch) CreateDomain accepts LogPublishingOptions keyed by LogType (here AUDIT_LOGS) with CloudWatchLogsLogGroupArn + Enabled, and DescribeDomain reports Processing=false with an Endpoint once the domain is active. Recorded behavior: the accepted domain becomes active and serves its signed data plane (GET / answers 200), the security plugin's audit config can be PATCHed at /_opendistro/_security/api/audit to enable REST and transport auditing, and a document GET that finds (200) and one that misses (404) each produce an AUTHENTICATED REST record and a GRANTED_PRIVILEGES transport record in the configured log group, identical outside time/routing/id fields and carrying no status, response or code field.

Emulator: the check is killed at the suite's 180 s ceiling with no assertion reached past CreateDomain — nothing in the recorded trajectory (domain active with an endpoint, data plane answering 200, audit config PATCH, audit records in the log group) is observed. The emulator accepted the CreateDomain and then never advanced the scenario, so the audit-log claim is entirely unexercised. The check's own internal deadlines (domain active 1500 s, data plane 300 s, delivery 900 s) are all longer than the ceiling, so the evidence here is the absence of convergence, not a specific refusal.

Why: a client that creates an audit-logging domain and waits for it to become usable waits forever; no audit trail is ever delivered, so a security reader cannot prove a document GET was authorised.

Fix: the opensearch CreateDomain/DescribeDomain lifecycle (advancing an accepted domain to Processing=false with an Endpoint), its signed data-plane surface, and the AUDIT_LOGS publishing path that ships security-plugin audit records to the configured CloudWatch Logs group. Related open issues cover neighbouring causes but do not carry this check's replay key: 20261102T090835Z-logs-opensearch-opensearch-createdomain-never-validates-logpublishingoptions-the-legacy-es.md, 20261029T182036Z-logs-opensearch-createdomain-accepts-logpublishingoptions-naming-a-log.md and 20261025T024958Z-logs-opensearch-opensearch-updatedomainconfig-accepts-logpublishingoptions-naming-a-cloudwatch-log.md; this one is about the accepted audit-logging domain never becoming usable at all.

Traces: logs__opensearch.osdocstat-audit-log-records-document-get-without-response-status.

## Evidence
Expected: CreateDomain with FGAC and LogPublishingOptions={'AUDIT_LOGS': {'CloudWatchLogsLogGroupArn': <granted group arn>, 'Enabled': True}} is accepted; DescribeDomain then reports an Endpoint with Processing=false; a SigV4-signed GET / against that endpoint answers 200; PATCH /_opendistro/_security/api/audit answers 200 and reads back enabled=true with disabled_rest_categories == []; a GET of a seeded document (200) and of a missing one (404) each produce an AUTHENTICATED REST and a GRANTED_PRIVILEGES transport audit record in the log group.

Actual (emulator): Check timed out after 180.0s — no assertion after the accepted CreateDomain was reached; the domain never reported an endpoint/Processing=false, the data plane never answered, and no audit record reached the log group.

## Steps to Reproduce
1. Create a CloudWatch Logs group and put a resource policy granting es.amazonaws.com delivery to it for the domain ARN.
2. CreateDomain (OpenSearch_2.11, t3.small.search x1, gp3 10 GiB) with advanced security (FGAC) enabled and LogPublishingOptions {'AUDIT_LOGS': {'CloudWatchLogsLogGroupArn': <group arn>, 'Enabled': true}}; observe it is accepted.
3. Poll DescribeDomain for Processing=false with an Endpoint.
4. Expected: the domain becomes active, a signed GET / on its endpoint answers 200, PATCH /_opendistro/_security/api/audit enables auditing, and document GETs (200 and 404) land AUTHENTICATED/GRANTED_PRIVILEGES records in the log group.
5. Observed: step 3 never completes; the check is killed at the suite ceiling with nothing past CreateDomain exercised.
