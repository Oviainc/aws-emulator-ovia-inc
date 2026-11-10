// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! decision every operation takes BEFORE it reads any catalog record.
//!
//! `AccessDeniedException` (HTTP 403, "You don't have sufficient access to perform this
//! action.") is a SERVICE-COMMON error of this model
//! (`smithy_query(query="service_info")` lists it under `common_errors`), so it is the
//! answer every operation here gives a caller whose permissions do not cover the action it
//! named. AWS takes that decision at the IAM layer, ahead of the service looking up what
//! the request named, so the refusal says nothing about whether the catalog holds that
//! record.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from), else the IAM user that owns the long-term
//!    key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's own accessors (`principal_policies`, `permissions_boundary_document`), so
//!    a document `iam:PutUserPolicy` wrote takes effect on the next request and deleting
//!    it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal
//! ARN does. That is the split every other gate in this tree makes, and the lookups stay
//! in this crate for the reason those keep theirs there: hoisting them into
//! `service-common` would make every service depending on that crate link the IAM and STS
//! service crates.
//!
//! ## The action and the resource
//!
//! The action is `<arn namespace>:<OperationName>` — this service's `arnNamespace`, which
//! is also its signing name ([`action_prefix`]) — so one policy statement scoped to this
//! service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the CATALOG RECORD the request addresses, as an AWS Marketplace catalog
//! ARN: `arn:aws:aws-marketplace:::catalog/<catalog>/<record type>/<id>`
//! ([`record_resource`]), and the catalog itself for an operation that addresses no single
//! record ([`catalog_resource`]). A Marketplace catalog belongs to the marketplace rather
//! than to one account in one Region, which is why those two ARN fields are empty — the
//! same shape as every other global, account-less AWS resource ARN. The record TYPE is the
//! model's own word for the record (the identifier member the operation takes, less its
//! `Id` suffix: `listing`, `product`, `offer`, `offerSet`), so a statement is scoped the
//! way a caller writes it.
//!
//! Taking the id from the REQUEST rather than from a stored record is also what keeps a
//! refused call from telling an unauthorized caller whether the record it named exists.
//!
//! This service publishes no resource-based policy of its own — the model declares no
//! policy operation and no policy member — so the decision here is the identity-based one
//! alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`) and also the name every request to it is signed
/// under — so the prefix is read off the request's own resolved identity rather than
/// restated here, and the ARNs this gate builds and the actions it asks about cannot name
/// two different services.
#[must_use]
pub fn action_prefix<'ctx>(ctx: &'ctx ::smithy_server_runtime::OperationContext<'_>) -> &'ctx str {
    ctx.identity().signing_name()
}

/// The ARN of the catalog itself, which is what an operation addressing no single record
/// (a search, a listing of purchase options) is judged against.
#[must_use]
pub fn catalog_resource(ctx: &::smithy_server_runtime::OperationContext<'_>, catalog: &str) -> String {
    marketplace_arn(action_prefix(ctx), &format!("catalog/{catalog}"))
}

/// The ARN of ONE catalog record of `record_type` (the model's own word for it, `listing`
/// or `product` or `offer` or `offerSet`) under `catalog`, as the request named it.
#[must_use]
pub fn record_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    catalog: &str,
    record_type: &str,
    id: &str,
) -> String {
    marketplace_arn(
        action_prefix(ctx),
        &format!("catalog/{catalog}/{record_type}/{id}"),
    )
}

/// An AWS Marketplace catalog ARN: `arn:aws:<namespace>:::<resource>`.
///
/// Region and account are EMPTY by construction, because an AWS Marketplace catalog is
/// global and belongs to the marketplace rather than to the calling account — so
/// `::engine::emulator_arn`, which always stamps this emulator's account id, is not the
/// builder for it. Nothing of the caller reaches these two fields, so no ARN this gate
/// builds can disclose an account.
fn marketplace_arn(namespace: &str, resource: &str) -> String {
    format!("arn:aws:{namespace}:::{resource}")
}

/// Authorize this request against `resource`, where `operation` is the operation's own
/// name ([`smithy_server_runtime::Operation::NAME`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation lifts into its
/// own error channel. Call it FIRST in a handler, before any catalog record is looked up.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may
/// take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", action_prefix(ctx));
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from the record it
/// addressed.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: {resource} \
             {reason}"
        )),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about carries this service's own prefix, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"aws-marketplace:*","Resource":"*"}]}"#;
        for operation in [
            "GetListing",
            "GetOffer",
            "GetOfferSet",
            "GetOfferTerms",
            "GetProduct",
            "ListFulfillmentOptions",
            "ListPurchaseOptions",
            "SearchFacets",
            "SearchListings",
        ] {
            let action = format!("aws-marketplace:{operation}");
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected the service-wide allow to cover {action}, got \
                 {reached:?}"
            );
        }
        let outside = decision(document, "marketplacecommerceanalytics:GenerateDataSet", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected the service-wide allow to grant nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: a catalog-record ARN carries no Region and no account, and names the record
    /// the request addressed under the catalog it belongs to.
    #[test]
    fn a_catalog_record_arn_is_global_and_account_less() {
        for (record_type, id) in [
            ("listing", "l-0123-cirrus-query-lens"),
            ("product", "p-0123-cirrus-query-lens"),
            ("offer", "o-0123-cirrus-query-lens"),
            ("offerSet", "os-0123-cirrus-query-lens"),
        ] {
            let built = marketplace_arn(
                "aws-marketplace",
                &format!("catalog/AWSMarketplace/{record_type}/{id}"),
            );
            assert_eq!(
                built,
                format!("arn:aws:aws-marketplace:::catalog/AWSMarketplace/{record_type}/{id}"),
                "case {record_type:?}: expected a global, account-less catalog ARN, got {built:?}"
            );
            assert!(
                !built.contains(::engine::emulator_account_id()),
                "case {record_type:?}: a catalog ARN discloses no account, got {built:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused it — every part taken from the request
    /// and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:iam::{}:user/catalog-reader",
            ::engine::emulator_account_id()
        );
        let action = "aws-marketplace:GetListing";
        let resource =
            marketplace_arn("aws-marketplace", "catalog/AWSMarketplace/listing/l-0001-analytics");
        let reason = "with an explicit deny in an identity-based policy";
        let refusal = not_authorized(&principal, action, &resource, reason);
        assert_eq!(
            refusal.message,
            Some(format!(
                "User: {principal} is not authorized to perform: {action} on resource: {resource} \
                 {reason}"
            )),
            "case \"an explicit deny\": expected IAM's wording, got {:?}",
            refusal.message
        );
    }
}
