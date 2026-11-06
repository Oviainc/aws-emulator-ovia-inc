Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-id-logins-provider-token-mismatch-rejected cognito-identity.get-id-malformed-login-token-value-rejected cognito-identity.external-service-exception-on-bad-provider-config cognito-identity.e2e-principal-tags-flow-into-sts-session-tags cognito-identity.delete-identities-partial-failure-rejects-whole-batch cognito-identity.list-identities-max-results-out-of-range-rejected cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity cognito-identity.developer-user-unknown-explicit-identity-rejected cognito-identity.lookup-developer-identity-cross-pool-isolation cognito-identity.set-identity-pool-roles-ambiguous-rule-resolution-rejected cognito-identity.set-identity-pool-roles-rules-type-missing-config-rejected cognito-identity.tag-resource-exceeds-max-tag-count-rejected cognito-identity.tag-resource-exceeds-tag-count-limit-rejected cognito-identity.tag-resource-rejects-reserved-aws-key cognito-identity.e2e-principal-tags-federation-to-sts-session-tags cognito-identity.deploy-cfn-role-attachment-enforced-on-credentials cognito-identity.deploy-cfn-role-attachment-missing-unauthenticated-role-rejected cognito-identity.deploy-identity-pool-cfn-stack-update-changes-runtime-behavior cognito-identity.role-mapping-rules-first-match-wins-in-order cognito-identity.role-mapping-rules-match-types-evaluated cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.get-id-logins-provider-token-mismatch-rejected|_refusal#10|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.get-id-malformed-login-token-value-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected GetId to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.external-service-exception-on-bad-provider-config|_client_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.e2e-principal-tags-flow-into-sts-session-tags|_client_error#19|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.delete-identities-partial-failure-rejects-whole-batch|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cognito-identity.list-identities-max-results-out-of-range-rejected|run#10|AssertionError|assert code_zero == "ValidationException", f"expected ValidationException, got {code_zero}"
Issue-Key: cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled|run#10|AssertionError|assert message == "Unauthenticated access is not supported for this identity pool.", (
Issue-Key: cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity|run#10|AssertionError|assert message == EXPECTED_MESSAGE, (
Issue-Key: cognito-identity.developer-user-unknown-explicit-identity-rejected|run#9|AssertionError|assert message == f"Identity '{fabricated_identity_id}' not found.", message
Issue-Key: cognito-identity.lookup-developer-identity-cross-pool-isolation|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.set-identity-pool-roles-ambiguous-rule-resolution-rejected|run#19|AssertionError|assert message == "RuleConfiguration is required for Rules", (
Issue-Key: cognito-identity.set-identity-pool-roles-rules-type-missing-config-rejected|run#13|AssertionError|assert message == "RuleConfiguration is required for Rules", (
Issue-Key: cognito-identity.tag-resource-exceeds-max-tag-count-rejected|run#11|AssertionError|assert message == "The resultant tag set must not have more than 50 user tags.", message
Issue-Key: cognito-identity.tag-resource-exceeds-tag-count-limit-rejected|run#13|AssertionError|assert message == "The resultant tag set must not have more than 50 user tags.", message
Issue-Key: cognito-identity.tag-resource-rejects-reserved-aws-key|run#6|AssertionError|assert message == "Caller is an end user and not allowed to mutate system tags", message
Issue-Key: cognito-identity.e2e-principal-tags-federation-to-sts-session-tags|_wait_for_assume_role_with_web_identity#43|ResponseParserError|resp = sts.assume_role_with_web_identity(
Issue-Key: cognito-identity|GetId|InvalidParameterException
Issue-Key: cognito-identity.role-mapping-rules-first-match-wins-in-order|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.role-mapping-rules-match-types-evaluated|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events|run#17|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity: GetId admits unvalidatable login tokens, SetPrincipalTagAttributeMap admits unconfigured providers, CFN-deployed pools are unaddressable, and 2 constraint + 9 message divergences (7 causes)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetId

## Description
This issue bundles SEVEN distinct net-new root causes reached through cognito-identity checks. All seven must be fixed and re-replayed before closing.

=== Cause 1 of 7: GetId accepts a Logins token the declared provider cannot validate ===
AWS: GetId models NotAuthorizedException (403) and ExternalServiceException (400). With a Logins entry naming a provider the pool DOES declare (a Cognito user pool in CognitoIdentityProviders) but a token that provider cannot validate (forged/garbage/malformed value), the call is refused with NotAuthorizedException and no identity is minted. The recorded message is "Invalid login token. Not a valid OpenId Connect identity token." (built for this condition).
Emulator: HTTP 200 with a freshly minted IdentityId — the token is never validated against the provider, and the new identity is observable in ListIdentities.
Why: the emulator mints a verified-looking identity for any unverified login, so authentication is effectively optional; every negative branch a client has for a bad/expired token is unreachable, and tests of 'logged-in vs anonymous' identity linkage are meaningless.
Fix: the GetId handler (ops/get_id.rs) must validate each Logins entry against the pool's declared provider (signature/issuer/aud for a user-pool token) and refuse with NotAuthorizedException — or ExternalServiceException when the provider itself cannot be consulted — before minting or linking an identity.
Traces: cognito-identity.get-id-logins-provider-token-mismatch-rejected, cognito-identity.get-id-malformed-login-token-value-rejected, cognito-identity.external-service-exception-on-bad-provider-config.

=== Cause 2 of 7: SetPrincipalTagAttributeMap does not validate IdentityProviderName against the pool's provider config ===
AWS: SetPrincipalTagAttributeMap models InvalidParameterException (400). IdentityProviderName must name a provider in the pool's federated-provider config (SupportedLoginProviders / CognitoIdentityProviders / SamlProviderARNs / OpenIdConnectProviderARNs); a developer-authenticated provider name is not in that set and the call is refused with the message "Provider <name> not configured for Identity Pool" (built from the request).
Emulator: HTTP 200, and the mapping is stored for a provider the pool does not have.
Why: principal-tag mappings can be written against providers that will never produce a token, so an ABAC setup that cannot work in production reads as configured here.
Fix: the SetPrincipalTagAttributeMap handler (ops/set_principal_tag_attribute_map.rs) must resolve IdentityProviderName against the pool's provider config and refuse with InvalidParameterException.
Traces: cognito-identity.e2e-principal-tags-flow-into-sts-session-tags.

=== Cause 3 of 7: two more member-constraint violations answer InvalidParameterException instead of ValidationException ===
AWS: these are protocol constraint violations, not operation logic: DeleteIdentities.IdentityIdsToDelete members carry @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, and ListIdentities.MaxResults carries @range{1,60} (Smithy; neither operation models ValidationException). AWS refuses ahead of the handler with HTTP 400 ValidationException and the "N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: …" message built from the request.
Emulator: HTTP 400 with code InvalidParameterException and only the bare constraint clause.
Why: SDK error handlers branch on the code; a modeled service exception is retried and logged differently from the framework ValidationException, so request-shape bugs are misclassified.
Fix: the shared pre-handler constraint-validation path for cognito-identity (the same fix as the open issue below) must also cover DeleteIdentities list members and ListIdentities.MaxResults.
Related but NOT covering: open issue 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md describes this exact classification defect, but its trace list carries delete-identities-oversized-batch-rejected / -too-many-requests-limit and NOT these two checks' replay keys.
Traces: cognito-identity.delete-identities-partial-failure-rejects-whole-batch, cognito-identity.list-identities-max-results-out-of-range-rejected.

=== Cause 4 of 7: nine refusals carry emulator-authored message text instead of the message AWS builds for the condition ===
AWS: the HTTP status and error code match in all nine cases; what differs is the message, and each of these messages is a known rule — either a fixed string AWS returns for that condition or a string built from the request: GetId/NotAuthorizedException for a pool with AllowUnauthenticatedIdentities=false -> "Unauthenticated access is not supported for this identity pool."; DescribeIdentity/GetOpenIdTokenForDeveloperIdentity for an unknown identity -> "Identity '<identity-id>' not found." (request-derived); LookupDeveloperIdentity for an identity in another pool -> "You do not have access to this identity"; SetIdentityPoolRoles with Type=Rules and no RulesConfiguration -> "RuleConfiguration is required for Rules"; TagResource over the 50-tag ceiling -> "The resultant tag set must not have more than 50 user tags."; TagResource with an 'aws:'-prefixed key -> "Caller is an end user and not allowed to mutate system tags".
Emulator: each of these is replaced by text the emulator authors, e.g. "Unauthenticated access is not supported for the identity pool 'us-west-2:…'.", "Identity '…' not found in identity pool '…'.", "Access to Identity '…' is forbidden.", "RoleMappings['graph.facebook.com'].RulesConfiguration is required when Type is 'Rules'.", "Assigning these tags would leave identity pool '…' with 51 tags: an identity pool can have as many as 50 tags.", "Tags key 'aws:reserved-tag' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use."
Why: operators and support tooling match on these strings, and the emulator's variants leak internal identifiers into messages AWS keeps identifier-free; the divergence is also the only thing keeping these nine checks red.
Fix: the message construction in the respective handlers (get_id.rs, describe_identity.rs/get_open_id_token_for_developer_identity.rs, lookup_developer_identity.rs, set_identity_pool_roles.rs, tag_resource.rs) — emit AWS's message for the condition rather than locally worded prose.
Related but NOT covering: open issue 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md names some of these checks against their PREVIOUS failure mode (NotImplemented stubs / wrong codes); the stubs are implemented now and only the message remains.
Traces: cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled, cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity, cognito-identity.developer-user-unknown-explicit-identity-rejected, cognito-identity.lookup-developer-identity-cross-pool-isolation, cognito-identity.set-identity-pool-roles-ambiguous-rule-resolution-rejected, cognito-identity.set-identity-pool-roles-rules-type-missing-config-rejected, cognito-identity.tag-resource-exceeds-max-tag-count-rejected, cognito-identity.tag-resource-exceeds-tag-count-limit-rejected, cognito-identity.tag-resource-rejects-reserved-aws-key.

=== Cause 5 of 7: an unsigned sts:AssumeRoleWithWebIdentity is unroutable AND its refusal is serialized as JSON to a query-protocol client ===
AWS: AssumeRoleWithWebIdentity is anonymous-capable: the Cognito basic (classic) flow calls it with no SigV4 credentials, presenting only the OpenID token, and AWS answers the STS query protocol (XML).
Emulator: the request is not routed — the body is {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — and that body is JSON, which the client's XML/query parser cannot read at all: botocore raises ResponseParserError ("not well-formed (invalid token): line 1, column 0, invalid XML received") instead of a ClientError.
Why: the whole Cognito basic flow is dead, and because the error is not even parseable the client cannot see a code to retry or branch on — it gets a parser crash, which no retry policy handles.
Fix: two parts — (a) the request router must resolve the target service for unsigned/anonymous requests (endpoint/host based), and (b) pre-dispatch refusals must be serialized in the protocol the targeted endpoint speaks (XML for STS's query protocol), never unconditionally as JSON.
Related but NOT covering: open issue 20261024T154246Z-sts-assumerolewithwebidentity-is-unroutable-unknownservice-trust-policy-conditions.md records the routing half under service token sts and does not carry this check's replay key; the JSON-body-to-an-XML-client half is not in it.
Traces: cognito-identity.e2e-principal-tags-federation-to-sts-session-tags.

=== Cause 6 of 7: Ref of a CloudFormation AWS::Cognito::IdentityPool still resolves to the pool NAME ===
AWS: Ref on AWS::Cognito::IdentityPool yields the identity-pool id ('<region>:<guid>', the modeled IdentityPoolId @pattern ^[\w-]+:[0-9a-f-]+$), so a stack Output or an intrinsic built from !Ref can be passed straight to GetId / DescribeIdentityPool / GetIdentityPoolRoles.
Emulator: the stack reaches CREATE_COMPLETE but Ref yields the template's IdentityPoolName, so every follow-up cognito-identity call on it is refused with HTTP 400 InvalidParameterException "Value 'sim2real-cip-…' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+:[0-9a-f-]+$".
Why: no CloudFormation-deployed identity pool can be addressed through the Cognito API, so infrastructure-as-code flows are a dead end even though the stack reports success.
Fix: the CloudFormation provider for AWS::Cognito::IdentityPool — create the pool in cognito-identity state and record the real pool id as the resource's physical id (what Ref/GetAtt/Outputs resolve to).
Related but NOT covering: open issue 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md describes the same provider defect as its Cause 2 but carries a different set of deploy checks, not these three.
Traces: cognito-identity.deploy-cfn-role-attachment-enforced-on-credentials, cognito-identity.deploy-cfn-role-attachment-missing-unauthenticated-role-rejected, cognito-identity.deploy-identity-pool-cfn-stack-update-changes-runtime-behavior.

=== Cause 7 of 7: four checks never converge and burn the whole 60 s harness budget ===
AWS: each of these scenarios converges promptly on real AWS — the enhanced flow vends a role session (role-mapping rule ordering / match types), GetOpenIdToken answers for a basic-flow pool, and CloudTrail delivers management events for pool administration.
Emulator: the checks poll (the inlined _poll_until / _set_roles retry helpers, 120-180 s budgets) for a condition the emulator never reaches and are killed at the harness cap with "Check timed out after 60.0s" — no response is ever produced to compare.
Why: a hang is worse than a wrong answer for a client: the call neither succeeds nor returns an error code, so timeouts and retry budgets are consumed with no diagnosis.
Fix: Hypothesis: the polled conditions are the ones Cause 1 of the companion issue (GetCredentialsForIdentity vending the ambient principal instead of a role session) and the cognito-identity CloudTrail management-event feed never satisfy; a fixer should re-replay these four after those land and, if they still hang, trace which call never settles.
Traces: cognito-identity.role-mapping-rules-first-match-wins-in-order, cognito-identity.role-mapping-rules-match-types-evaluated, cognito-identity.basic-flow-get-open-id-token-refused-while-role-mappings-are-configured, cognito-identity.cloudtrail-logs-pool-management-but-not-identity-data-events.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

Cause 1 — get-id-logins-provider-token-mismatch-rejected: Logins={<declared user-pool provider>: 'forged.not-a-real.user-pool-id-token'} -> "expected the call to be refused, but it SUCCEEDED and returned {'IdentityId': 'us-west-2:05378d88-…', 'ResponseMetadata': {'HTTPStatusCode': 200, …}}" (expected NotAuthorizedException / "Invalid login token. Not a valid OpenId Connect identity token."). get-id-malformed-login-token-value-rejected: "expected GetId to be refused, but it SUCCEEDED and returned {'IdentityId': 'us-west-2:74b9c498-…', 'HTTPStatusCode': 200}". external-service-exception-on-bad-provider-config: "expected the call to be refused, but it SUCCEEDED and returned {'IdentityId': 'us-west-2:41850745-…', 'HTTPStatusCode': 200}".

Cause 2 — e2e-principal-tags-flow-into-sts-session-tags: "expected a rejection, but the call SUCCEEDED and returned {'IdentityPoolId': 'us-west-2:415efaae-…', 'IdentityProviderName': 'sim2real-tagdevprov-…', 'UseDefaults': False, 'PrincipalTags': {'department': 'department'}, 'HTTPStatusCode': 200}" (expected ('InvalidParameterException', 'Provider <name> not configured for Identity Pool')).

Cause 3 — delete-identities-partial-failure-rejects-whole-batch: DeleteIdentities(IdentityIdsToDelete=[<valid id>, 'not-a-real-identity-id']) -> "expected ValidationException, got InvalidParameterException". list-identities-max-results-out-of-range-rejected: "expected ValidationException, got InvalidParameterException".

Cause 4 — message-only divergences (code and status matched; the quoted text is the emulator's):
- server-side-unauthenticated-role-blocked-without-unauth-access-enabled: expected "Unauthenticated access is not supported for this identity pool.", got "Unauthenticated access is not supported for the identity pool 'us-west-2:ed6257b6-…'."; same in unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity.
- developer-user-unknown-explicit-identity-rejected: expected "Identity '<id>' not found.", got "Identity '<id>' not found in identity pool '<pool id>'."
- lookup-developer-identity-cross-pool-isolation: expected 'You do not have access to this identity', got "Access to Identity '<id>' is forbidden."
- set-identity-pool-roles-ambiguous-rule-resolution-rejected and -rules-type-missing-config-rejected: expected "RuleConfiguration is required for Rules", got "RoleMappings['graph.facebook.com'].RulesConfiguration is required when Type is 'Rules'." (code InvalidParameterException matched).
- tag-resource-exceeds-max-tag-count-rejected: expected "The resultant tag set must not have more than 50 user tags.", got "Assigning these tags would leave identity pool '…' with 51 tags: an identity pool can have as many as 50 tags."; tag-resource-exceeds-tag-count-limit-rejected: the same with 52 tags.
- tag-resource-rejects-reserved-aws-key: code InvalidParameterException asserted and matched; expected "Caller is an end user and not allowed to mutate system tags", got "Tags key 'aws:reserved-tag' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use."

Cause 5 — e2e-principal-tags-federation-to-sts-session-tags: botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' — raised from the check's _wait_for_assume_role_with_web_identity(base_sts, role_arn, web_identity_token, session_name) loop.

Cause 6 — deploy-cfn-role-attachment-enforced-on-credentials: "An error occurred (InvalidParameterException) when calling the GetId operation: Value 'sim2real-cip-9de7e6c29f81' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w-]+:[0-9a-f-]+$" (the value came from {"Ref": "IdentityPool"} in the stack the check deploys). Same refusal in deploy-cfn-role-attachment-missing-unauthenticated-role-rejected ('sim2real-cip-af094f7dd493') and deploy-identity-pool-cfn-stack-update-changes-runtime-behavior ('sim2real-behav-pool-3523873a789e').

Cause 7 — role-mapping-rules-first-match-wins-in-order, role-mapping-rules-match-types-evaluated, basic-flow-get-open-id-token-refused-while-role-mappings-are-configured, cloudtrail-logs-pool-management-but-not-identity-data-events: "Error: Check timed out after 60.0s" (no response captured).

Model (smithy_query): GetId errors ExternalServiceException 400, InvalidParameterException 400, LimitExceededException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429, auth [] / optionalAuth true; SetPrincipalTagAttributeMap.IdentityProviderName required @length{1,128}, errors include InvalidParameterException 400; DeleteIdentities.IdentityIdsToDelete @length{1,60} over IdentityId @length{1,55} @pattern ^[\\w-]+:[0-9a-f-]+$; ListIdentities.MaxResults @range{1,60}; TagResource errors InvalidParameterException 400 / NotAuthorizedException 403.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check ids in trace_ids.
2. Cause 1: create a user pool + app client, CreateIdentityPool(CognitoIdentityProviders=[{ProviderName:<issuer>, ClientId:<client>}]), then GetId(IdentityPoolId=<pool>, Logins={<issuer>: 'forged.not-a-real.user-pool-id-token'}) -> expect 403 NotAuthorizedException and no new identity in ListIdentities; observe 200 with a new IdentityId.
3. Cause 2: SetPrincipalTagAttributeMap(IdentityPoolId=<pool>, IdentityProviderName='<a developer provider never added to the pool>', UseDefaults=False, PrincipalTags={'department':'department'}) -> expect 400 InvalidParameterException 'Provider <name> not configured for Identity Pool'; observe 200.
4. Cause 3: DeleteIdentities(IdentityIdsToDelete=[<valid id>,'not-a-real-identity-id']) and ListIdentities(MaxResults out of 1..60) -> expect 400 ValidationException; observe 400 InvalidParameterException.
5. Cause 4: trigger each listed refusal and compare only the message.
6. Cause 5: call sts:AssumeRoleWithWebIdentity with an UNSIGNED client and a Cognito OpenID token -> expect an XML STS response (or an XML-serialized error); observe a JSON {"__type":"UnknownService"} body that the SDK cannot parse.
7. Cause 6: CreateStack with AWS::Cognito::IdentityPool + AWS::Cognito::IdentityPoolRoleAttachment and pass {"Ref": "IdentityPool"} to GetId -> expect the pool id; observe the pool name and an InvalidParameterException.
8. Cause 7: replay the four timing-out check ids and observe the 60 s harness kill with no response.
