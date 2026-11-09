// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon Bedrock's own control plane: the guardrail an
//! `AWS::Bedrock::Guardrail` declares. One file of [`super`]'s per-type arms, dispatched to
//! by its `Provisioner`.
//!
//! A guardrail is a resource Amazon Bedrock MINTS an identity for: `CreateGuardrail` takes
//! the guardrail's name and answers with the `guardrailId` it assigned (`^[a-z0-9]+$`), the
//! `guardrailArn` built around that id and the `DRAFT` version the create opens. Those are
//! the values the type publishes as return values, and the id is the one every Bedrock
//! guardrail API takes as its `guardrailIdentifier` — whose own pattern admits the id or the
//! ARN and NOT the name — so a stack that creates a guardrail reports what that service
//! assigned rather than the name the template asked for.

use super::*;

/// Amazon Bedrock's control-plane signing name, from its model (`sigv4Name: bedrock`,
/// `endpointPrefix: bedrock`), which speaks `restJson1` — so its calls ride on
/// [`call_rest_json`], each on the URI its own operation's `@http` binding declares.
pub(super) const BEDROCK_SIGNING_NAME: &str = "bedrock";

/// The guardrail collection `CreateGuardrail` is bound to (`POST /guardrails`); one
/// guardrail is addressed under it by its `guardrailIdentifier`
/// (`GET`/`PUT`/`DELETE /guardrails/{guardrailIdentifier}`).
pub(super) const BEDROCK_GUARDRAILS_URI: &str = "/guardrails";

/// The error code Amazon Bedrock answers for a guardrail it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its reads, updates and deletes).
pub(super) const BEDROCK_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::Bedrock::Guardrail` property naming the KMS key the guardrail is encrypted
/// with, and the member `CreateGuardrail` takes it as: the type publishes the key's ARN
/// while the operation's member is the key IDENTIFIER (`kmsKeyId`, whose pattern admits an
/// ARN, a key id or an alias), so this is the one property whose member name is not its own
/// lower-initial spelling.
const KMS_KEY_PROPERTY: &str = "kmsKeyArn";
const KMS_KEY_MEMBER: &str = "kmsKeyId";

/// The same, one level down: the cross-region configuration's property names the guardrail
/// PROFILE by ARN while the modelled `GuardrailCrossRegionConfig` of Amazon Bedrock's own
/// model declares its member as the profile IDENTIFIER, whose pattern admits the profile id
/// or its ARN.
const CROSS_REGION_PROPERTY: &str = "crossRegionConfig";
const PROFILE_PROPERTY: &str = "guardrailProfileArn";
const PROFILE_MEMBER: &str = "guardrailProfileIdentifier";

/// The member `CreateGuardrail` takes the guardrail's name in, which is the one member
/// CloudFormation decides.
const NAME_MEMBER: &str = "name";

/// The members an answered guardrail reports: the three values the type publishes as return
/// values, read off the answer rather than recomputed here, as (member, attribute).
const GUARDRAIL_ATTRIBUTES: &[(&str, &str)] = &[
    ("guardrailId", "GuardrailId"),
    ("guardrailArn", "GuardrailArn"),
    ("version", "Version"),
];

/// Create the guardrail an `AWS::Bedrock::Guardrail` declares, so a client that deployed
/// the stack and calls `GetGuardrail` with the `GuardrailId` the stack publishes reads the
/// guardrail back — and so an invocation guarded by it is really evaluated against the
/// policies the template declared.
///
/// The whole declared document is the request once its member names are re-cased
/// ([`camel_cased`]), because every property of this type is the member of the same name
/// with a lower-cased initial all the way down (`ContentPolicyConfig.FiltersConfig[].Type`
/// is `contentPolicyConfig.filtersConfig[].type`), with the two exceptions above
/// ([`KMS_KEY_PROPERTY`], [`PROFILE_PROPERTY`]) renamed; the declared tags travel beside
/// the ones CloudFormation gives the resource ([`tags_of`]), as the `TagList`
/// `CreateGuardrail`'s own `tags` member takes.
///
/// The identity is Amazon Bedrock's: the ARN it minted becomes this resource's physical id
/// — the primary identifier of the type, and a `guardrailIdentifier` every guardrail API
/// takes — and the id, the ARN and the version it answered are the resource's attributes.
pub(super) async fn create_bedrock_guardrail(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = guardrail_request(resource)?;
    // A declaration that states no name is deployed under the name CloudFormation computed
    // for it, which is this resource's physical id until the service assigns one.
    if !request.contains_key(NAME_MEMBER) {
        request.insert(
            NAME_MEMBER.to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(
            "tags".to_string(),
            tags_as_list(&tags, LOWER_CASED_TAG),
        );
    }
    let answered = call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "POST",
        BEDROCK_GUARDRAILS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let attributes = guardrail_attributes(&answered)?;
    Ok(Created {
        // Amazon Bedrock assigned the guardrail's identity, so the ARN it answered is what
        // `Ref` resolves to and what every call on the guardrail is addressed by.
        physical_resource_id: attributes.get("GuardrailArn").cloned(),
        attributes,
    })
}

/// Apply a changed declaration to the guardrail the stack created, through `UpdateGuardrail`
/// — the one call that changes a guardrail that exists.
///
/// The request carries the whole declared document, the same way the create does: the update
/// takes the same members and REPLACES the configuration, so a policy the declaration no
/// longer states leaves the guardrail without it rather than keeping the one an earlier
/// deployment gave it.
pub(super) async fn update_bedrock_guardrail(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = declared_guardrail_members(declared)?;
    if !request.contains_key(NAME_MEMBER) {
        request.insert(
            NAME_MEMBER.to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let answered = call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "PUT",
        &guardrail_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Created::attributes(guardrail_attributes(&answered)?))
}

/// Delete the guardrail a stack created, by the identifier it was created under — this
/// resource's physical id. One the service no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries the
/// request's own values) is already in the state the delete asks for.
pub(super) async fn delete_bedrock_guardrail(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "DELETE",
        &guardrail_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BEDROCK_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The guardrail's configuration as Amazon Bedrock reports it, for drift detection:
/// `GetGuardrail`'s own answer, read by the identifier that is this resource's physical id.
/// One it no longer holds (`ResourceNotFoundException`) was deleted.
///
/// The members its read reports under the names this type declares are the guardrail's name,
/// its description and the two blocked-messaging strings; the policies it reports are the
/// guardrail's EVALUATED policies rather than the `…Config` documents the type declares, and
/// a property the read does not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_bedrock_guardrail(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "GET",
        &guardrail_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[
                ("Name", "name"),
                ("Description", "description"),
                ("BlockedInputMessaging", "blockedInputMessaging"),
                ("BlockedOutputsMessaging", "blockedOutputsMessaging"),
            ],
        ))),
        Err(refusal) if refusal.code == BEDROCK_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI one guardrail is addressed at: the collection plus the `guardrailIdentifier`
/// `@httpLabel`, escaped as a single path segment ([`percent_encoded`]) so a value carrying
/// URI structure addresses one guardrail rather than another path.
fn guardrail_uri(identifier: &str) -> String {
    format!(
        "{BEDROCK_GUARDRAILS_URI}/{}",
        percent_encoded(identifier)
    )
}

/// The members of a guardrail create or update request, from the resource's own declared
/// properties.
fn guardrail_request(resource: &StoredResource) -> Result<Map<String, Value>, Refusal> {
    declared_guardrail_members(&resource.properties)
}

/// One declared property document as the request members Amazon Bedrock takes: every member
/// name re-cased ([`camel_cased`]) and the two renamed members renamed. The tags are left
/// out — the create adds them, the update does not take them — so one reader serves both.
fn declared_guardrail_members(declared: &Value) -> Result<Map<String, Value>, Refusal> {
    let mut request = match camel_cased(declared, 0)? {
        Value::Object(members) => members,
        // A declaration that is not an object declares no member of the request.
        _ => Map::new(),
    };
    request.remove("tags");
    if let Some(key) = request.remove(KMS_KEY_PROPERTY) {
        request.insert(KMS_KEY_MEMBER.to_string(), key);
    }
    if let Some(Value::Object(cross_region)) = request.get_mut(CROSS_REGION_PROPERTY) {
        if let Some(profile) = cross_region.remove(PROFILE_PROPERTY) {
            cross_region.insert(PROFILE_MEMBER.to_string(), profile);
        }
    }
    Ok(request)
}

/// The attributes an answered guardrail reports ([`GUARDRAIL_ATTRIBUTES`]).
///
/// An answer carrying no id and no ARN is reported as a refusal rather than recorded as a
/// created resource: there would be no handle a client could call Amazon Bedrock with.
fn guardrail_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let mut attributes = BTreeMap::new();
    for (member, attribute) in GUARDRAIL_ATTRIBUTES {
        if let Some(value) = reported_text(answered, member) {
            attributes.insert((*attribute).to_string(), value);
        }
    }
    if !attributes.contains_key("GuardrailId") || !attributes.contains_key("GuardrailArn") {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Bedrock control plane answered a guardrail without an id \
                      and an ARN"
                .to_string(),
        });
    }
    Ok(attributes)
}
