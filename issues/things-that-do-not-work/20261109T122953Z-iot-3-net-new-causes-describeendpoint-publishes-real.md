Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.concurrent-shadow-version-conditioned-updates iot.delete-role-alias-succeeds-despite-active-credentials iot.e2e-topic-rule-destination-confirm-then-deliver iot.iot-concurrent-shadow-updates-converge-to-one-final-version iot.iot-detach-policy-revokes-mtls-publish-and-reattach-restores iot.iot-mtls-inactive-certificate-rejected-before-authorization iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected iot.iot-mtls-retained-publish-rejected-without-retainpublish-action iot.iot-named-shadow-delta-and-version-monotonicity-across-updates iot.iot-named-shadow-isolated-from-classic-shadow iot.iot-shadow-update-with-a-stale-version-is-rejected-and-the-document-is-unchanged iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition iot.retained-message-binary-payload-byte-equality iot.role-alias-delete-invalidates-credential-exchange iot.shadow-client-token-echoed-not-persisted iot.shadow-delta-only-differing-leaves iot.shadow-metadata-timestamp-per-leaf iot.shadow-named-vs-classic-independent iot.shadow-nested-document-deep-merge iot.shadow-oversize-document-rejected iot.shadow-update-get-delta-and-delete-lifecycle iot.shadow-version-conflict-stale-update iot.topic-rule-with-unconfirmed-destination-blocks-delivery iot.attach-policy-nonexistent-policy-rejected
Issue-Key: iot.concurrent-shadow-version-conditioned-updates|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.delete-role-alias-succeeds-despite-active-credentials|_fetch_credentials_with_retry#24|PublishedUrlRejected|_url = ctx.endpoint_url(endpoint)
Issue-Key: iot.e2e-topic-rule-destination-confirm-then-deliver|_self_test_endpoint_with_retry#40|PublishedUrlRejected|_url, _route = ctx.endpoint_route(receiver_url)
Issue-Key: iot.iot-concurrent-shadow-updates-converge-to-one-final-version|_ats_data_client#2|PublishedUrlRejected|url = ctx.endpoint_url(host)
Issue-Key: iot.iot-detach-policy-revokes-mtls-publish-and-reattach-restores|_publish#20|PublishedUrlRejected|url = ctx.endpoint_url(host, aws_port=8443)
Issue-Key: iot.iot-mtls-inactive-certificate-rejected-before-authorization|_publish#9|PublishedUrlRejected|url = ctx.endpoint_url(host, aws_port=8443)
Issue-Key: iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected|_publish#13|PublishedUrlRejected|_url = ctx.endpoint_url(host, aws_port=8443)
Issue-Key: iot.iot-mtls-retained-publish-rejected-without-retainpublish-action|_publish#11|PublishedUrlRejected|url = ctx.endpoint_url(host, aws_port=8443)
Issue-Key: iot.iot-named-shadow-delta-and-version-monotonicity-across-updates|_ats_data_client#2|PublishedUrlRejected|url = ctx.endpoint_url(host)
Issue-Key: iot.iot-named-shadow-isolated-from-classic-shadow|_ats_data_client#2|PublishedUrlRejected|url = ctx.endpoint_url(host)
Issue-Key: iot.iot-shadow-update-with-a-stale-version-is-rejected-and-the-document-is-unchanged|_ats_data_client#2|PublishedUrlRejected|url = ctx.endpoint_url(host)
Issue-Key: iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition|_publish_mtls#21|PublishedUrlRejected|url = ctx.endpoint_url(host, aws_port=8443)
Issue-Key: iot.retained-message-binary-payload-byte-equality|run#3|PublishedUrlRejected|iot_data = ctx.client("iot-data", endpoint_url=ctx.endpoint_url(endpoint))
Issue-Key: iot.role-alias-delete-invalidates-credential-exchange|run#24|PublishedUrlRejected|url = f"{ctx.endpoint_url(endpoint)}/role-aliases/{alias_name}/credentials"
Issue-Key: iot.shadow-client-token-echoed-not-persisted|_iot_data#2|PublishedUrlRejected|url = ctx.endpoint_url(ep)
Issue-Key: iot.shadow-delta-only-differing-leaves|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-metadata-timestamp-per-leaf|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-named-vs-classic-independent|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-nested-document-deep-merge|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-oversize-document-rejected|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-update-get-delta-and-delete-lifecycle|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.shadow-version-conflict-stale-update|_iot_data#2|PublishedUrlRejected|return ctx.client("iot-data", endpoint_url=ctx.endpoint_url(ep))
Issue-Key: iot.topic-rule-with-unconfirmed-destination-blocks-delivery|_self_test_endpoint_with_retry#1|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: iot.attach-policy-nonexistent-policy-rejected|run#9|AssertionError|assert err == ("ResourceNotFoundException", "Policy not found"), (
Issue-Commit: ec57509239c646de823e7950d2544f6a600b68b2

# iot: 3 net-new causes — DescribeEndpoint publishes real-AWS data/credential hostnames the emulator does not serve (22 further checks), a topic-rule HTTP destination is published as a real execute-api host, and AttachPolicy's missing-policy refusal is emulator-authored prose

> _Note: this issue was identified against the code base at commit `ec57509239c646de823e7950d2544f6a600b68b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Three causes, all grounded in failing checks of the iot service. Each must be closed before this issue is done.

Cause 1 of 3: DescribeEndpoint announces an address outside the emulator, so the whole IoT data plane is unreachable (22 further checks).
AWS: DescribeEndpoint (Smithy: GET /endpoint, query endpointType, output endpointAddress; errors InvalidRequestException, ThrottlingException, UnauthorizedException, InternalFailureException) returns the address a client must then address for that endpoint type — iot:Data-ATS for the shadow/retained-message/publish HTTPS data plane, iot:CredentialProvider for the role-alias credential exchange, iot:Jobs for jobs-data. The contract is that the published address is served by the same provider that published it.
Emulator: it publishes real AWS hostnames — '<id>-ats.iot.<region>.amazonaws.com' for iot:Data-ATS and '<id>.credentials.iot.<region>.amazonaws.com' for iot:CredentialProvider — which are not hosts the emulator serves; the call to the published address is refused before any request is made ('its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<path>'). The specific generated id, host and port differ per run and are not part of the requirement; the requirement is that endpointAddress names a listener the emulator answers on for the requested endpointType.
Why: every client that discovers the data endpoint the documented way — shadow update/get/delete, named vs classic shadow isolation, delta and metadata projection, retained messages, mTLS publish/authorization, the credential-provider exchange for a role alias — sends its traffic to real AWS instead of the emulator. None of that behavior can be exercised or relied on.
Fix: the DescribeEndpoint handler in the iot service (endpointType-to-published-address resolution) must publish the emulator's own host:port for each endpoint type, and the data-plane listeners must be reachable at that address. The open issue 20261109T113725Z-iot-describeendpoint-publishes-a-real-aws-hostname-id.md describes the same root cause but does not carry the replay keys of the 22 checks listed here, so they are filed with it named.
Traces: iot.concurrent-shadow-version-conditioned-updates, iot.delete-role-alias-succeeds-despite-active-credentials, iot.e2e-topic-rule-destination-confirm-then-deliver, iot.iot-concurrent-shadow-updates-converge-to-one-final-version, iot.iot-detach-policy-revokes-mtls-publish-and-reattach-restores, iot.iot-mtls-inactive-certificate-rejected-before-authorization, iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected, iot.iot-mtls-retained-publish-rejected-without-retainpublish-action, iot.iot-named-shadow-delta-and-version-monotonicity-across-updates, iot.iot-named-shadow-isolated-from-classic-shadow, iot.iot-shadow-update-with-a-stale-version-is-rejected-and-the-document-is-unchanged, iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition, iot.retained-message-binary-payload-byte-equality, iot.role-alias-delete-invalidates-credential-exchange, iot.shadow-client-token-echoed-not-persisted, iot.shadow-delta-only-differing-leaves, iot.shadow-metadata-timestamp-per-leaf, iot.shadow-named-vs-classic-independent, iot.shadow-nested-document-deep-merge, iot.shadow-oversize-document-rejected, iot.shadow-update-get-delta-and-delete-lifecycle, iot.shadow-version-conflict-stale-update.

Cause 2 of 3: the HTTP endpoint used as a topic-rule destination is published as a real execute-api hostname.
AWS: the confirmation/delivery URL a client registers as an iot TopicRuleDestination is an address the provider actually serves; the destination's confirmation POST and subsequent deliveries reach it. Recorded behavior only.
Emulator: the endpoint announced for the destination is 'https://<api-id>.execute-api.<region>.amazonaws.com/' — a real AWS host the emulator does not serve — so the destination confirmation/delivery leg is refused before any HTTP request leaves ('its host is not one of the emulator's own hosts'). Only the host matters here; the api id and port vary per run.
Why: the gate that an unconfirmed topic-rule destination must block delivery (and that a confirmed one must deliver) cannot be observed at all; a client wiring a rule to an HTTP destination discovers an unreachable address.
Fix: the published-endpoint resolution for the HTTP endpoint backing topic-rule destinations (the apigateway/execute-api endpoint publisher consumed by the iot topic-rule destination path) must announce the emulator's own listener.
Traces: iot.topic-rule-with-unconfirmed-destination-blocks-delivery.

Cause 3 of 3: AttachPolicy's not-found refusal uses emulator-authored wording.
AWS: AttachPolicy (Smithy: PUT /target-policies/{policyName}, required policyName and target; errors include ResourceNotFoundException with httpError 404) refuses a policyName that does not exist with 404 ResourceNotFoundException and the fixed message text 'Policy not found' — the message carries no request-derived values.
Emulator: it answers 404 ResourceNotFoundException with 'Policy <policyName> does not exist', substituting the requested policy name into emulator-authored prose instead of AWS's fixed text.
Why: a client that matches on AWS's message (log scrapers, error-class mapping, SDK retry/branching on message text) does not recognize the refusal, and the message leaks a value AWS never includes.
Fix: the AttachPolicy handler's not-found refusal message in the iot service. The open issue 20261109T114414Z-iot-policy-target-plane-4-causes-attach-detach.md describes nearby Attach/Detach/ListAttachedPolicies wording but does not carry this check's replay key.
Traces: iot.attach-policy-nonexistent-policy-rejected.

## Evidence
Cause 1 — expected: DescribeEndpoint(endpointType='iot:Data-ATS'/'iot:CredentialProvider') returns an address the emulator serves, and the subsequent shadow/retained/credential call succeeds. Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>-ats.iot.<region>.amazonaws.com (resp. https://<id>.credentials.iot.<region>.amazonaws.com) (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

Cause 2 — expected: the HTTP destination URL is served by the emulator so the confirmation POST and the delivery can be observed. Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.execute-api.<region>.amazonaws.com/ ... its host is not one of the emulator's own hosts.

Cause 3 — expected ('ResourceNotFoundException', 'Policy not found'). Actual ('ResourceNotFoundException', 'Policy <policyName> does not exist').

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: call iot DescribeEndpoint with endpointType 'iot:Data-ATS' (then 'iot:CredentialProvider') and try to address the returned endpointAddress for UpdateThingShadow / GetRetainedMessage / the role-alias credential exchange; the address is a real amazonaws.com host the emulator does not serve.
3. Cause 2: create an HTTP topic-rule destination from the published execute-api endpoint and attempt the confirmation POST; the published host is not the emulator's.
4. Cause 3: call iot AttachPolicy with a policyName that was never created and a valid certificate target; compare the ResourceNotFoundException message with AWS's 'Policy not found'.
