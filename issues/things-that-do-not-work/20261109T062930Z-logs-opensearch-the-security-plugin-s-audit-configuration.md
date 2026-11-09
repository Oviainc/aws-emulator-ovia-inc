Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle
Issue-Key: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: dd6dc626874166bf5c09c57665a1e26f5c7b5618

# logs__opensearch: the security plugin's audit-configuration API is unrouted — GET /_opendistro/_security/api/audit answers 400 'no handler found', so audit logging can never be configured and AUDIT_LOGS records never arrive (1 cause)

> _Note: this issue was identified against the code base at commit `dd6dc626874166bf5c09c57665a1e26f5c7b5618`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
One net-new root cause. Related open issues cover the CONTROL-plane half only — 20261102T090835Z-logs-opensearch-opensearch-createdomain-never-validates-logpublishingoptions-the-legacy-es.md (CreateDomain does not validate LogPublishingOptions) and 20261029T182036Z-logs-opensearch-createdomain-accepts-logpublishingoptions-naming-a-log.md — neither covers this data-plane route, and this check carries neither of their replay keys.

Cause 1 of 1 — the security plugin's audit-configuration resource is not registered on the domain's REST data plane.
AWS: Smithy: LogType AUDIT_LOGS is documented "Audit logs contain records of user requests for access to the domain"; on a fine-grained-access-control domain WHICH requests are audited is read and written through the security plugin's own REST API on DomainStatus$Endpoint (Smithy: "Domain-specific endpoint used to submit index, search, and data upload requests to the domain"). `GET /_plugins/_security/api/audit` — and its legacy alias `GET /_opendistro/_security/api/audit`, which the opendistro-era tooling and every existing setup script still use — answers HTTP 200 with `{"_readonly":[...],"config":{"enabled":<bool>,"audit":{"enable_rest":<bool>,"enable_transport":<bool>,"disabled_rest_categories":[...],"disabled_transport_categories":[...],...},"compliance":{...}}}`, and `PATCH` on the same path with a JSON-Patch array (`[{"op":"replace","path":"/config/audit/enable_rest","value":true}, ...]`) answers HTTP 200 and makes the change effective; re-reading then shows the patched values, so the step is idempotent. The REST data plane is not in the Smithy model, so recorded behavior only for the emulator's status and wording.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_opendistro/_security/api/audit] and method [GET]"}`. Fixed text: "no handler found for uri [", "] and method ["; variable parts: the request URI and the request method. The body also carries no engine error envelope (`{"error":{"type":...,"reason":...},"status":400}`), so a caller branching on `error.type` reads nothing. The PATCH half of the same path is never reached because the check stops on the GET; the route is simply not registered for either method.
Why: this is the only API by which a caller enables REST and transport auditing and clears the disabled-category lists. With it refused, (a) the documented audit configuration is unreadable, so an idempotent setup step cannot tell "already enabled" from "needs patching", and (b) no audit record is ever produced for a document request, so the AUDIT_LOGS CloudWatch group the domain names stays empty and a client cannot show that a request was authenticated and authorized. The failure is a bare 400 with no error type, so it is also unclassifiable.
Fix: the domain REST data-plane router in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_rest.rs (which already registers the security plugin's role and role-mapping API) — register the audit-configuration resource for GET and PATCH under both `/_plugins/_security/api/audit` and the legacy `/_opendistro/_security/api/audit`, backed by per-domain audit configuration in that crate's fgac/security state, and have the audit configuration gate the audit records emitted to the AUDIT_LOGS log group.
Traces: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle (fails in phase 2/5 `setup`)

## Evidence
Expected: GET <endpoint>/_opendistro/_security/api/audit -> HTTP 200 with a `config` object exposing `enabled` and `audit.enable_rest` / `audit.enable_transport` / `audit.disabled_rest_categories` / `audit.disabled_transport_categories`, then PATCH on the same path -> HTTP 200, and a second read reporting the patched configuration unchanged.
Actual: `[phase 2/5 setup] expected the audit config to be readable, got 400: {'error': 'no handler found for uri [/_opendistro/_security/api/audit] and method [GET]'}` — the CloudFormation-deployed FGAC domain answered GET / with 200 just before, so the data plane is reachable; only this route is missing. Downstream, the check's audit assertions (one REST AUTHENTICATED record and one GRANTED_PRIVILEGES transport record per document GET, indistinguishable between the 200 and the 404 GET, with no status-bearing field) are never reachable.

## Steps to Reproduce
1. Deploy the stack's FGAC OpenSearch domain (LogPublishingOptions enabling AUDIT_LOGS, ES_APPLICATION_LOGS, SEARCH_SLOW_LOGS and INDEX_SLOW_LOGS, each to its own CloudWatch log group) and wait until a signed GET <endpoint>/ answers 200.
2. SigV4-sign ('es') and send GET <endpoint>/_opendistro/_security/api/audit -> HTTP 400 {'error': 'no handler found for uri [/_opendistro/_security/api/audit] and method [GET]'} instead of 200 with the audit config. The same holds for the current path /_plugins/_security/api/audit.
3. Send PATCH <endpoint>/_opendistro/_security/api/audit with [{"op":"replace","path":"/config/enabled","value":true},{"op":"replace","path":"/config/audit/enable_rest","value":true},{"op":"replace","path":"/config/audit/enable_transport","value":true},{"op":"replace","path":"/config/audit/disabled_rest_categories","value":[]},{"op":"replace","path":"/config/audit/disabled_transport_categories","value":[]}] -> the same unregistered-route refusal.
4. Index a document (PUT <endpoint>/<index>/_doc/<id>?refresh=true), GET it, and GET an unseeded id, then FilterLogEvents on the AUDIT_LOGS group -> no audit record for either GET.
