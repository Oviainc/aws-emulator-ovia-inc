Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# UpdateIdentitySource turns a Cognito source into an OpenID Connect source while keeping the Cognito-only default principalEntityType, the state CreateIdentitySource refuses to create

_Filed: 2026-10-28T20:55:35Z_
_Operation: UpdateIdentitySource | Class: validation_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIdentitySource` resolves an omitted `principalEntityType` to `crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE` (`AWS::Cognito`) for a Cognito user pool source, and for an OpenID Connect source it REFUSES the request rather than synthesise an entity type the caller never chose.

`UpdateIdentitySource` replaces the stored configuration with whatever arm the request carries and applies no such check: when a Cognito source that took the service default is updated to an `openIdConnectConfiguration` and the update carries no `principalEntityType`, the handler writes the OIDC configuration and leaves `principal_entity_type` at `AWS::Cognito`.

The result is exactly the state the create path exists to prevent: `GetIdentitySource`/`ListIdentitySources` report an OpenID Connect source whose `principalEntityType` is the Cognito-only default the caller never asked for, and `IsAuthorizedWithToken` then mints principals of that type for tokens from the OIDC issuer (`token::principal_entity` reads `source.principal_entity_type` straight off the stored source). One of the two sibling writers of this field enforces the rule and the other does not, so the same field can hold a value the crate's own constant reserves for Cognito sources.

## Evidence

The rule, stated and enforced on the create path — `src/ops/create_identity_source.rs:258`:

```
        let principal_entity_type = match (&input.principal_entity_type, &configuration) {
            (Some(entity_type), _) => entity_type.clone(),
            (None, crate::state::IdentitySourceConfiguration::CognitoUserPool(_)) => {
                crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE.to_string()
            }
            (None, crate::state::IdentitySourceConfiguration::OpenIdConnect(_)) => {
                return Err(invalid(crate::ops::InvalidInput::new(
                    crate::ops::paths::PRINCIPAL_ENTITY_TYPE,
                    "an OpenID Connect identity source generates principals of the entity type \
                     the request names, so it cannot be omitted",
                )));
            }
        };
```

with the reason spelled out at `src/ops/create_identity_source.rs:252`: "Rather than SYNTHESISE an entity type that would then be reported as if the caller had chosen it, the request is refused ... the alternative (inventing the value a read must report) is the one thing an emulator must not do."

The constant is Cognito-only by its own doc, `src/state.rs:89`:

```
/// The Cedar entity type a Cognito identity source's principals carry when the
/// create request named no `principalEntityType`.
...
/// for an Amazon Cognito user pool
/// source that default is the `AWS::Cognito` entity type Verified Permissions
/// generates principals as when the policy store maps none of its own.
pub const DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE: &str = "AWS::Cognito";
```

The sibling writer that skips the check, `src/ops/update_identity_source.rs:263`:

```
        source.configuration = configuration;
        // "Specifies the data type of principals generated for identities
        // authenticated by the identity source": optional here and REQUIRED on
        // every read, so a request naming none leaves the stored value as it is
        // rather than clearing a member a read must still report.
        if let Some(principal_entity_type) = input.principal_entity_type.clone() {
            source.principal_entity_type = principal_entity_type;
        }
```

The arm reached is `UpdateConfiguration::openIdConnectConfiguration` (`src/ops/update_identity_source.rs:166`), so the stored configuration becomes `IdentitySourceConfiguration::OpenIdConnect` while `principal_entity_type` stays `AWS::Cognito`.

The value is then used for the decision, `src/state/token.rs:518`:

```
    Ok(ScopeEntity {
        entity_type: source.principal_entity_type.clone(),
        entity_id: prefixed_id(prefix.as_deref(), value),
    })
```

No test pins this combination: the Cognito→OIDC row in `tests/all/update_and_delete_lifecycle.rs:231` creates the source WITH `"principalEntityType": "MyCorp::User"`, and the row that relies on the Cognito default (`tests/all/update_and_delete_lifecycle.rs:210`, expecting `"AWS::Cognito"`) stays on a Cognito configuration.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode OFF.
2. CreateIdentitySource with {"policyStoreId": "<id>", "configuration": {"cognitoUserPoolConfiguration": {"userPoolArn": "arn:aws:cognito-idp:us-east-1:000000000000:userpool/us-east-1_1a2b3c4d5"}}} and NO principalEntityType. GetIdentitySource reports principalEntityType "AWS::Cognito" (the service default for a Cognito source).
3. Call UpdateIdentitySource with {"policyStoreId": "<id>", "identitySourceId": "<IS…>", "updateConfiguration": {"openIdConnectConfiguration": {"issuer": "https://oidc.example.com", "tokenSelection": {"accessTokenOnly": {"principalIdClaim": "sub"}}}}} and NO principalEntityType.
4. Observe HTTP 200. GetIdentitySource now reports configuration.openIdConnectConfiguration together with principalEntityType "AWS::Cognito" — the Cognito-only default on an OIDC source, which the same request shape is refused for by CreateIdentitySource (400 ValidationException naming /principalEntityType).
5. Call IsAuthorizedWithToken with an access token from that issuer: the principal it decides for carries entityType "AWS::Cognito".
