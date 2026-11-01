// SPDX-License-Identifier: CC0-1.0

//! @op AssociateApplicationStatusCheck
//!
//! Associates one application status check with the instances the request names, or with the
//! tags it names, and STORES that association: a tag association is kept as the check's own
//! `TargetTagAssociations` member, which `DescribeApplicationStatusChecks` reports, and an
//! instance association as the per-check result that instance then holds
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::application_status`]), which
//! `DescribeApplicationStatus` reports — "once you create an association, health monitoring
//! automatically begins for the specified instances", and a check that is monitoring but has
//! reported nothing yet is `initializing`.
//!
//! Every rule the operation publishes is enforced before anything is written: exactly one of
//! the two target members, an id that is an application status check id of a check the account
//! holds, no blank tag key, and the documented ceiling of tag associations per check. An
//! instance the account does not hold is reported on its own row instead — the response
//! carries "the associations that were successfully created" beside "the associations that
//! failed to be created", so one unusable instance does not refuse the whole request.
//!
//! Associating something twice changes nothing and answers the same way, so a retry with the
//! same `ClientToken` is already "the service ignores the request but does not return an
//! error": there is no token to record, because the answer is a function of the request and
//! of what the check is associated with.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store is
//! reached through [`crate::state::require_service_state`], the way every handler beside this
//! one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssociateApplicationStatusCheck`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Associates an application status check with instances or tags. Once you create an
//! association, health monitoring automatically begins for the specified instances or for
//! instances that match the specified tags. The following rules apply: You must specify
//! either TargetTagAssociations or InstanceIds, but not both. Specifying both results in an
//! InvalidParameterCombination error. You must own the application status check. The check
//! must already exist in your account. You must not leave tag keys blank. You can create a
//! maximum of 50 tag associations for each application status check. You can use
//! DisassociateApplicationStatusCheck to remove associations. You can associate tags so
//! that the application status check automatically monitors all current and future
//! instances that have the specified tags.
//!
//! ## Input
//!
//! * `ApplicationStatusCheckId` (ApplicationStatusCheckId) **required** — The ID of the
//!   application status check to associate.
//! * `ClientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier that you
//!   provide to ensure that the operation completes no more than one time. If you retry a
//!   request with the same token, the service ignores the request but does not return an
//!   error. For more information, see Ensuring idempotency.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `InstanceIds` (InstanceIdList) — The IDs of the instances to associate with the
//!   application status check.
//! * `TargetTagAssociations` (CustomKeyValuePairRequestSet) — The tags to associate the
//!   application status check with. Each tag is a key-value pair. When you associate tags,
//!   the application status check automatically monitors all instances that have the
//!   specified tags.
//!
//! ## Output
//!
//! * `SuccessfulResults` (SuccessfulAssociationResponseSet) — The associations that were
//!   successfully created.
//! * `UnsuccessfulResults` (UnsuccessfulAssociationResponseSet) — The associations that
//!   failed to be created.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateApplicationStatusCheckError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateApplicationStatusCheckError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#AssociateApplicationStatusCheck")]
#[derive(Debug)]
pub enum AssociateApplicationStatusCheckErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_application_status_check::AssociateApplicationStatusCheckError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateApplicationStatusCheckError = crate::scaffolded::Scaffolded<AssociateApplicationStatusCheckErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateApplicationStatusCheckErrorModeled> for AssociateApplicationStatusCheckError {
    fn from(modeled: AssociateApplicationStatusCheckErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateApplicationStatusCheck` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateApplicationStatusCheck<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateApplicationStatusCheck<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateApplicationStatusCheck<Eng>
{
    type Input = AssociateApplicationStatusCheckInput;
    type Output = AssociateApplicationStatusCheckOutput;
    type Error = AssociateApplicationStatusCheckError;

    const NAME: &'static str = "AssociateApplicationStatusCheck";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_APPLICATION_STATUS_CHECK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_APPLICATION_STATUS_CHECK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use ::std::time::SystemTime;

        use crate::scaffolded::Scaffolded;
        use crate::state::{
            deduplicate_ids, holds_instance_association, holds_tag_association,
            initializing_application_status_detail, instance_association_failure_reason,
            invalid_parameter_combination, missing_parameter,
            require_known_application_status_check, require_service_state,
            system_time_to_date_time, tag_association_bytes, tag_association_limit_exceeded,
            tag_association_value, validate_tag_size, ASSOCIATION_TYPE_EC2_TAG,
            ASSOCIATION_TYPE_INSTANCE_ID, MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS,
            MAX_CLIENT_TOKEN_CHARS, MAX_RESOURCE_IDS, MAX_STORED_BYTES,
        };

        // `ctx` carries request metadata only; this operation reads none of it — what a check
        // is associated with is a fact of the check and of the instances, not of the Region
        // the request was signed for. The store is reached through the engine held on `self`.
        let _ = ctx;

        // "The ID of the application status check to associate": a value that is not an id of
        // this kind is refused BEFORE anything else is read, and a well-formed id that names no
        // check of the account right after it — "you must own the application status check. The
        // check must already exist in your account". The SUBJECT of the call is resolved first,
        // so a caller can tell "not an application status check id" and "no such check" from
        // every other reason a request is refused, and that ORDER lives in the one reader both
        // halves come from.
        let application_status_check_id = input.application_status_check_id.clone();

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();
        require_known_application_status_check(&store, &application_status_check_id)?;

        // How many tag associations the check holds already — what the documented ceiling is
        // counted against, both for the request's own size and for what it would add.
        let held_pairs = store
            .application_status_checks
            .get(&application_status_check_id)
            .map_or(0, |check| {
                check
                    .target_tag_associations
                    .as_deref()
                    .unwrap_or_default()
                    .len()
            });

        // "A unique, case-sensitive identifier that you provide to ensure that the operation
        // completes no more than one time" (`$ClientToken`): this operation needs no record of
        // the token (see the module docs), but the value a caller sends is still held to the
        // length every other token of this crate is.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"
            )));
        }

        // "You must specify either TargetTagAssociations or InstanceIds, but not both.
        // Specifying both results in an InvalidParameterCombination error" — a rule about which
        // members the request SPECIFIES, so it is read off their presence rather than off their
        // contents, and a request that specifies neither (or specifies one with nothing in it)
        // has named no target at all, which is a member it left out.
        let asked_tags = input.target_tag_associations.as_deref().unwrap_or_default();
        let asked_instances = input.instance_ids.as_deref().unwrap_or_default();
        if input.target_tag_associations.is_some() && input.instance_ids.is_some() {
            return Err(invalid_parameter_combination(
                "You must specify either TargetTagAssociations or InstanceIds, but not both",
            ));
        }
        if asked_tags.is_empty() && asked_instances.is_empty() {
            return Err(missing_parameter(
                "You must specify either TargetTagAssociations or InstanceIds",
            ));
        }

        // "The tags to associate the application status check with. Each tag is a key-value
        // pair": "you must not leave tag keys blank", a value is as long as any other tag
        // value, and the same pair named twice names one association. The LIST is bounded
        // first, by the ceiling one check holds: a request naming more pairs than that can
        // never be carried out, so it is refused before any work proportional to it is done.
        if asked_tags.len() > MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS {
            return Err(tag_association_limit_exceeded(held_pairs, asked_tags.len()));
        }
        let mut asked_pairs: Vec<(String, String)> = Vec::with_capacity(asked_tags.len());
        for pair in asked_tags {
            let key = pair.key.clone().unwrap_or_default();
            if key.is_empty() {
                return Err(Scaffolded::invalid_parameter_value(
                    "A tag key cannot be empty",
                ));
            }
            let value = pair.value.clone().unwrap_or_default();
            // Held to the length every tag of this service is, and to nothing else: these are
            // the tags an instance CARRIES, matched rather than created, so a key in the
            // reserved namespace a create may not take is a key this check may legitimately
            // monitor instances by.
            validate_tag_size(&key, &value)?;
            if !asked_pairs
                .iter()
                .any(|(held_key, held_value)| held_key == &key && held_value == &value)
            {
                asked_pairs.push((key, value));
            }
        }

        // "The IDs of the instances to associate with the application status check": the list
        // carries no modelled `@length`, so it is bounded the way every id list of this crate
        // is, and one instance named twice is one association.
        if asked_instances.len() > MAX_RESOURCE_IDS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "InstanceIds carries {} ids, above the maximum of {MAX_RESOURCE_IDS}",
                asked_instances.len()
            )));
        }
        let asked_instance_ids = deduplicate_ids(asked_instances);

        // What this call would do to each item it was given, decided before anything is
        // written: an instance the account holds is associated, and one it does not hold
        // fails on its own row with the reason this service refuses that id with.
        let mut successful: Vec<SuccessfulAssociationResponseObject> = Vec::new();
        let mut unsuccessful: Vec<UnsuccessfulAssociationResponseObject> = Vec::new();
        let mut instances_to_associate: Vec<String> = Vec::new();
        for instance_id in &asked_instance_ids {
            match instance_association_failure_reason(&store, instance_id) {
                Some(reason) => unsuccessful.push(UnsuccessfulAssociationResponseObject {
                    application_status_check_id: Some(application_status_check_id.clone()),
                    association_type: Some(ASSOCIATION_TYPE_INSTANCE_ID.to_string()),
                    association_value: Some(instance_id.clone()),
                    reason: Some(reason),
                }),
                None => {
                    // An instance this check already monitors is already associated as asked,
                    // so the item succeeds and nothing is written for it a second time.
                    if !holds_instance_association(
                        &store,
                        &application_status_check_id,
                        instance_id,
                    ) {
                        instances_to_associate.push(instance_id.clone());
                    }
                    successful.push(SuccessfulAssociationResponseObject {
                        application_status_check_id: Some(application_status_check_id.clone()),
                        association_type: Some(ASSOCIATION_TYPE_INSTANCE_ID.to_string()),
                        association_value: Some(instance_id.clone()),
                    });
                }
            }
        }

        // The tag pairs this call would add: the ones the check does not monitor already.
        let mut pairs_to_store: Vec<CustomTagKeyValueResponsePair> = Vec::new();
        for (key, value) in &asked_pairs {
            let held = store
                .application_status_checks
                .get(&application_status_check_id)
                .is_some_and(|check| holds_tag_association(check, key, value));
            if !held {
                pairs_to_store.push(CustomTagKeyValueResponsePair {
                    key: Some(key.clone()),
                    value: Some(value.clone()),
                });
            }
            successful.push(SuccessfulAssociationResponseObject {
                application_status_check_id: Some(application_status_check_id.clone()),
                association_type: Some(ASSOCIATION_TYPE_EC2_TAG.to_string()),
                association_value: Some(tag_association_value(key, value)),
            });
        }

        // "You can create a maximum of 50 tag associations for each application status
        // check", counted over the pairs the check would hold afterwards.
        if held_pairs.saturating_add(pairs_to_store.len())
            > MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS
        {
            return Err(tag_association_limit_exceeded(
                held_pairs,
                pairs_to_store.len(),
            ));
        }

        // The caller-chosen bytes this call would retain for the process's lifetime: the keys
        // and values of the pairs it stores. An instance association stores only two ids this
        // service minted itself, so it adds no caller-chosen bytes.
        let payload_bytes = tag_association_bytes(&pairs_to_store);
        if store.would_exceed_stored_bytes(payload_bytes) {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                 caller-supplied data, so it cannot store {payload_bytes} more",
                store.stored_bytes()
            )));
        }

        // DryRun sits AFTER every parameter has been validated and BEFORE the first write:
        // the flag asks whether the request WOULD have worked, so one that would have been
        // refused is refused with that reason instead, and one that would have succeeded
        // leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        let now = SystemTime::now();

        // The result each newly associated instance will hold, read off the check BEFORE the
        // write below takes a mutable borrow of the store.
        let detail = store
            .application_status_checks
            .get(&application_status_check_id)
            .map(|check| initializing_application_status_detail(check, now))
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the application status check '{application_status_check_id}' was \
                     resolved and then could not be read"
                ))
            })?;

        // The tag half: the pairs go on the check itself, the member
        // `DescribeApplicationStatusChecks` reports — "the tags associated with the
        // application status check. Instances with these tags are automatically monitored by
        // this check" — so there is one copy of them and a later describe cannot disagree
        // with this answer.
        if !pairs_to_store.is_empty() || !instances_to_associate.is_empty() {
            store.add_stored_bytes(payload_bytes);
            let check = store
                .application_status_checks
                .get_mut(&application_status_check_id)
                .ok_or_else(|| {
                    Scaffolded::internal_error(format!(
                        "the application status check '{application_status_check_id}' was \
                         resolved and then could not be written"
                    ))
                })?;
            check
                .target_tag_associations
                .get_or_insert_with(Vec::new)
                .append(&mut pairs_to_store);
            // "The date and time when the application status check was last updated"
            // (`$LastUpdatedAt`): an association of either half is such an update of the
            // check, so a call that made one is dated here whichever half it named.
            check.last_updated_at = Some(system_time_to_date_time(now));
        }

        // The instance half: health monitoring begins, so each newly associated instance
        // holds this check's result from now on — `initializing` until something reports one.
        for instance_id in instances_to_associate {
            store
                .application_status
                .entry(instance_id)
                .or_default()
                .details
                .insert(application_status_check_id.clone(), detail.clone());
        }

        Ok(AssociateApplicationStatusCheckOutput {
            successful_results: Some(successful),
            unsuccessful_results: Some(unsuccessful),
        })
    }
}
