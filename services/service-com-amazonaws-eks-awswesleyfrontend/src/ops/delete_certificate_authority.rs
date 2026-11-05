// SPDX-License-Identifier: CC0-1.0

//! @op DeleteCertificateAuthority
//! IMPLEMENTED. Removes a [`crate::state::CertificateAuthorityRecord`] from the
//! trust bundle of a stored [`crate::state::ClusterRecord`]: it mints the
//! [`crate::state::UpdateRecord`] for the asynchronous removal and answers with that
//! update plus the certificate authority it is about.
//! `DescribeCertificateAuthority` reads the CA back and `DescribeUpdate` the update.
//!
//! The removal is asynchronous, so the CA reports `distributionStatus` `DELETING`
//! while the update runs and the cluster is `UPDATING`. When the clock completes the
//! update ([`crate::state::UPDATE_COMPLETION_DELAY`] later) the record leaves the
//! cluster's trust bundle, which is held on the update
//! ([`crate::state::PendingClusterConfig::certificate_authority_removal`]) and
//! applied by whichever call first runs after the due instant.
//!
//! The cluster must exist in the region the request is signed for, run no other
//! update and be `ACTIVE`; the CA must be one of that cluster's and must not be the
//! one signing for it, nor the sole successor Amazon EKS appended to keep a rotation
//! path open.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteCertificateAuthority`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes a certificate authority (CA) from your cluster. Deleting a certificate authority
//! removes its public certificate from the cluster's trust bundle. You can't delete the
//! certificate authority that's currently signing certificates for the cluster (its
//! signingStatus is IN_USE) — to remove the outgoing CA, first activate the successor CA
//! with ActivateCertificateAuthority . Amazon EKS also protects a successor CA from
//! deletion in certain cases to keep a valid rotation path — for example, a successor that
//! Amazon EKS appended can't be deleted while it's the only successor on the cluster. This
//! is an asynchronous operation that returns an update object.
//!
//! ## Input
//!
//! * `certificateAuthorityId` (String) **required** — The ID of the certificate authority to
//!   delete. You can't delete the certificate authority that's currently signing certificates
//!   for the cluster.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `certificateAuthority` (CertificateAuthoritySummary) — Summary information about the
//!   certificate authority that is being deleted.
//! * `update` (Update) — An object representing the asynchronous update that removes the
//!   certificate authority from the cluster's trust bundle.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PendingClusterConfig, UpdateRecord, UpdateScope, ACTIVE_CERTIFICATE_AUTHORITY_UNDELETABLE,
    CA_BY_EKS, CA_DISTRIBUTION_DELETING, CA_SIGNING_NOT_USED, STATUS_ACTIVE, STATUS_UPDATING,
    UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID,
    UPDATE_TYPE_CERTIFICATE_AUTHORITY,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteCertificateAuthorityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteCertificateAuthorityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`, as the other
// certificate-authority operations widen it: this SERVICE models the shape and many
// of its operations declare it, but this one does not, and it is the class every
// operation of the Amazon EKS-managed plane refuses a cluster this service does not
// manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DeleteCertificateAuthority",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum DeleteCertificateAuthorityErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_certificate_authority::DeleteCertificateAuthorityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteCertificateAuthorityError = crate::scaffolded::Scaffolded<DeleteCertificateAuthorityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteCertificateAuthorityErrorModeled> for DeleteCertificateAuthorityError {
    fn from(modeled: DeleteCertificateAuthorityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteCertificateAuthority` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteCertificateAuthority<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteCertificateAuthority<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteCertificateAuthority<Eng>
{
    type Input = DeleteCertificateAuthorityInput;
    type Output = DeleteCertificateAuthorityOutput;
    type Error = DeleteCertificateAuthorityError;

    const NAME: &'static str = "DeleteCertificateAuthority";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CERTIFICATE_AUTHORITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CERTIFICATE_AUTHORITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let cluster_name = input.cluster_name.clone();
        let authority_id = input.certificate_authority_id.clone();
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteCertificateAuthorityError::from(
                DeleteCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name identifies a cluster only within one region, so the cluster
        // whose trust bundle this changes is the one in the region the request is
        // signed for. It is resolved BEFORE anything else: `clusterName` is the
        // `@httpLabel` that names the resource being acted on, and a call against a
        // cluster that does not exist is a 404 whatever else the request carries.
        let key = (region, cluster_name.clone());
        let Some(cluster) = guard.clusters.get_mut(&key) else {
            return Err(
                DeleteCertificateAuthorityErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&cluster_name),
                )
                .into(),
            );
        };

        // A cluster certificate authority is the Amazon EKS control plane's own CA, and
        // a cluster `RegisterCluster` connected has no such control plane: this operation
        // does not apply to it. The gate is a fact about the CLUSTER, so it is answered
        // as soon as the cluster is resolved.
        if cluster.is_connected() {
            return Err(DeleteCertificateAuthorityErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&cluster_name),
            )
            .into());
        }

        // Materialise the distribution moves the clock already owes this cluster's
        // certificate authorities, so the gates below judge their state at `now`:
        // each was due at its stored instant whether or not anybody called.
        for authority in cluster.certificate_authorities.values_mut() {
            authority.materialize_due_transitions(now);
        }
        let cluster = &*cluster;

        // The second `@httpLabel` names a resource too, so a CA this cluster does not
        // hold is a 404 before anything about the cluster's or the CA's state is
        // judged.
        let Some(authority) = cluster.certificate_authority(&authority_id) else {
            return Err(
                DeleteCertificateAuthorityErrorModeled::ResourceNotFoundException(
                    // The certificate authority is the resource reported missing, so
                    // the payload names no cluster: see
                    // [`crate::state::child_resource_not_found`].
                    crate::state::child_resource_not_found(format!(
                        "No certificate authority found with id: {authority_id} for cluster: \
                         {cluster_name}."
                    )),
                )
                .into(),
            );
        };
        let signing_status = authority.signing_status.clone();
        let created_by = authority.created_by.clone();
        let never_signed = authority.activated_at.is_none();

        // `clientRequestToken` is `@idempotencyToken`: "A unique, case-sensitive
        // identifier that you provide to ensure the idempotency of the request." An
        // SDK generates it once per call and RESENDS the same value when it retries,
        // so a retry of the call that started a removal is answered with THAT update
        // rather than starting a second one. A blank token identifies nothing, so it
        // never matches.
        let replayed_token = input
            .client_request_token
            .as_deref()
            // An `@idempotencyToken` is an OPAQUE, case-sensitive string, so the raw
            // value is what is matched and stored; trimming only decides whether the
            // caller sent a token at all.
            .filter(|token| !token.trim().is_empty());
        if let Some(update) = replayed_token.and_then(|token| cluster.update_for_token(token)) {
            // The CA is read back from the cluster, so an absorbed retry reports the
            // record as it stands now — and reports none once the removal has
            // completed and taken the record out of the trust bundle.
            let summary = certificate_authority_of(update)
                .and_then(|id| cluster.certificate_authority(id))
                .map(crate::state::CertificateAuthorityRecord::to_summary);
            return Ok(DeleteCertificateAuthorityOutput {
                update: Some(update.to_update()),
                certificate_authority: summary,
            });
        }

        // One update at a time per cluster: while another is running the cluster can
        // take no update at all, so that is judged BEFORE anything about the named
        // certificate authority's own state — the caller's move is to wait for THAT
        // update (`DescribeUpdate` on the id below) and retry.
        if let Some(in_flight) = cluster.in_flight_update() {
            return Err(
                DeleteCertificateAuthorityErrorModeled::ResourceInUseException(
                    crate::state::update_in_progress(
                        UPDATE_TYPE_CERTIFICATE_AUTHORITY,
                        &cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // Changing the trust bundle of a control plane that is still being created —
        // or is being deleted, or has failed — is work the cluster cannot take. This
        // operation models no `InvalidRequestException`, and a cluster busy with its
        // own lifecycle is in use, so that is what it reports, naming the status.
        if cluster.status != STATUS_ACTIVE {
            return Err(in_use(
                &cluster_name,
                format!(
                    "Cluster {cluster_name} is in {} status; it must be {STATUS_ACTIVE} to \
                     delete a certificate authority",
                    cluster.status
                ),
            ));
        }

        // "You can't delete the certificate authority that's currently signing
        // certificates for the cluster (its signingStatus is IN_USE) — to remove the
        // outgoing CA, first activate the successor CA": a CA that signs, or that an
        // activation is on its way to making the signer, is in use. The refusal is
        // the one fixed sentence the recorded service answers it with, remediation
        // included; the request named the CA, so nothing varies in it.
        if signing_status != CA_SIGNING_NOT_USED {
            return Err(in_use(
                &cluster_name,
                ACTIVE_CERTIFICATE_AUTHORITY_UNDELETABLE.to_string(),
            ));
        }

        // "Amazon EKS also protects a successor CA from deletion in certain cases to
        // keep a valid rotation path — for example, a successor that Amazon EKS
        // appended can't be deleted while it's the only successor on the cluster." A
        // SUCCESSOR is a CA that has never signed, which is what tells one apart from
        // the outgoing CA an activation retired — that one the caller may delete.
        let successors = cluster
            .certificate_authorities
            .values()
            .filter(|candidate| candidate.activated_at.is_none())
            .count();
        if created_by == CA_BY_EKS && never_signed && successors == 1 {
            return Err(in_use(
                &cluster_name,
                format!(
                    "Certificate authority {authority_id} is the only successor certificate \
                     authority of cluster {cluster_name} and was appended by Amazon EKS; it \
                     cannot be deleted."
                ),
            ));
        }

        // The removal is asynchronous, so it is owed at the same instant the update
        // that carries it is owed its `Successful` status, and it is held ON that
        // update until then.
        let due = now + UPDATE_COMPLETION_DELAY;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id,
            r#type: UPDATE_TYPE_CERTIFICATE_AUTHORITY.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            // "A key-value map that contains the parameters associated with the
            // update": the certificate authority this removes from the trust bundle.
            params: vec![UpdateParam {
                r#type: Some(UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID.to_string()),
                value: Some(authority_id.clone()),
            }],
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(due),
            pending_config: Some(PendingClusterConfig {
                certificate_authority_removal: Some(authority_id.clone()),
                ..Default::default()
            }),
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            // The trust bundle this removes a CA from is the cluster's own, so
            // this is an update of the cluster, not of a resource inside it.
            scope: UpdateScope::Cluster,
        };
        let answer = update.to_update();

        let cluster = guard.clusters.get_mut(&key).ok_or_else(|| {
            DeleteCertificateAuthorityError::from(
                DeleteCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                }),
            )
        })?;
        // The cluster is under an update until the clock completes it, and the CA is
        // on its way out of the trust bundle meanwhile: it is still a record a caller
        // can read, reporting the distribution status of a CA being removed, which is
        // why the answer carries `DELETING` rather than no CA at all.
        cluster.status = STATUS_UPDATING.to_string();
        cluster.record_update(update);
        let summary = cluster
            .certificate_authority_mut(&authority_id)
            .map(|record| {
                record.distribution_status = CA_DISTRIBUTION_DELETING.to_string();
                // Nothing is owed this record's distribution any more: the next move
                // it has is leaving the trust bundle.
                record.distribution_complete_at = None;
                record.to_summary()
            });

        ::tracing::info!(
            operation = "DeleteCertificateAuthority",
            cluster = %cluster_name,
            certificate_authority_id = %authority_id,
            outcome = "deleting",
            "started the removal of a certificate authority from a cluster's trust bundle"
        );

        Ok(DeleteCertificateAuthorityOutput {
            update: Some(answer),
            certificate_authority: summary,
        })
    }
}

/// The certificate authority an update is about: the value of its
/// `CertificateAuthorityId` parameter, which every certificate-authority update of
/// this service carries.
fn certificate_authority_of(update: &UpdateRecord) -> Option<&str> {
    update
        .params
        .iter()
        .find(|param| param.r#type.as_deref() == Some(UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID))
        .and_then(|param| param.value.as_deref())
}

/// The refusal for a cluster or a certificate authority that is in use: the class
/// the service answers a rotation step it cannot take right now with. One helper so
/// no two of this handler's gates word or classify it differently.
fn in_use(cluster_name: &str, message: String) -> DeleteCertificateAuthorityError {
    DeleteCertificateAuthorityErrorModeled::ResourceInUseException(ResourceInUseException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}
