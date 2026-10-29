// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateAdminAccount
//!
//! Tears down the Firewall Manager default administrator association that
//! `AssociateAdminAccount` persisted in
//! [`crate::state::ComAmazonawsFmsAwsfms20180101State::admin_account`].
//!
//! ### Who may call it
//!
//! The modelled documentation reproduced below is written in the second person
//! about the administrator itself — "If you are the default administrator, all
//! Firewall Manager administrators within the organization must first
//! disassociate their accounts before you can disassociate your account" — so
//! the caller of a successful disassociation IS the account whose administrator
//! role is removed, and a request that names no account is about the role the
//! CALLER holds. Firewall Manager's model offers no operation by which one
//! account removes another account's administrator role (`PutAdminAccount` only
//! creates or updates one), which is why "all administrators must FIRST
//! disassociate THEIR accounts" is addressed to them and not to the default
//! administrator.
//!
//! An account can hold either of the two administrator roles this service
//! records, and this operation removes whichever one the caller holds:
//!
//! * the DELEGATED administrator role `PutAdminAccount` onboarded for it
//!   ([`crate::state::ComAmazonawsFmsAwsfms20180101State::organization_admins`]),
//!   which goes `OFFBOARDING` and then `OFFBOARDING_COMPLETE` — "the account
//!   has been removed as an Firewall Manager administrator"
//!   (`AdminAccountSummary$Status`) — after which the organisation holds it as
//!   an administrator no longer;
//! * otherwise the organisation's DEFAULT administrator association
//!   ([`crate::state::ComAmazonawsFmsAwsfms20180101State::admin_account`]),
//!   which comes out LAST (below).
//!
//! The two are separate records, so one account could in principle hold both;
//! the DELEGATED role is resolved first, because it is the role the account was
//! given LAST and so the one it leaves first, and because the default
//! association's own precondition counts the organisation's other
//! administrators — which would otherwise include the caller's own delegated
//! role and refuse the call on the strength of it.
//!
//! A caller that holds neither has nothing it is allowed to disassociate, which
//! Firewall Manager answers with `AccessDeniedException`
//! ([`crate::scaffolded::Scaffolded::access_denied`]): an authorization refusal,
//! not a missing resource, and a shape this service's model does not declare at
//! all. With NO association made and no administrator role of its own, that is
//! the same refusal — the one [`crate::state::require_administrator`], the gate
//! every organisation-scoped operation of this service shares, states; this
//! handler resolves the role mutably (it changes it) and states that refusal
//! through [`crate::state::no_administrator_refusal`], the gate's own wording.
//!
//! Every request this emulator serves is signed by the single emulator account
//! (`engine::emulator_account_id`), so "the caller holds the role" is read
//! against that account id.
//!
//! ### First in, last out
//!
//! The default administrator's association is the organisation's FIRST, so it
//! comes out LAST: "all Firewall Manager administrators within the organization
//! must first disassociate their accounts before you can disassociate your
//! account". So while the organisation still holds any Firewall Manager
//! administrator other than the default one — the accounts
//! `PutAdminAccount` onboards, which
//! [`crate::state::ComAmazonawsFmsAwsfms20180101State::other_administrator_accounts`]
//! reads off the same enumeration `ListAdminAccountsForOrganization` reports —
//! the tear-down the caller asks for is not possible, which is exactly what the
//! modelled `InvalidOperationException` reports ("the operation failed because
//! there was nothing to do or the operation wasn't possible"). The refusal names
//! the administrators that still have to leave, so the caller knows which
//! accounts must call this operation first; each of them leaves through the
//! delegated branch above, which is also what makes them stop being counted
//! here.
//!
//! The precondition belongs to the DEFAULT administrator's association alone: a
//! delegated administrator is not the organisation's first administrator, so
//! nothing has to leave before it may.
//!
//! The precondition is checked on the call that STARTS the tear-down, not on a
//! repeat of one already recorded (see the idempotence below): an administrator
//! onboarded after the request was accepted cannot retroactively refuse it.
//!
//! ### The disassociation is not instantaneous
//!
//! `AccountRoleStatus` (the enum `GetAdminAccount` reports, values in
//! [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`]) carries the non-terminal
//! `PENDING_DELETION` / `DELETING` beside the terminal `DELETED`, so the
//! administrator role is being torn down for a while after this call rather than
//! vanishing with it. This handler records the request by moving the association
//! to `PENDING_DELETION` and storing the instant the tear-down completes
//! ([`crate::state::DISASSOCIATION_SPAN`] from now); nothing sleeps for it.
//! [`AdminAccountRecord::is_active_at`](crate::state::AdminAccountRecord::is_active_at)
//! derives from that instant whether an administrator still holds the slot, and
//! [`settle_admin_account_at`](crate::state::ComAmazonawsFmsAwsfms20180101State::settle_admin_account_at)
//! persists the `DELETED` status that follows, for whichever operation looks
//! next.
//!
//! A delegated administrator's removal is not instantaneous either, for the same
//! modelled reason: `OrganizationStatus` carries the non-terminal `OFFBOARDING`
//! before `OFFBOARDING_COMPLETE`, so
//! [`OrganizationAdminRecord::start_offboarding`](crate::state::OrganizationAdminRecord::start_offboarding)
//! records the instant the removal completes
//! ([`crate::state::ADMIN_OFFBOARDING_SPAN`] from now) on the record's own
//! pending-transition pair, which every reader already derives its status from.
//!
//! The whole request decides from ONE reading of the clock (`SystemTime` is not
//! monotonic), so this handler cannot see the association active and then gone
//! between the check and the write; a caller whose association completes its
//! tear-down first is simply a caller that holds none, and gets the shared
//! refusal.
//!
//! Asking again while the tear-down is still pending is IDEMPOTENT, for either
//! role: the caller still holds it and the request is already recorded, so it
//! answers 200 and leaves the due instant where it is rather than extending it.
//! Once the removal has COMPLETED the caller holds that role no longer, so a
//! further call is the refusal above — and `PutAdminAccount` is what makes a
//! removed account an administrator again.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DisassociateAdminAccount::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateAdminAccount`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json")
//! ```
//!
//! Disassociates an Firewall Manager administrator account. To set a different account as
//! an Firewall Manager administrator, submit a PutAdminAccount request. To set an account
//! as a default administrator account, you must submit an AssociateAdminAccount request.
//! Disassociation of the default administrator account follows the first in, last out
//! principle. If you are the default administrator, all Firewall Manager administrators
//! within the organization must first disassociate their accounts before you can
//! disassociate your account.
//!
//! ## Errors
//!
//! * `InternalErrorException` (HTTP 400) — The operation failed because of a system problem,
//!   even though the request was valid. Retry your request.
//! * `InvalidOperationException` (HTTP 400) — The operation failed because there was nothing
//!   to do or the operation wasn't possible. For example, you might have submitted an
//!   AssociateAdminAccount request for an account ID that was already set as the Firewall
//!   Manager administrator. Or you might have tried to access a Region that's disabled by
//!   default, and that you need to enable for the Firewall Manager administrator account and
//!   for Organizations before you can access it.
//! * `ResourceNotFoundException` (HTTP 400) — The specified resource was not found.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisassociateAdminAccountError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-fms-awsfms-20180101.json");

#[derive(Debug, Clone, Default)]
pub struct DisassociateAdminAccountOutput;

impl DisassociateAdminAccountOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DISASSOCIATE_ADMIN_ACCOUNT_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DisassociateAdminAccountOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DisassociateAdminAccountOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateAdminAccountError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.fms#DisassociateAdminAccount")]
#[derive(Debug)]
pub enum DisassociateAdminAccountErrorModeled {
    InternalErrorException(InternalErrorException),
    InvalidOperationException(InvalidOperationException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_admin_account::DisassociateAdminAccountError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateAdminAccountError = crate::scaffolded::Scaffolded<DisassociateAdminAccountErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateAdminAccountErrorModeled> for DisassociateAdminAccountError {
    fn from(modeled: DisassociateAdminAccountErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateAdminAccount` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateAdminAccount<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateAdminAccount<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateAdminAccount<Eng>
{
    type Input = DisassociateAdminAccountInput;
    type Output = DisassociateAdminAccountOutput;
    type Error = DisassociateAdminAccountError;

    const NAME: &'static str = "DisassociateAdminAccount";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_ADMIN_ACCOUNT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DISASSOCIATE_ADMIN_ACCOUNT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `DisassociateAdminAccountRequest` has no members, so nothing in the request
        // body selects what this call acts on: the association is organisation-wide
        // and the whole decision is made from state. `ctx` carries the Region the
        // request was signed for, which the shared refusal names.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DisassociateAdminAccountErrorModeled::InternalErrorException(
                InternalErrorException {
                    message: Some("the Firewall Manager store is unavailable".to_string()),
                },
            )
            .into());
        };

        // The account every request this emulator serves is signed by — so, the
        // account asking to disassociate.
        let caller = ::engine::emulator_account_id();
        // The Region the request was signed for, which the shared refusal names.
        let region = crate::state::request_region(ctx.parts());
        // ONE reading of the clock for the whole request: this handler checks the
        // association and then writes to it, and `SystemTime` is not monotonic, so
        // a second reading could see it active and then gone.
        let now = ::std::time::SystemTime::now();

        let mut store = state.lock();
        // A DELEGATED administrator — an account `PutAdminAccount` onboarded —
        // is an administrator in its own right, and the administrator role THIS
        // caller holds is its own: the default administrator's association is a
        // different record, which the account holding it tears down. So when
        // the caller holds such a role, this call removes THAT role: the
        // account goes `OFFBOARDING` and the clock completes the removal. First
        // in, last out is a rule about the DEFAULT administrator's association,
        // so nothing has to leave before a delegated administrator may.
        if let Some(delegated) = store.organization_admin_mut_at(caller, now) {
            delegated.start_offboarding(now);
            return Ok(DisassociateAdminAccountOutput);
        }
        // The organisation's other Firewall Manager administrators, as of the
        // same instant, read BEFORE the association is resolved mutably: the
        // first-in-last-out precondition below is about them, and this reading
        // writes nothing, so a call that is refused for any reason leaves the
        // store exactly as it was.
        let other_administrators = store.other_administrator_accounts(now);
        // The administrator gate every organisation-scoped operation goes through,
        // resolved mutably because this one CHANGES the association: with none —
        // including one already torn down, which the clock-derived predicate reads
        // as gone — the caller is not an administrator at all, so it gets the same
        // refusal `crate::state::require_administrator` states.
        let Some(admin) = store.active_admin_account_mut_at(now) else {
            return Err(crate::state::no_administrator_refusal(&region));
        };
        // An association exists, but a disassociation is the holder's own call: an
        // account that holds neither this association nor an administrator role of
        // its own has nothing it may tear down.
        if admin.account_id != caller {
            return Err(crate::scaffolded::Scaffolded::access_denied(format!(
                "account {caller} is not a Firewall Manager administrator, \
                 so it cannot disassociate an administrator account"
            )));
        }

        // Already asked for: keep the completion instant the first call set rather
        // than pushing it out, so a retry cannot stretch the tear-down.
        if !admin.removal_pending() {
            // First in, last out: the default administrator's association is the
            // one the organisation's other administrators were onboarded under,
            // so each of them has to disassociate before this one may.
            if !other_administrators.is_empty() {
                return Err(
                    DisassociateAdminAccountErrorModeled::InvalidOperationException(
                        InvalidOperationException {
                            message: Some(format!(
                                "account {caller} is the Firewall Manager default administrator: \
                                 all Firewall Manager administrators within the organization must \
                                 first disassociate their accounts before you can disassociate \
                                 your account, and these have not: {}",
                                other_administrators.join(", ")
                            )),
                        },
                    )
                    .into(),
                );
            }
            admin.role_status = crate::state::ROLE_STATUS_PENDING_DELETION.to_string();
            admin.removal_due = Some(now + crate::state::DISASSOCIATION_SPAN);
        }
        Ok(DisassociateAdminAccountOutput)
    }
}
