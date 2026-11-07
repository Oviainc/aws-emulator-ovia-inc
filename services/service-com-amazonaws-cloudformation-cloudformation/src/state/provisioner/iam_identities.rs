// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the AWS Identity and Access Management identities a
//! stack declares: roles and users, with the policies, tags and in-place changes each of them
//! takes. One file of [`super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;

/// Create the role an `AWS::IAM::Role` declares, with its trust policy and its inline
/// policies.
///
/// `AssumeRolePolicyDocument` is REQUIRED on this resource type and on IAM's own
/// `CreateRole`, so a declaration carrying none describes no role that could be created:
/// the resource is refused here, naming the property, rather than a role being created
/// with a trust policy this module invented or a stack completing over a role nothing
/// created.
///
/// A name IAM will not create a role under — `EntityAlreadyExists`, which is what an IAM
/// name the ACCOUNT already holds answers, since role names are account-global — fails the
/// resource like any other refusal: the role that is there is not this stack's, and a
/// stack never reports a resource it did not create.
///
/// `ManagedPolicyArns` is applied too, with `AttachRolePolicy` per entry: IAM is what
/// judges whether a policy ARN is attachable, so a policy it refuses fails the resource
/// like any other refusal rather than leaving the stack complete over a role without the
/// permissions its template declared.
///
/// The role carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateRole`'s own `Tags` member.
pub(super) async fn create_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let Some(trust_policy) = resource
        .properties
        .get("AssumeRolePolicyDocument")
        .and_then(document_text)
    else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property AssumeRolePolicyDocument cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    };
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    let mut role_tags = Vec::new();
    for (key, value) in tags_of(&resource.properties, propagated) {
        role_tags.push(
            ::aws_sdk_iam::types::Tag::builder()
                .key(&key)
                .value(&value)
                .build()
                .map_err(|error| Refusal {
                    code: "InternalFailure".to_string(),
                    message: format!("the tag {key} cannot be sent to IAM: {error}"),
                })?,
        );
    }
    let created = client
        .create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust_policy)
        // `Path`, `Description` and `MaxSessionDuration` are spelled the same on the
        // resource type and on `CreateRole`; a property the template leaves out is not
        // sent.
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .set_description(resource.properties.get("Description").and_then(scalar_text))
        .set_max_session_duration(whole_number(resource.properties.get("MaxSessionDuration")))
        .set_tags((!role_tags.is_empty()).then_some(role_tags))
        .send()
        .await;
    let mut attributes = BTreeMap::new();
    match created {
        Ok(answer) => {
            // What IAM answered wins over the computed ARN: it is the role's real
            // identity.
            if let Some(role) = answer.role() {
                attributes.insert("Arn".to_string(), role.arn().to_string());
                // `RoleId` is the UNIQUE ID IAM mints for a role ("Returns the stable and
                // unique string identifying the role"), which only IAM can know: without
                // it the attribute keeps the value the uniform convention read off the
                // role's NAME, and a client comparing the stack's output with IAM's own
                // record sees two different ids for one role.
                attributes.insert("RoleId".to_string(), role.role_id().to_string());
            }
        }
        Err(error) => {
            // What IAM answered is what the resource is: a name it will not create a role
            // under is a role this deployment did not create, and CloudFormation reports
            // no resource it did not create. IAM role names are unique across the whole
            // ACCOUNT, so this is also what a template whose role name is not
            // Region-qualified gets when it is deployed a second time — the refusal
            // belongs to the stack, not to this module.
            return Err(iam_refusal_of(&error));
        }
    }
    // "Policies — adds or updates an inline policy document that is embedded in the
    // specified IAM role", each entry a `{PolicyName, PolicyDocument}` pair.
    for policy in inline_policies(&resource.properties) {
        let (Some(name), Some(body)) = (
            policy.get("PolicyName").and_then(scalar_text),
            policy.get("PolicyDocument").and_then(document_text),
        ) else {
            continue;
        };
        client
            .put_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .policy_document(body)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    // "ManagedPolicyArns — a list of Amazon Resource Names (ARNs) of the IAM managed
    // policies that you want to attach to the role", each applied with the call that
    // attaches one.
    for policy_arn in managed_policy_arns(&resource.properties) {
        client
            .attach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    Ok(Created::attributes(attributes))
}

/// The properties of `AWS::IAM::Role` a deployment can change on the role the stack
/// already has, each through the IAM call that changes that one thing on an existing role.
///
/// These are the type's no-interruption properties: the role keeps its name, and so its
/// `RoleId`, its ARN and its physical id, and everything that trusts or assumes it.
/// `Tags` is one of them, applied in [`retag_role`] rather than in [`update_role`] —
/// where every tag a role is to carry is reconciled at once.
///
/// Three properties are deliberately NOT here, so a deployment that changes one is reported
/// as not applied rather than as a success that dropped it:
///
/// * `RoleName`, which this emulator replaces the role over rather than updating: the name
///   IS the resource's physical id, so a changed one is a different resource to
///   [`super::provisioning::is_same_resource`] and never reaches [`update_role`] at all;
/// * `Path`, which decides the role's ARN and which no IAM call moves on an existing role;
/// * `PermissionsBoundary`, which [`create_role`] does not apply either, so claiming to
///   apply a change to it would record a configuration nothing put in force.
pub(super) const ROLE_IN_PLACE_PROPERTIES: &[&str] = &[
    "AssumeRolePolicyDocument",
    "Description",
    "ManagedPolicyArns",
    "MaxSessionDuration",
    "Policies",
    "Tags",
];

/// IAM's own default for a role's maximum session length, which is what a role goes back
/// to when a deployment stops declaring `MaxSessionDuration`: `UpdateRole`'s own
/// documentation of that member — "If you do not specify a value for this setting, the
/// default value of one hour is applied".
///
/// Read from the IAM crate, which owns that default and applies it to every role it
/// creates, so the two cannot drift apart.
pub(super) const DEFAULT_MAX_SESSION_DURATION: i32 =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::DEFAULT_MAX_SESSION_DURATION_SECONDS;

/// Apply a CHANGED `AWS::IAM::Role` declaration to the role the stack already has, through
/// the IAM call that changes each property on an existing role — the trust policy with
/// `UpdateAssumeRolePolicy`, the description and the session length with `UpdateRole`, the
/// inline policies with `PutRolePolicy`/`DeleteRolePolicy`, the attached managed policies
/// with `AttachRolePolicy`/`DetachRolePolicy`, and the tags with `TagRole`/`UntagRole`.
///
/// The role therefore keeps its identity: the same name, the same `RoleId` and the same
/// ARN, so every attribute the stack published for it still resolves and nothing that
/// trusts the role has to be re-pointed. Only what the two revisions state differently is
/// sent, so a property neither of them changed reaches IAM not at all.
///
/// `Ok(None)` is a change no IAM call makes to an existing role — a property outside
/// [`ROLE_IN_PLACE_PROPERTIES`] — which the deployment then reports as not applied rather
/// than as a success that dropped the caller's change.
///
/// The role's `Tags` are the one property this does NOT carry: they travel in
/// [`retag_role`] with the ones CloudFormation gives the resource, which change
/// independently of any declaration.
pub(super) async fn update_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let held = &resource.properties;
    // A declaration that spells out the name the role ALREADY has is no change to the name:
    // this call is reached only for a resource that kept its physical id, which for this
    // type IS the role's name, so one revision merely wrote down what the other left to
    // CloudFormation.
    let changed: Vec<String> = changed_properties(held, declared)
        .into_iter()
        .filter(|property| {
            property != "RoleName"
                || declared.get("RoleName").and_then(scalar_text).as_deref()
                    != Some(resource.physical_resource_id.as_str())
        })
        .collect();
    if changed
        .iter()
        .any(|property| !ROLE_IN_PLACE_PROPERTIES.contains(&property.as_str()))
    {
        return Ok(None);
    }
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    let is_changed = |property: &str| changed.iter().any(|name| name == property);

    // "Updates the policy that grants an IAM entity permission to assume a role": the
    // declared document REPLACES the one the role holds. The property is required on this
    // resource type, so a declaration that drops it describes no role IAM would accept —
    // refused naming the property, exactly as the create refuses it.
    if is_changed("AssumeRolePolicyDocument") {
        let Some(trust_policy) = declared
            .get("AssumeRolePolicyDocument")
            .and_then(document_text)
        else {
            return Err(Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "Property AssumeRolePolicyDocument cannot be empty for resource of type {}",
                    resource.resource_type
                ),
            });
        };
        client
            .update_assume_role_policy()
            .role_name(role_name)
            .policy_document(trust_policy)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }

    // "Updates the description or maximum session duration setting of a role" — one call
    // for the two, carrying only the member this deployment states differently. A
    // description the deployment no longer declares is sent as the empty string
    // (`roleDescriptionType` admits it), and a session length it no longer declares goes
    // back to IAM's documented default ([`DEFAULT_MAX_SESSION_DURATION`]), so a dropped
    // property is reset rather than silently kept.
    if is_changed("Description") || is_changed("MaxSessionDuration") {
        let mut call = client.update_role().role_name(role_name);
        if is_changed("Description") {
            call = call.description(
                declared
                    .get("Description")
                    .and_then(scalar_text)
                    .unwrap_or_default(),
            );
        }
        if is_changed("MaxSessionDuration") {
            // A declaration that states the property but not as a whole number states no
            // session length IAM could be asked for, and is refused as the malformed
            // property it is rather than quietly resetting the role to the default.
            let duration = match declared.get("MaxSessionDuration") {
                None => DEFAULT_MAX_SESSION_DURATION,
                Some(value) => whole_number(Some(value))
                    .ok_or_else(|| malformed_property("MaxSessionDuration"))?,
            };
            call = call.max_session_duration(duration);
        }
        call.send().await.map_err(|error| iam_refusal_of(&error))?;
    }

    // "Policies — adds or updates an inline policy document that is embedded in the
    // specified IAM role": a policy this deployment declares with a new document is put
    // again under the same name, and one it no longer declares is deleted off the role.
    if is_changed("Policies") {
        let held_policies = inline_policies(held);
        let declared_policies = inline_policies(declared);
        for policy in declared_policies {
            let (Some(name), Some(body)) = (
                policy.get("PolicyName").and_then(scalar_text),
                policy.get("PolicyDocument").and_then(document_text),
            ) else {
                continue;
            };
            if held_policies.iter().any(|held| held == policy) {
                continue;
            }
            client
                .put_role_policy()
                .role_name(role_name)
                .policy_name(name)
                .policy_document(body)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
        for name in dropped_policy_names(held_policies, declared_policies) {
            client
                .delete_role_policy()
                .role_name(role_name)
                .policy_name(name)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
    }

    // "ManagedPolicyArns — a list of Amazon Resource Names (ARNs) of the IAM managed
    // policies that you want to attach to the role": one the deployment adds is attached,
    // one it no longer lists is detached.
    if is_changed("ManagedPolicyArns") {
        let held_arns = managed_policy_arns(held);
        let declared_arns = managed_policy_arns(declared);
        for policy_arn in declared_arns.iter().filter(|arn| !held_arns.contains(arn)) {
            client
                .attach_role_policy()
                .role_name(role_name)
                .policy_arn(policy_arn)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
        for policy_arn in held_arns
            .iter()
            .filter(|arn| !declared_arns.contains(arn))
        {
            client
                .detach_role_policy()
                .role_name(role_name)
                .policy_arn(policy_arn)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
    }

    // The role is the one the stack created, under the same name: its ARN and its `RoleId`
    // are the ones the create already recorded, so this reports no attribute of its own.
    Ok(Some(Created::default()))
}

/// Bring the tags of the role a resource record names to the ones this deployment gives it,
/// through IAM's own `TagRole` and `UntagRole`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set [`create_role`] applied — so a key outside it is one this
/// deployment no longer gives the role and is removed, and a key in it with another value is
/// re-valued ("adds or updates the specified tags", `TagRole`). A key the declaration stops
/// stating while the STACK still states it therefore keeps the stack's value rather than
/// being removed. Where the two sets already agree nothing is called at all, which is what
/// makes this safe to ask on every deployment.
///
/// The role's tags are read from `GetRole`, which reports them on the role itself. A role
/// IAM no longer holds has no tags to reconcile.
pub(super) async fn retag_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let desired = tags_of(declared, propagated);
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    let held: Vec<(String, String)> = match client.get_role().role_name(role_name).send().await {
        Ok(answer) => answer
            .role()
            .map(|role| {
                role.tags()
                    .iter()
                    .map(|tag| (tag.key().to_string(), tag.value().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            return Err(refusal);
        }
    };
    let mut added = Vec::new();
    for (key, value) in desired.iter().filter(|(key, value)| {
        held.iter()
            .find(|(held_key, _)| held_key == key)
            .is_none_or(|(_, held_value)| held_value != value)
    }) {
        added.push(
            ::aws_sdk_iam::types::Tag::builder()
                .key(key)
                .value(value)
                .build()
                .map_err(|error| Refusal {
                    code: UNIDENTIFIED_FAILURE.to_string(),
                    message: format!("the tag {key} cannot be sent to IAM: {error}"),
                })?,
        );
    }
    let removed: Vec<String> = held
        .iter()
        .filter(|(key, _)| !desired.iter().any(|(wanted, _)| wanted == key))
        .map(|(key, _)| key.clone())
        .collect();
    if !added.is_empty() {
        client
            .tag_role()
            .role_name(role_name)
            .set_tags(Some(added))
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    if !removed.is_empty() {
        client
            .untag_role()
            .role_name(role_name)
            .set_tag_keys(Some(removed))
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    Ok(())
}

/// The `PolicyName`s a declaration stated and the new one no longer does — the inline
/// policies to delete off the role. A declaration whose entry carries no name states no
/// policy, so there is nothing to delete for it.
pub(super) fn dropped_policy_names(held: &[Value], declared: &[Value]) -> Vec<String> {
    let declared_names: Vec<String> = declared
        .iter()
        .filter_map(|policy| policy.get("PolicyName").and_then(scalar_text))
        .collect();
    held.iter()
        .filter_map(|policy| policy.get("PolicyName").and_then(scalar_text))
        .filter(|name| !declared_names.contains(name))
        .collect()
}

/// The top-level properties two revisions of one declaration state DIFFERENTLY: a property
/// one of them adds, one it drops, and one both state with another value. The union of
/// both documents' keys, so a property the new revision no longer declares is a change
/// too, in the order the held revision wrote them and then the declared one.
pub(super) fn changed_properties(held: &Value, declared: &Value) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    for (property, value) in held.as_object().into_iter().flatten() {
        if declared.get(property) != Some(value) {
            changed.push(property.clone());
        }
    }
    for (property, value) in declared.as_object().into_iter().flatten() {
        if held.get(property) != Some(value) && !changed.contains(property) {
            changed.push(property.clone());
        }
    }
    changed
}

/// Delete the role a stack created. "You must delete the policies that are associated
/// with the role" first — an attached managed policy is DETACHED and an inline policy
/// deleted — so each one the template declared goes before the role does; a policy or a
/// role IAM no longer has is already in the state the delete asks for.
pub(super) async fn delete_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    for policy_arn in managed_policy_arns(&resource.properties) {
        if let Err(error) = client
            .detach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
        {
            let refusal = iam_refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    for policy in inline_policies(&resource.properties) {
        let Some(name) = policy.get("PolicyName").and_then(scalar_text) else {
            continue;
        };
        if let Err(error) = client
            .delete_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .send()
            .await
        {
            let refusal = iam_refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    match client.delete_role().role_name(role_name).send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The role's configuration as IAM reports it, for drift detection.
///
/// A role IAM no longer holds (`NoSuchEntity`) was deleted. The members compared are the
/// ones the resource type and `GetRole` spell identically and the create sends — `Path`,
/// `Description` and `MaxSessionDuration` — so a role whose description or session length
/// was changed outside the stack is MODIFIED.
pub(super) async fn describe_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_role()
        .role_name(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(role) = answer.role() {
        actual.insert("Path".to_string(), Value::String(role.path().to_string()));
        if let Some(description) = role.description() {
            actual.insert(
                "Description".to_string(),
                Value::String(description.to_string()),
            );
        }
        if let Some(duration) = role.max_session_duration() {
            actual.insert("MaxSessionDuration".to_string(), Value::from(duration));
        }
    }
    Ok(Some(Value::Object(actual)))
}

/// The roles an `AWS::IAM::InstanceProfile` declaration lists, each the role NAME
/// `AddRoleToInstanceProfile` takes — a `Ref` to a role of the same template resolves to
/// that role's name ([`super::provisioning::reconcile`]).
///
/// The property DOCUMENT is what is read, not the stored record, so the create and the
/// delete walk the same list.
pub(super) fn instance_profile_roles(properties: &Value) -> Vec<String> {
    declared_list(properties, "Roles")
        .iter()
        .filter_map(scalar_text)
        .filter(|role| !role.is_empty())
        .collect()
}

/// Create the instance profile an `AWS::IAM::InstanceProfile` declares, and put the role it
/// names in it.
///
/// "Ref returns the instance profile name", which is the name CloudFormation created the
/// profile under — the resource's physical id — so the stack publishes a name every IAM call
/// on a profile and every `IamInstanceProfile` reference accepts.
///
/// `Roles` is REQUIRED on this resource type ("the name of the role to associate with the
/// instance profile; only one role can be assigned"), so a declaration carrying none
/// describes no profile an instance could use: the resource is refused here, naming the
/// property, rather than a profile being created that holds no role. `Path` is spelled the
/// same on the type and on `CreateInstanceProfile`; a property the template leaves out is
/// not sent. The type publishes no `Tags` property, so no tags are propagated to a profile.
pub(super) async fn create_instance_profile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let roles = instance_profile_roles(&resource.properties);
    if roles.is_empty() {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property Roles cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    }
    let client = iam_client(ctx, region);
    let profile_name = &resource.physical_resource_id;
    let created = client
        .create_instance_profile()
        .instance_profile_name(profile_name)
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .send()
        .await
        .map_err(|error| iam_refusal_of(&error))?;
    let mut attributes = BTreeMap::new();
    // What IAM answered wins over the computed ARN: it carries the account and the path IAM
    // built the profile under, which is what a client reads back off the profile.
    if let Some(profile) = created.instance_profile() {
        attributes.insert("Arn".to_string(), profile.arn().to_string());
    }
    // "AddRoleToInstanceProfile — adds the specified IAM role to the specified instance
    // profile": the role is not a member of the create, so each declared one is its own
    // call, and a role IAM refuses to add fails the resource rather than leaving the stack
    // complete over a profile that grants nothing.
    for role in roles {
        client
            .add_role_to_instance_profile()
            .instance_profile_name(profile_name)
            .role_name(role)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    Ok(Created::attributes(attributes))
}

/// Delete the instance profile a stack created. "You must remove the role from the instance
/// profile" first, so each role the declaration named is taken out before the profile goes;
/// a role or a profile IAM no longer holds is already in the state the delete asks for.
pub(super) async fn delete_instance_profile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let profile_name = &resource.physical_resource_id;
    for role in instance_profile_roles(&resource.properties) {
        if let Err(error) = client
            .remove_role_from_instance_profile()
            .instance_profile_name(profile_name)
            .role_name(role)
            .send()
            .await
        {
            let refusal = iam_refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    match client
        .delete_instance_profile()
        .instance_profile_name(profile_name)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The instance profile's configuration as IAM reports it, for drift detection.
///
/// A profile IAM no longer holds (`NoSuchEntity`) was deleted. The members compared are the
/// ones the resource type and `GetInstanceProfile` spell identically and the create applies:
/// `Path`, and the `Roles` the profile holds — read as the role NAMES the declaration lists
/// them by, so a role removed from the profile outside the stack is MODIFIED.
pub(super) async fn describe_instance_profile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_instance_profile()
        .instance_profile_name(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(profile) = answer.instance_profile() {
        actual.insert("Path".to_string(), Value::String(profile.path().to_string()));
        actual.insert(
            "Roles".to_string(),
            Value::Array(
                profile
                    .roles()
                    .iter()
                    .map(|role| Value::String(role.role_name().to_string()))
                    .collect(),
            ),
        );
    }
    Ok(Some(Value::Object(actual)))
}

/// The `Policies` entries a role declaration carries, as a slice — empty for a role that
/// declares none, so the create, the update and the delete walk them the same way.
///
/// The property DOCUMENT is what is read, not the stored record, so the same reader serves
/// the configuration a resource was created with and the one a deployment now declares for
/// it.
pub(super) fn inline_policies(properties: &Value) -> &[Value] {
    declared_list(properties, "Policies")
}

/// The `ManagedPolicyArns` a role declaration carries, each as the ARN text
/// `AttachRolePolicy` takes.
pub(super) fn managed_policy_arns(properties: &Value) -> Vec<String> {
    declared_text_list(properties, "ManagedPolicyArns")
}

/// One list-valued property of a declaration, as a slice — empty for a property the
/// template does not declare or declares as something other than a list.
pub(super) fn declared_list<'a>(properties: &'a Value, property: &str) -> &'a [Value] {
    properties
        .get(property)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// The NAMES one list-valued property states, in the order the declaration writes them —
/// [`declared_list`] read as the text each entry is ([`scalar_text`]).
///
/// The one reading of a list of names, so the arms that take one (a role's managed policy
/// ARNs, a user's groups, a CodeArtifact repository's upstreams and external connections)
/// cannot read the same declaration differently. An entry that is no single value is not a
/// name and contributes nothing.
pub(super) fn declared_text_list(properties: &Value, property: &str) -> Vec<String> {
    declared_list(properties, property)
        .iter()
        .filter_map(scalar_text)
        .collect()
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::ManagedPolicy
// ---------------------------------------------------------------------------

/// The properties an `AWS::IAM::ManagedPolicy` lists the entities it is attached to under,
/// each applied with the IAM call that attaches a managed policy to an entity of that kind
/// ([`attach_managed_policy_to`], [`detach_managed_policy_from`]).
///
/// "Groups — the name (friendly name, not ARN) of the group to attach the policy to", and
/// the same for `Roles` and `Users`: a declaration listing one is asking for the attachment
/// a client makes with `AttachGroupPolicy`/`AttachRolePolicy`/`AttachUserPolicy`, so the
/// policy's deployment makes those very calls and its deletion undoes them.
pub(super) const MANAGED_POLICY_HOLDERS: &[&str] = &["Groups", "Roles", "Users"];

/// Attach the managed policy `policy_arn` to one entity of the kind `holder_property`
/// names, through IAM's own attach call for that kind.
pub(super) async fn attach_managed_policy_to(
    client: &::aws_sdk_iam::Client,
    holder_property: &str,
    entity: &str,
    policy_arn: &str,
) -> Result<(), Refusal> {
    let sent = match holder_property {
        "Groups" => client
            .attach_group_policy()
            .group_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
        "Roles" => client
            .attach_role_policy()
            .role_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
        _ => client
            .attach_user_policy()
            .user_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
    };
    sent
}

/// Detach the managed policy `policy_arn` from one entity of the kind `holder_property`
/// names. An entity, or an attachment, IAM no longer has is already in the state the
/// detach asks for, judged on IAM's own not-found CODE.
pub(super) async fn detach_managed_policy_from(
    client: &::aws_sdk_iam::Client,
    holder_property: &str,
    entity: &str,
    policy_arn: &str,
) -> Result<(), Refusal> {
    let sent = match holder_property {
        "Groups" => client
            .detach_group_policy()
            .group_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
        "Roles" => client
            .detach_role_policy()
            .role_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
        _ => client
            .detach_user_policy()
            .user_name(entity)
            .policy_arn(policy_arn)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| iam_refusal_of(&error)),
    };
    match sent {
        Err(refusal) if refusal.code == NO_SUCH_ENTITY => Ok(()),
        other => other,
    }
}

/// The name the managed policy is created under: the `ManagedPolicyName` the declaration
/// asks for, and otherwise the name CloudFormation generated for the resource, which
/// [`super::template`] records as its `Name`.
///
/// Read from the declaration rather than from the resource's physical id, because for this
/// type that id is the policy's ARN — the identity IAM mints — and not a name any create
/// could be addressed with.
pub(super) fn managed_policy_name(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "ManagedPolicyName")
        .or_else(|| {
            resource
                .attributes
                .get("Name")
                .filter(|name| !name.is_empty())
                .cloned()
        })
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{} declares no ManagedPolicyName", resource.resource_type),
        })
}

/// Create the customer managed policy an `AWS::IAM::ManagedPolicy` declares, and attach it
/// to every entity the declaration lists.
///
/// `PolicyDocument` is REQUIRED on this resource type and on IAM's own `CreatePolicy`, so a
/// declaration carrying none describes no policy that could be created: the resource is
/// refused here, naming the property, rather than a policy being created with a document
/// this module invented.
///
/// The resource's identity is IAM's own: "Ref returns the ARN of the managed policy", which
/// is the handle every IAM operation on a managed policy takes (`GetPolicy$PolicyArn`,
/// `AttachRolePolicy$PolicyArn`) — so what the stack reports for the resource is the ARN
/// `CreatePolicy` answered with, and a template that passes that `Ref` to another resource's
/// `ManagedPolicyArns` or `PermissionsBoundary` passes an ARN IAM will resolve.
pub(super) async fn create_managed_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let Some(document) = resource.properties.get("PolicyDocument").and_then(document_text) else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property PolicyDocument cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    };
    let client = iam_client(ctx, region);
    let created = client
        .create_policy()
        .policy_name(managed_policy_name(resource)?)
        .policy_document(document)
        // `Path` and `Description` are spelled the same on the resource type and on
        // `CreatePolicy`; a property the template leaves out is not sent.
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .set_description(resource.properties.get("Description").and_then(scalar_text))
        .send()
        .await
        .map_err(|error| iam_refusal_of(&error))?;
    let policy = created.policy();
    let Some(arn) = policy.and_then(|policy| policy.arn()) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses a
        // managed policy without one, so this is reported as a failure rather than
        // recorded as a resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "IAM answered no Arn for the managed policy it created".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("PolicyArn".to_string(), arn.to_string());
    // `PolicyId` is the UNIQUE ID IAM mints for a policy, which only IAM can know.
    if let Some(policy_id) = policy.and_then(|policy| policy.policy_id()) {
        attributes.insert("PolicyId".to_string(), policy_id.to_string());
    }
    // Every entity the declaration lists gets the attachment a client would make. IAM is
    // what judges whether it can be made, so an entity it refuses fails the resource
    // rather than leaving the stack complete over a grant that does not exist.
    for holder in MANAGED_POLICY_HOLDERS {
        for entity in declared_list(&resource.properties, holder)
            .iter()
            .filter_map(scalar_text)
        {
            attach_managed_policy_to(&client, holder, &entity, arn).await?;
        }
    }
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// The properties of `AWS::IAM::ManagedPolicy` a deployment can change on the policy the
/// stack already has, each through the IAM call that changes that one thing on an existing
/// policy: the document with `CreatePolicyVersion` (set as the default version, which is
/// what the policy grants from then on) and the attachment lists with the attach and detach
/// calls.
///
/// `ManagedPolicyName`, `Path` and `Description` are deliberately NOT here: the name IS what
/// the resource is created under, the path decides the ARN, and no IAM call moves either on
/// an existing policy — nor the description, which only `CreatePolicy` takes — so a
/// deployment that changes one is reported as not applied rather than as a success that
/// dropped it.
pub(super) const MANAGED_POLICY_IN_PLACE_PROPERTIES: &[&str] =
    &["PolicyDocument", "Groups", "Roles", "Users"];

/// Apply a CHANGED `AWS::IAM::ManagedPolicy` declaration to the policy the stack already
/// has, through the IAM calls that change each property on an existing policy.
///
/// The policy therefore keeps its identity — the same ARN and the same `PolicyId` — so
/// every attachment it already has, and every attribute the stack published for it, still
/// resolves. Only what the two revisions state differently is sent.
///
/// `Ok(None)` is a change no IAM call makes to an existing policy — a property outside
/// [`MANAGED_POLICY_IN_PLACE_PROPERTIES`] — which the deployment then reports as not
/// applied.
pub(super) async fn update_managed_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let held = &resource.properties;
    // A declaration that spells out the name the policy ALREADY has is no change to the
    // name: this call is reached only for a resource that kept its identity.
    let changed: Vec<String> = changed_properties(held, declared)
        .into_iter()
        .filter(|changed_property| {
            changed_property != "ManagedPolicyName"
                || declared
                    .get("ManagedPolicyName")
                    .and_then(scalar_text)
                    .is_some_and(|name| {
                        property(resource, "ManagedPolicyName").as_deref() != Some(name.as_str())
                    })
        })
        .collect();
    if changed
        .iter()
        .any(|property| !MANAGED_POLICY_IN_PLACE_PROPERTIES.contains(&property.as_str()))
    {
        return Ok(None);
    }
    let client = iam_client(ctx, region);
    let policy_arn = &resource.physical_resource_id;

    // "Creates a new version of the specified managed policy": the declared document
    // becomes a new version of the policy the stack already has, set as the DEFAULT one —
    // "the operation sets this version as the policy's default version" — so every entity
    // the policy is attached to is governed by the document the template now declares.
    if changed.iter().any(|property| property == "PolicyDocument") {
        let Some(document) = declared.get("PolicyDocument").and_then(document_text) else {
            return Err(Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "Property PolicyDocument cannot be empty for resource of type {}",
                    resource.resource_type
                ),
            });
        };
        client
            .create_policy_version()
            .policy_arn(policy_arn)
            .policy_document(document)
            .set_as_default(true)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }

    // An entity the deployment adds to a list gets the attachment; one it no longer lists
    // has it removed, which is how a template revokes a grant it once declared.
    for holder in MANAGED_POLICY_HOLDERS {
        if !changed.iter().any(|property| property == holder) {
            continue;
        }
        let held_entities: Vec<String> = declared_list(held, holder)
            .iter()
            .filter_map(scalar_text)
            .collect();
        let declared_entities: Vec<String> = declared_list(declared, holder)
            .iter()
            .filter_map(scalar_text)
            .collect();
        for entity in declared_entities
            .iter()
            .filter(|entity| !held_entities.contains(entity))
        {
            attach_managed_policy_to(&client, holder, entity, policy_arn).await?;
        }
        for entity in held_entities
            .iter()
            .filter(|entity| !declared_entities.contains(entity))
        {
            detach_managed_policy_from(&client, holder, entity, policy_arn).await?;
        }
    }

    // The policy is the one the stack created, under the same ARN: its identity and its
    // `PolicyId` are the ones the create already recorded, so this reports no attribute of
    // its own.
    Ok(Some(Created::default()))
}

/// Delete the managed policy a stack created, addressed by the ARN that is its physical id.
///
/// "Before you can delete a managed policy, you must first detach the policy from all
/// users, groups, and roles that it is attached to" and "you must delete all the policy's
/// versions" — so every attachment the declaration asked for is detached and every
/// non-default version is deleted before the policy itself goes. A policy, an attachment or
/// a version IAM no longer has is already in the state the delete asks for, judged on IAM's
/// own not-found code.
pub(super) async fn delete_managed_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let policy_arn = &resource.physical_resource_id;
    for holder in MANAGED_POLICY_HOLDERS {
        for entity in declared_list(&resource.properties, holder)
            .iter()
            .filter_map(scalar_text)
        {
            detach_managed_policy_from(&client, holder, &entity, policy_arn).await?;
        }
    }
    // Every version but the default, which `DeletePolicy` deletes with the policy itself.
    // A deployment that changed the document left the earlier versions behind, exactly as
    // a client's own `CreatePolicyVersion` does.
    match client
        .list_policy_versions()
        .policy_arn(policy_arn)
        .send()
        .await
    {
        Ok(answer) => {
            for version in answer.versions() {
                if version.is_default_version() {
                    continue;
                }
                let Some(version_id) = version.version_id() else {
                    continue;
                };
                if let Err(error) = client
                    .delete_policy_version()
                    .policy_arn(policy_arn)
                    .version_id(version_id)
                    .send()
                    .await
                {
                    let refusal = iam_refusal_of(&error);
                    if refusal.code != NO_SUCH_ENTITY {
                        return Err(refusal);
                    }
                }
            }
        }
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    match client.delete_policy().policy_arn(policy_arn).send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The managed policy's configuration as IAM reports it, for drift detection.
///
/// A policy IAM no longer holds (`NoSuchEntity`) was deleted. The members compared are the
/// ones the resource type and `GetPolicy` spell identically and the create sends — `Path`
/// and `Description` — so a policy whose description was changed outside the stack is
/// MODIFIED.
pub(super) async fn describe_managed_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_policy()
        .policy_arn(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = iam_refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(policy) = answer.policy() {
        if let Some(path) = policy.path() {
            actual.insert("Path".to_string(), Value::String(path.to_string()));
        }
        if let Some(description) = policy.description() {
            actual.insert(
                "Description".to_string(),
                Value::String(description.to_string()),
            );
        }
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::User
// ---------------------------------------------------------------------------

/// The `LoginProfile` an `AWS::IAM::User` declaration carries, as the two members
/// `CreateLoginProfile` takes: "a login profile is created when you create a password for
/// the user", and the type declares that password beside the flag that forces a change at
/// the next sign-in.
pub(super) fn declared_login_profile(properties: &Value) -> Option<(String, Option<bool>)> {
    let profile = properties.get("LoginProfile")?;
    let password = profile.get("Password").and_then(scalar_text)?;
    let reset_required = profile.get("PasswordResetRequired").and_then(Value::as_bool);
    Some((password, reset_required))
}

/// The groups an `AWS::IAM::User` declaration lists, each the friendly name
/// `AddUserToGroup` takes.
pub(super) fn declared_groups(properties: &Value) -> Vec<String> {
    declared_text_list(properties, "Groups")
}

/// Create the user an `AWS::IAM::User` declares, with its path, its permissions boundary,
/// its inline policies, its attached managed policies, its group memberships and its
/// console password — each through the IAM call a client makes for it.
///
/// A name IAM will not create a user under — `EntityAlreadyExists`, which is what an IAM
/// name the ACCOUNT already holds answers, since user names are account-global — fails the
/// resource like any other refusal: the user that is there is not this stack's, and a stack
/// never reports a resource it did not create.
///
/// The user carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateUser`'s own `Tags` member.
pub(super) async fn create_iam_user(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let client = iam_client(ctx, region);
    let user_name = &resource.physical_resource_id;
    let mut user_tags = Vec::new();
    for (key, value) in tags_of(&resource.properties, propagated) {
        user_tags.push(
            ::aws_sdk_iam::types::Tag::builder()
                .key(&key)
                .value(&value)
                .build()
                .map_err(|error| Refusal {
                    code: UNIDENTIFIED_FAILURE.to_string(),
                    message: format!("the tag {key} cannot be sent to IAM: {error}"),
                })?,
        );
    }
    let created = client
        .create_user()
        .user_name(user_name)
        // `Path` and `PermissionsBoundary` are spelled the same on the resource type and
        // on `CreateUser`; a property the template leaves out is not sent.
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .set_permissions_boundary(
            resource
                .properties
                .get("PermissionsBoundary")
                .and_then(scalar_text),
        )
        .set_tags((!user_tags.is_empty()).then_some(user_tags))
        .send()
        .await;
    let mut attributes = BTreeMap::new();
    match created {
        Ok(answer) => {
            // What IAM answered wins over the computed ARN and id: they are the user's
            // real identity, and `UserId` — "the stable and unique string identifying the
            // user" — is one only IAM can know.
            if let Some(user) = answer.user() {
                attributes.insert("Arn".to_string(), user.arn().to_string());
                attributes.insert("UserId".to_string(), user.user_id().to_string());
            }
        }
        Err(error) => return Err(iam_refusal_of(&error)),
    }
    // "Policies — adds or updates an inline policy document that is embedded in the
    // specified IAM user", each entry a `{PolicyName, PolicyDocument}` pair.
    for policy in inline_policies(&resource.properties) {
        let (Some(name), Some(body)) = (
            policy.get("PolicyName").and_then(scalar_text),
            policy.get("PolicyDocument").and_then(document_text),
        ) else {
            continue;
        };
        client
            .put_user_policy()
            .user_name(user_name)
            .policy_name(name)
            .policy_document(body)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    // "ManagedPolicyArns — a list of ARNs of the IAM managed policies that you want to
    // attach to the user", each applied with the call that attaches one.
    for policy_arn in managed_policy_arns(&resource.properties) {
        client
            .attach_user_policy()
            .user_name(user_name)
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    // "Groups — a list of group names that the user is added to", each the membership
    // `AddUserToGroup` writes.
    for group in declared_groups(&resource.properties) {
        client
            .add_user_to_group()
            .user_name(user_name)
            .group_name(group)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    // "LoginProfile — creates a login profile for the user, so that the user can access
    // the Amazon Web Services Management Console": the password the declaration carries is
    // the one `CreateLoginProfile` takes.
    if let Some((password, reset_required)) = declared_login_profile(&resource.properties) {
        client
            .create_login_profile()
            .user_name(user_name)
            .password(password)
            .set_password_reset_required(reset_required)
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }
    Ok(Created::attributes(attributes))
}

/// The properties of `AWS::IAM::User` a deployment can change on the user the stack already
/// has, each through the IAM call that changes that one thing on an existing user.
///
/// These are the type's no-interruption properties: the user keeps its name, and so its
/// `UserId`, its credentials and everything attached to it. `Path` is among them because
/// `UpdateUser` moves it on an existing user ("updates the name and/or the path of the
/// specified IAM user") while "the user's unique identifier remains the same" — which is
/// exactly what a path update must not disturb. `Tags` is one of them too, applied in
/// [`retag_iam_user`] rather than here, where every tag the user is to carry is reconciled
/// at once.
///
/// `UserName` is deliberately not here: the name IS the resource's physical id, so a
/// changed one is a different resource to [`super::provisioning::is_same_resource`] and
/// never reaches [`update_iam_user`] at all. `PermissionsBoundary` is not here either,
/// because this emulator applies the boundary at CREATE time through `CreateUser` and
/// claiming to apply a change to it would record a configuration nothing put in force.
pub(super) const USER_IN_PLACE_PROPERTIES: &[&str] = &[
    "Groups",
    "LoginProfile",
    "ManagedPolicyArns",
    "Path",
    "Policies",
    "Tags",
];

/// Apply a CHANGED `AWS::IAM::User` declaration to the user the stack already has, through
/// the IAM call that changes each property on an existing user — the path with
/// `UpdateUser`, the inline policies with `PutUserPolicy`/`DeleteUserPolicy`, the attached
/// managed policies with `AttachUserPolicy`/`DetachUserPolicy`, the memberships with
/// `AddUserToGroup`/`RemoveUserFromGroup` and the console password with
/// `CreateLoginProfile`/`UpdateLoginProfile`/`DeleteLoginProfile`.
///
/// The user therefore keeps its identity: the same name and the same `UserId`, so an output
/// that published either still resolves and every credential the user holds goes on
/// working.
///
/// `Ok(None)` is a change no IAM call makes to an existing user — a property outside
/// [`USER_IN_PLACE_PROPERTIES`] — which the deployment then reports as not applied.
pub(super) async fn update_iam_user(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let held = &resource.properties;
    // A declaration that spells out the name the user ALREADY has is no change to the
    // name: this call is reached only for a resource that kept its physical id, which for
    // this type IS the user's name.
    let changed: Vec<String> = changed_properties(held, declared)
        .into_iter()
        .filter(|property| {
            property != "UserName"
                || declared.get("UserName").and_then(scalar_text).as_deref()
                    != Some(resource.physical_resource_id.as_str())
        })
        .collect();
    if changed
        .iter()
        .any(|property| !USER_IN_PLACE_PROPERTIES.contains(&property.as_str()))
    {
        return Ok(None);
    }
    let client = iam_client(ctx, region);
    let user_name = &resource.physical_resource_id;
    let is_changed = |property: &str| changed.iter().any(|name| name == property);

    // "Updates the name and/or the path of the specified IAM user": only the path is sent,
    // because a changed name is a different resource. A path the deployment no longer
    // declares goes back to the one IAM gives a user created without one, so a dropped
    // property is reset rather than silently kept.
    if is_changed("Path") {
        client
            .update_user()
            .user_name(user_name)
            .new_path(
                declared
                    .get("Path")
                    .and_then(scalar_text)
                    .unwrap_or_else(|| DEFAULT_IAM_PATH.to_string()),
            )
            .send()
            .await
            .map_err(|error| iam_refusal_of(&error))?;
    }

    // "Policies": a policy this deployment declares with a new document is put again under
    // the same name, and one it no longer declares is deleted off the user.
    if is_changed("Policies") {
        let held_policies = inline_policies(held);
        let declared_policies = inline_policies(declared);
        for policy in declared_policies {
            let (Some(name), Some(body)) = (
                policy.get("PolicyName").and_then(scalar_text),
                policy.get("PolicyDocument").and_then(document_text),
            ) else {
                continue;
            };
            if held_policies.iter().any(|held| held == policy) {
                continue;
            }
            client
                .put_user_policy()
                .user_name(user_name)
                .policy_name(name)
                .policy_document(body)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
        for name in dropped_policy_names(held_policies, declared_policies) {
            client
                .delete_user_policy()
                .user_name(user_name)
                .policy_name(name)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
    }

    // "ManagedPolicyArns": one the deployment adds is attached, one it no longer lists is
    // detached.
    if is_changed("ManagedPolicyArns") {
        let held_arns = managed_policy_arns(held);
        let declared_arns = managed_policy_arns(declared);
        for policy_arn in declared_arns.iter().filter(|arn| !held_arns.contains(arn)) {
            client
                .attach_user_policy()
                .user_name(user_name)
                .policy_arn(policy_arn)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
        for policy_arn in held_arns
            .iter()
            .filter(|arn| !declared_arns.contains(arn))
        {
            client
                .detach_user_policy()
                .user_name(user_name)
                .policy_arn(policy_arn)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
    }

    // "Groups": a group the deployment adds the user to is a membership written, one it no
    // longer lists is a membership removed.
    if is_changed("Groups") {
        let held_groups = declared_groups(held);
        let declared_in = declared_groups(declared);
        for group in declared_in.iter().filter(|group| !held_groups.contains(group)) {
            client
                .add_user_to_group()
                .user_name(user_name)
                .group_name(group)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
        for group in held_groups
            .iter()
            .filter(|group| !declared_in.contains(group))
        {
            client
                .remove_user_from_group()
                .user_name(user_name)
                .group_name(group)
                .send()
                .await
                .map_err(|error| iam_refusal_of(&error))?;
        }
    }

    // The console password: declared where the user had none is a profile CREATED,
    // declared differently is the one call that replaces a password on a user that has one
    // (`UpdateLoginProfile`), and no longer declared is the profile DELETED — so a template
    // that stops granting console access takes it away.
    if is_changed("LoginProfile") {
        match (
            declared_login_profile(held),
            declared_login_profile(declared),
        ) {
            (None, Some((password, reset_required))) => {
                client
                    .create_login_profile()
                    .user_name(user_name)
                    .password(password)
                    .set_password_reset_required(reset_required)
                    .send()
                    .await
                    .map_err(|error| iam_refusal_of(&error))?;
            }
            (Some(_), Some((password, reset_required))) => {
                client
                    .update_login_profile()
                    .user_name(user_name)
                    .password(password)
                    .set_password_reset_required(reset_required)
                    .send()
                    .await
                    .map_err(|error| iam_refusal_of(&error))?;
            }
            (Some(_), None) => {
                client
                    .delete_login_profile()
                    .user_name(user_name)
                    .send()
                    .await
                    .map_err(|error| iam_refusal_of(&error))?;
            }
            (None, None) => {}
        }
    }

    // The user is the one the stack created, under the same name: its ARN and its `UserId`
    // are the ones the create already recorded, so this reports no attribute of its own.
    Ok(Some(Created::default()))
}
