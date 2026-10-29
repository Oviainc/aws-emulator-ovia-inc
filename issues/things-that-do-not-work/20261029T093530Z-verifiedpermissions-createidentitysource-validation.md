Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f

# A cognitoUserPoolConfiguration.userPoolArn naming a pool that was never created is accepted, although this crate already looks pools up by id in token.rs

_Filed: 2026-10-29T09:35:30Z_
_Operation: CreateIdentitySource | Class: validation_

> _Note: this issue was identified against the code base at commit `d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIdentitySource` (and `UpdateIdentitySource`, through the same shared helper) resolves a Cognito `userPoolArn` only by STRING SHAPE — partition, account, region, and that the pool id's prefix matches the region. It never asks whether a user pool with that id exists. `crate::state::resolve_user_pool_arn` justifies that gap in its own doc comment by claiming the crate cannot reach the cognito-idp pool registry, and that claim is contradicted twice by this same crate: `Cargo.toml` declares the cognito-idp service crate as a dependency, and `crate::state::token::published_key_set` already reaches that crate's state and looks a pool up by id with `pools.pool(&user_pool_id)`.

So the one path that NEEDS the pool (token verification) uses the existing lookup, while the two paths that STORE the reference do not — and `CreateIdentitySource` models `ResourceNotFoundException` precisely for a request that "references a resource that doesn't exist".

The consequence is not cosmetic: a source created against a nonexistent pool is reported by `GetIdentitySource`/`ListIdentitySources` as a live source, and the first `IsAuthorizedWithToken` presenting a token whose `iss` is that pool's issuer takes the `published_key_set` -> `None` branch, which this crate classifies as `TokenRefusal::NotCheckable` and answers as `InternalServerException` (HTTP 500). A caller that only ever used the documented API therefore gets a server fault for state the control plane accepted.

## Evidence

The comment that states the limitation, in `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:617-623`:

    /// What this CANNOT check: whether a pool with that id was ever created. The
    /// cognito-idp emulator holds its user pools in its own state struct, and
    /// [`service_state`] reaches per-service state BY TYPE — that type lives in the
    /// cognito-idp crate, which this crate does not depend on (one crate per
    /// service), and no shared crate publishes a pool registry. The partition, the
    /// account, the region and the region the pool id names are therefore the whole
    /// of the resolution this service can perform.

The same crate DOES depend on it — `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/Cargo.toml:36`:

    service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice = { workspace = true }

and ALREADY looks a pool up by id through that dependency — `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state/token.rs:666-675`:

    pub fn published_key_set<Eng: Send + Sync + 'static>(engine: &Eng, issuer: &str) -> Option<Value> {
        let pools = ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
            ::state::service_state(engine)?;
        ...
        let pool = pools.pool(&user_pool_id)?;

The lookup it calls is public — `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/state.rs:811-814`:

    /// The stored pool, or `None` when no pool carries that id.
    pub fn pool(&self, user_pool_id: &str) -> Option<&UserPool> {
        self.user_pools.get(user_pool_id)
    }

The create path that skips it — `src/ops/create_identity_source.rs:198-203`:

        let region = crate::state::request_region(ctx.parts());
        let configuration = crate::ops::persisted_identity_source_configuration(
            &input.configuration,
            &region,
        )
        .map_err(invalid)?;

and the shared helper that performs only the string resolution — `src/ops/mod.rs:419-421`:

            let (region, user_pool_id) =
                crate::state::resolve_user_pool_arn(user_pool_arn, request_region)
                    .map_err(|invalid| invalid.within(C::MEMBER_PATH))?;

The modelled member that references the pool, `CognitoUserPoolConfiguration$userPoolArn`: "The Amazon Resource Name (ARN) of the Amazon Cognito user pool that contains the identities to be authorized." `CreateIdentitySource` models `ResourceNotFoundException` (HTTP 404): "The request failed because it references a resource that doesn't exist."

The downstream 500 — `src/ops/is_authorized_with_token.rs:453-460`:

        crate::state::token::TokenRefusal::NotCheckable { issuer, reason } => {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: format!(
                    "the signature of the token the issuer {issuer} minted could not be \
                     checked: {reason}"

Test coverage: `tests/all/identity_sources.rs` and `tests/all/identity_source_refusal_pointers.rs` exercise the ARN-shape refusals (partition/account/region/pool-id-prefix); no test creates an identity source against a pool id that was never created by cognito-idp, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreatePolicyStore(validationSettings={mode: OFF}) and note the policyStoreId.
2. Without creating any Cognito user pool, call CreateIdentitySource(policyStoreId, configuration={cognitoUserPoolConfiguration: {userPoolArn: "arn:aws:cognito-idp:us-east-1:<emulator-account>:userpool/us-east-1_neverCreated"}}).
3. Observe HTTP 200 and a minted identitySourceId, instead of the modelled ResourceNotFoundException for the pool the request references.
4. GetIdentitySource(policyStoreId, identitySourceId) reports the configuration as a live source.
5. Present any token whose `iss` is https://cognito-idp.us-east-1.amazonaws.com/us-east-1_neverCreated to IsAuthorizedWithToken against that store and observe HTTP 500 InternalServerException ("... could not be checked: it publishes no key set this service can read").
