Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a0bcc3233750d466bb98593c36c54004977c8656

# The not-found refusal echoes the whole 2048-character virtualClusterIdentifier/endpointIdentifier, so ResourceNotFoundException.message runs past its modelled 1024-character bound

_Filed: 2026-10-29T05:15:13Z_
_Operation: GetManagedEndpointSessionCredentials | Class: error-response_

> _Note: this issue was identified against the code base at commit `a0bcc3233750d466bb98593c36c54004977c8656`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This is the one operation of the service whose resource identifiers are `String2048` (length 1..2048) rather than a `ResourceIdString` (length 1..64). Both not-found refusals pass that identifier straight into `ResourceKind::not_found_message`, which interpolates it whole into the message of the modelled `ResourceNotFoundException` — a member the model bounds at 1024 characters. An identifier longer than 993 characters (`"Virtual cluster "` + identifier + `" doesn't exist."` = 31 + len) therefore produces an error whose `message` cannot be held by the shape the model declares for it, and no truncation is applied anywhere in the crate.

Every OTHER caller of `not_found_message` in this crate passes a `ResourceIdString` (max 64) or an ARN this service built (well under 1024), so the shared wording stays inside the bound for them; this handler is the single site where the identifier it is given is itself allowed to be twice the message bound.

## Evidence

Model (smithy_query query="shape" shape="ResourceNotFoundException"): `"message": {"target": "String1024", "length": {"min": 1, "max": 1024}, "pattern": "\\S"}`.

Model (smithy_query query="shape" shape="GetManagedEndpointSessionCredentialsRequest"): `"endpointIdentifier": {"target": "String2048", "httpLabel": true, "required": true, "length": {"min": 1, "max": 2048}, "pattern": "\\S"}` and `"virtualClusterIdentifier": {"target": "String2048", "httpLabel": true, "required": true, "length": {"min": 1, "max": 2048}, "pattern": "\\S"}` — so the boundary admits a 2048-character identifier and hands it to the handler.

Handler, both refusals built from that identifier with no length guard:
- services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/get_managed_endpoint_session_credentials.rs:216 — `return Err(cluster_not_found(&input.virtual_cluster_identifier));`
- .../get_managed_endpoint_session_credentials.rs:367-373 — `fn cluster_not_found(identifier: &str) -> ... ResourceNotFoundException { message: Some(crate::state::ResourceKind::VirtualCluster.not_found_message(identifier)) }`
- .../get_managed_endpoint_session_credentials.rs:222 and :381-388 — the same for `endpoint_not_found(&input.endpoint_identifier)`.

The shared wording that does the interpolation, services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/state.rs:687-689:
```
pub fn not_found_message(self, identifier: &str) -> String {
    format!("{} {identifier} doesn't exist.", self.resource_type())
}
```

Nothing in the crate justifies the overflow, and the suite that pins this wording only uses short identifiers — tests/all/not_found_message.rs:108-115 spans "both published length bounds of `ResourceIdString`" (`"z".repeat(64)`) and tests/all/not_found_message.rs:539-575 passes this operation an ARN this service built (~90 characters), so bounding the message leaves every existing assertion unchanged.

## Steps to Reproduce

1. Create a virtual cluster and a managed endpoint so the harness is live (not required for the refusal, but shows the handler is reached).
2. POST `/virtualclusters/<1500 'a' characters>/endpoints/someendpoint/credentials` with body `{"executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole", "credentialType": "TOKEN"}` — a 1500-character `virtualClusterIdentifier` satisfies `String2048` (1..2048, pattern `\S`), so the boundary admits it.
3. Observe the 400 ResourceNotFoundException: `message` is `"Virtual cluster <1500 chars> doesn't exist."` — 1531 characters, past the 1024-character maximum the model declares for that member.
4. Same for the endpoint: POST `/virtualclusters/{realCluster}/endpoints/<1500 characters>/credentials`.
