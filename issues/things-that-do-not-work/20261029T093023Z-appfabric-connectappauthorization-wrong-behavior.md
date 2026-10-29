Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# A repeat connect of an already-Connected app authorization re-stamps updatedAt, though the handler's own comment calls it "the same write"

_Filed: 2026-10-29T09:30:23Z_
_Operation: ConnectAppAuthorization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ConnectAppAuthorization` writes `status = Connected` and `updated_at = now()` unconditionally, with a comment stating that connecting an authorization that is already `Connected` "is the same write, so a retry is answered the same way". It is not the same write and the retry is not answered the same way: `updated_at` is moved to the instant of the retry, and `updatedAt` is a member of the `AppAuthorizationSummary` this operation returns as well as of the `AppAuthorization` that `GetAppAuthorization` and `ListAppAuthorizations` report. So a caller that re-sends the connect call (it carries no idempotency token and clients do retry it) sees the authorization's `updatedAt` advance although nothing about the record changed.

The crate decides the opposite way everywhere else: the sibling `UpdateAppAuthorization` only stamps when the call actually applies something — "a call that supplies NEITHER has no change to apply: it leaves the record — including its status and `updatedAt` — exactly as it stands" — and the ingestion suite pins the same rule for ingestion records ("Nothing changed the record, so nothing dated it either"). The fix is to stamp `updated_at` only when the connect actually moves the status, which leaves the first connect, every response body and every existing test unchanged.

## Evidence

The handler's comment and the write under it — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/connect_app_authorization.rs:212-220:
```
        // The handshake this emulator can complete: the authorization becomes
        // usable, which is the state the rest of the chain depends on. Connecting
        // one that is already `Connected` is the same write, so a retry is
        // answered the same way rather than refused.
        authorization.status = crate::state::APP_AUTHORIZATION_STATUS_CONNECTED.to_string();
        authorization.updated_at = ::std::time::SystemTime::now();
        let id = authorization.id.clone();
        // Read the response back out of the record this call just wrote.
        let app_authorization_summary = authorization.to_summary();
```
and `to_summary` reports that stamp — src/state.rs:482-490:
```
    pub fn to_summary(&self) -> AppAuthorizationSummary {
        AppAuthorizationSummary {
            ...
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
```

The sibling applies the opposite rule for the same record — src/ops/update_app_authorization.rs:285-288 and 308-317:
```
        // `credential` and `tenant` are both optional, so a call that supplies
        // NEITHER has no change to apply: it leaves the record — including its
        // status and `updatedAt` — exactly as it stands, and reports it.
        let applied = input.credential.is_some() || input.tenant.is_some();
```
```
        if applied {
            ...
            record.updated_at = ::std::time::SystemTime::now();
        }
```

The crate's own test suite states the rule for the other dated record — tests/all/ingestion_destination_loss.rs:238-243:
```
        } else {
            // Nothing changed the record, so nothing dated it either.
            assert!(
                (updated_after - updated_before).abs() < 1e-6,
                "case {case:?}: expected updatedAt to be left alone, got {updated_after:?} after {updated_before:?}"
            );
```

No test pins the re-stamp: tests/all/update_app_authorization_contract.rs:386-389 only asserts `updatedAt` does not go BACKWARDS, and tests/all/connected_authorization_rule.rs:133-147 asserts only that the set-up connect answers HTTP 200. (Model context: `AppAuthorization$updatedAt` is documented as "The timestamp of when the app authorization was last updated.")

## Steps to Reproduce

1. Create an app bundle, then CreateAppAuthorization with authType=apiKey and a matching credential.
2. POST /appbundles/{bundle}/appauthorizations/{auth}/connect — note status Connected and the `updatedAt` in the returned appAuthorizationSummary.
3. Wait a moment and POST the same connect call again.
4. Observe the second response's appAuthorizationSummary.updatedAt (and GetAppAuthorization's appAuthorization.updatedAt) is later than the first, although the record's status, tenant, app and tags are unchanged — the comment at connect_app_authorization.rs:212-215 claims the retry is "the same write" "answered the same way".
5. For contrast, PATCH the same authorization with an empty body: UpdateAppAuthorization applies nothing and leaves updatedAt exactly as it stands.
