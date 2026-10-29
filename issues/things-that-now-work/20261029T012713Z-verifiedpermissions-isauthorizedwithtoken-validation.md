Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0b0649dc1b525c86072f831fd9c0b574c3cd2f61

# IsAuthorizedWithToken enforces only the identity-source clause of its `entities` rule, so a slice carrying principals or an attributed action entity is accepted

_Filed: 2026-10-29T01:27:13Z_
_Operation: IsAuthorizedWithToken | Class: validation_

> _Note: this issue was identified against the code base at commit `0b0649dc1b525c86072f831fd9c0b574c3cd2f61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`IsAuthorizedWithTokenInput$entities` carries a three-part rule in its own `<important>` block: (a) "You can't include principals in this parameter, only resource and action entities", (b) "This parameter can't include any entities of a type that matches the user or group entity types that you defined in your identity source", and (c) "For action entities, you can include only their `Identifier` and `EntityType`".

The handler implements only (b). It builds `owned_entity_types` out of the matched identity source's `principalEntityType` plus the group entity type it maps, and refuses an `entities` item whose `uid.type` is one of those two — and nothing else. A slice naming any OTHER principal entity (e.g. `User::"mallory"` with its own attributes, in a store whose identity source generates `AWS::Cognito` principals) passes the check, is appended to the Cedar entity store beside the principal the token built, and is evaluated: a policy conditioned on that principal's attributes can then decide the request, which is exactly what clause (a) exists to prevent for a call whose principal must come "from **only** the `identityToken` or `accessToken` passed to the operation". Likewise an action entity carrying `attributes`, `parents` or `tags` is converted and evaluated, where (c) allows only its identifier and type.

The crate's own code shows the partial reading: the doc-comment on `owned_entity_types` quotes clause (b) verbatim and nothing else, so the two other clauses of the same sentence block were never implemented. The sibling single-request operation has the mirror-image gap already filed (`IsAuthorized` accepts action entities although its `entities` doc forbids actions), so this is the same class of omission on the token-based operation's own, differently-worded rule.

## Evidence

Smithy — `smithy_query(query="doc", shape="IsAuthorizedWithTokenInput", member="entities")`:
```
<important> <p>You can't include principals in this parameter, only resource and action entities. This parameter can't include any entities of a type that matches the user or group entity types that you defined in your identity source.</p> <ul> <li> <p>The <code>IsAuthorizedWithToken</code> operation takes principal attributes from <b><i>only</i></b> the <code>identityToken</code> or <code>accessToken</code> passed to the operation.</p> </li> <li> <p>For action entities, you can include only their <code>Identifier</code> and <code>EntityType</code>.</p> </li> </ul> </important>
```

The only check the handler makes — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/is_authorized_with_token.rs:286-320:
```
        // The principal and its groups are entities THIS call builds out of the token, so
        // a request supplying its own entities of those types is refused rather than
        // silently overridden — or colliding with them in the slice.
        let owned = &identity.owned_entity_types;
...
        let conflicting: Option<String> = supplied
            .iter()
...
            .find(|entity_type| owned.iter().any(|owned| owned == entity_type))
            .map(str::to_string);
        if let Some(entity_type) = conflicting {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::ENTITIES,
                format!(
                    "it carries an entity of type {entity_type}, which is an entity type the \
                     identity source that issued the token generates"
                ),
            )));
        }
        supplied.extend(identity.entities());
```
`owned` is exactly two types — src/state/token.rs:175-184:
```
    /// The entity types the identity SOURCE generates: its `principalEntityType` and, when
    /// it maps groups, the entity type it maps them to.
    ///
    /// They are the types an authorization request may not supply entities of —
    /// "this parameter can't include any entities of a type that matches the user or
    /// group entity types that you defined in your identity source" (the operation's
    /// `entities` documentation) — because this call builds those entities itself, out of
    /// the token.
    pub owned_entity_types: Vec<String>,
```
— i.e. the crate quotes clause (b) and leaves (a) and (c) unquoted and unimplemented. They are built in src/state/token.rs:351-356 from `source.principal_entity_type` and the group mapping only.

Nothing downstream compensates: `cedar::entities_to_cedar_json` (src/state/cedar.rs:1318-1351) only counts and converts, `entity_item_to_cedar_json` (src/state/cedar.rs:1354-1397) copies `attributes`, `parents` and `tags` through for every item, and `cedar::build_entities` (src/state/cedar.rs:919-936) refuses only "a payload that is no Cedar entity store at all".

Test coverage pins ONLY the implemented clause — tests/all/is_authorized_with_token.rs:533-564 has exactly two `entities` refusal rows, "an entity of the group type the identity source maps, for a token naming none" and "an entity of the type the identity source itself generates", both expecting `Expected::InvalidInput(vec!["/entities"])`. No row supplies a principal of another type, and no row supplies an action entity with attributes, so no test states a reason for accepting either.

## Steps to Reproduce

1. CreatePolicyStore (validation OFF), then CreateIdentitySource with a Cognito user pool configuration and `principalEntityType` `App::User` (so `owned_entity_types == ["App::User"]`, no group mapping).
2. CreatePolicy with a static policy conditioned on a principal of a DIFFERENT type, e.g. `permit(principal == Other::User::"mallory", action == App::Action::"read", resource == App::Photo::"p") when { principal.admin == true };`
3. Call IsAuthorizedWithToken with a valid `identityToken` from that source, `action`/`resource` set, and `entities` = `{"entityList": [{"identifier": {"entityType": "Other::User", "entityId": "mallory"}, "attributes": {"admin": {"boolean": true}}}]}`.
4. Observe HTTP 200: the caller-supplied principal entity is accepted and evaluated, although the member documents "You can't include principals in this parameter, only resource and action entities".
5. Repeat with `entities` = `{"entityList": [{"identifier": {"entityType": "App::Action", "entityId": "read"}, "attributes": {"x": {"long": 1}}}]}`: also accepted, although the member documents "For action entities, you can include only their Identifier and EntityType".
6. Contrast with `entityType` `App::User` (the source's own type), which IS refused 400 at is_authorized_with_token.rs:311-319.
