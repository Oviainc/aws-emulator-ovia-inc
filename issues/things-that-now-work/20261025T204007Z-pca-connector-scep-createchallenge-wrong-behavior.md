Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# CreateChallenge's ClientToken replay hands out a challenge password for a connector under teardown

_Filed: 2026-10-25T20:40:07Z_
_Operation: CreateChallenge | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateChallenge's `ClientToken` fast path returns the stored challenge — password included — BEFORE the connector is looked up, so it never applies the connector-state gate that the rest of the same handler applies. After `DeleteConnector` has been accepted (connector `Status` = `DELETING`, record still present until the teardown instant comes due), a repeat CreateChallenge carrying the token of the earlier successful call answers 202 with the challenge and its password, while the very same call with a fresh token is refused 400 `ValidationException`/`INVALID_STATE` two branches further down.

The sibling create in this crate decided the opposite for exactly this situation: `CreateConnector`'s token path explicitly refuses to answer with a resource that has been accepted for teardown and falls through instead. `CreateChallenge`'s token path carries no equivalent guard, so the two creates disagree about whether a `ClientToken` answers for a resource that is being torn down, and the handler's own comment about a connector that is not serving being unable to "hand one out" is broken by the branch above it.

## Evidence

The token path, taken before any connector lookup — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/create_challenge.rs:211-229:
```
        if let Some(existing) = store.challenge_for_token(input.client_token.as_deref()) {
            if let Some(challenge) = store.challenge(&existing.arn) {
...
                return Ok(CreateChallengeOutput {
                    challenge: Some(challenge.to_challenge()),
                });
```
(`to_challenge()` includes the password: state.rs:184-191 `password: Some(self.password.clone())`.)

The gate it skips — create_challenge.rs:254-266:
```
        // A connector that is not yet serving cannot authenticate a certificate request
        // with a challenge password, so it cannot hand one out either.
        if !connector.is_active() {
            return Err(CreateChallengeErrorModeled::ValidationException(ValidationException {
```

The sibling create's token path, which DOES exclude a resource under teardown — create_connector.rs:227-237:
```
        // `ClientToken` is how the caller says "this is a retry, not a second connector":
        // ... If that connector has since been
        // deleted — gone, or accepted for teardown and reporting `DELETING` — there is
        // nothing to answer with, so the call falls through and creates one.
        if let Some(existing) = store.connector_for_token(input.client_token.as_deref()) {
            if store
                .connectors
                .get(&existing.arn)
                .is_some_and(|connector| !connector.is_deleting())
```

And the crate's own statement of the rule — src/state.rs:311-314:
```
    /// True while the connector's teardown is running: it is still reported, but nothing can
    /// be built on it and it is about to disappear.
    pub fn is_deleting(&self) -> bool {
```

Test coverage: the connector side of this rule is pinned by tests/all/connector_deletion.rs:569 `a_client_token_does_not_answer_with_a_connector_under_teardown`; the challenge side has no equivalent — tests/all/connector_lifecycle.rs:793-830 only exercises CreateChallenge token reuse against ACTIVE connectors.

## Steps to Reproduce

1. CreateConnector with a valid `CertificateAuthorityArn` and no `MobileDeviceManagement`; wait for `Status` to settle to ACTIVE (TRANSITION_SPAN = 500ms).
2. CreateChallenge with `ConnectorArn` = that connector and `ClientToken` = "t1" -> 202, note `Challenge.Arn` and `Challenge.Password`.
3. DeleteConnector on that connector -> 202; GetConnector confirms `Status` = `DELETING`.
4. Within the teardown window, CreateChallenge again with the SAME `ConnectorArn` and `ClientToken` = "t1" -> observed: 202 with the challenge and its password.
5. Within the same window, CreateChallenge with the same `ConnectorArn` and a FRESH `ClientToken` = "t2" -> 400 `ValidationException` with `Reason` = `INVALID_STATE` (create_challenge.rs:256).
6. Compare with CreateConnector: repeat a create with the token of a connector that is `DELETING` -> the token is ignored and a new connector is minted (create_connector.rs:232-237).
