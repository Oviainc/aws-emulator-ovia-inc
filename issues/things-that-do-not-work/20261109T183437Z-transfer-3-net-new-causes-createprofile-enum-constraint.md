Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-profile-invalid-type-rejected transfer.create-server-rejects-invalid-protocol-and-idp-config transfer.create-webapp-without-identity-center-denied
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-profile-invalid-type-rejected|run#4|AssertionError|assert error == ('ValidationException', "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [
Issue-Key: transfer.create-server-rejects-invalid-protocol-and-idp-config|run#10|AssertionError|assert msg_ftps == msg_ftp, (
Issue-Key: transfer.create-webapp-without-identity-center-denied|run#7|AssertionError|raise AssertionError(f'expected CreateWebApp to be denied, but it succeeded and returned {created!r}')
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# transfer: 3 net-new causes — CreateProfile enum-constraint message orders the value set wrongly, CreateServer refuses FTPS with the certificate rule instead of the SERVICE_MANAGED family rule, and CreateWebApp is not authorization-gated at all

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebApp

## Description
Three distinct root causes in Transfer's request-validation and authorization surface, each grounded in a replayed behavioral check.

Cause 1 of 3: the CreateProfile enum-constraint refusal renders the ProfileType value set in the wrong order.
AWS: Smithy — CreateProfile.ProfileType is required and targets enum ProfileType; a value outside the set is refused before the operation's modeled errors with ValidationException, HTTP 400, message "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [PARTNER, LOCAL]" (recorded behavior for the ordering; the model declares the members as LOCAL, PARTNER). Fixed text: "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]"; the variable parts are the member name taken from the request ('profileType') and the enum members, which AWS renders as [PARTNER, LOCAL]. No profile is created.
Emulator: the code and status are right and no profile is created, but the message ends "Member must satisfy enum value set: [LOCAL, PARTNER]" — the model's declaration order rather than AWS's.
Why: a client or test that matches AWS's refusal text (the suite does, and so do SDK-level assertions and log scrapers) does not match; the message is a requirement, and its value-set ordering is part of it.
Fix: the enum-constraint message builder used by CreateProfile's request validation. Related but not the same operation: open issue 20261029T193150Z-transfer-refusal-fidelity-six-operations-answer-emulator-authored.md already reports an enum set rendered in the wrong order for other operations.
Traces: transfer.create-profile-invalid-type-rejected.

Cause 2 of 3: CreateServer with FTPS under IdentityProviderType=SERVICE_MANAGED is refused by the per-protocol certificate rule instead of the identity-provider family rule.
AWS: Smithy — CreateServer carries InvalidRequestException (client, HTTP 400) and Protocol enum [SFTP, FTP, FTPS, AS2]. Recorded behavior: AWS gates the FTP/FTPS FAMILY on the identity provider first, so both Protocols=['SFTP','FTP'] and Protocols=['SFTP','FTPS'] with EndpointType=PUBLIC and IdentityProviderType=SERVICE_MANAGED are refused with InvalidRequestException and the SAME message, "FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED" (fixed text; the variable part is the IdentityProviderType value from the request). The per-protocol rules (VPC endpoint for FTP, ACM Certificate for FTPS) are never reached from a SERVICE_MANAGED request.
Emulator: the FTP request gets the correct family message, but the FTPS request is refused with 'Certificate must be specified when Protocols includes FTPS' — a later per-protocol rule evaluated before the identity-provider gate.
Why: the caller is told to supply an ACM certificate, which AWS will still refuse because SERVICE_MANAGED cannot serve FTPS at all; the diagnostic sends the client down a configuration path that cannot succeed, and the two defects are no longer distinguishable from AWS's.
Fix: validation ordering in the CreateServer handler's validation — the Protocols/IdentityProviderType family gate must run before the FTPS certificate and FTP endpoint-type checks.
Traces: transfer.create-server-rejects-invalid-protocol-and-idp-config.

Cause 3 of 3: CreateWebApp performs no authorization check, so a caller with no sso:CreateApplication permission gets a web app.
AWS: Smithy — CreateWebApp declares AccessDeniedException (client, HTTP 403). Transfer creates the Identity Center application on the caller's behalf, so a caller lacking sso:CreateApplication is refused with AccessDeniedException and the message "User: <caller ARN> is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/<Identity Center instance id>/* because no resource-based policy allows the sso:CreateApplication action" — fixed text apart from the caller ARN (from the signing credentials) and the instance id (parsed from IdentityProviderDetails.IdentityCenterConfig.InstanceArn). No web app is created, so ListWebApps holds none with the request's tags.
Emulator: the call returns HTTP 200 with a WebAppId (a web app is created and listable) even when the IdentityCenterConfig names an SSO instance and a role the caller is not permitted to use.
Why: an overly permissive create — the denial path a client must handle cannot be exercised, and the caller is handed a web-app resource AWS would never have created, so code validated here fails on AWS and the emulator's state diverges (an extra billable resource).
Fix: the CreateWebApp handler's authorization gate (evaluate sso:CreateApplication for the caller before provisioning, and emit the modeled AccessDeniedException with AWS's message template).
Traces: transfer.create-webapp-without-identity-center-denied.

## Evidence
Cause 1: AssertionError: ('ValidationException', "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [LOCAL, PARTNER]") — expected message ends "enum value set: [PARTNER, LOCAL]"; code and status match.

Cause 2: AssertionError: FTP and FTPS share ONE family-level refusal under SERVICE_MANAGED; got 'Certificate must be specified when Protocols includes FTPS' vs 'FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED' — expected both CreateServer calls (Protocols ['SFTP','FTP'] and ['SFTP','FTPS'], EndpointType PUBLIC, IdentityProviderType SERVICE_MANAGED) refused InvalidRequestException with the identity-provider family message; actual: FTPS answered the certificate rule.

Cause 3: AssertionError: expected CreateWebApp to be denied, but it succeeded and returned {'WebAppId': 'webapp-00000000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — expected AccessDeniedException (403) with message 'User: <caller arn> is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/<instance id>/* because no resource-based policy allows the sso:CreateApplication action' and no web app created.

## Steps to Reproduce
1. Replay the behavioral suite checks listed in trace_ids against the emulator.
2. Cause 1: transfer:CreateProfile(As2Id=<any>, ProfileType='BOGUS') — compare the ValidationException message's enum value set ordering against AWS's [PARTNER, LOCAL].
3. Cause 2: transfer:CreateServer(Protocols=['SFTP','FTP'], EndpointType='PUBLIC', IdentityProviderType='SERVICE_MANAGED') and then the same call with Protocols=['SFTP','FTPS'] — the two InvalidRequestException messages differ; AWS returns the identical family-level message for both.
4. Cause 3: transfer:CreateWebApp(IdentityProviderDetails={'IdentityCenterConfig': {'InstanceArn': 'arn:aws:sso:::instance/ssoins-0000000000000000', 'Role': 'arn:aws:iam::<account>:role/does-not-exist'}}) as a caller without sso:CreateApplication — the call returns 200 with a WebAppId and the web app is listed by ListWebApps; AWS answers AccessDeniedException and creates nothing.
