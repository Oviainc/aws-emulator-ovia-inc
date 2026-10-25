Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-external-id-success-with-correct-id sts.assume-role-packed-policy-too-large-rejected sts.get-access-key-info-resolves-temporary-asia-key sts.get-access-key-info-returns-correct-account sts.get-caller-identity-userid-matches-created-role-id sts.get-federation-token-policy-caps-caller-permissions sts.assume-role-managed-policy-arns-exceeding-max-count-rejected sts.assume-role-session-tags-unicode-value-round-trip sts.assume-role-transitive-tag-survives-role-chaining sts.e2e-session-tag-propagation-through-role-chain sts.server-side-source-identity-missing-permission-rejected sts.role-chain-with-a-conflicting-source-identity-denied-and-the-matching-one-permitted sts.get-caller-identity-rejects-wrong-secret-key sts.presigned-get-caller-identity-url-is-authenticated-by-query-string-sigv4 sts.assume-role-with-web-identity-nonexistent-role-denied sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-external-id-success-with-correct-id|run#22|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.assume-role-packed-policy-too-large-rejected|run#15|AssertionError|assert control_creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.get-access-key-info-resolves-temporary-asia-key|run#13|AssertionError|assert temp_key_id.startswith("ASIA"), (
Issue-Key: sts.get-access-key-info-returns-correct-account|run#6|AssertionError|assert access_key_id.startswith("ASIA"), (
Issue-Key: sts.get-caller-identity-userid-matches-created-role-id|run#28|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.get-federation-token-policy-caps-caller-permissions|run#34|AssertionError|assert narrow["Credentials"]["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.assume-role-managed-policy-arns-exceeding-max-count-rejected|run#15|AssertionError|assert message == _OVER_LIMIT_MESSAGE, message
Issue-Key: sts.assume-role-session-tags-unicode-value-round-trip|_error#28|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: sts.assume-role-transitive-tag-survives-role-chaining|_poll#39|Timeout|time.sleep(interval_s)
Issue-Key: sts.e2e-session-tag-propagation-through-role-chain|_poll_until#37|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: sts.server-side-source-identity-missing-permission-rejected|run#26|AssertionError|assert message == expected_message, (
Issue-Key: sts.role-chain-with-a-conflicting-source-identity-denied-and-the-matching-one-permitted|_poll#55|Timeout|time.sleep(interval_s)
Issue-Key: sts.get-caller-identity-rejects-wrong-secret-key|run#6|AssertionError|assert code == "SignatureDoesNotMatch", (
Issue-Key: sts.presigned-get-caller-identity-url-is-authenticated-by-query-string-sigv4|run#13|AssertionError|assert status == 200, (
Issue-Key: sts.assume-role-with-web-identity-nonexistent-role-denied|<lambda>#11|ResponseParserError|lambda: sts.assume_role_with_web_identity(
Issue-Key: sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens|run#46|AssertionError|assert cached_allows >= 1, (
Issue-Commit: fe685894da71d873af94e76be025d1114dc569e7

# STS: 9 fidelity divergences — temporary AccessKeyIds are not ASIA-class, session tags/SourceIdentity never decide authorization, PolicyArns over-limit message, wrong auth error codes, query-string SigV4 and anonymous AssumeRoleWithWebIdentity unroutable

> _Note: this issue was identified against the code base at commit `fe685894da71d873af94e76be025d1114dc569e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
NINE distinct root causes in the STS service, numbered below. Each must be closed before this issue is submitted.

Cause 1 of 9: temporary security credentials are issued with a non-ASIA access key id class.
AWS: Smithy — sts#Credentials.AccessKeyId targets accessKeyIdType with @length min 16 max 128 and @pattern ^[\w]*$; the model states no prefix, so this is recorded behavior only. Every STS-vended temporary credential (AssumeRole, AssumeRoleWithWebIdentity, GetSessionToken, GetFederationToken) carries an AccessKeyId in the ASIA key class (first four characters 'ASIA'), as distinct from the AKIA class used for long-term IAM user keys. GetAccessKeyInfo and every SDK/credential-provider heuristic that distinguishes a temporary from a long-term key reads that prefix.
Emulator: the AccessKeyId handed back begins with 'EMUL' — one observed value is 'EMULATORASYO5XOV4OJO' — for AssumeRole, GetSessionToken and GetFederationToken alike. The prefix is the fixed text at issue; the remaining characters are per-run generated values and are not part of the requirement.
Why: a client that classifies a credential by its key-id prefix (aws-sdk credential resolution, `aws sts get-access-key-info`, audit tooling, policy that branches on long-term vs temporary keys) mis-classifies every emulator session as a long-term IAM user key, and code paths that only engage for temporary credentials are never exercised.
Fix: the credential-minting component shared by the STS session-issuing handlers (AssumeRole / AssumeRoleWithWebIdentity / GetSessionToken / GetFederationToken) — the access-key-id generator must emit the ASIA class for temporary credentials and keep AKIA for long-term IAM user keys.
Traces: sts.assume-role-external-id-success-with-correct-id, sts.assume-role-packed-policy-too-large-rejected, sts.get-access-key-info-resolves-temporary-asia-key, sts.get-access-key-info-returns-correct-account, sts.get-caller-identity-userid-matches-created-role-id, sts.get-federation-token-policy-caps-caller-permissions.

Cause 2 of 9: AssumeRole's PolicyArns over-limit refusal carries a generic constraint message instead of STS's own.
AWS: Smithy — AssumeRole.PolicyArns targets sts#policyDescriptorListType, which carries NO @length trait, so the 10-ARN cap is a service-side rule (documented: 'up to 10 managed policy ARNs'); recorded behavior is error code ValidationError, HTTP 400, with the message fixed text 'The limit of ARNs for the PolicyArns parameter was exceeded. 10 ARNs can be requested at most in an API request.' — the whole message is fixed text; the limit 10 is the only value in it and it is the service's own constant, not a request value.
Emulator: the code ValidationError and HTTP 400 are correct, but the message is the generic Smithy-constraint rendering '1 validation error detected: Value \'[<the eleven ARNs>]\' at \'policyArns\' failed to satisfy constraint: Member must have length less than or equal to 10' — i.e. it echoes the request's ARN list and names the member path 'policyArns' rather than stating STS's limit sentence.
Why: a client that matches on the refusal message (error-classification tables, user-facing messages, retry decisions keyed on the documented sentence) does not recognise the refusal, and the message leaks the whole request list.
Fix: AssumeRole's input validation / its error serializer — emit the service-specific over-limit message for PolicyArns rather than the generic constraint-violation renderer.
Traces: sts.assume-role-managed-policy-arns-exceeding-max-count-rejected.

Cause 3 of 9: session tags (including transitive tags carried across a role chain) are never placed into the session's request context, so aws:PrincipalTag conditions never deny.
AWS: Smithy — AssumeRole.Tags targets tagListType (@length min 0 max 50) and AssumeRole.TransitiveTagKeys targets tagKeyListType (@length min 0 max 50). A tag passed to AssumeRole becomes the session's aws:PrincipalTag/<key> context value byte-exactly (unicode values included), a key listed in TransitiveTagKeys survives into every subsequent AssumeRole hop, and a policy condition on aws:PrincipalTag/<key> therefore allows a session carrying the matching value and refuses one that does not, with code AccessDenied, HTTP 403. The refusal's message is the standard authorization sentence: fixed text 'User: ' + the session ARN + ' is not authorized to perform: ' + the action + ' on resource: ' + the resource, where the ARN, action and resource come from the request and the emulator's state.
Emulator: the tag-gated call SUCCEEDS. Under a session whose tag value does not match the policy condition, s3:ListAllMyBuckets returns 200 and ListObjectsV2 returns 'ListObjectsV2 succeeded' where AWS returns AccessDenied; the role-chaining variant polls for the denial and never observes it (the harness deadline miss is the verdict — the check only polls for a denial after proving the session's credentials live).
Why: a developer relying on session tags for tenant isolation (the canonical aws:PrincipalTag multi-tenancy pattern) is told their isolation works when every tenant's session can read every other tenant's data.
Fix: the STS session-issuing handlers' state (persist Tags/TransitiveTagKeys onto the minted session and propagate transitive keys across chained AssumeRole) and the policy-evaluation component that builds the request context, which must expose aws:PrincipalTag/<key> for condition evaluation.
Traces: sts.assume-role-session-tags-unicode-value-round-trip, sts.assume-role-transitive-tag-survives-role-chaining, sts.e2e-session-tag-propagation-through-role-chain. Related open issue 20261024T065715Z-sts-sessions-carry-no-policy-scoping-packedpolicysize-oversized.md records the broader session-policy scoping gap and owns other checks' replay keys; it does not carry these three, so they are filed here and that issue is named for cross-reference.

Cause 4 of 9: a trust policy that omits sts:SetSourceIdentity is refused naming the wrong action.
AWS: Smithy — AssumeRole.SourceIdentity targets sourceIdentityType (@length min 2 max 64, @pattern ^[\w+=,.@-]*$). When a chained AssumeRole carries a propagated source identity into a target role whose trust policy grants sts:AssumeRole but not sts:SetSourceIdentity, the call is refused with AccessDenied, HTTP 403, and the refusal names the action the caller is not authorized for as sts:AssumeRole. Message template: fixed text 'User: ' + caller session ARN + ' is not authorized to perform: sts:AssumeRole on resource: ' + the target role ARN — the ARNs are request/state values, the rest is fixed text.
Emulator: the message instead names sts:SetSourceIdentity: 'User: <session ARN> is not authorized to perform: sts:SetSourceIdentity on resource: <role ARN>'. Code and status match; only the action token in the message diverges.
Why: a client that parses the denied action out of the refusal (or decodes it from an encoded authorization message) attributes the denial to the wrong action and remediates the wrong policy statement.
Fix: the AssumeRole handler's authorization step — the action reported in the not-authorized message for the source-identity leg must be sts:AssumeRole.
Traces: sts.server-side-source-identity-missing-permission-rejected.

Cause 5 of 9: SourceIdentity is not sticky and is not enforceable across a role chain.
AWS: Smithy — AssumeRole.SourceIdentity (sourceIdentityType) and the AssumeRole output member SourceIdentity. A session assumed with SourceIdentity set carries that value into subsequent AssumeRole hops WITHOUT it being passed again, so a target role whose trust policy requires sts:SourceIdentity StringEquals that value admits the matching session and refuses a session carrying a different value with AccessDenied; and attempting to CHANGE the value mid-chain is refused with ValidationError and the message 'The source identity is already set for this assume role session' (recorded behavior; the model does not state it).
Emulator: neither leg is enforced — the chain into the condition-gated role is never refused for a non-matching source identity, so the check's bounded poll for the denial expires (60s harness deadline) with no refusal ever observed.
Why: sts:SourceIdentity is the mechanism for attributing actions to an original human/workload identity across role chains and for pinning a chain to one identity; with it unenforced, any session can chain into a role that is supposed to admit only one source identity.
Fix: the AssumeRole handler's state (persist SourceIdentity on the session and propagate it through chained AssumeRole) and the trust-policy evaluation component (support the sts:SourceIdentity condition key; reject an attempt to set a conflicting value with ValidationError).
Traces: sts.role-chain-with-a-conflicting-source-identity-denied-and-the-matching-one-permitted.

Cause 6 of 9: a valid access key id paired with a wrong secret is refused as InvalidClientTokenId instead of SignatureDoesNotMatch.
AWS: recorded behavior only (the model lists no auth errors on GetCallerIdentity). A request signed with an access key id that EXISTS but with the wrong secret access key fails signature verification: error code SignatureDoesNotMatch, HTTP 403, message fixed text 'The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details.' InvalidClientTokenId is reserved for a key id that does not resolve at all.
Emulator: returns code 'InvalidClientTokenId' for a real key id with a wrong secret, conflating 'unknown key' with 'bad signature'.
Why: the two codes drive different client behavior — SignatureDoesNotMatch points at clock skew / signing-key derivation / a wrong secret, InvalidClientTokenId points at a deleted or wrong-partition key; SDK credential-refresh and retry logic branch on them, so a caller is sent to debug the wrong thing.
Fix: the request-authentication component (the SigV4 verification path shared by the services) — distinguish 'key id not found' from 'computed signature mismatch' and emit SignatureDoesNotMatch with the documented message for the latter.
Traces: sts.get-caller-identity-rejects-wrong-secret-key.

Cause 7 of 9: query-string (presigned) SigV4 requests are not accepted — Action is only read from the POST form body.
AWS: Smithy — the service's protocol is awsQuery; the SigV4 'Authenticating Requests: Using Query Parameters' scheme is an alternative to the Authorization header for any SigV4 service, so a presigned GET whose Action/Version and X-Amz-* parameters live in the query string is valid. A presigned GetCallerIdentity URL returns HTTP 200 and a GetCallerIdentityResponse whose Arn/Account/UserId equal what the signed POST path returns for the same credentials; corrupting the signature yields HTTP 403 with code exactly SignatureDoesNotMatch.
Emulator: the presigned GET is refused with HTTP 400 and the XML body '<ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code><Message>unknown action: request is missing the `Action` field</Message></Error></ErrorResponse>' — the query-string Action parameter is not consulted at all.
Why: `aws eks get-token`, aws-iam-authenticator and every Kubernetes/webhook integration that proves identity by handing over a presigned STS URL cannot work against the emulator; the request is rejected before authentication is even attempted.
Fix: the awsQuery request parser / protocol front end — resolve the Action (and Version) from the query string for GET requests as well as from the x-www-form-urlencoded body, and accept query-parameter SigV4 as an authentication scheme.
Traces: sts.presigned-get-caller-identity-url-is-authenticated-by-query-string-sigv4.

Cause 8 of 9: an unsigned AssumeRoleWithWebIdentity request is not routed to STS and answers a JSON UnknownService envelope.
AWS: Smithy — AssumeRoleWithWebIdentity is modeled with optionalAuth (auth: []), i.e. it is callable with NO SigV4 credentials, and the service protocol is awsQuery with xmlNamespace https://sts.amazonaws.com/doc/2011-06-15/. A well-formed but nonexistent RoleArn with a garbage (unparseable) WebIdentityToken is refused with the modeled InvalidIdentityTokenException — awsQuery code InvalidIdentityToken, HTTP 400, message fixed text 'The ID Token provided is not a valid JWT. (You may see this error if you sent an Access Token)' — serialized as an XML ErrorResponse.
Emulator: because the request carries no SigV4 credential scope, no X-Amz-Target and (for this client) no usable Host header, the front end cannot resolve a target service and answers a JSON body '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'. botocore then fails with ResponseParserError ('invalid XML received') because an awsQuery client is handed JSON.
Why: every anonymous STS entry point (AssumeRoleWithWebIdentity, AssumeRoleWithSAML) is unreachable, and the response is not even in the client's protocol, so the caller gets a parser crash rather than a modeled error.
Fix: the service-routing front end — route unsigned awsQuery requests by the Action parameter (and by endpoint/Host) so anonymous STS operations reach the STS handlers, and ensure the no-route fallback serializes in the requested protocol.
Traces: sts.assume-role-with-web-identity-nonexistent-role-denied. Related open issue 20261024T154246Z-sts-assumerolewithwebidentity-is-unroutable-unknownservice-trust-policy-conditions.md records the same unroutability for other checks and owns their replay keys; it does not carry this check, so it is filed here and that issue is named for cross-reference.

Cause 9 of 9: an AssumeRole ALLOW is not served from cache after the trust policy tightens.
AWS: recorded behavior only (nothing in the model describes it). After UpdateAssumeRolePolicy tightens a trust policy so the caller no longer satisfies the Allow, GetRole reflects the new document essentially immediately while AssumeRole keeps returning fresh, fully usable credentials for roughly five minutes before the first refusal; the refusal, once it arrives, is exactly AccessDenied. So the latency to the first denial is strictly greater than the latency to the control plane reporting the new document.
Emulator: the very FIRST AssumeRole issued after the tightening is already refused — the control plane reported the new document at 0.0s and the refusal arrived at 0.0s, with zero cached ALLOWs observed.
Why: a client written against AWS's eventual consistency (a revocation runbook that waits out the cache, a test that asserts a tightening is NOT yet effective, retry/backoff tuned to that window) behaves differently against the emulator; conversely code that passes here because the change is instantaneous will leave a real-AWS window of continued access unhandled.
Fix: the AssumeRole handler's trust-policy decision path / the session-authorization state — model the ALLOW-decision cache so a tightened trust policy becomes effective on AssumeRole only after the configured propagation window, while GetRole reflects it immediately.
Traces: sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator (62/172 passed). Verbatim replay output per cause:

Cause 1 — sts.assume-role-external-id-success-with-correct-id: "AssertionError: temporary AssumeRole access key id must be in the ASIA class; got 'EMUL'"; sts.assume-role-packed-policy-too-large-rejected: "AssertionError: expected an ASIA-prefixed temporary AccessKeyId, got 'EMULATORASYO5XOV4OJO'"; sts.get-access-key-info-resolves-temporary-asia-key: "AssertionError: expected an ASIA-prefixed temporary access key from AssumeRole, got prefix 'EMUL'"; sts.get-access-key-info-returns-correct-account: "AssertionError: expected a temporary (ASIA) access key id from GetSessionToken, got 'EMUL'"; sts.get-caller-identity-userid-matches-created-role-id: "AssertionError: a temporary AssumeRole access key id must be in the ASIA class; got prefix 'EMUL'"; sts.get-federation-token-policy-caps-caller-permissions: "AssertionError: a federation token's access key id must be in the ASIA class; got prefix 'EMUL'".
Expected: AccessKeyId starts with 'ASIA'.

Cause 2 — sts.assume-role-managed-policy-arns-exceeding-max-count-rejected, at `assert message == _OVER_LIMIT_MESSAGE` (the preceding `assert code == "ValidationError"` passed): got "1 validation error detected: Value '[arn:aws:iam::aws:policy/ReadOnlyAccess, ... , arn:aws:iam::aws:policy/AmazonSQSReadOnlyAccess]' at 'policyArns' failed to satisfy constraint: Member must have length less than or equal to 10".
Expected message: "The limit of ARNs for the PolicyArns parameter was exceeded. 10 ARNs can be requested at most in an API request."

Cause 3 — sts.assume-role-session-tags-unicode-value-round-trip, at `_error(lambda: no_tag_iam.list_roles())`: "AssertionError: expected the call to be refused, but it succeeded" (tag value under test: "Café-Zürich-北京"). sts.e2e-session-tag-propagation-through-role-chain: "AssertionError: condition not met within 30s; last=('ok', 'ListObjectsV2 succeeded')". sts.assume-role-transitive-tag-survives-role-chaining: "Error: Check timed out after 60.0s" while polling for the denial that never arrives.
Expected: AccessDenied, HTTP 403, on the call made under a session whose aws:PrincipalTag value does not satisfy the policy condition.

Cause 4 — sts.server-side-source-identity-missing-permission-rejected: "AssertionError: unexpected refusal message: expected 'User: <assumed-role session ARN> is not authorized to perform: sts:AssumeRole on resource: <target role ARN>', got 'User: <same session ARN> is not authorized to perform: sts:SetSourceIdentity on resource: <same role ARN>'" (the ARNs in the recorded run are per-run generated names and only fill the template).

Cause 5 — sts.role-chain-with-a-conflicting-source-identity-denied-and-the-matching-one-permitted: "Error: Check timed out after 60.0s". The check's own `_poll_denied` docstring states it is only ever called after a positive control proved the credential live, so "a permissive implementation simply never reaches the condition and the deadline miss IS the verdict".

Cause 6 — sts.get-caller-identity-rejects-wrong-secret-key: "AssertionError: expected SignatureDoesNotMatch for a real access key id paired with a wrong secret, got 'InvalidClientTokenId'".

Cause 7 — sts.presigned-get-caller-identity-url-is-authenticated-by-query-string-sigv4: "AssertionError: a freshly presigned GetCallerIdentity URL was not honoured: expected HTTP 200, got 400 with body starting '<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code><Message>unknown action: request is missing the `Action` field</Message></Error></ErrorResponse>'".

Cause 8 — sts.assume-role-with-web-identity-nonexistent-role-denied: "botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed: b'{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'".
Expected: XML ErrorResponse, code InvalidIdentityToken, HTTP 400, message "The ID Token provided is not a valid JWT. (You may see this error if you sent an Access Token)".

Cause 9 — sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens: "AssertionError: AssumeRole was refused on the FIRST attempt after the trust policy was tightened, with no cached ALLOW observed: the control plane reported the new document at 0.0s and the refusal arrived at 0.0s."

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay suite simparity-behavioral-ga-prod-v1.
2. Re-replay exactly the check ids listed in trace_ids; each cause's checks are named under its own 'Traces:' line in the description.
3. Manual repro per cause:
   - Cause 1: sts.assume_role(RoleArn=<any assumable role>, RoleSessionName='x') and inspect Credentials['AccessKeyId'] — it starts with 'EMUL', not 'ASIA'. Same for get_session_token() and get_federation_token(Name='x').
   - Cause 2: sts.assume_role(RoleArn=<role>, RoleSessionName='x', PolicyArns=[{'arn': a} for a in 11 distinct AWS managed policy ARNs]) — the code is ValidationError but the message is the generic constraint rendering.
   - Cause 3: create a role whose inline policy allows s3:ListAllMyBuckets only when aws:PrincipalTag/<key> StringEquals a value; assume it WITHOUT that tag and call s3.list_buckets() — it succeeds instead of AccessDenied. Repeat with TransitiveTagKeys across a second AssumeRole hop.
   - Cause 4: chain AssumeRole with SourceIdentity set into a role whose trust policy grants sts:AssumeRole but not sts:SetSourceIdentity — the AccessDenied message names sts:SetSourceIdentity.
   - Cause 5: assume role A with SourceIdentity='alice', then from that session assume role B whose trust policy requires sts:SourceIdentity StringEquals 'alice'; repeat from a session with SourceIdentity='bob' — the second is not refused.
   - Cause 6: sign a GetCallerIdentity request with an existing access key id and a deliberately wrong secret — the code is InvalidClientTokenId instead of SignatureDoesNotMatch.
   - Cause 7: presign GetCallerIdentity (query-string SigV4) and GET the URL with plain HTTP — HTTP 400 InvalidAction 'request is missing the `Action` field'.
   - Cause 8: call AssumeRoleWithWebIdentity with no credentials, a well-formed nonexistent RoleArn and WebIdentityToken='eyJhbGciOiJSUzI1NiJ9.test-token' — a JSON UnknownService body comes back to an awsQuery client.
   - Cause 9: create a role trusting the caller, AssumeRole successfully, UpdateAssumeRolePolicy to a document the caller no longer satisfies, then AssumeRole again immediately — it is refused on the first attempt with no cached ALLOW.
