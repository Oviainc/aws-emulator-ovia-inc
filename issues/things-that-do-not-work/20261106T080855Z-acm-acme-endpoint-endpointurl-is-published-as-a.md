Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-endpoint-directory-url-immutable-server-generated acm.acme-endpoint-directory-url-managed-by-service acm.acme-endpoint-server-managed-directory-url acm.describe-acme-endpoint-returns-server-managed-urls
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.acme-endpoint-directory-url-immutable-server-generated|_assert_directory_url#7|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.acme-endpoint-directory-url-managed-by-service|_assert_directory_url#6|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.acme-endpoint-server-managed-directory-url|_assert_directory_url#12|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Key: acm.describe-acme-endpoint-returns-server-managed-urls|_assert_directory_url#11|AssertionError|assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:]), (
Issue-Commit: 31743e3f01dbef30663c2400e173e53480be5db4

# ACM ACME endpoint EndpointUrl is published as a path-prefixed /_aws/... URL on the emulator's own listener host, not as the acm-acme-enroll.<region>.<domain>/<id>/directory directory URL AWS assigns (1 cause)

> _Note: this issue was identified against the code base at commit `31743e3f01dbef30663c2400e173e53480be5db4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAcmeEndpoint

## Description
Cause 1 of 1: the server-managed ACME directory URL (AcmeEndpoint.EndpointUrl, returned by DescribeAcmeEndpoint and minted by CreateAcmeEndpoint) has the wrong shape. Failing checks: acm.acme-endpoint-directory-url-immutable-server-generated, acm.acme-endpoint-directory-url-managed-by-service, acm.acme-endpoint-server-managed-directory-url, acm.describe-acme-endpoint-returns-server-managed-urls.

AWS: recorded behavior only — the Smithy model documents AcmeEndpoint.EndpointUrl as only "<p>The URL of the ACME endpoint.</p>" (target String, no pattern), and CreateAcmeEndpointRequest/UpdateAcmeEndpoint expose no EndpointUrl input member at all, so the value is entirely service-assigned. The recordings show that assigned value is an https URL whose authority is the regional ACME enrollment endpoint and whose path is only the endpoint id plus the ACME directory segment: `https://acm-acme-enroll.<region>.<domain>/<endpoint-id>/directory`. Fixed parts of that template: the scheme `https`, the leading DNS label `acm-acme-enroll`, the second label being the request's region, at least one further domain label, and the trailing path segment `directory`; variable parts: the region (from the request/endpoint), the remaining domain labels (the deployment's own domain), and `<endpoint-id>` (the server-minted id of this endpoint). No query string and no fragment.

Emulator: EndpointUrl is built as the emulator's own listener authority with the service label and region folded into a `/_aws/` path prefix — e.g. `https://<listener-host>:<port>/_aws/acm-acme-enroll/<region>/<endpoint-id>/directory`. The hostname therefore carries no `acm-acme-enroll` label and no region label, and the path has three extra leading segments (`/_aws`, `/acm-acme-enroll`, `/<region>`) before `/<endpoint-id>/directory`. The value is otherwise stable across repeated Describes and distinct per endpoint, so only the URL's composition diverges, not its lifecycle. (The specific host and port in any one run are per-environment and are not the finding; the missing service/region DNS labels and the `/_aws/...` path prefix are.)

Why: an ACME client is handed the directory URL and treats it as the service's own directory: it derives the enrollment host from the URL authority, and resolves newNonce/newAccount/newOrder relative to the directory's path. Because the authority announces no `acm-acme-enroll.<region>` host and the directory sits three segments deeper than AWS places it, any client that routes, pins, matches a TLS/SNI name, builds its own `/<id>/directory` URL from the endpoint id and region, or resolves ACME resources relative to the documented path layout targets a URL that does not exist here. A caller that only round-trips the opaque string still works, which is exactly why this stays silent until a real ACME client is pointed at the endpoint.

Fix: the ACME-endpoint URL construction in the acm crate — the EndpointUrl built when CreateAcmeEndpoint mints the endpoint and stored in/served from the endpoint's state and echoed by DescribeAcmeEndpoint. It must compose the URL from the service label `acm-acme-enroll`, the endpoint's region and the deployment's domain as the authority, with the path being exactly `/<endpoint-id>/directory`, rather than appending `/_aws/acm-acme-enroll/<region>` to the listener's own base URL.

Traces: the four checks above; each asserts the hostname's first two labels are ["acm-acme-enroll", <region>] and that the path matches `/<id>/directory` with nothing after it, and all four fail on the same assertion with the same `/_aws/...` URL shape.

## Evidence
Expected (AWS, from the recordings): EndpointUrl = https://acm-acme-enroll.us-west-2.<domain>/<endpoint-id>/directory — scheme https, DNS labels [acm-acme-enroll, us-west-2, ...], path /<endpoint-id>/directory, no query, no fragment.

Actual (emulator): EndpointUrl = 'https://127.0.0.1:44251/_aws/acm-acme-enroll/us-west-2/6fcc92c2-c12a-4e19-bb89-c92fb003dee7/directory' (and, in the other runs, 'https://127.0.0.1:38495/_aws/acm-acme-enroll/us-west-2/0e00484b-3391-4807-acb2-19f6e6a1dd46/directory', 'https://127.0.0.1:36917/_aws/acm-acme-enroll/us-west-2/65775084-50a9-4393-992d-a9737eee21cd/directory', 'https://127.0.0.1:38503/_aws/acm-acme-enroll/us-west-2/45dcb3fa-9480-4c0a-a890-4e3c906007b4/directory'). The host/port and the uuid differ per run; the invariant divergence is that the authority has no acm-acme-enroll/<region> labels and the path is prefixed with /_aws/acm-acme-enroll/<region>.

Replay assertion (identical in all four checks):
  assert labels[:2] == ["acm-acme-enroll", region] and len(labels) > 2 and all(labels[2:])
  AssertionError: EndpointUrl '...' must be on acm-acme-enroll.us-west-2.<domain>

Status codes are 200 throughout — CreateAcmeEndpoint/DescribeAcmeEndpoint succeed and the endpoint reaches ACTIVE; only the URL representation diverges.

## Steps to Reproduce
1. acm.create_acme_endpoint(AuthorizationBehavior="PRE_APPROVED", Contact="NOT_REQUIRED", CertificateAuthority={"PublicCertificateAuthority": {}}, Tags=[{"Key":"Name","Value":"<unique>"}]) -> AcmeEndpointArn.
2. Poll acm.describe_acme_endpoint(AcmeEndpointArn=arn)["AcmeEndpoint"] until Status == ACTIVE.
3. Read AcmeEndpoint.EndpointUrl and urlsplit() it: the hostname's first two labels are not ["acm-acme-enroll", <region>] (they are the emulator's listener host) and the path is /_aws/acm-acme-enroll/<region>/<id>/directory instead of /<id>/directory.
4. Re-run the four behavioral checks: acm.acme-endpoint-directory-url-immutable-server-generated, acm.acme-endpoint-directory-url-managed-by-service, acm.acme-endpoint-server-managed-directory-url, acm.describe-acme-endpoint-returns-server-managed-urls.
