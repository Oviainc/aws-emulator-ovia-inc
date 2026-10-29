Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d7344a303bafe2b6a404f5912cec42ec875cddf1

# UpdateIdentitySource's ValidationException names `/configuration/...` for a member the operation calls `updateConfiguration`, so fieldList points at a member the request has not got

_Filed: 2026-10-29T04:12:16Z_
_Operation: UpdateIdentitySource | Class: error-response_

> _Note: this issue was identified against the code base at commit `d7344a303bafe2b6a404f5912cec42ec875cddf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateIdentitySource` refuses a Cognito `userPoolArn` this emulator cannot resolve (wrong account, wrong Region, a pool id that disagrees with its own ARN, an ARN that is no user-pool ARN) with the modelled `ValidationException`, and the `fieldList` entry it carries names the member at fault with the shared constant `crate::ops::paths::COGNITO_USER_POOL_ARN` = `/configuration/cognitoUserPoolConfiguration/userPoolArn`.

That pointer is correct for `CreateIdentitySource`, whose input member IS `configuration`. `UpdateIdentitySource` has no `configuration` member at all: its provider union arrives in the **required** member `updateConfiguration`. So the one machine-readable half of the refusal — the JSON pointer a client is supposed to follow to fix its request — addresses a member that does not exist in the request it is refusing, and the member that does carry the bad value is never named.

The crate states the rule the code breaks in two places: the `paths` module's own doc ("Each is the member's location in the request body as the model names it") and `service.rs`'s boundary-refusal renderer, which says it spells a member "the same spelling this service's own input checks report in `ValidationException$fieldList` ([`crate::ops::paths`])". The two halves have in fact drifted for this one operation: a MODEL-level refusal of the same member (an over-long or pattern-breaking `userPoolArn`) goes through `service.rs::json_pointer`, which builds the pointer from the framework's dotted path and therefore says `/updateConfiguration/cognitoUserPoolConfiguration/userPoolArn`, while the handler's own refusal of the same member says `/configuration/cognitoUserPoolConfiguration/userPoolArn`. One request member, two different pointers, from one crate.

The sibling `principalEntityType` refusal in the same pair of handlers uses `paths::PRINCIPAL_ENTITY_TYPE` = `/principalEntityType`, which is right for both operations — this is the only path constant shared by the two writers whose spelling differs between them.

## Evidence

The constant and the rule it is declared under — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/mod.rs:118-121 and 156-158:
```
/// The JSON-pointer path of every request member this service's own input checks
/// can refuse — ONE home for the paths rule (b) names, so a member is spelled the
/// same way wherever it is refused from. Each is the member's location in the
/// request body as the model names it (`smithy_query(query="operation", ...)`).
pub mod paths {
...
    /// The Cognito user pool an identity source's configuration references.
    pub const COGNITO_USER_POOL_ARN: &str =
        "/configuration/cognitoUserPoolConfiguration/userPoolArn";
```

The one place that raises it — src/state.rs:620-635 (inside `resolve_user_pool_arn`):
```
pub fn resolve_user_pool_arn(
    arn: &str,
    request_region: &str,
) -> Result<(String, String), crate::ops::InvalidInput> {
    let invalid = |reason: String| {
        crate::ops::InvalidInput::new(crate::ops::paths::COGNITO_USER_POOL_ARN, reason)
    };
```

The UpdateIdentitySource handler that turns it into the response — src/ops/update_identity_source.rs:176-190:
```
        let invalid = |invalid: crate::ops::InvalidInput| -> UpdateIdentitySourceError {
            UpdateIdentitySourceErrorModeled::ValidationException(
                crate::ops::validation_exception([invalid]),
            )
            .into()
        };

        // The provider reference is resolved BEFORE any state is touched: a
        // configuration this service cannot resolve must change nothing.
        let region = crate::state::request_region(ctx.parts());
        let configuration = crate::ops::persisted_identity_source_configuration(
            &input.update_configuration,
            &region,
        )
        .map_err(invalid)?;
```
(`&input.update_configuration` — the handler itself reads the member as `updateConfiguration`.)

The model's member name, from `smithy_query(query="shape", shape="UpdateIdentitySourceInput")` / OPERATIONS.md:147:
```
## UpdateIdentitySource
- **Input:** `policyStoreId` (...required), `identitySourceId` (...required), `updateConfiguration` (UpdateConfiguration, required), `principalEntityType` (...)
```
and what `ValidationExceptionField$path` is for, from `smithy_query(query="shape", shape="ValidationExceptionField")`: `"path": ... "doc": "<p>The path to the specific element that Verified Permissions found to be not valid.</p>"`.

The other half of the crate, which spells the same member correctly — src/service.rs:136-139:
```
/// than as a dotted path, because that pointer is the machine-readable half of
/// the message — the same spelling this service's own input checks report in
/// `ValidationException$fieldList` ([`crate::ops::paths`]). `None` when there is
/// nothing to state, which keeps the framework's wording.
```
built by src/service.rs:74-92 (`fn json_pointer(dotted: &str) -> String`), which turns the framework's `updateConfiguration.cognitoUserPoolConfiguration.userPoolArn` into `/updateConfiguration/cognitoUserPoolConfiguration/userPoolArn`.

Test coverage: the `fieldList` path is pinned per-operation in tests/all/policy_validation.rs:288-303 — but only for the CREATE:
```
        (
            // Every resource this emulator holds is in its own account, so an
            // ARN naming another one names a pool that does not exist here.
            "a Cognito user pool this emulator cannot resolve",
            "CreateIdentitySource",
            json!({ ... "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": ... } } }),
            "/configuration/cognitoUserPoolConfiguration/userPoolArn",
        ),
```
(asserted at tests/all/policy_validation.rs:383-387). The UpdateIdentitySource rows for the same refusals — tests/all/update_and_delete_lifecycle.rs:426-462 ("a pool ARN in another account", "a pool ARN in another region", "an ARN that names no Cognito user pool at all") — assert only the HTTP status and the error code (lines 465-475) and never look at `fieldList`, so nothing pins or justifies the wrong pointer.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "OFF" } } -> note policyStoreId PS.
2. CreateIdentitySource { policyStoreId: PS, configuration: { cognitoUserPoolConfiguration: { userPoolArn: "arn:aws:cognito-idp:us-east-1:000000000000:userpool/us-east-1_1a2b3c4d5" } } } (signed for us-east-1) -> note identitySourceId IS.
3. UpdateIdentitySource { policyStoreId: PS, identitySourceId: IS, updateConfiguration: { cognitoUserPoolConfiguration: { userPoolArn: "arn:aws:cognito-idp:us-east-1:999999999999:userpool/us-east-1_1a2b3c4d5" } } } (signed for us-east-1).
4. Observed: 400 ValidationException whose fieldList is [{ path: "/configuration/cognitoUserPoolConfiguration/userPoolArn", message: "no Amazon Cognito user pool ... exists in account 999999999999 ..." }] — a path naming a `configuration` member the UpdateIdentitySource request does not have.
5. Expected per the crate's own paths rule ("the member's location in the request body as the model names it") and `UpdateIdentitySourceInput`'s member list: path "/updateConfiguration/cognitoUserPoolConfiguration/userPoolArn".
6. Contrast the MODEL-level refusal of the same member, which the crate already spells correctly: UpdateIdentitySource { policyStoreId: PS, identitySourceId: IS, updateConfiguration: { cognitoUserPoolConfiguration: { userPoolArn: "arn:aws:s3:::x" + 2500 chars } } } -> the boundary refusal message addresses "/updateConfiguration/cognitoUserPoolConfiguration/userPoolArn" (src/service.rs::json_pointer), i.e. the two refusal paths of one crate disagree about one member.
