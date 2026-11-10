// SPDX-License-Identifier: CC0-1.0

//! @op DescribeCertificateAuthority
//! IMPLEMENTED. A pure read of the [`crate::state::CertificateAuthorityRecord`] that
//! `CreateCluster` seeded or `CreateCertificateAuthority` appended on the cluster's
//! record: the `CertificateAuthority` this answers with carries the stored id,
//! `createdAt`, provenance, statuses, validity period and public-certificate `data`,
//! not a recomputed copy. Its `scheduledEvents` are computed from that stored
//! validity period, as the model says they are, and `rollbackAvailable` from whether
//! a rollback window is still open at the time of the read.
//!
//! The one write it makes is materialising a distribution move the CLOCK already
//! owed — `IN_PROGRESS` to `COMPLETE` once [`crate::state::CA_DISTRIBUTION_DELAY`]
//! has passed since the append. That move was due whether or not anybody called, so
//! it does not change what a later caller sees.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeCertificateAuthority`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns detailed information about a certificate authority (CA) in your cluster,
//! including its validity period, signing and distribution status, provenance, scheduled
//! auto-activation events, and public certificate data.
//!
//! ## Input
//!
//! * `certificateAuthorityId` (String) **required** — The ID of the certificate authority to
//!   describe.
//! * `clusterName` (String) **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `certificateAuthority` (CertificateAuthority) — An object containing detailed
//!   information about the certificate authority.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeCertificateAuthorityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeCertificateAuthorityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DescribeCertificateAuthority",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum DescribeCertificateAuthorityErrorModeled {
    ClientException(ClientException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_certificate_authority::DescribeCertificateAuthorityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeCertificateAuthorityError = crate::scaffolded::Scaffolded<DescribeCertificateAuthorityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeCertificateAuthorityErrorModeled> for DescribeCertificateAuthorityError {
    fn from(modeled: DescribeCertificateAuthorityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeCertificateAuthority` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeCertificateAuthority<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeCertificateAuthority<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeCertificateAuthority<Eng>
{
    type Input = DescribeCertificateAuthorityInput;
    type Output = DescribeCertificateAuthorityOutput;
    type Error = DescribeCertificateAuthorityError;

    const NAME: &'static str = "DescribeCertificateAuthority";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CERTIFICATE_AUTHORITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CERTIFICATE_AUTHORITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeCertificateAuthorityError::from(
                DescribeCertificateAuthorityErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name identifies a cluster only within one region, and a CA id is
        // only meaningful on its own cluster, so the two are resolved in that order:
        // an absent cluster is reported as the missing CLUSTER, the way every other
        // operation that takes a `clusterName` label reports one, and only a cluster
        // that exists can be missing a certificate authority.
        let Some(cluster) = guard.clusters.get_mut(&(region, input.cluster_name.clone())) else {
            return Err(
                DescribeCertificateAuthorityErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&input.cluster_name),
                )
                .into(),
            );
        };

        // A cluster certificate authority is the Amazon EKS control plane's own CA, and
        // a cluster `RegisterCluster` connected has no such control plane: this operation
        // does not apply to it. The gate is a fact about the CLUSTER, so it precedes
        // resolving the certificate authority id.
        if cluster.is_connected() {
            return Err(DescribeCertificateAuthorityErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        match cluster.certificate_authority_mut(&input.certificate_authority_id) {
            Some(record) => {
                // The one write this read makes: the `IN_PROGRESS` -> `COMPLETE`
                // distribution move the clock already owed this CA. It was due at
                // the stored instant whether or not anybody called, so applying it
                // here does not change what a later caller sees.
                record.materialize_due_transitions(now);
                Ok(DescribeCertificateAuthorityOutput {
                    certificate_authority: Some(record.to_certificate_authority(now)),
                })
            }
            None => Err(
                DescribeCertificateAuthorityErrorModeled::ResourceNotFoundException(
                    // The certificate-authority plane is what answered that it holds
                    // no such certificate authority, and a refusal this service
                    // carries through from another is reported WRAPPED, naming that
                    // service, the status it answered, the id of the call and how many
                    // attempts were made: see
                    // [`crate::state::certificate_authority_not_found`]. The payload
                    // names no cluster — see
                    // [`crate::state::child_resource_not_found`].
                    crate::state::certificate_authority_not_found(),
                )
                .into(),
            ),
        }
    }
}
