Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# GetChallengePassword hands out the password of a challenge whose connector has been deleted and is tearing down

_Filed: 2026-10-28T16:00:34Z_
_Operation: GetChallengePassword | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetChallengePassword` resolves the challenge through its owning connector and returns the stored password without ever looking at that connector's `ConnectorStatus`. After `DeleteConnector` has been accepted (202, connector `Status` = `DELETING`, record still present until the teardown instant comes due), a `GetChallengePassword` on one of that connector's challenges answers 200 with the SCEP challenge password.

That contradicts the rule the same crate enforces on the only other path that hands a challenge password out. `CreateChallenge` gates on exactly this state with the crate's own helper (`!connector.is_active()` -> 400 `ValidationException`/`INVALID_STATE`) and states the reason in a comment that is not specific to minting: a connector that is not serving "cannot authenticate a certificate request with a challenge password, so it cannot hand one out either". The state module says the same about a connector under teardown: "nothing can be built on it and it is about to disappear". `GetChallengePassword` is the path that skips that existing helper, so the rule the crate enforces for one path is bypassable through the other.

The handler's own `not_found` doc already claims the behaviour the code does not implement: it says a read "after the owning connector was deleted" lands in the 404, while the sibling reader (`get_connector.rs`) is careful to spell out the distinction the crate actually models (404 only "once the teardown the delete scheduled has come due and the record is gone; until then the connector is reported with `Status` `DELETING`").

The modelled error this operation already declares covers the case: `ResourceNotFoundException`'s own doc says the resource "might have a status other than `ACTIVE`". Note this is specifically about the *password*: reporting the connector itself, or challenge *metadata*, while the record is in `DELETING` is deliberate (`GetConnector` is documented and tested to report `DELETING`), so nothing about the reported `Status` needs to change.

## Evidence

Model (smithy_query doc=ResourceNotFoundException, an error GetChallengePassword declares): "<p>The operation tried to access a nonexistent resource. The resource might be incorrectly specified, or it might have a status other than <code>ACTIVE</code>.</p>"

The read with no connector-state check — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/get_challenge_password.rs:162-167:
```
        let stored = store
            .challenge(&region, &input.challenge_arn)
            .ok_or_else(|| not_found(&input.challenge_arn))?;
        Ok(GetChallengePasswordOutput {
            password: Some(stored.password.clone()),
        })
```
and the lookup it uses, which filters only on Region — src/state.rs:453-458:
```
    pub fn challenge(&self, region: &str, challenge_arn: &str) -> Option<&StoredChallenge> {
        let connector_arn = connector_arn_of_challenge(challenge_arn)?;
        self.connector(region, connector_arn)?
            .challenges
            .get(challenge_arn)
    }
```

The sibling path that DOES apply the gate — src/ops/create_challenge.rs:269-279:
```
        // A connector that is not yet serving cannot authenticate a certificate request
        // with a challenge password, so it cannot hand one out either.
        if !connector.is_active() {
            return Err(CreateChallengeErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "Connector {connector_arn} has status {}; it must be {} to create a challenge",
```

The crate's own statement of the rule — src/state.rs:330-334:
```
    /// True while the connector's teardown is running: it is still reported, but nothing can
    /// be built on it and it is about to disappear.
    pub fn is_deleting(&self) -> bool {
        self.status == STATUS_DELETING
    }
```

The handler doc that the code under it breaks — src/ops/get_challenge_password.rs:171-173:
```
/// No challenge is stored under this ARN in the Region the request was signed for — HTTP 404
/// `ResourceNotFoundException`. A read after `DeleteChallenge`, after the owning connector was
/// deleted, or with an ARN naming another Region's challenge, lands here.
```
against the precise wording the sibling reader uses for the same situation — src/ops/get_connector.rs:174-177:
```
/// `ResourceNotFoundException`. A read after `DeleteConnector` lands here once the teardown the
/// delete scheduled has come due and the record is gone; until then the connector is reported
/// with `Status` `DELETING`.
```

Test coverage: the suite pins the rule for the CreateChallenge path only — tests/all/challenge_client_token.rs:279-285 (row "connector accepted for teardown" -> `Expected::Refused(400, "ValidationException", Some("INVALID_STATE"))`) and tests/all/challenge_client_token.rs:470-475:
```
                assert!(
                    !retried.body.to_string().contains(password.as_str()),
                    "case {name:?}: a refused retry must not hand out the challenge password, \
                     got {:?}",
```
No test calls `GetChallengePassword` while the owning connector is `DELETING` (grep for DELETING under tests/ matches no get_challenge_password assertion), so nothing currently pins the 200.

## Steps to Reproduce

1. CreateConnector with a valid `CertificateAuthorityArn` and no `MobileDeviceManagement`; let the pending transition settle so `GetConnector` reports `Status` = `ACTIVE`.
2. CreateChallenge with that `ConnectorArn` -> 202; note `Challenge.Arn` and `Challenge.Password`.
3. DeleteConnector on the connector -> 202; GetConnector confirms `Status` = `DELETING` (the record is still present until the teardown instant comes due, state.rs TRANSITION_SPAN).
4. Within that window, CreateChallenge with the same `ConnectorArn` and a fresh `ClientToken` -> 400 `ValidationException` with `Reason` = `INVALID_STATE` (create_challenge.rs:271).
5. Within the same window, GET /challengePasswords/{ChallengeArn} for the challenge from step 2 -> observed: 200 with `Password` equal to the password of a connector the caller has already deleted, while the sibling create refuses to hand that same password out one call earlier.
