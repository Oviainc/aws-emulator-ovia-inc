Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 032f9a5bcc1a20e2d342a3403d3d533abd316530

# GetIdentitySource reports no groupConfiguration for a Cognito source whose groups the authorization path maps to the default AWS::CognitoGroup type

_Filed: 2026-10-29T10:33:43Z_
_Operation: GetIdentitySource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `032f9a5bcc1a20e2d342a3403d3d533abd316530`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For an Amazon Cognito user pool identity source created WITHOUT a `groupConfiguration`, two paths of this same crate disagree about the stored source:

* the authorization path materialises the documented default group entity type `AWS::CognitoGroup` for it (`state/token.rs`), maps the token's `cognito:groups` claim to parents of that type, AND adds that type to `owned_entity_types` — which is what `IsAuthorizedWithToken` uses to refuse, with HTTP 400 `ValidationException`, any `entities` slice the caller supplies carrying an entity of that type;
* the read path reports the containing `configuration.cognitoUserPoolConfiguration.groupConfiguration` as ABSENT (`ops/get_identity_source.rs`), and `ListIdentitySources` re-types that same view, so no read of the API reports the group entity type at all.

The consequence is observable: a caller creates a Cognito identity source with no `groupConfiguration`, reads it back and is told the source has no group configuration, then has its own `AWS::CognitoGroup` entities refused 400 on the grounds that they are "an entity type the identity source that issued the token generates" — state that no read reports.

It is also the opposite of the rule this crate states for the sibling default on the same resource: `principalEntityType` is RESOLVED to the service default at create time and PERSISTED, precisely so a read can report it. The group entity type is the one default this crate applies only at authorization time and never persists, so `IdentitySource::group_entity_type` stays `None` and every reader sees "maps no groups".

## Evidence

Model (kind A) — the OUTPUT detail member carries the default itself. `smithy_query(query="shape", shape="CognitoGroupConfigurationDetail")`:
  "groupEntityType": ... "doc": "<p>The name of the schema entity type that's mapped to the user pool group. Defaults to <code>AWS::CognitoGroup</code>.</p>"

The authorization side APPLIES that default — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state/token.rs:79-87:
    /// The Cedar entity type an Amazon Cognito user pool source's GROUPS carry when the
    /// identity source configured none, from `CognitoGroupConfiguration$groupEntityType`'s own
    /// model documentation: "the name of the schema entity type that's mapped to the user pool
    /// group. Defaults to `AWS::CognitoGroup`." The member is required inside an optional
    /// structure, so an absent group configuration is that default ...
    pub const DEFAULT_COGNITO_GROUP_ENTITY_TYPE: &str = "AWS::CognitoGroup";

src/state/token.rs:541-553:
    fn group_mapping(source: &IdentitySource) -> Option<GroupMapping<'_>> {
        match &source.configuration {
            IdentitySourceConfiguration::CognitoUserPool(CognitoUserPoolSource {
                group_entity_type, user_pool_id, .. }) => Some(GroupMapping {
                claim: GROUPS_CLAIM,
                entity_type: group_entity_type
                    .as_deref()
                    .unwrap_or(DEFAULT_COGNITO_GROUP_ENTITY_TYPE),

and src/state/token.rs:349-356 puts it into the refusal set:
    // From the SOURCE's configuration, so an `entities` payload of the source's own group
    // type is refused whether or not THIS token names a group.
    let mut owned_entity_types = vec![source.principal_entity_type.clone()];
    if let Some(mapped) = mapping.as_ref() { ... owned_entity_types.push(mapped.entity_type.to_string()); }

which src/ops/is_authorized_with_token.rs:306-326 turns into the 400:
    .find(|entity_type| owned.iter().any(|owned| owned == entity_type))
    ... "it carries an entity of type {entity_type}, which is an entity type the identity source that issued the token generates"

The READ side does NOT apply it — src/ops/get_identity_source.rs:101-105:
    group_configuration: cognito.group_entity_type.as_ref().map(|entity_type| {
        CognitoGroupConfigurationDetail {
            group_entity_type: Some(entity_type.clone()),
        }
    }),

and the stored field is left unresolved — src/state.rs:541-542:
    /// `groupConfiguration.groupEntityType`, when the request set one.
    pub group_entity_type: Option<String>,

The crate's own stated rule for the sibling default on the SAME resource — src/state.rs:92-97:
    /// `CreateIdentitySource$principalEntityType` is optional while
    /// `GetIdentitySource$principalEntityType` is **required**, so an omitted value
    /// has to resolve to the service's own default; for an Amazon Cognito user pool
    /// source that default is the `AWS::Cognito` entity type ...
    pub const DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE: &str = "AWS::Cognito";
implemented at src/ops/create_identity_source.rs:221-224:
    (None, crate::state::IdentitySourceConfiguration::CognitoUserPool(_)) => {
        crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE.to_string()
    }

Test coverage, both sides:
* tests/all/is_authorized_with_token.rs:678-699 pins the authorization reading WITH a stated reason — "or the service's own default where it configures none" / "and none at all — which is the documented `AWS::CognitoGroup` default" / `("no group configuration", None, "AWS::CognitoGroup"),`
* tests/all/identity_source_configuration_writers.rs:141-158 + 285-289 pin the read omitting `groupConfiguration` for case "cognito: no optional member" (exact `assert_eq!(from_create["configuration"], configuration, ...)`), and state a reason only for the `clientIds` dimension ("absent on the request and a list on every read, so it is stored — and reported — empty"), none for `groupConfiguration`.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode = OFF.
2. CreateIdentitySource with configuration.cognitoUserPoolConfiguration = {userPoolArn: "arn:aws:cognito-idp:us-east-1:<account>:userpool/us-east-1_1a2b3c4d5"} and NO groupConfiguration.
3. GetIdentitySource for the new source: configuration.cognitoUserPoolConfiguration carries no `groupConfiguration` member, so no read reports a group entity type. ListIdentitySources reports the same (it re-types ops/get_identity_source.rs::configuration_detail).
4. Call IsAuthorizedWithToken against the same store with an identity token from that pool whose `cognito:groups` claim names a group: the decision is made with a parent entity of type `AWS::CognitoGroup` (tests/all/is_authorized_with_token.rs:699 pins this).
5. Call IsAuthorizedWithToken again supplying entities = {entityList: [ {identifier: {entityType: "AWS::CognitoGroup", entityId: "x"}} ]}: the call is refused HTTP 400 ValidationException "...which is an entity type the identity source that issued the token generates", naming a type no read of the identity source reports.
