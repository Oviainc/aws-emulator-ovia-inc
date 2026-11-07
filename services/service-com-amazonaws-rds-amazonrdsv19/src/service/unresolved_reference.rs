// SPDX-License-Identifier: CC0-1.0

//! The one refusal this front door has to answer BEFORE the framework reads the request:
//! the not-found fault of a resource the request REFERENCES, for a request the boundary
//! validator would otherwise refuse for an absent member before any handler ran.
//!
//! Amazon RDS resolves the resources a create REFERENCES before it reports anything absent
//! from the request itself: a `CreateDBInstance` naming a `DBClusterIdentifier` that
//! resolves to nothing is answered `404 DBClusterNotFoundFault` even when the request also
//! left out a member the shape declares `@required`, because the missing cluster is the
//! obstacle the caller has to clear — a client branching on that fault (to create the
//! cluster first) never takes that branch if it is told to correct a member instead.
//!
//! Every member of `CreateDBInstanceMessage` that carries `@required` also carries
//! `@clientOptional`, which is Smithy's statement that presence is NOT enforced at the
//! boundary. The framework enforces it anyway — an absent required member is a codec error
//! raised while the input is being read, so the handler never runs and the service never
//! gets to resolve anything — and the status that refusal renders with belongs to the
//! protocol, so it cannot be reworded into a modelled 404. This module therefore sits in
//! front of that read, and ONLY there: it answers a request the framework was about to
//! refuse without reaching the handler, and declines every other request, so a request that
//! does reach the handler is refused in exactly the order the handler itself evaluates it.
//!
//! The answer is rendered by the protocol from the operation's own modelled error, not
//! written here, so this refusal and the identical one the handler answers for a complete
//! request are the same bytes.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{OperationContext, OperationError, Service};

use crate::ops::create_db_instance::{CreateDBInstanceError, CreateDBInstanceErrorModeled};
use crate::ops::types::DBClusterNotFoundFault;

/// The `Action` whose references are resolved here.
const CREATE_DB_INSTANCE: &str = "CreateDBInstance";

/// The member naming the DB cluster a new DB instance would join.
const DB_CLUSTER_IDENTIFIER: &str = "DBClusterIdentifier";

/// The members `CreateDBInstanceMessage` declares `@required` (every one of them also
/// `@clientOptional`), from
/// `smithy_query(query="operation",operation="CreateDBInstance",
/// model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json")`.
/// A request that leaves one of them out is one the framework refuses before the handler
/// runs, which is the only request this module answers.
const REQUIRED_MEMBERS: [&str; 3] = ["DBInstanceIdentifier", "DBInstanceClass", "Engine"];

/// The `DBClusterIdentifier` a `CreateDBInstance` request names, when that request is one
/// the framework would refuse for an absent required member before the handler ran.
///
/// `None` for every other request: another action, a request carrying all of
/// [`REQUIRED_MEMBERS`] (which reaches the handler, where the cluster is resolved in the
/// handler's own order), or one naming no cluster at all (which references nothing to
/// resolve, and is refused for its absent member as the framework states it).
fn referenced_db_cluster(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    if aws_smithy_schema_server::extract_action(body)? != CREATE_DB_INSTANCE {
        return None;
    }
    let pairs = aws_smithy_schema_server::parse_form_urlencoded(body);
    let named = |member: &str| {
        pairs
            .iter()
            .any(|(name, value)| name == member && !value.is_empty())
    };
    if REQUIRED_MEMBERS.iter().all(|member| named(member)) {
        return None;
    }
    pairs
        .into_iter()
        .find(|(name, value)| name == DB_CLUSTER_IDENTIFIER && !value.is_empty())
        .map(|(_, value)| value)
}

/// The modelled `404 DBClusterNotFoundFault` for such a request, or `None` when there is
/// nothing to answer: a request this module does not claim, a cluster that resolves, a
/// credential scope naming no region this service stores records under, or no store at all.
///
/// Reads the store and writes nothing to it, which is the whole of what this gate does: it
/// decides a refusal and leaves the state exactly as the next caller would have found it.
pub fn refuse_absent_db_cluster<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> Option<RtResponse> {
    let named = referenced_db_cluster(req)?;
    let region = crate::state::request_region(ctx).ok()?;
    let state = crate::state::service_state(engine)?;
    if state.lock().resolve_db_cluster(&region, &named).is_some() {
        return None;
    }
    let error = CreateDBInstanceError::from(CreateDBInstanceErrorModeled::DBClusterNotFoundFault(
        DBClusterNotFoundFault {
            message: Some(crate::state::db_cluster_cannot_be_accessed_message(&named)),
        },
    ));
    // The wire code is the model's own `awsQueryError` code where it declares one, exactly
    // as the framework's error tail picks it, so a client matching on the code sees the
    // same value whichever path answered.
    let code = error
        .query_error_code()
        .unwrap_or_else(|| error.shape_name());
    svc.definition()
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            code,
            error.http_status(),
        )
        .ok()
}
