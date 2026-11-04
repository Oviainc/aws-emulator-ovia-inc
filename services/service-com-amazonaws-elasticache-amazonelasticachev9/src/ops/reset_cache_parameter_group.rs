// SPDX-License-Identifier: CC0-1.0

//! @op ResetCacheParameterGroup
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ResetCacheParameterGroup::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ResetCacheParameterGroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json")
//! ```
//!
//! Modifies the parameters of a cache parameter group to the engine or system default
//! value. You can reset specific parameters by submitting a list of parameter names. To
//! reset the entire cache parameter group, specify the ResetAllParameters and
//! CacheParameterGroupName parameters.
//!
//! ## Input
//!
//! * `CacheParameterGroupName` (String) **required** — The name of the cache parameter group
//!   to reset.
//! * `ParameterNameValues` (ParameterNameValueList) — An array of parameter names to reset to
//!   their default values. If ResetAllParameters is true, do not use ParameterNameValues. If
//!   ResetAllParameters is false, you must specify the name of at least one parameter to
//!   reset.
//! * `ResetAllParameters` (Boolean) — If true, all parameters in the cache parameter group
//!   are reset to their default values. If false, only the parameters listed by
//!   ParameterNameValues are reset to their default values. Valid values: true | false
//!
//! ## Output
//!
//! * `CacheParameterGroupName` (String) — The name of the cache parameter group.
//!
//! ## Errors
//!
//! * `CacheParameterGroupNotFoundFault` (HTTP 404) — The requested cache parameter group name
//!   does not refer to an existing cache parameter group.
//! * `InvalidCacheParameterGroupStateFault` (HTTP 400) — The current state of the cache
//!   parameter group does not allow the requested operation to occur.
//! * `InvalidGlobalReplicationGroupStateFault` (HTTP 400) — The Global datastore is not
//!   available or in primary-only state.
//! * `InvalidParameterCombinationException` (HTTP 400) — Two or more incompatible parameters
//!   were specified.
//! * `InvalidParameterValueException` (HTTP 400) — The value for a parameter is invalid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ResetCacheParameterGroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ResetCacheParameterGroupError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.elasticache#ResetCacheParameterGroup")]
#[derive(Debug)]
pub enum ResetCacheParameterGroupErrorModeled {
    CacheParameterGroupNotFoundFault(CacheParameterGroupNotFoundFault),
    InvalidCacheParameterGroupStateFault(InvalidCacheParameterGroupStateFault),
    InvalidGlobalReplicationGroupStateFault(InvalidGlobalReplicationGroupStateFault),
    InvalidParameterCombinationException(InvalidParameterCombinationException),
    InvalidParameterValueException(InvalidParameterValueException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::reset_cache_parameter_group::ResetCacheParameterGroupError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ResetCacheParameterGroupError = crate::scaffolded::Scaffolded<ResetCacheParameterGroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ResetCacheParameterGroupErrorModeled> for ResetCacheParameterGroupError {
    fn from(modeled: ResetCacheParameterGroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `message` as this operation's modelled `InvalidParameterValueException`
/// ("The value for a parameter is invalid", HTTP 400) — the refusal for a value
/// a member of this request may not take, so every place that reaches it words
/// it the same way.
fn invalid_parameter_value(message: String) -> ResetCacheParameterGroupError {
    ResetCacheParameterGroupErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            message: Some(message),
        },
    )
    .into()
}

/// `message` as this operation's modelled `InvalidParameterCombinationException`
/// ("Two or more incompatible parameters were specified", HTTP 400) — the
/// refusal for what `ResetAllParameters` and `ParameterNameValues` say TOGETHER,
/// as opposed to what either of them is worth on its own.
fn invalid_parameter_combination(message: String) -> ResetCacheParameterGroupError {
    ResetCacheParameterGroupErrorModeled::InvalidParameterCombinationException(
        InvalidParameterCombinationException {
            message: Some(message),
        },
    )
    .into()
}

/// The `ResetCacheParameterGroup` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ResetCacheParameterGroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ResetCacheParameterGroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ResetCacheParameterGroup<Eng>
{
    type Input = ResetCacheParameterGroupInput;
    type Output = ResetCacheParameterGroupOutput;
    type Error = ResetCacheParameterGroupError;

    const NAME: &'static str = "ResetCacheParameterGroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RESET_CACHE_PARAMETER_GROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RESET_CACHE_PARAMETER_GROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // "Modifies the parameters of a cache parameter group to the engine or
        // system default value": the values the named group holds for the
        // parameters this request names are REMOVED from the stored group, so
        // each of them reports its family's engine default again — the one copy
        // `DescribeCacheParameters` lays over those defaults.
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::STORE_UNAVAILABLE,
            ));
        };

        // The FORM of the name is checked before the request is interpreted at
        // all: a value the service's identifier grammar does not admit names no
        // group it could ever hold, so it is reported as the malformed
        // parameter it is rather than as a group that does not exist or as one
        // of the service's own default groups (whose names carry a `.`, which
        // the grammar does not admit either). Worded by the one helper every
        // identifier refusal of this service goes through
        // (`crate::state::identifier_refusal`), the same precedence
        // `DeleteCacheParameterGroup` applies to this member.
        if let Some(message) = crate::state::identifier_refusal(
            "Cache Parameter Group Name",
            "CacheParameterGroupName",
            &input.cache_parameter_group_name,
            crate::state::PARAMETER_GROUP_NAME_MAX_LEN,
        ) {
            return Err(invalid_parameter_value(message));
        }

        // "If `ResetAllParameters` is true, do not use `ParameterNameValues`. If
        // `ResetAllParameters` is false, you must specify the name of at least
        // one parameter to reset": exactly one of the two members says what is
        // reset, so a request where both do, or neither does, is the
        // combination `InvalidParameterCombinationException` refuses.
        let requested = input.parameter_name_values.unwrap_or_default();
        let reset_all = input.reset_all_parameters.unwrap_or(false);
        if reset_all && !requested.is_empty() {
            return Err(invalid_parameter_combination(
                "ParameterNameValues must not be supplied when ResetAllParameters is true."
                    .to_string(),
            ));
        }
        if !reset_all && requested.is_empty() {
            return Err(invalid_parameter_combination(
                "ParameterNameValues must name at least one parameter to reset when \
                 ResetAllParameters is false."
                    .to_string(),
            ));
        }
        let now = ::std::time::SystemTime::now();
        // One store lock spans the whole call: every named parameter is held
        // against the group's family BEFORE any of them is removed, so a
        // request naming one parameter the family does not publish resets
        // nothing at all.
        let mut store = state.lock();

        // "The requested cache parameter group name does not refer to an
        // existing cache parameter group" is the modelled 404, resolved through
        // the one lookup every handler naming a `CacheParameterGroupName`
        // shares. This operation answers the first of the service's two
        // templates for that fault
        // (`crate::service::cache_parameter_group_not_found`).
        let requested_name = input.cache_parameter_group_name;
        let Some(group) = store.parameter_group(&requested_name) else {
            return Err(
                ResetCacheParameterGroupErrorModeled::CacheParameterGroupNotFoundFault(
                    CacheParameterGroupNotFoundFault {
                        message: Some(
                            crate::service::cache_parameter_group_not_found::joined_message(
                                &requested_name,
                            ),
                        ),
                    },
                )
                .into(),
            );
        };
        // The name is "stored as a lowercase string", so the stored group's own
        // name is what is written and answered rather than the spelling the
        // request used.
        let name = group.name.clone();
        let family = group.family.clone();
        // Every parameter the group holds a value for, which is what the whole
        // group being reset resets. A parameter it holds no value for already
        // reports the engine's own default, so resetting it changes nothing.
        let held_names: ::std::collections::BTreeSet<String> = if reset_all {
            group.parameter_values.keys().cloned().collect()
        } else {
            ::std::collections::BTreeSet::new()
        };
        // A family's own default group is the service's, not the customer's:
        // its values ARE the engine defaults, so no parameter of it can be put
        // back to one. Only a name the identifier rule above admits reaches
        // here, so this is the refusal of a legal identifier that falls in the
        // reserved namespace.
        if name.starts_with(crate::state::DEFAULT_PARAMETER_GROUP_PREFIX) {
            return Err(invalid_parameter_value(
                crate::state::DEFAULT_PARAMETER_GROUP_IMMUTABLE.to_string(),
            ));
        }
        // The same window `ModifyCacheParameterGroup` documents for the list it
        // shares with this operation ("A maximum of 20 parameters may be
        // modified per request"); `ParameterNameValueList` carries no modelled
        // `@length`, so it is refused here. Measured only once the group this
        // request is about has been resolved, so a list of any length against a
        // group the account does not hold is still answered with the modelled
        // 404.
        if requested.len() > crate::state::MAX_PARAMETERS_PER_REQUEST {
            return Err(invalid_parameter_value(format!(
                "Invalid value for ParameterNameValues: {} parameters were supplied; a maximum of \
                 {} parameters may be reset per request.",
                requested.len(),
                crate::state::MAX_PARAMETERS_PER_REQUEST
            )));
        }

        // Which parameters this request puts back to the engine default: every
        // one the group holds a value for when `ResetAllParameters` is true,
        // and the named ones otherwise. A parameter of a group is a parameter
        // of its FAMILY, so each name has to be one the family publishes and
        // one a customer may change at all; the set is collected first and
        // applied below, so the whole request is all-or-nothing. Naming one
        // parameter twice asks for the one change it already asked for, which
        // the service can make.
        let reset: ::std::collections::BTreeSet<String> = if reset_all {
            held_names
        } else {
            let mut named = ::std::collections::BTreeSet::new();
            for pair in requested {
                // Both members of `ParameterNameValue` are optional, so a pair
                // that names no parameter reaches this handler; it asks for no
                // change this operation can make. The VALUE a pair carries is
                // not read: a reset puts the parameter back to the engine's own
                // value, whatever the group holds for it.
                let parameter_name = pair.parameter_name.unwrap_or_default();
                if parameter_name.is_empty() {
                    return Err(invalid_parameter_value(
                        "Invalid value for ParameterNameValues: every parameter name and value \
                         pair must name a parameter."
                            .to_string(),
                    ));
                }
                let Some(parameter) = store.family_parameter(&family, &parameter_name) else {
                    return Err(invalid_parameter_value(
                        crate::state::unknown_parameter_message(&parameter_name),
                    ));
                };
                if let Err(message) = crate::state::check_parameter_modifiable(parameter) {
                    return Err(invalid_parameter_value(message));
                }
                named.insert(parameter_name);
            }
            named
        };

        // Whether any value this request CHANGES belongs to a parameter the
        // engine applies only at a restart (`Parameter$ChangeType`), which is
        // what the clusters using the group are then owed a reboot for. A
        // parameter whose set value is already the engine's own, and one the
        // group holds no value for, is not changed by the reset.
        let mut deferred_to_reboot = false;
        for parameter_name in &reset {
            let set_value = store
                .parameter_group(&name)
                .and_then(|group| group.set_value(parameter_name));
            let Some(set_value) = set_value else {
                continue;
            };
            let Some(parameter) = store.family_parameter(&family, parameter_name) else {
                continue;
            };
            if parameter.value.as_deref() != Some(set_value)
                && parameter.change_type == crate::state::CHANGE_TYPE_REQUIRES_REBOOT
            {
                deferred_to_reboot = true;
            }
        }

        // Removing the entry is what puts the parameter back to the engine
        // default: the group holds only the values it DIFFERS from its family's
        // defaults by. The group is resolved for the write through the same
        // lookup that found it above, under the one lock that has been held
        // since, so it is still the record that was validated against.
        if let Some(group) = store.parameter_group_mut(&name) {
            for parameter_name in &reset {
                group.parameter_values.remove(parameter_name);
            }
        }
        // Resetting a cache parameter group is one of the events this service
        // records, under the same `cache-parameter-group` source the create and
        // the modify record; see `crate::state::EventRecord`.
        let message = if reset_all {
            "Reset all parameters of cache parameter group".to_string()
        } else {
            format!(
                "Reset {} parameter(s) of cache parameter group",
                reset.len()
            )
        };
        store.record_event(
            crate::state::SOURCE_TYPE_CACHE_PARAMETER_GROUP,
            &name,
            message,
            now,
        );
        // A value the engine picks up only when a node restarts leaves every
        // cluster associated with this group running the value it was started
        // with until it is rebooted, which is what
        // `CacheParameterGroupStatus$ParameterApplyStatus` reports.
        if deferred_to_reboot {
            store.defer_parameter_change_to_reboot(&name);
        }

        // `CacheParameterGroupNameMessage` carries the group's name alone, which
        // is the stored one.
        Ok(ResetCacheParameterGroupOutput {
            cache_parameter_group_name: Some(name),
        })
    }
}
