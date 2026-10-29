Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.create-session-native-x509-round-trip rolesanywhere.create-session-disabled-profile-rejected rolesanywhere.access-denied-without-permission-rejected rolesanywhere.create-trust-anchor-malformed-certificate-rejected rolesanywhere.put-attribute-mapping-invalid-specifier-rejected
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.create-session-native-x509-round-trip|_poll_create_session#13|Timeout|time.sleep(3)
Issue-Key: rolesanywhere.create-session-disabled-profile-rejected|_poll_create_session#13|Timeout|time.sleep(3)
Issue-Key: rolesanywhere.access-denied-without-permission-rejected|_poll_for_stable_denial#25|Timeout|time.sleep(interval_s)
Issue-Key: rolesanywhere.create-trust-anchor-malformed-certificate-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: rolesanywhere.put-attribute-mapping-invalid-specifier-rejected|run#8|AssertionError|assert error.get("Code") == "ValidationException", f"expected ValidationException, got {error.get('Code')!r}"
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# rolesanywhere does not serve the native CreateSession data plane and accepts calls AWS refuses

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSession

## Description
Four net-new root causes on the rolesanywhere request-acceptance surface: the native X.509 data plane is not served at all, IAM authorization is not enforced, CreateTrustAnchor accepts certificate data AWS rejects, and PutAttributeMapping answers NotImplemented where AWS validates. Four causes, numbered below; a fixer must close all four before submitting.

Cause 1 of 4 (CreateSession - the native data plane is never served):
AWS: CreateSession is IAM Roles Anywhere's native data plane. It is NOT in this checkout's Smithy model (smithy_query query=operation operation=CreateSession on services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json answers "operation 'CreateSession' not found"), so this is recorded behavior only. AWS serves it as POST /sessions on the rolesanywhere endpoint, hand-signed with the SigV4-derived algorithm AWS4-X509-RSA-SHA256: the Authorization header carries Credential=<client-certificate serial number>/<date>/<region>/rolesanywhere/aws4_request, the signature is an RSA PKCS#1 v1.5 SHA-256 signature over the standard string-to-sign, and the DER client certificate is base64 in X-Amz-X509. With an enabled profile whose roleArns name a role trusting rolesanywhere.amazonaws.com, and a trust anchor holding the signing certificate's CA, AWS answers 201 with a body containing exactly one credentialSet entry: roleArn echoing the requested role, assumedRoleUser whose arn ends in '/<session name>' where the default session name is the signing certificate's serial number hex-encoded, and credentials {accessKeyId, secretAccessKey, sessionToken, expiration} that are usable to call sts:GetCallerIdentity and resolve to an :assumed-role/<role name>/ identity.
Emulator: POST /sessions never answers 201. The check polls the signed request until its budget is exhausted and the check reports 'Check timed out after 60.0s', so no credential set is ever vended; the operation has no handler in the service crate (no src/ops/create_session.rs exists and the operation is absent from the model the handlers are generated from).
Why: the entire reason to use IAM Roles Anywhere is exchanging an X.509 identity for temporary AWS credentials. A client built against this emulator can create trust anchors and profiles but can never obtain credentials, so the product's only data-plane path is untestable and any workload wired to the emulator hangs or retries until it gives up rather than receiving a modelled refusal.
Fix: a CreateSession handler for the service (POST /sessions) plus its route registration and state wiring - verify the X-Amz-X509 certificate chains to a registered, enabled trust anchor, resolve the profile/role, and vend a credentialSet with the hex-serial default session name; it does not exist yet, so it must be added alongside the generated ops rather than corrected in one.
Traces: rolesanywhere.create-session-native-x509-round-trip, rolesanywhere.create-session-disabled-profile-rejected.

Cause 2 of 4 (CreateSession against a DISABLED profile is not refused):
AWS: when the profile named by profileArn has enabled=false, the same signed CreateSession request is refused 403 with error code AccessDeniedException - the response carries a Coral-style x-amzn-ErrorType header whose part before the ':' is AccessDeniedException - and the body contains no credentialSet. Re-enabling the profile makes the identical signed request succeed again with 201 (recorded behavior only; CreateSession is not in the model, though the control-plane operations model AccessDeniedException with httpError 403).
Emulator: unreachable - the positive-control leg of the same check never gets a 201 from /sessions (cause 1), so the disabled-profile refusal is never exercised; the check dies on the 60s budget. Independently, DisableProfile is an unimplemented stub (covered by its own auto-filed issue), so even a CreateSession handler would have no disabled profile to refuse.
Why: 'a disabled profile vends no credentials' is the kill switch operators rely on; an emulator that cannot express it lets a client conclude revocation worked when nothing was checked.
Fix: the CreateSession handler added in cause 1 must consult the profile's enabled flag (and the trust anchor's) before vending, and refuse with 403 AccessDeniedException and the matching x-amzn-ErrorType header.
Traces: rolesanywhere.create-session-disabled-profile-rejected.

Cause 3 of 4 (CreateProfile ignores an explicit IAM Deny on the calling identity):
AWS: CreateProfile models AccessDeniedException with httpError 403 (Smithy: CreateProfile errors AccessDeniedException httpError 403, ValidationException httpError 400). A request signed with an IAM user's own access key, where that user carries an inline policy with Effect Deny for Action rolesanywhere:CreateProfile, is refused with HTTP 403 at the authorization layer and creates no profile - the name is absent from ListProfiles afterwards. Deleting the Deny statement makes the identical call succeed and the profile becomes visible in ListProfiles.
Emulator: the call signed with the denied identity's credentials keeps succeeding. The check's denial poll repeatedly creates and deletes the profile waiting for a non-success answer and never sees one, so the check reports 'Check timed out after 60.0s'. The request's signing identity and its attached policies are not consulted on the rolesanywhere control plane at all.
Why: any negative authorization test is silently inverted. A caller validating least-privilege policy against the emulator sees every scoped-down or explicitly-denied identity succeed, so a policy that actually blocks the workload in production looks correct here, and a 403 the client is written to handle never arrives.
Fix: the service's request-authorization path (the per-operation authorization check applied before the CreateProfile handler's state write) must evaluate the caller's identity policies and refuse with 403 AccessDeniedException without mutating state.
Traces: rolesanywhere.access-denied-without-permission-rejected.

Cause 4 of 4 (CreateTrustAnchor accepts a CERTIFICATE_BUNDLE source that is not a PEM certificate):
AWS: CreateTrustAnchor.source is @required and targets Source {sourceType: TrustAnchorType, sourceData: SourceData}; the operation models ValidationException with httpError 400 (Smithy: CreateTrustAnchor, source -> Source, errors ValidationException httpError 400). With sourceType CERTIFICATE_BUNDLE and an x509CertificateData value that is not a PEM-encoded X.509 certificate, AWS refuses the request 400 ValidationException whose message is the FIXED text 'Error creating TrustAnchor. Bad certificate data' - it has no variable part, no submitted value and no id interpolated (recorded behavior only; the model does not document the message). No trust anchor is created, so the submitted name does not appear in ListTrustAnchors. The same call shape with a genuine CA certificate succeeds.
Emulator: answers 201 Created and mints a trust anchor whose source.sourceData.x509CertificateData is the junk string verbatim, with enabled=true and a trustAnchorArn, i.e. the certificate data is never parsed. Note this is a DIFFERENT cause from the open issue '20261029T192145Z-rolesanywhere-validation-refusals-render-patterns-and-enum-sets.md' cause 2, which covers the unvalidated AWS_ACM_PCA source; this one is the CERTIFICATE_BUNDLE PEM parse, and neither covers the other.
Why: an unusable trust anchor is stored and reported as healthy. A client that relies on the service to reject bad certificate material believes its anchor can authenticate certificates; the failure then surfaces only as unexplained session rejections, and the negative path cannot be tested at all against the emulator.
Fix: the CreateTrustAnchor handler's validation of the source member - parse x509CertificateData as PEM for sourceType CERTIFICATE_BUNDLE and refuse with 400 ValidationException and the fixed message before any state write.
Traces: rolesanywhere.create-trust-anchor-malformed-certificate-rejected.

Also recorded here because the suite did not mark it as covered: rolesanywhere.put-attribute-mapping-invalid-specifier-rejected refuses with error Code 'NotImplemented' instead of 400 ValidationException with the message 'Unable to validate specifier (CN) on the target (SUBJECT_ALTERNATIVE_NAME)' (fixed text 'Unable to validate specifier (' / ') on the target (' / ')', variable parts the submitted specifier and the certificateField's internal target name). Its root cause is the unimplemented PutAttributeMapping handler (services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/put_attribute_mapping.rs) that the suite's auto-filed UNIMPLEMENTED PutAttributeMapping issue already owns; once that handler exists it must also perform this specifier/certificateField compatibility validation (x509SAN accepts only DNS, URI and Name-style specifiers) and must not persist the rejected mapping rule.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Overall 2/26 passed; rolesanywhere 2/26 (8%), 24 failed (12 of them blocked by already-filed unimplemented stubs).

Cause 1 - rolesanywhere.create-session-native-x509-round-trip [mismatch]:
  Error: Check timed out after 60.0s
  The check signs POST <rolesanywhere endpoint>/sessions with AWS4-X509-RSA-SHA256 and polls via _poll_create_session until status == 201; it asserts result['status'] == 201, len(credentialSet) == 1, entry['roleArn'] == the role ARN, entry['assumedRoleUser']['arn'].endswith('/' + hex(cert serial)), and that entry['credentials'] call sts:GetCallerIdentity to an ':assumed-role/<role>/' Arn. None of these is reached: 201 never arrives.
  Corroboration: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/create_session.rs does not exist, and smithy_query for CreateSession on services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json answers "operation 'CreateSession' not found".

Cause 2 - rolesanywhere.create-session-disabled-profile-rejected [mismatch]:
  Error: Check timed out after 60.0s
  Expected after disable_profile: status 403, x-amzn-ErrorType prefix 'AccessDeniedException', and no 'credentialSet' key in the body; expected on the restore leg: 201 again. Actual: the positive-control poll before the disable never returns 201, so the budget expires.

Cause 3 - rolesanywhere.access-denied-without-permission-rejected [mismatch]:
  Error: Check timed out after 60.0s
  Expected: with an inline Deny for rolesanywhere:CreateProfile on the calling IAM user, CreateProfile answers HTTP status 403 and the profile name is absent from ListProfiles. Actual: every attempt with those credentials returns a created profile (the check's _poll_for_stable_denial deletes each unexpected profile and retries until the 60s budget is gone, then the check times out). No 4xx is ever produced for the denied identity.

Cause 4 - rolesanywhere.create-trust-anchor-malformed-certificate-rejected [mismatch]:
  Expected: ClientError with Code 'ValidationException' and Message 'Error creating TrustAnchor. Bad certificate data'.
  Actual: HTTP 201 with a created trust anchor - AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'trustAnchor': {'trustAnchorId': '<uuid>', 'trustAnchorArn': 'arn:aws:rolesanywhere:<region>:<account>:trust-anchor/<uuid>', 'name': '<submitted name>', 'source': {'sourceType': 'CERTIFICATE_BUNDLE', 'sourceData': {'x509CertificateData': 'not a valid PEM certificate'}}, 'enabled': True, 'createdAt': ..., 'updatedAt': ..., 'notificationSettings': []}} (ids/names/timestamps are per-run values, not requirements).

Supplementary - rolesanywhere.put-attribute-mapping-invalid-specifier-rejected [mismatch]:
  AssertionError: expected ValidationException, got 'NotImplemented' (for PutAttributeMapping with certificateField='x509SAN', mappingRules=[{'specifier': 'CN'}]).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1.
2. Cause 1: run check rolesanywhere.create-session-native-x509-round-trip - generate a CA + leaf certificate, CreateTrustAnchor with sourceType CERTIFICATE_BUNDLE holding the CA PEM, CreateProfile (enabled=true) pointing at an IAM role that trusts rolesanywhere.amazonaws.com, then POST {profileArn, roleArn, trustAnchorArn} to <rolesanywhere endpoint>/sessions signed AWS4-X509-RSA-SHA256 with the leaf key, Credential=<cert serial>/<date>/<region>/rolesanywhere/aws4_request and the DER cert base64 in X-Amz-X509. Expect 201 with one credentialSet; observe the request never returns 201.
3. Cause 2: run check rolesanywhere.create-session-disabled-profile-rejected - after the same setup, DisableProfile and re-issue the identical signed POST /sessions. Expect 403 with x-amzn-ErrorType AccessDeniedException and no credentialSet.
4. Cause 3: run check rolesanywhere.access-denied-without-permission-rejected - create an IAM user with an Allow for rolesanywhere:CreateProfile/ListProfiles/TagResource and iam:PassRole, confirm CreateProfile succeeds with that user's own access key, then attach a second inline policy with Effect Deny for rolesanywhere:CreateProfile and re-issue the identical CreateProfile. Expect HTTP 403 and no profile in ListProfiles; observe it still succeeds.
5. Cause 4: run check rolesanywhere.create-trust-anchor-malformed-certificate-rejected - POST /trustanchors with source {sourceType: CERTIFICATE_BUNDLE, sourceData: {x509CertificateData: 'not a valid PEM certificate'}}. Expect 400 ValidationException 'Error creating TrustAnchor. Bad certificate data' and nothing in ListTrustAnchors; observe 201 and a stored anchor.
6. Supplementary: PUT /profiles/{profileId}/mappings with certificateField x509SAN and mappingRules [{specifier: 'CN'}]. Expect 400 ValidationException 'Unable to validate specifier (CN) on the target (SUBJECT_ALTERNATIVE_NAME)'; observe error code NotImplemented.
