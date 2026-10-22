// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-verifiedpermissions-verifiedpermissions. One file per operation.

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_verifiedpermissions_verifiedpermissions_ops_generated.rs"));

/// The sentence a `ValidationException` this service raises carries in its
/// required `message`.
///
/// It is a FIXED, identifier-free sentence on purpose: `ValidationException`'s
/// own model documentation says "The request failed because one or more input
/// parameters don't satisfy their constraint requirements. The output is
/// provided as a list of fields and a reason for each field that isn't valid",
/// so the specifics belong in `fieldList`, which is parseable, and not spliced
/// into prose a client can only regex. The same split is what this service's
/// other errors do — `ResourceNotFoundException` reports the identifier in
/// `resourceId`, not in its message.
pub const INVALID_INPUT_MESSAGE: &str = "Invalid input";

/// One offending request member: the JSON-pointer `path` of the member at fault
/// and the `reason` it was refused.
///
/// This is what every service-side input check in this crate fails with, so the
/// member's location survives all the way to the wire instead of being flattened
/// into a message string. [`validation_exception`] turns it into the modelled
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInput {
    /// The member's location in the request, as a JSON pointer
    /// (`/definition/static/statement`), which is the form
    /// `ValidationExceptionField$path` carries.
    pub path: String,
    /// Why the member was refused — the text that becomes
    /// `ValidationExceptionField$message`.
    pub reason: String,
}

impl InvalidInput {
    /// An [`InvalidInput`] naming `path` and explaining it with `reason`.
    pub fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// This member, re-rooted under one request of a batch.
    ///
    /// A check that evaluates ONE authorization request names the member as that
    /// request carries it (`/principal`); inside a batch the same member lives at
    /// `/requests/1/principal`, so the check keeps one implementation and the
    /// batch says which of its requests it was. Pointer construction stays here
    /// rather than at each call site.
    pub fn within_batch_request(mut self, index: usize) -> Self {
        self.path = format!("{}/{index}{}", paths::REQUESTS, self.path);
        self
    }
}

impl std::fmt::Display for InvalidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

/// The modelled `ValidationException` for one or more offending request members.
///
/// Prefer this over `crate::scaffolded::Scaffolded::validation`: this service's
/// model declares its OWN `ValidationException` on every operation, and that
/// shape carries `fieldList`, so a client reading `fieldList` to point at the
/// bad member gets it. The synthetic scaffold shape has a `message` and nothing
/// else.
pub fn validation_exception(
    invalid: impl IntoIterator<Item = InvalidInput>,
) -> types::ValidationException {
    let field_list: Vec<types::ValidationExceptionField> = invalid
        .into_iter()
        .map(|field| types::ValidationExceptionField {
            path: field.path,
            message: field.reason,
        })
        .collect();
    types::ValidationException {
        message: INVALID_INPUT_MESSAGE.to_string(),
        // `fieldList` is optional in the model, and an error that names no field
        // is not one this service raises — every caller passes at least one — but
        // an empty list would still be a lie about the shape, so it stays absent
        // rather than being serialized as `[]`.
        field_list: (!field_list.is_empty()).then_some(field_list),
    }
}

/// The JSON-pointer path of every request member this service's own input checks
/// can refuse — ONE home for the paths rule (b) names, so a member is spelled the
/// same way wherever it is refused from. Each is the member's location in the
/// request body as the model names it (`smithy_query(query="operation", ...)`).
pub mod paths {
    /// `CreatePolicy`/`UpdatePolicy`'s Cedar policy text.
    pub const STATIC_STATEMENT: &str = "/definition/static/statement";
    /// `PutSchema`'s Cedar schema JSON.
    pub const SCHEMA_CEDAR_JSON: &str = "/definition/cedarJson";
    /// The `policyStoreId` every operation in this service takes.
    pub const POLICY_STORE_ID: &str = "/policyStoreId";
    /// `ListPolicies`' pagination cursor.
    pub const NEXT_TOKEN: &str = "/nextToken";
    /// `CreatePolicyStoreAlias`' alias name.
    pub const ALIAS_NAME: &str = "/aliasName";
    /// `CreateIdentitySource`/`UpdateIdentitySource`'s principal entity type.
    pub const PRINCIPAL_ENTITY_TYPE: &str = "/principalEntityType";
    /// The Cognito user pool an identity source's configuration references.
    pub const COGNITO_USER_POOL_ARN: &str =
        "/configuration/cognitoUserPoolConfiguration/userPoolArn";
    /// An authorization request's `principal`.
    pub const PRINCIPAL: &str = "/principal";
    /// An authorization request's `action`.
    pub const ACTION: &str = "/action";
    /// An authorization request's `resource`.
    pub const RESOURCE: &str = "/resource";
    /// An authorization request's `context`, and each arm of that union.
    pub const CONTEXT_MAP: &str = "/context/contextMap";
    /// `context`'s Cedar-JSON arm.
    pub const CONTEXT_CEDAR_JSON: &str = "/context/cedarJson";
    /// An authorization request's `entities`, whichever arm it carries.
    pub const ENTITIES: &str = "/entities";
    /// `entities`' Cedar-JSON arm.
    pub const ENTITIES_CEDAR_JSON: &str = "/entities/cedarJson";
    /// An `entityList` item's attribute map.
    pub const ENTITY_LIST_ATTRIBUTES: &str = "/entities/entityList/attributes";
    /// An `entityList` item's parent list.
    pub const ENTITY_LIST_PARENTS: &str = "/entities/entityList/parents";
    /// An `entityList` item's tag map.
    pub const ENTITY_LIST_TAGS: &str = "/entities/entityList/tags";

    /// A batch operation's request list. One member of one request in it is
    /// named by [`super::InvalidInput::within_batch_request`].
    pub const REQUESTS: &str = "/requests";
}

/// The modelled `ResourceNotFoundException` for a reference that names no live
/// resource: the constant [`crate::state::RESOURCE_NOT_FOUND_MESSAGE`], with the
/// identity of what was not found in the members that model it.
///
/// The exception models that identity in its own required members — `resourceId`
/// is "The unique ID of the resource referenced in the failed request" and
/// `resourceType` is "The resource type of the resource referenced in the failed
/// request" (`smithy_query(query="shape", shape="ResourceNotFoundException")`) —
/// so the identifier has a modelled home and the message stays a fixed sentence.
/// A message formatted from the request would both duplicate those members in
/// unparseable prose and leak the looked-up value (an id, a name, a whole ARN
/// including the account id) into logs. This is the ONE place that builds the
/// shape, so every not-found path of this service answers the same text.
///
/// `resource_id` is the id of the missing resource (for a request that named it
/// by ARN, [`crate::state::resource_id_from_arn`] of that ARN) and
/// `resource_type` one of `ResourceType`'s values
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub fn resource_not_found(
    resource_id: &str,
    resource_type: &str,
) -> self::types::ResourceNotFoundException {
    self::types::ResourceNotFoundException {
        message: crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// One entry of [`types::ConflictException::resources`]: the id of a resource
/// the failed request collided with, and its `ResourceType`
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
///
/// Both members are required by the model, so this is the ONE place that shapes
/// an entry and every conflict reports the collision the same way.
pub fn resource_conflict(resource_id: &str, resource_type: &str) -> self::types::ResourceConflict {
    self::types::ResourceConflict {
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `ConflictException` for a request that collided with the
/// resources in `resources`: the constant [`crate::state::CONFLICT_MESSAGE`],
/// with the collision itself reported in the required `resources` list.
///
/// `resources` is "the list of resources referenced with this failed request"
/// (`smithy_query(query="shape", shape="ConflictException")`), so a client
/// learns WHICH resource it collided with by reading structured members rather
/// than by regexing prose — and the message is therefore the exception's fixed
/// summary, identical whatever the cause. Build each entry with
/// [`resource_conflict`]. This is the ONE place that builds the shape, so every
/// conflicting path of this service answers the same text.
pub fn conflict(
    resources: impl IntoIterator<Item = self::types::ResourceConflict>,
) -> self::types::ConflictException {
    self::types::ConflictException {
        message: crate::state::CONFLICT_MESSAGE.to_string(),
        resources: resources.into_iter().collect(),
    }
}

/// The modelled `TooManyTagsException` for a tagging request that would take
/// `resource_arn` past [`crate::state::MAX_TAGS_PER_RESOURCE`]: the constant
/// [`crate::state::TOO_MANY_TAGS_MESSAGE`], with the resource at the limit in
/// the modelled `resourceName` member rather than in the message.
pub fn too_many_tags(resource_arn: &str) -> self::types::TooManyTagsException {
    self::types::TooManyTagsException {
        message: Some(crate::state::TOO_MANY_TAGS_MESSAGE.to_string()),
        resource_name: Some(resource_arn.to_string()),
    }
}

/// The modelled `InvalidStateException` for a policy store whose deletion
/// protection is enabled: the constant
/// [`crate::state::DELETION_PROTECTION_MESSAGE`], which names no resource.
pub fn deletion_protection_enabled() -> self::types::InvalidStateException {
    self::types::InvalidStateException {
        message: crate::state::DELETION_PROTECTION_MESSAGE.to_string(),
    }
}

/// Emit the structured audit record for a state-changing or destructive
/// operation: when it happened, what operation ran, what it touched and how it
/// came out.
///
/// The framework's own per-request telemetry is gated on recording being active,
/// so a state change otherwise leaves no trace; this record is always emitted.
///
/// It carries SERVICE METADATA ONLY. Nothing is copied out of the request's
/// headers — not the `Authorization` header, not the access-key id inside it, not
/// the user agent: a header is credential-bearing content whether or not any one
/// field of it looks like a secret, and in a normal developer setup the signing
/// key is the developer's real one. Caller attribution, if it is ever wanted, has
/// to arrive as an off-by-default emulator setting with a non-reversible digest
/// and a stated record lifetime, which is not this change.
pub fn audit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_id: &str,
    outcome: &str,
) {
    let at_epoch_millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    ::tracing::info!(
        target: "audit",
        at_epoch_millis = %at_epoch_millis,
        service = %ctx.identity().signing_name(),
        operation = %operation,
        resource_id = %resource_id,
        outcome = %outcome,
        "verifiedpermissions state change"
    );
}
