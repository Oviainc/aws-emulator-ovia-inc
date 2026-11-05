// SPDX-License-Identifier: CC0-1.0

//! @op CreateCertificateAuthority
//! IMPLEMENTED. Appends a successor certificate authority to a stored
//! [`crate::state::ClusterRecord`]'s trust bundle: it mints one
//! [`crate::state::CertificateAuthorityRecord`] on that cluster, plus the
//! [`crate::state::UpdateRecord`] for the asynchronous update that tracks the CA's
//! trust distribution, and answers with both. `DescribeCertificateAuthority` reads
//! the CA back and `DescribeUpdate` the update.
//!
//! The appended CA does not begin signing (`signingStatus` `NOT_USED`) and its
//! `distributionStatus` starts `IN_PROGRESS`, with the clock owing `COMPLETE`
//! [`crate::state::CA_DISTRIBUTION_DELAY`] later — the same instant the update it
//! belongs to is owed `Successful` and the cluster goes back from `UPDATING` to
//! `ACTIVE`. The due instant is stored on the records and materialised by whichever
//! read first runs after it.
//!
//! The cluster must exist in the region the request is signed for, run no other
//! update, and be `ACTIVE`; a cluster already holding
//! [`crate::state::MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`] CAs is refused as a
//! request that is invalid given the state that cluster is in.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCertificateAuthority`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Appends a successor certificate authority (CA) to your cluster, beginning the CA
//! rotation process. A cluster certificate authority is the root of trust for your
//! cluster's control plane. It signs the certificates that secure communication between the
//! Kubernetes API server and its clients, and its public certificate is distributed to your
//! cluster's trust bundle so that worker nodes and clients can verify the API server's
//! identity. Each cluster can have at most two certificate authorities at a time: the
//! outgoing CA that's currently signing (its signingStatus is IN_USE) and one successor CA
//! (signingStatus of NOT_USED) that you can later activate to complete the rotation.
//! Appending a successor CA adds its public certificate to the cluster's trust bundle so
//! that the cluster trusts both CAs simultaneously (the dual trust period), but it doesn't
//! begin signing certificates. Amazon EKS then distributes the successor CA to the Amazon
//! Web Services managed components in your cluster; you can track this through the CA's
//! distributionStatus. The successor CA can't be activated until its distributionStatus is
//! COMPLETE. To activate it as the cluster's signer, use ActivateCertificateAuthority .
//! This is an asynchronous operation that returns an update object. If you don't append a
//! successor CA yourself, Amazon EKS appends one automatically before the outgoing CA
//! approaches expiration. For more information, see Rotate the Amazon EKS cluster
//! certificate authority in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `certificateAuthority` (CertificateAuthoritySummary) — Summary information about the
//!   certificate authority that was created, including its ID and initial signing and
//!   distribution status.
//! * `update` (Update) — An object representing the asynchronous update that adds the
//!   certificate authority to the cluster's trust bundle.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
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
    self, CertificateAuthorityRecord, MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER, STATUS_ACTIVE,
    STATUS_UPDATING,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateCertificateAuthorityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCertificateAuthorityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// Two classes are widened in through `extra_errors`: this SERVICE models both shapes
// and many of its operations declare them, but this one lists neither.
// `ClientException` is the class every operation of the Amazon EKS-managed plane
// refuses a cluster this service does not manage with
// ([`crate::state::connected_cluster_refusal`]), and `InvalidRequestException` — "the
// request is invalid given the state of the cluster" — is the class a cluster that
// already holds every certificate authority it may hold refuses a further append
// with, which is what the recorded service answers for it.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#CreateCertificateAuthority",
    extra_errors = ["ClientException", "InvalidRequestException"]
)]
#[derive(Debug)]
pub enum CreateCertificateAuthorityErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_certificate_authority::CreateCertificateAuthorityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCertificateAuthorityError = crate::scaffolded::Scaffolded<CreateCertificateAuthorityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCertificateAuthorityErrorModeled> for CreateCertificateAuthorityError {
    fn from(modeled: CreateCertificateAuthorityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCertificateAuthority` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCertificateAuthority<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCertificateAuthority<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCertificateAuthority<Eng>
{
    type Input = CreateCertificateAuthorityInput;
    type Output = CreateCertificateAuthorityOutput;
    type Error = CreateCertificateAuthorityError;

    const NAME: &'static str = "CreateCertificateAuthority";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CERTIFICATE_AUTHORITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CERTIFICATE_AUTHORITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateCertificateAuthorityError::from(
                CreateCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let cluster_name = input.cluster_name.clone();
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name identifies a cluster only within one region, so the cluster
        // this appends to is the one in the region the request is signed for. It is
        // resolved BEFORE anything else: `clusterName` is the `@httpLabel` that names
        // the resource being acted on, and a call against a cluster that does not
        // exist is a 404 whatever else the request carries.
        let key = (region, cluster_name.clone());
        let Some(cluster) = guard.clusters.get_mut(&key) else {
            return Err(
                CreateCertificateAuthorityErrorModeled::ResourceNotFoundException(
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
            return Err(CreateCertificateAuthorityErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&cluster_name),
            )
            .into());
        }

        // Materialise the distribution moves the clock already owes this cluster's
        // certificate authorities, so the ceiling below counts their state at `now`
        // and an absorbed retry reports it. Each was due at its stored instant
        // whether or not anybody called.
        for authority in cluster.certificate_authorities.values_mut() {
            authority.materialize_due_transitions(now);
        }
        let cluster = &*cluster;

        // `clientRequestToken` is `@idempotencyToken`: "A unique, case-sensitive
        // identifier that you provide to ensure the idempotency of the request." An
        // SDK generates it once per call and RESENDS the same value when it retries,
        // so a retry of the call that appended a CA is answered with that CA and the
        // update that appended it rather than appending a second one. A blank token
        // identifies nothing, so it never matches: absorbing on it would make two
        // unrelated calls that both sent `""` look like a retry of each other.
        let replayed_token = input
            .client_request_token
            .as_deref()
            // An `@idempotencyToken` is an OPAQUE, case-sensitive string, so the raw
            // value is what is matched and stored; trimming only decides whether the
            // caller sent a token at all.
            .filter(|token| !token.trim().is_empty());
        if let Some(existing) =
            replayed_token.and_then(|token| cluster.certificate_authority_for_token(token))
        {
            // The update is read back from the cluster, so an absorbed retry reports
            // the status the distribution has actually reached by now.
            let update = existing
                .update_id
                .as_deref()
                .and_then(|id| cluster.updates.get(id));
            return Ok(CreateCertificateAuthorityOutput {
                update: update.map(crate::state::UpdateRecord::to_update),
                certificate_authority: Some(existing.to_summary()),
            });
        }

        // One update at a time per cluster: while another is running the cluster is
        // in use, which is exactly what `ResourceInUseException` reports, worded by
        // [`crate::state::update_in_progress`] for the update type this call would
        // have minted.
        if let Some(in_flight) = cluster.in_flight_update() {
            return Err(
                CreateCertificateAuthorityErrorModeled::ResourceInUseException(
                    crate::state::update_in_progress(
                        state::UPDATE_TYPE_CERTIFICATE_AUTHORITY,
                        &cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // Rotating the signer of a control plane that is still being created — or is
        // being deleted, or has failed — is work the cluster cannot take. This
        // operation models no `InvalidRequestException`, and a cluster busy with its
        // own lifecycle is in use, so that is what it reports, naming the status.
        if cluster.status != STATUS_ACTIVE {
            return Err(
                CreateCertificateAuthorityErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(cluster_name.clone()),
                        message: Some(format!(
                            "Cluster {cluster_name} is in {} status; it must be {STATUS_ACTIVE} \
                             to append a certificate authority",
                            cluster.status
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // "Each cluster can have at most two certificate authorities at a time: the
        // outgoing CA that's currently signing ... and one successor CA ... that you
        // can later activate to complete the rotation." A cluster already holding the
        // ceiling has a rotation in flight, so what this reports is not a quota the
        // account can have raised but a fact about the cluster's own state: the
        // request is invalid given that state, and the remediation is to delete one
        // of the certificate authorities the cluster holds. That is the class — and
        // the sentence, carrying the ceiling it names — the recorded service answers
        // it with; nothing but the ceiling varies, so nothing else is interpolated.
        if cluster.certificate_authorities.len() >= MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER {
            return Err(
                CreateCertificateAuthorityErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(cluster_name.clone()),
                        message: Some(format!(
                            "Cluster already has the maximum number of certificate \
                             authorities ({MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER}). Delete \
                             an existing CA before creating a new one."
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // The CA id is derived from the cluster's ARN and its CA counter, so it is
        // distinct from every id this cluster has already used, including a deleted
        // CA's; the update id is minted the way every update id in this service is.
        let cluster_arn = cluster.arn.clone();
        let seq = cluster.certificate_authorities_created;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let authority = CertificateAuthorityRecord::successor(
            &cluster_arn,
            seq,
            now,
            // The FILTERED token, so what the record stores is exactly what a later
            // retry can match on: a blank token identifies nothing and is not kept.
            replayed_token.map(str::to_owned),
            update_id.clone(),
        );
        let update = authority.append_update_record(update_id.clone());
        let output = CreateCertificateAuthorityOutput {
            update: Some(update.to_update()),
            certificate_authority: Some(authority.to_summary()),
        };

        let cluster = guard.clusters.get_mut(&key).ok_or_else(|| {
            CreateCertificateAuthorityError::from(
                CreateCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                }),
            )
        })?;
        // The cluster is under an update until the clock completes it, which is the
        // same instant the appended CA's trust distribution becomes `COMPLETE`.
        cluster.status = STATUS_UPDATING.to_string();
        cluster.updates.insert(update_id, update);
        cluster.certificate_authorities_created = seq + 1;
        cluster
            .certificate_authorities
            .insert(authority.id.clone(), authority);

        Ok(output)
    }
}
