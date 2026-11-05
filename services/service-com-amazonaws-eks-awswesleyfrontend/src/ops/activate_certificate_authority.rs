// SPDX-License-Identifier: CC0-1.0

//! @op ActivateCertificateAuthority
//! IMPLEMENTED. Promotes a successor [`crate::state::CertificateAuthorityRecord`]
//! of a stored [`crate::state::ClusterRecord`] to be the cluster's signer: it mints
//! the [`crate::state::UpdateRecord`] for the asynchronous promotion and answers
//! with that update plus the certificate authority it is about.
//! `DescribeCertificateAuthority` reads the CA back and `DescribeUpdate` the update.
//!
//! The promotion is asynchronous, so the CA reports `signingStatus` `ACTIVATING`
//! while the update runs and the cluster is `UPDATING`. When the clock completes the
//! update ([`crate::state::UPDATE_COMPLETION_DELAY`] later) the promoted CA becomes
//! `IN_USE` with its `activatedAt`/`activatedBy` recorded, and whichever CA was
//! signing is retired to `NOT_USED` — it stays in the cluster's trust bundle, with
//! a [`crate::state::CA_ROLLBACK_WINDOW`] rollback window open on it. That change
//! is held on the update ([`crate::state::CertificateAuthorityActivation`]) and
//! applied by whichever call first runs after the due instant.
//!
//! The cluster must exist in the region the request is signed for, run no other
//! update and be `ACTIVE`; the CA must be one of that cluster's, must not already
//! be signing, and must be fully distributed.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ActivateCertificateAuthority`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Activates a successor certificate authority (CA) as the signing certificate authority
//! for your cluster, completing a CA rotation. When you activate a successor CA, Amazon EKS
//! promotes it to be the cluster's signer (its signingStatus becomes IN_USE) and the
//! outgoing CA is retired (NOT_USED). The outgoing CA remains in the cluster's trust bundle
//! but no longer signs certificates. The successor CA you activate must already be present
//! on the cluster and fully distributed (its distributionStatus must be COMPLETE). This is
//! an asynchronous operation that returns an update object you can track with
//! DescribeUpdate . Before you activate the successor CA, make sure the worker nodes you
//! manage and your external clients have been updated to trust it, so they maintain
//! connectivity to the API server after activation. For a limited period after activation,
//! CA rollback is available to revert to the outgoing CA if needed. If you don't activate
//! the successor CA yourself, Amazon EKS activates it automatically as the expiration
//! deadline approaches. For more information, see Rotate the Amazon EKS cluster certificate
//! authority in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `certificateAuthorityId` (String) **required** — The ID of the certificate authority to
//!   activate as the cluster's signing certificate authority. This certificate authority must
//!   already exist on the cluster and have a distributionStatus of COMPLETE.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `certificateAuthority` (CertificateAuthoritySummary) — Summary information about the
//!   certificate authority that is being activated.
//! * `update` (Update) — An object representing the asynchronous update that promotes the
//!   certificate authority to be the cluster's signer.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
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
    self, CertificateAuthorityActivation, PendingClusterConfig, UpdateRecord, UpdateScope,
    CA_BY_CUSTOMER,
    CA_DISTRIBUTION_COMPLETE, CA_DISTRIBUTION_IN_PROGRESS, CA_SIGNING_ACTIVATING, CA_SIGNING_IN_USE,
    CA_SIGNING_NOT_USED, STATUS_ACTIVE, STATUS_UPDATING, UPDATE_COMPLETION_DELAY,
    UPDATE_IN_PROGRESS, UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID, UPDATE_PARAM_SIGNING_STATUS,
    UPDATE_TYPE_CERTIFICATE_AUTHORITY,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ActivateCertificateAuthorityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ActivateCertificateAuthorityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// Three classes are widened in through `extra_errors`, as `CreateCertificateAuthority`
// widens `ClientException`: this SERVICE models all three shapes and many of its
// operations declare them, but this one lists none. `ClientException` is the class
// every operation of the Amazon EKS-managed plane refuses a cluster this service does
// not manage with ([`crate::state::connected_cluster_refusal`]),
// `ResourceInUseException` — "the specified resource is in use" — is the class a
// rotation step is refused with while the cluster or the certificate authority it
// names is busy, and `InvalidRequestException` — "the request is invalid given the
// state of the cluster" — is the class a promotion that asks for the state the
// cluster is already in is refused with; both are what the recorded service answers
// for those cases.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ActivateCertificateAuthority",
    extra_errors = ["ClientException", "InvalidRequestException", "ResourceInUseException"]
)]
#[derive(Debug)]
pub enum ActivateCertificateAuthorityErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::activate_certificate_authority::ActivateCertificateAuthorityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ActivateCertificateAuthorityError = crate::scaffolded::Scaffolded<ActivateCertificateAuthorityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ActivateCertificateAuthorityErrorModeled> for ActivateCertificateAuthorityError {
    fn from(modeled: ActivateCertificateAuthorityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ActivateCertificateAuthority` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ActivateCertificateAuthority<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ActivateCertificateAuthority<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ActivateCertificateAuthority<Eng>
{
    type Input = ActivateCertificateAuthorityInput;
    type Output = ActivateCertificateAuthorityOutput;
    type Error = ActivateCertificateAuthorityError;

    const NAME: &'static str = "ActivateCertificateAuthority";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ACTIVATE_CERTIFICATE_AUTHORITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ACTIVATE_CERTIFICATE_AUTHORITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let cluster_name = input.cluster_name.clone();
        let authority_id = input.certificate_authority_id.clone();
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ActivateCertificateAuthorityError::from(
                ActivateCertificateAuthorityErrorModeled::ServerException(ServerException {
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
        // whose signer this rotates is the one in the region the request is signed
        // for. It is resolved BEFORE anything else: `clusterName` is the
        // `@httpLabel` that names the resource being acted on, and a call against a
        // cluster that does not exist is a 404 whatever else the request carries.
        let key = (region, cluster_name.clone());
        let Some(cluster) = guard.clusters.get_mut(&key) else {
            return Err(
                ActivateCertificateAuthorityErrorModeled::ResourceNotFoundException(
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
            return Err(ActivateCertificateAuthorityErrorModeled::ClientException(
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
        //
        // What the refusal STATES is trust-bundle membership: this operation promotes
        // a certificate authority that is already "present on the cluster", i.e. in
        // the cluster's trust bundle, so an id the bundle does not hold — one never
        // appended, or one a deletion has since taken out of it — is reported as not
        // being in that cluster's TRUSTED LIST, which is the fact a rotation client
        // acts on (its staged successor is gone, rather than a record merely not
        // being found). The sentence is built from the request's own two
        // `@httpLabel`s, and ends in no full stop.
        let Some(authority) = cluster.certificate_authority(&authority_id) else {
            return Err(
                ActivateCertificateAuthorityErrorModeled::ResourceNotFoundException(
                    // The certificate authority is the resource reported missing, so
                    // the payload names no cluster: see
                    // [`crate::state::child_resource_not_found`].
                    crate::state::child_resource_not_found(format!(
                        "Certificate authority {authority_id} is not in the trusted list for \
                         cluster {cluster_name}"
                    )),
                )
                .into(),
            );
        };
        let signing_status = authority.signing_status.clone();
        let distribution_status = authority.distribution_status.clone();

        // `clientRequestToken` is `@idempotencyToken`: "A unique, case-sensitive
        // identifier that you provide to ensure the idempotency of the request." An
        // SDK generates it once per call and RESENDS the same value when it retries,
        // so a retry of the call that started an activation is answered with THAT
        // update rather than starting a second one. A blank token identifies
        // nothing, so it never matches.
        let replayed_token = input
            .client_request_token
            .as_deref()
            // An `@idempotencyToken` is an OPAQUE, case-sensitive string, so the raw
            // value is what is matched and stored; trimming only decides whether the
            // caller sent a token at all.
            .filter(|token| !token.trim().is_empty());
        if let Some(update) = replayed_token.and_then(|token| cluster.update_for_token(token)) {
            // The CA is read back from the cluster, so an absorbed retry reports the
            // signing status the promotion has actually reached by now.
            let summary = certificate_authority_of(update)
                .and_then(|id| cluster.certificate_authority(id))
                .map(crate::state::CertificateAuthorityRecord::to_summary);
            return Ok(ActivateCertificateAuthorityOutput {
                update: Some(update.to_update()),
                certificate_authority: summary,
            });
        }

        // One update at a time per cluster: while another is running the cluster can
        // take no update at all, so that is judged BEFORE anything about the named
        // certificate authority's own state. The append that puts a successor on the
        // cluster is itself an asynchronous update, so a rotation's own next step
        // arrives while that update is in flight, and what such a caller has to do is
        // wait for THAT update (`DescribeUpdate` on the id below) and retry — not
        // conclude anything about the CA it named.
        if let Some(in_flight) = cluster.in_flight_update() {
            return Err(
                ActivateCertificateAuthorityErrorModeled::ResourceInUseException(
                    crate::state::update_in_progress(
                        UPDATE_TYPE_CERTIFICATE_AUTHORITY,
                        &cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // Rotating the signer of a control plane that is still being created — or is
        // being deleted, or has failed — is work the cluster cannot take either. This
        // operation models no `InvalidRequestException`, and a cluster busy with its
        // own lifecycle is in use, so that is what it reports, naming the status.
        if cluster.status != STATUS_ACTIVE {
            return Err(in_use(
                &cluster_name,
                format!(
                    "Cluster {cluster_name} is in {} status; it must be {STATUS_ACTIVE} to \
                     activate a certificate authority",
                    cluster.status
                ),
            ));
        }

        // With the cluster idle, the state of the certificate authority the request
        // names is what decides: the two refusals a caller meets in a rotation — a
        // successor whose trust distribution has not finished, and a CA that is
        // already the signer — are facts about the CA.
        //
        // "Activates a SUCCESSOR certificate authority": the CA this promotes is one
        // that is not signing.
        //
        // The CA that ALREADY signs for the cluster is the state this call asks for,
        // so there is nothing for it to do: the request is invalid given the state
        // the cluster is in, which is the class — and the sentence — the recorded
        // service answers it with. It names both resources the request addressed,
        // the certificate authority and the cluster, and the refusal leaves the
        // signer, its `activatedBy`/`activatedAt` and the cluster untouched: no
        // update is started below.
        if signing_status == CA_SIGNING_IN_USE {
            return Err(
                ActivateCertificateAuthorityErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(cluster_name.clone()),
                        message: Some(format!(
                            "Certificate authority {authority_id} is already active for \
                             cluster {cluster_name}"
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // A CA a promotion already took to `ACTIVATING` is not the signer yet but is
        // being worked on, so promoting it again is nothing this can do either: that
        // one is IN USE.
        if signing_status != CA_SIGNING_NOT_USED {
            return Err(in_use(
                &cluster_name,
                format!(
                    "Certificate authority {authority_id} is in {signing_status} signing \
                     status; only a certificate authority in {CA_SIGNING_NOT_USED} signing \
                     status can be activated."
                ),
            ));
        }

        // "The successor CA you activate must already be present on the cluster and
        // fully distributed (its distributionStatus must be COMPLETE)." A CA whose
        // trust distribution is still running is being worked on — that distribution
        // IS the asynchronous update the append returned — so it is IN USE, which is
        // the class the recorded service answers a not-yet-activatable CA with.
        if distribution_status == CA_DISTRIBUTION_IN_PROGRESS {
            return Err(in_use(
                &cluster_name,
                format!(
                    "Certificate authority {authority_id} is not in a valid state for \
                     activation. Its distributionStatus must be {CA_DISTRIBUTION_COMPLETE}."
                ),
            ));
        }

        // Any other distribution status a CA can hold — its distribution failed, or
        // it is being deleted — is not something waiting changes: the request names
        // a certificate authority that cannot be activated at all, which is what the
        // argument-level parameter error reports.
        if distribution_status != CA_DISTRIBUTION_COMPLETE {
            return Err(
                ActivateCertificateAuthorityErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(cluster_name.clone()),
                        message: Some(format!(
                            "Certificate authority {authority_id} of cluster {cluster_name} has \
                             distributionStatus {distribution_status}; it must be \
                             {CA_DISTRIBUTION_COMPLETE} to be activated"
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // The promotion is asynchronous, so it is owed at the same instant the update
        // that carries it is owed its `Successful` status, and it is held ON that
        // update until then.
        let due = now + UPDATE_COMPLETION_DELAY;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_CERTIFICATE_AUTHORITY.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            // "A key-value map that contains the parameters associated with the
            // update": the certificate authority this promotes, and the signing
            // status it is promoted to — which is what tells an activation update
            // apart from the append update that put the CA on the cluster, since
            // both carry the one certificate-authority update type.
            params: vec![
                UpdateParam {
                    r#type: Some(UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID.to_string()),
                    value: Some(authority_id.clone()),
                },
                UpdateParam {
                    r#type: Some(UPDATE_PARAM_SIGNING_STATUS.to_string()),
                    value: Some(CA_SIGNING_IN_USE.to_string()),
                },
            ],
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(due),
            pending_config: Some(PendingClusterConfig {
                certificate_authority_activation: Some(CertificateAuthorityActivation {
                    certificate_authority_id: authority_id.clone(),
                    activated_at: due,
                    // The caller asked for this activation, not Amazon EKS's own
                    // expiration-deadline automation.
                    activated_by: CA_BY_CUSTOMER.to_string(),
                }),
                ..Default::default()
            }),
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            // The signer this promotes is the cluster's own, so this is an update
            // of the cluster rather than of a resource inside it.
            scope: UpdateScope::Cluster,
        };
        let answer = update.to_update();

        let cluster = guard.clusters.get_mut(&key).ok_or_else(|| {
            ActivateCertificateAuthorityError::from(
                ActivateCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                }),
            )
        })?;
        // The cluster is under an update until the clock completes it, and the CA is
        // on its way to signing meanwhile: the outgoing CA keeps signing until the
        // promotion is due, which is why the successor reports the transient
        // `ACTIVATING` rather than `IN_USE` here.
        cluster.status = STATUS_UPDATING.to_string();
        cluster.record_update(update);
        let summary = cluster
            .certificate_authority_mut(&authority_id)
            .map(|record| {
                record.signing_status = CA_SIGNING_ACTIVATING.to_string();
                record.to_summary()
            });

        Ok(ActivateCertificateAuthorityOutput {
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
/// the recorded service answers a rotation step it cannot take right now with. One
/// helper so no two of this handler's gates word or classify it differently.
fn in_use(cluster_name: &str, message: String) -> ActivateCertificateAuthorityError {
    ActivateCertificateAuthorityErrorModeled::ResourceInUseException(ResourceInUseException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}
