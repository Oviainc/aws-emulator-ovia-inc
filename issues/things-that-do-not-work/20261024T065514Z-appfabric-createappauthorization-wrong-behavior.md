Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# An oauth2 app authorization is returned without authUrl, so the OAuth connect flow has no URL to follow

_Filed: 2026-10-24T06:55:14Z_
_Operation: CreateAppAuthorization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AppFabric's oauth2 authorization flow is: CreateAppAuthorization returns `authUrl` (the application's OAuth consent page), the admin visits it, and the authorization code that comes back is passed to ConnectAppAuthorization in `authRequest`. The modeled AppAuthorization member `authUrl` is documented as "The application URL for the OAuth flow" and real AppFabric populates it for an authorization created with authType=oauth2.

The emulator never computes the member: `AppAuthorizationRecord::to_app_authorization` hard-codes `auth_url: None`, so CreateAppAuthorization and GetAppAuthorization always answer an AppAuthorization with no authUrl, including for authType=oauth2. Expected: an oauth2 authorization reports an authUrl (a stable, emulator-served URL derived from the stored app/tenant is enough for the field to be present and to round-trip through a Get); apiKey authorizations legitimately omit it. Actual: the field is absent for every authorization, so a client driving the documented oauth2 flow cannot proceed past the create call.

## Evidence

Spec: shape AppAuthorization member `authUrl` (target String) — "The application URL for the OAuth flow"; ConnectAppAuthorization's `authRequest` doc describes the code returned "after permission is granted in the application OAuth page (after clicking on the AuthURL)", which only exists if the create call reported an AuthURL.
Handler/state: src/state.rs lines 350-356 — `auth_url: None` with a comment that no URL is served; src/ops/create_app_authorization.rs lines 339-362 builds the response from that record, and src/ops/get_app_authorization.rs line 180 reads the same.
Test coverage: no test in tests/all/ mentions authUrl, so nothing pins the absence.

## Steps to Reproduce

1. POST /appbundles -> note appBundle.arn
2. POST /appbundles/{arn}/appauthorizations with app=SLACK, authType=oauth2, credential.oauth2Credential={clientId,clientSecret}, tenant={tenantIdentifier,tenantDisplayName}
3. Inspect the 201 body: appAuthorization has no authUrl member, where AWS returns the application's OAuth consent URL
4. GET /appbundles/{arn}/appauthorizations/{authorizationArn} -> still no authUrl
