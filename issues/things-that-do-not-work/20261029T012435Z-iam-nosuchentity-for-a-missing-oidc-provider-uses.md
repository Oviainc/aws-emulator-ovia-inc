Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-nonexistent-entity-returns-nosuchentity iam.op-contract-add-client-id-to-openid-connect-provider iam.op-contract-remove-client-id-from-oidc-provider iam.e2e-oidc-add-client-id-to-deleted-provider-rejected iam.e2e-oidc-client-id-federation-workflow cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected
Issue-Key: iam.get-nonexistent-entity-returns-nosuchentity|_expect#22|AssertionError|assert got_message == message, f"{label}: expected message {message!r}, got {got_message!r}"
Issue-Key: iam.op-contract-add-client-id-to-openid-connect-provider|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-remove-client-id-from-oidc-provider|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.e2e-oidc-add-client-id-to-deleted-provider-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iam.e2e-oidc-client-id-federation-workflow|run#25|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected|run#20|AssertionError|assert message == expected, f"expected AWS's exact message {expected!r}, got {message!r}"
Issue-Commit: 8344c82f0c4ff2c2802cbb1bf8afe5fff854e291

# IAM NoSuchEntity for a missing OIDC provider uses the wrong message text: 'OpenId connect Provider <arn> cannot be found.' instead of AWS's 'OpenIDConnect Provider not found for arn <arn>'

> _Note: this issue was identified against the code base at commit `8344c82f0c4ff2c2802cbb1bf8afe5fff854e291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIDConnectProvider

## Description
One net-new root cause for iam.

Cause 1 of 1: every IAM OIDC-provider lookup that misses answers NoSuchEntityException with a message whose fixed text is not AWS's. The three operations that resolve a provider by ARN (GetOpenIDConnectProvider, AddClientIDToOpenIDConnectProvider, RemoveClientIDFromOpenIDConnectProvider) all build their NoSuchEntity message from one shared helper, `crate::state::open_id_connect_provider_not_found`, so the single wrong template surfaces on all of them.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) models NoSuchEntityException on GetOpenIDConnectProvider, AddClientIDToOpenIDConnectProvider and RemoveClientIDFromOpenIDConnectProvider with httpError 404 and awsQueryError code `NoSuchEntity`, documented as "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." The model does not state the message text, so this is recorded behavior: for an ARN the account holds no OIDC provider under, AWS words the message

    OpenIDConnect Provider not found for arn <OpenIDConnectProviderArn>

where `OpenIDConnect Provider not found for arn ` is the FIXED text and the single variable part is the `OpenIDConnectProviderArn` value taken verbatim from the request (the recordings show it as the full `arn:aws:iam::<account>:oidc-provider/<url-without-scheme>` the caller passed). The status (404) and the code (`NoSuchEntity`) the emulator returns already match; only the message diverges.

Emulator: the same calls return 404 / `NoSuchEntity` but with the message

    OpenId connect Provider <OpenIDConnectProviderArn> cannot be found.

i.e. a different fixed text, the ARN moved into the middle, a differently-cased "OpenId connect" spelling, and a trailing period AWS does not emit. Replay examples: GetOpenIDConnectProvider got 'OpenId connect Provider arn:aws:iam::000000000000:oidc-provider/sim2real-behav-....example.com cannot be found.' where AWS gives 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/sim2real-behav-....example.com'; AddClientIDToOpenIDConnectProvider and RemoveClientIDFromOpenIDConnectProvider diverge identically on the same lookup.

Why: clients and tooling that key off the NoSuchEntity message rather than only the code get the wrong answer — error-message assertions in IaC/CI harnesses, log scrapers and user-facing passthroughs of the IAM message all see text no AWS region ever produces. It also breaks cross-service flows that surface IAM's wording: the Cognito OIDC-federation check fails on exactly this string when it deletes an OIDC provider under a live identity pool and reads the refusal back, so the divergence leaks out of IAM into consumers.

Fix: the shared message builder in IAM's state module — `crate::state::open_id_connect_provider_not_found` in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs — must format AWS's template (`OpenIDConnect Provider not found for arn {arn}`, no trailing period) from the request's ARN. Fixing the one helper corrects all three call sites; no handler-local change is needed.

Traces: iam.get-nonexistent-entity-returns-nosuchentity, iam.op-contract-add-client-id-to-openid-connect-provider, iam.op-contract-remove-client-id-from-oidc-provider, iam.e2e-oidc-add-client-id-to-deleted-provider-rejected, iam.e2e-oidc-client-id-federation-workflow, cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected. Call sites: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_open_id_connect_provider.rs:195, .../src/ops/add_client_id_to_open_id_connect_provider.rs:217, .../src/ops/remove_client_id_from_open_id_connect_provider.rs:217. (Not covered by the open stub issues 20261025T000319Z-iam-deleteopenidconnectprovider-stub.md or 20261025T000319Z-iam-updateopenidconnectproviderthumbprint-stub.md, which are about unimplemented operations, not this message template.)

## Evidence
Expected (AWS, per recordings): HTTP 404, Error.Code = NoSuchEntity, Error.Message = "OpenIDConnect Provider not found for arn <request's OpenIDConnectProviderArn>".
Actual (emulator): HTTP 404, Error.Code = NoSuchEntity, Error.Message = "OpenId connect Provider <request's OpenIDConnectProviderArn> cannot be found."

Verbatim replay assertions:
- iam.get-nonexistent-entity-returns-nosuchentity: "GetOpenIDConnectProvider: expected message 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/sim2real-behav-9cc126245be74f768a4d47debff8d93b.example.com', got 'OpenId connect Provider arn:aws:iam::000000000000:oidc-provider/sim2real-behav-9cc126245be74f768a4d47debff8d93b.example.com cannot be found.'"
- iam.op-contract-add-client-id-to-openid-connect-provider: "expected message 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/sim2real-does-not-exist.example.com', got 'OpenId connect Provider ... cannot be found.'"
- iam.op-contract-remove-client-id-from-oidc-provider: "expected message 'OpenIDConnect Provider not found for arn arn:aws:iam::000000000000:oidc-provider/cognito-idp.us-west-2.amazonaws.com/does-not-exist.example.org', got 'OpenId connect Provider ... cannot be found.'"
- iam.e2e-oidc-add-client-id-to-deleted-provider-rejected and iam.e2e-oidc-client-id-federation-workflow: same divergence after the provider is deleted.
- cognito-identity__cognito-idp__iam.xsvc-cognito-identity-2-iam-oidc-provider-deleted-under-a-live-pool-rejected: "expected AWS's exact message 'OpenIDConnect Provider not found for arn ...', got 'OpenId connect Provider ... cannot be found.'"

The account id, generated provider host and pool id in these strings are per-run values that only fill the template's variable slot; the divergence is in the template's fixed text.

## Steps to Reproduce
1. Boot the emulator and point an IAM client at it.
2. Call GetOpenIDConnectProvider with a well-formed ARN for which no provider exists, e.g. OpenIDConnectProviderArn=arn:aws:iam::<account>:oidc-provider/does-not-exist.example.com.
3. Observe the 404 NoSuchEntity response's Error.Message is "OpenId connect Provider <arn> cannot be found." rather than AWS's "OpenIDConnect Provider not found for arn <arn>".
4. Repeat with AddClientIDToOpenIDConnectProvider(OpenIDConnectProviderArn=<same missing arn>, ClientID=anything) and RemoveClientIDFromOpenIDConnectProvider(same) — identical wrong message.
5. Also reproducible after a real delete: CreateOpenIDConnectProvider, DeleteOpenIDConnectProvider, then AddClientIDToOpenIDConnectProvider on the deleted ARN.
6. Re-replay the checks in trace_ids to confirm.
