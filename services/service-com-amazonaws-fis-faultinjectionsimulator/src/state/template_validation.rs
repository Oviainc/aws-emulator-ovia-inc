// SPDX-License-Identifier: CC0-1.0

//! Whether an experiment template's TARGETS and ACTIONS are admissible: the refusals
//! `CreateExperimentTemplate` and `UpdateExperimentTemplate` answer with when the target
//! map, the action map or the references between them do not hold together.

use super::*;

/// `None` when every target of the template a request builds satisfies what FIS documents
/// for a target; otherwise the `ValidationException` that refuses the request.
///
/// Taken over the STORED targets rather than over one request's input shape, so a created
/// and an updated template are judged by the same rule. A template declaring NO target is
/// admissible — `targets` is not `@required` on the create, and an action that needs none
/// (the built-in wait) runs without one.
///
/// The rules the model cannot express. Two of them are stated by
/// `CreateExperimentTemplateTargetInput` itself ("You must specify at least one Amazon
/// Resource Name (ARN) or at least one resource tag. You cannot specify both ARNs and
/// tags"), which is why `resourceArns` and `resourceTags` are modelled as two independent
/// optional members:
///
/// * a target identifies its resources by EXACTLY ONE of them, so one that names both and
///   one that names neither are each refused rather than stored;
/// * `selectionMode` is one of the three documented forms ([`selection_mode_count`]), so a
///   value outside them is refused instead of being resolved as the documented default.
///
/// An EMPTY `resourceArns` list, and an empty `resourceTags` map, name no resource and no
/// tag: both are "not specified" here, which is what the documented sentence requires of
/// them ("at least one").
///
/// The other two are the published target-resource-type contract, the same one
/// [`TARGET_RESOURCE_TYPE_CATALOG`] serves to `ListTargetResourceTypes` and
/// `GetTargetResourceType` — the counterpart of what [`actions_refusal`] applies to an
/// action from [`ACTION_CATALOG`]:
///
/// * `resourceType` is "the resource type", one of the target resource types FIS
///   PUBLISHES, so a type the catalog holds no entry for — the same miss
///   `GetTargetResourceType` refuses with its own `ValidationException` — is refused rather
///   than stored;
/// * the parameters that type's catalogued contract marks required are supplied, because
///   `TargetResourceTypeParameter$required` says a target of that type HAS TO set them and
///   a target without them names a resolution the published contract cannot identify.
pub fn targets_refusal(
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> Option<ValidationException> {
    // The map's iteration order is not stable, so the refusal is taken over the target
    // names in order: one request always earns the same message.
    let mut entries: Vec<(&String, &ExperimentTemplateTarget)> = targets.iter().collect();
    entries.sort_by_key(|(name, _)| *name);
    for (name, target) in entries {
        // `resourceType` is required on the create's and the update's target input alike, so
        // a stored target always carries one; a record without one names no published
        // resource type and is refused by this rule.
        let resource_type = target.resource_type.as_deref().unwrap_or_default();
        let Some(entry) = target_resource_type_catalog_entry(resource_type) else {
            return Some(invalid_input(format!(
                "targets.{name}.resourceType must be a resource type FIS publishes, \
                 got {resource_type:?}"
            )));
        };
        let names_arns = target.resource_arns.as_ref().is_some_and(|arns| !arns.is_empty());
        let names_tags = target
            .resource_tags
            .as_ref()
            .is_some_and(|tags| !tags.is_empty());
        if names_arns && names_tags {
            return Some(invalid_input(format!(
                "targets.{name} must not specify both resourceArns and resourceTags."
            )));
        }
        if !names_arns && !names_tags {
            return Some(invalid_input(format!(
                "targets.{name} must specify at least one resourceArns entry or at least one \
                 resourceTags entry."
            )));
        }
        // `selectionMode` is required on the create's and the update's target input alike,
        // so a stored target always carries one; an absent one is refused by this rule
        // rather than silently resolved as the documented default.
        let selection_mode = target.selection_mode.as_deref().unwrap_or_default();
        if selection_mode_count(selection_mode, 0).is_none() {
            return Some(invalid_input(format!(
                "targets.{name}.selectionMode must be {SELECTION_MODE_ALL}, \
                 {SELECTION_MODE_COUNT_PREFIX}n) or {SELECTION_MODE_PERCENT_PREFIX}n), \
                 got {selection_mode:?}"
            )));
        }
        if let Some(refusal) = target_parameters_refusal(name, target, entry) {
            return Some(refusal);
        }
    }
    None
}

/// `None` when `target` supplies every parameter its resource type's catalogued contract
/// `entry` marks required; otherwise the `ValidationException` that refuses the template.
///
/// `TargetResourceTypeParameter$required` is reported by `GetTargetResourceType` for every
/// published resource type, so which parameters a target of that type needs is part of the
/// contract this service already serves — the same table decides it here, exactly as
/// [`action_parameters_refusal`] decides an action's. The parameters are judged in the
/// order the catalogued entry records them (sorted by name), so a target missing several of
/// them always earns the same message.
pub(super) fn target_parameters_refusal(
    name: &str,
    target: &ExperimentTemplateTarget,
    entry: &TargetResourceTypeCatalogEntry,
) -> Option<ValidationException> {
    let resource_type = entry.id;
    for parameter in entry.parameters.iter().filter(|parameter| parameter.required) {
        if target
            .parameters
            .as_ref()
            .is_none_or(|parameters| !parameters.contains_key(parameter.name))
        {
            return Some(invalid_input(format!(
                "targets.{name}.parameters must carry '{}' for resource type {resource_type}.",
                parameter.name
            )));
        }
    }
    None
}

/// `None` when every action of the template a request builds satisfies what FIS documents
/// for it; otherwise the refusal it earns, in the CLASS that refusal belongs to.
///
/// Taken over the STORED actions rather than over one request's input shape, so a created
/// and an updated template are judged by the same rule. `targets` is the SAME template's
/// target map, because two of the rules below are about how an action and a target of one
/// template refer to each other.
///
/// The first rule is the template's own: it DECLARES at least one action. `actions` is
/// `@required` on the create and the maps carry no `@length`, so the "at least one" floor
/// is server-side — an experiment with no action has nothing to carry out.
///
/// The rules on one action, none of them expressible as a modelled constraint:
///
/// * `startAfter` is "the name of the action that must be completed before the current
///   action starts", so it names an action the same template declares, and the
///   "completed before" ordering it states has to be realisable: the actions ordered by it
///   form a DAG, because an action inside a cycle waits for an action that waits for it and
///   so can never start ([`actions_cycle_refusal`]);
/// * `actionId` is "the ID of the action", one of the actions FIS PUBLISHES
///   ([`ACTION_CATALOG`], which `GetAction` reports from), so an id the catalog holds no
///   action for is refused rather than stored and later started;
/// * the parameters that action's catalogued contract marks required are supplied — for the
///   built-in wait that is its `duration`, whose documented 1 to 720 minute range is
///   checked with it, and for every other published action the parameters that action's
///   own entry records as required;
/// * each entry of `targets` names, under a target key that action SUPPORTS, a target the
///   same template declares, of the resource type the action's entry records for that key.
pub fn actions_refusal(
    actions: &HashMap<String, ExperimentTemplateAction>,
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> Option<TemplateRefusal> {
    if actions.is_empty() {
        return Some(TemplateRefusal::Invalid(invalid_input(
            "actions must declare at least one action.",
        )));
    }
    // The map's iteration order is not stable, so the refusal is taken over the action
    // names in order: one request always earns the same message. The entries are sorted
    // as PAIRS, so no path through this function can turn a lookup miss into an
    // acceptance.
    let mut entries: Vec<(&String, &ExperimentTemplateAction)> = actions.iter().collect();
    entries.sort_by_key(|(name, _)| *name);
    for (name, action) in entries {
        if action
            .start_after
            .iter()
            .flatten()
            .any(|earlier| !actions.contains_key(earlier))
        {
            return Some(TemplateRefusal::Invalid(invalid_input(format!(
                "actions.{name}.startAfter must name actions the template declares."
            ))));
        }
        // An `actionId` is required on the create and inherited by an update that omits it
        // ([`ExperimentTemplateRecord::updated_actions`]), so a stored action carries one;
        // a record without one names no published action and is refused by this rule — as
        // a REFERENCE that resolves to nothing, which is a missing resource and not a
        // malformed request ([`invalid_action_reference`]).
        let action_id = action.action_id.as_deref().unwrap_or_default();
        let Some(entry) = action_catalog_entry(action_id) else {
            return Some(TemplateRefusal::NotFound(invalid_action_reference(name)));
        };
        if let Some(refusal) = action_parameters_refusal(name, action, entry) {
            return Some(TemplateRefusal::Invalid(refusal));
        }
        if let Some(refusal) = action_targets_refusal(name, action, entry, targets) {
            return Some(TemplateRefusal::Invalid(refusal));
        }
    }
    // The ordering rule is about the action graph as a WHOLE rather than about one entry,
    // so it is judged once every action is itself admissible — and after the rule above
    // that every `startAfter` names a declared action, which is what makes the edges of
    // that graph meaningful.
    actions_cycle_refusal(actions).map(TemplateRefusal::Invalid)
}

/// `None` when the `startAfter` ordering over `actions` is realisable — the actions form a
/// directed acyclic graph; otherwise the `ValidationException` that refuses the template.
///
/// `startAfter` is "the name of the action that must be completed before the current action
/// starts", so a set of actions that reaches itself through it describes an order no run
/// can carry out: every action of the cycle waits for another action of the cycle to
/// complete first, and none of them can ever start. Such a template is refused here rather
/// than persisted, for the same reason a `startAfter` naming an undeclared action is.
///
/// Each action is settled once and each `startAfter` entry followed once, so the walk costs
/// O(actions + `startAfter` entries) — linear in the request. It carries the chain it is
/// resolving on its OWN stack rather than on the call stack, so however many actions a
/// request names, the depth it reaches is an allocation and not a recursion. The refusal is
/// deterministic: the actions are entered in name order and each one's `startAfter` in the
/// order the template records it, so one template always earns the same message.
pub(super) fn actions_cycle_refusal(
    actions: &HashMap<String, ExperimentTemplateAction>,
) -> Option<ValidationException> {
    let mut names: Vec<&str> = actions.keys().map(String::as_str).collect();
    names.sort_unstable();
    // The actions already shown to reach no cycle: each one is walked once.
    let mut settled: HashSet<&str> = HashSet::with_capacity(actions.len());
    for root in names {
        let Some(cycle) = cycle_from(root, actions, &mut settled) else {
            continue;
        };
        // The cycle is reported as the chain of actions it runs through, from an action ON
        // it back to that same action, so the caller sees which `startAfter` entries close
        // it. The member the message names is that action.
        let chain = cycle.join(" startAfter ");
        return Some(invalid_input(format!(
            "actions.{}.startAfter must not form a cycle, got {chain}.",
            cycle.first().copied().unwrap_or_default()
        )));
    }
    None
}

/// The chain of actions closing a cycle reachable from `root`, from an action ON the cycle
/// back to itself, or `None` when nothing `root` waits for — directly or transitively —
/// reaches one.
///
/// `waiting` is the chain currently being resolved, each entry holding how many of that
/// action's `startAfter` entries have been followed; an action that appears on it twice is
/// an action waiting for itself. A `startAfter` entry the map holds no action for has no
/// outgoing edge: it is already refused by the rule above this one, and contributes no
/// false cycle here.
pub(super) fn cycle_from<'a>(
    root: &'a str,
    actions: &'a HashMap<String, ExperimentTemplateAction>,
    settled: &mut HashSet<&'a str>,
) -> Option<Vec<&'a str>> {
    if settled.contains(&root) {
        return None;
    }
    let mut waiting: Vec<(&'a str, usize)> = vec![(root, 0)];
    while let Some((name, followed)) = waiting.last().copied() {
        let earlier = actions
            .get(name)
            .and_then(|action| action.start_after.as_ref())
            .and_then(|entries| entries.get(followed))
            .map(String::as_str);
        let Some(earlier) = earlier else {
            // Every action this one waits for is settled, so this one is too.
            settled.insert(name);
            waiting.pop();
            continue;
        };
        if let Some((_, followed)) = waiting.last_mut() {
            *followed += 1;
        }
        let Some((earlier, _)) = actions.get_key_value(earlier) else {
            continue;
        };
        let earlier = earlier.as_str();
        if settled.contains(&earlier) {
            continue;
        }
        if let Some(start) = waiting.iter().position(|(seen, _)| *seen == earlier) {
            let mut cycle: Vec<&str> = waiting[start..].iter().map(|(seen, _)| *seen).collect();
            cycle.push(earlier);
            return Some(cycle);
        }
        waiting.push((earlier, 0));
    }
    None
}

/// `None` when `action` supplies every parameter its catalogued contract `entry` marks
/// required, and the built-in wait's `duration` names a span within the documented range;
/// otherwise the `ValidationException` that refuses the template.
///
/// `ActionParameter$required` is reported by `GetAction` for every published action, so
/// which parameters an action needs is part of the contract this service already serves —
/// the same table decides it here, rather than only the wait action being checked. The
/// parameters are judged in the order the catalogued entry records them (sorted by name),
/// so an action missing several of them always earns the same message.
pub(super) fn action_parameters_refusal(
    name: &str,
    action: &ExperimentTemplateAction,
    entry: &ActionCatalogEntry,
) -> Option<ValidationException> {
    let action_id = entry.id;
    for parameter in entry.parameters.iter().filter(|parameter| parameter.required) {
        if action
            .parameters
            .as_ref()
            .is_none_or(|parameters| !parameters.contains_key(parameter.name))
        {
            return Some(invalid_input(format!(
                "actions.{name}.parameters must carry '{}' for action {action_id}.",
                parameter.name
            )));
        }
    }
    // The wait is the one action FIS documents a RANGE for its `duration`: "duration — the
    // duration of the wait", 1 to 720 minutes. Another action's `duration` names the span
    // of that action, for which no bound is published, so its value is stored as sent.
    if action_id != WAIT_ACTION_ID {
        return None;
    }
    let duration = action
        .parameters
        .as_ref()
        .and_then(|parameters| parameters.get(DURATION_PARAMETER))?;
    match wait_duration(duration) {
        Ok(_) => None,
        Err(WaitDurationProblem::OutOfRange) => Some(invalid_input(format!(
            "'{DURATION_PARAMETER}' must be within a range of \
             {WAIT_DURATION_MIN_MINUTES} to {WAIT_DURATION_MAX_MINUTES} minutes."
        ))),
        Err(WaitDurationProblem::NotADuration) => Some(invalid_input(format!(
            "actions.{name}.parameters.'{DURATION_PARAMETER}' must be a \
             duration in ISO 8601 format."
        ))),
    }
}

/// `None` when every entry of `action`'s `targets` map refers to a target of the same
/// template that its catalogued contract `entry` admits; otherwise the
/// `ValidationException` that refuses the template.
///
/// An action's `targets` map says WHICH of the template's targets the action acts on, under
/// the key the action names that target by — `ActionTarget`, which `GetAction` reports for
/// every published action, records those keys and the `resourceType` each one takes. So
/// three things have to hold of one entry, and none of them is expressible in the model
/// (the map is a free `string -> string` one, and a target lives on another member of the
/// template): the key is a target the action supports, the value names a target the SAME
/// template declares, and that target's `resourceType` is the one the action takes under
/// that key. The entries are judged in key order, so an action outside more than one of
/// them always earns the same message.
pub(super) fn action_targets_refusal(
    name: &str,
    action: &ExperimentTemplateAction,
    entry: &ActionCatalogEntry,
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> Option<ValidationException> {
    let action_id = entry.id;
    let declared = action.targets.as_ref()?;
    let mut keys: Vec<(&String, &String)> = declared.iter().collect();
    keys.sort_by_key(|(key, _)| *key);
    for (key, target_name) in keys {
        let Some(supported) = entry.targets.iter().find(|target| target.name == *key) else {
            return Some(invalid_input(format!(
                "actions.{name}.targets.{key} is not a target of action {action_id}."
            )));
        };
        let Some(target) = targets.get(target_name) else {
            return Some(invalid_input(format!(
                "actions.{name}.targets.{key} must name a target the template declares, \
                 got {target_name:?}"
            )));
        };
        let resource_type = target.resource_type.as_deref().unwrap_or_default();
        if resource_type != supported.resource_type {
            return Some(invalid_input(format!(
                "actions.{name}.targets.{key} must name a target of resourceType {}, \
                 got {resource_type:?}",
                supported.resource_type
            )));
        }
    }
    None
}

/// `None` when a request carries no more actions than one template may hold; otherwise the
/// `ServiceQuotaExceededException` that refuses it.
///
/// See [`MAX_ACTIONS_PER_TEMPLATE`]: the cap is a published quota, and it is also what
/// keeps the per-request cost of [`run_plan`] proportional to the request rather than to
/// its square.
pub fn actions_quota_refusal(
    actions: &HashMap<String, ExperimentTemplateAction>,
) -> Option<ServiceQuotaExceededException> {
    if actions.len() <= MAX_ACTIONS_PER_TEMPLATE {
        return None;
    }
    Some(ServiceQuotaExceededException {
        message: Some(format!(
            "The number of actions in an experiment template cannot exceed \
             {MAX_ACTIONS_PER_TEMPLATE}."
        )),
    })
}

/// Why a template record is not one this service admits, carrying the error FIS answers it
/// with.
///
/// The three error CLASSES the admissibility sequence ([`template_refusal`]) produces. Which
/// operation is being refused decides nothing here: a handler lifts the payload into its
/// own operation's error enum, and that lift is the only per-operation part of the refusal.
#[derive(Debug)]
pub enum TemplateRefusal {
    /// A 400 `ValidationException`: the record describes a template FIS would not run.
    Invalid(ValidationException),
    /// A 404 `ResourceNotFoundException`: the record REFERS to a resource that does not
    /// resolve — an `actionId` the published catalog holds no action for.
    NotFound(ResourceNotFoundException),
    /// A 402 `ServiceQuotaExceededException`: the record is past a published quota.
    QuotaExceeded(ServiceQuotaExceededException),
}

/// Why a key of one of this template's maps is outside the modelled bounds of that map's
/// KEY target, or `None` when every one of them is within them.
///
/// The boundary validator does not reach a map's key (see [`map_keys_refusal`]), so these
/// are the modelled constraints a handler of this service applies itself. Judged over the
/// stored RECORD, so a created and an updated template are judged alike, and in a FIXED
/// order — the maps in the order the model declares them, each one's entries by name — so a
/// template outside more than one of them always earns the same message.
///
/// An action's `targets` map is not judged here: its key has to be one of the target keys
/// the action's published entry records ([`action_targets_refusal`]), and every one of
/// those is within `ExperimentTemplateActionTargetName`'s bounds, so a key outside them is
/// already refused as a target that action does not take.
pub(super) fn template_map_key_refusal(template: &ExperimentTemplateRecord) -> Option<ValidationException> {
    let mut targets: Vec<(&String, &ExperimentTemplateTarget)> = template.targets.iter().collect();
    targets.sort_by_key(|(name, _)| *name);
    for (name, target) in targets {
        if let Some(refusal) = map_key_refusal("targets", TARGET_NAME_BOUNDS, name) {
            return Some(refusal);
        }
        // `resourceTags` is a `TagMap`, so the tags a target identifies resources by are
        // bounded by the same key target as a resource's own tags.
        if let Some(refusal) = map_keys_refusal(
            &format!("targets.{name}.resourceTags"),
            TAG_KEY_BOUNDS,
            target
                .resource_tags
                .iter()
                .flatten()
                .map(|(key, _)| key.as_str()),
        ) {
            return Some(refusal);
        }
        if let Some(refusal) = map_keys_refusal(
            &format!("targets.{name}.parameters"),
            PARAMETER_NAME_BOUNDS,
            target
                .parameters
                .iter()
                .flatten()
                .map(|(key, _)| key.as_str()),
        ) {
            return Some(refusal);
        }
    }
    let mut actions: Vec<(&String, &ExperimentTemplateAction)> = template.actions.iter().collect();
    actions.sort_by_key(|(name, _)| *name);
    for (name, action) in actions {
        if let Some(refusal) = map_key_refusal("actions", ACTION_NAME_BOUNDS, name) {
            return Some(refusal);
        }
        if let Some(refusal) = map_keys_refusal(
            &format!("actions.{name}.parameters"),
            PARAMETER_NAME_BOUNDS,
            action
                .parameters
                .iter()
                .flatten()
                .map(|(key, _)| key.as_str()),
        ) {
            return Some(refusal);
        }
    }
    map_keys_refusal(
        "tags",
        TAG_KEY_BOUNDS,
        template.tags.keys().map(String::as_str),
    )
}

/// `None` when `template` is a record this service admits; otherwise the refusal it earns.
///
/// The ONE admissibility sequence of an experiment template, judged over the RECORD rather
/// than over a request's shape — which is how FIS states these rules, about a template and
/// not about one call. `CreateExperimentTemplate` judges the record it builds by this and
/// `UpdateExperimentTemplate` the candidate it would commit, so the two cannot disagree
/// about which templates are admissible and a rule added or reordered here is applied by
/// both at once. Expressing it once is what makes that agreement hold rather than something
/// each handler has to remember.
///
/// The ORDER is part of the rule: a record outside more than one of these earns the first
/// refusal below, so one request always reports the same message.
pub fn template_refusal(template: &ExperimentTemplateRecord) -> Option<TemplateRefusal> {
    // What the model cannot express is that `roleArn` names an IAM ROLE, what FIS documents
    // for a stop condition's `source`/`value` pair, which values the two `experimentOptions`
    // enums admit, how a target identifies its resources and scopes them, the resource types
    // FIS publishes with the parameters each one requires of a target, and the actions
    // FIS publishes with the parameters and the targets each one takes. The modelled
    // constraints — the required members, the lengths and patterns, the 50-entry tag map —
    // were already refused at the boundary, with the one exception the boundary validator
    // does not reach: a map's KEY target, judged first below.
    //
    // The modelled bounds of every KEY of this record's maps come FIRST, because they are
    // constraints the model itself states rather than rules of the service: a template
    // whose target is named by something no `ExperimentTemplateTargetName` can hold is
    // refused on that name and not on what the target says.
    if let Some(refusal) = template_map_key_refusal(template) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    if let Some(refusal) = role_arn_refusal("roleArn", Some(template.role_arn.as_str())) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    if let Some(refusal) = stop_conditions_refusal(&template.stop_conditions) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    if let Some(refusal) = template_experiment_options_refusal(&template.experiment_options) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    // A log configuration says WHERE the experiment's logs are delivered, so one that names
    // no destination is refused rather than stored.
    if let Some(refusal) = log_configuration_refusal(template.log_configuration.as_ref()) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    // The targets are judged BEFORE the actions, because an action refers to them: a
    // template whose target is not one FIS admits — one naming no published resource type,
    // one missing a parameter that type requires, one identifying its resources by both or
    // by neither member, one scoped outside the documented forms — is refused on that target
    // rather than on the action that happens to name it.
    if let Some(refusal) = targets_refusal(&template.targets) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
    // The action count is capped because it is a published quota, and because it is what
    // bounds the work the action graph of this template costs every later
    // `StartExperiment` — a quota, so FIS answers it with its own error class.
    if let Some(refusal) = actions_quota_refusal(&template.actions) {
        return Some(TemplateRefusal::QuotaExceeded(refusal));
    }
    // An action FIS publishes no action for, one missing a parameter its published contract
    // requires, one naming a target the template does not declare, a wait beyond the
    // documented bound and an action order that cannot be carried out at all are each
    // refused HERE rather than persisted: a template a call accepts is one the service
    // would run.
    if let Some(refusal) = actions_refusal(&template.actions, &template.targets) {
        return Some(refusal);
    }
    None
}
