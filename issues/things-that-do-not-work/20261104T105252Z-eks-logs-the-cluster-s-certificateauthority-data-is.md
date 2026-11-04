Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid
Issue-Key: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid|_k8s#9|SSLError|ssl_ctx = ssl.create_default_context(
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks__logs: the cluster's certificateAuthority.data is not a PEM certificate, so the audit-log check cannot reach the Kubernetes API at all

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: cluster.certificateAuthority.data is an emulator placeholder instead of a Base64-encoded PEM certificate, so no TLS client can verify or reach the cluster endpoint, and the audit-log behavior behind it is unobservable.
  AWS: Smithy: Certificate.data is documented as 'The Base64-encoded certificate data required to communicate with your cluster. Add this to the certificate-authority-data section of the kubeconfig file for your cluster.' Base64-decoding the member yields a PEM X.509 certificate that verifies the endpoint DescribeCluster advertises. With that CA in place, an authenticated kubectl request is served and the cluster's audit log records the access-entry username for both the allowed and the forbidden verb.
  Emulator: the member decodes to the literal text 'EMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this is not a ...' — not a PEM block. Loading it as a trust anchor fails: 'ssl.SSLError: no start line: cadata does not contain a certificate (_ssl.c:3977)' from ssl.create_default_context(...)/load_verify_locations(cadata=...).
  Why: the kubeconfig assembled from DescribeCluster is unusable, so every client that talks to the cluster over TLS fails before issuing a request. Nothing downstream — the API server's allow/forbid decision, and the audit record that names the access-entry username in CloudWatch Logs — can be reached or verified.
  Fix: the cluster's certificateAuthority state/serializer must return the Base64-encoded PEM certificate of the CA that signs the served Kubernetes endpoint. Open issues 20261104T060239Z-ecr-eks-logs-a-cluster-s-certificateauthority-data.md and 20261104T053901Z-eks-auth-the-whole-pod-identity-data-plane.md describe the same placeholder under the ecr__eks__logs and eks-auth headings; this check is reported unowned under the eks__logs heading, and the same cause is filed for the 28 affected checks under the eks heading.
  Traces: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout; service eks__logs 0/1 passed.

eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid [error]:
  File "ssl.py", line 707, in create_default_context -> context.load_verify_locations(cafile, capath, cadata)
  ssl.SSLError: no start line: cadata does not contain a certificate (_ssl.c:3977)
The cadata passed in is the base64-decoded cluster.certificateAuthority.data from the emulator's DescribeCluster response; in other checks of this run the same field decodes to '-----BEGIN CERTIFICATE-----\nEMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this is not a ...' and kubectl reports 'error: unable to load root certificates: unable to parse bytes as PEM block'.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — Certificate.data: 'The Base64-encoded certificate data required to communicate with your cluster. Add this to the certificate-authority-data section of the kubeconfig file for your cluster.'

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid.
3. Manually: CreateCluster, wait for it to be usable, DescribeCluster and base64-decode cluster.certificateAuthority.data -> expect a PEM X.509 certificate (begins '-----BEGIN CERTIFICATE-----' and parses as a certificate), not placeholder prose. Load it as the trust anchor for the advertised endpoint and issue an authenticated Kubernetes request -> expect the request to be served, and the cluster's audit log group to carry a record naming the access-entry username for both the permitted and the forbidden verb.
