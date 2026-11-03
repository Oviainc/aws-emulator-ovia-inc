// SPDX-License-Identifier: CC0-1.0

//! WHICH endpoint the model hosts each operation of this service on.
//!
//! # The rule
//!
//! Every operation of this service carries an `@endpoint` host prefix, and that prefix is
//! what names the host an SDK sends the operation to. This service's model declares THREE
//! of them, so the crate records three endpoint classes rather than a prefix and
//! "everything else":
//!
//! * `api.` — the control plane: the environment lifecycle and the tagging operations;
//! * `env.` — a PER-ENVIRONMENT data endpoint, which has to resolve a request to one
//!   environment's Apache Airflow web server before any of this service's own code runs
//!   (see [`super::env_endpoint`]);
//! * `ops.` — the internal operations endpoint, which hosts the one operation the model
//!   marks internal.
//!
//! Which class an operation belongs to decides HOW a request this service cannot serve is
//! WORDED: a request to the per-environment endpoint naming an environment that resolves to
//! nothing is refused with that front door's own fixed sentence, which names no
//! environment, while on EVERY other endpoint the refusal is worded by this service's own
//! layer from the name the request asked for (see
//! [`crate::state::environment_not_found_message`]). The shape that sentence travels in
//! then depends on who answers the client: this service's own modelled error channel
//! renders the protocol's error document, while the operation that PROXIES the
//! environment's Apache Airflow REST API relays the body the endpoint it called sent (see
//! [`super::env_endpoint`]). So this classification is a fact a refusal depends on, and it
//! is held here once instead of being restated — or assumed — at each place that needs it.
//!
//! The table below is checked against the shipped model by
//! `tests/all/endpoint_host_prefix_census.rs`, so it cannot drift from what the model
//! declares.

/// One endpoint of this service, named by the host prefix the model's `@endpoint` trait
/// declares for the operations hosted on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// `api.` — the control plane.
    ControlPlane,
    /// `env.` — the per-environment Apache Airflow data endpoint.
    EnvironmentData,
    /// `ops.` — the internal operations endpoint.
    Operations,
}

impl Endpoint {
    /// The `@endpoint` host prefix this endpoint is named by.
    pub const fn host_prefix(self) -> &'static str {
        match self {
            Self::ControlPlane => "api.",
            Self::EnvironmentData => "env.",
            Self::Operations => "ops.",
        }
    }
}

/// The endpoint the model hosts each operation on, by the routing key (the operation's
/// shape name) the protocol resolves for it.
///
/// Source: this service's Smithy model —
/// `smithy_query(query="operation",operation="CreateCliToken,CreateEnvironment,CreateWebLoginToken,DeleteEnvironment,GetEnvironment,InvokeRestApi,ListEnvironments,ListTagsForResource,PublishMetrics,TagResource,UntagResource,UpdateEnvironment",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`
/// reports each operation's `endpoint.hostPrefix`: `env.` for the three token/REST
/// operations addressed to one environment's web server, `ops.` for `PublishMetrics`, and
/// `api.` for the rest.
pub const OPERATION_ENDPOINTS: [(&str, Endpoint); 12] = [
    ("CreateCliToken", Endpoint::EnvironmentData),
    ("CreateEnvironment", Endpoint::ControlPlane),
    ("CreateWebLoginToken", Endpoint::EnvironmentData),
    ("DeleteEnvironment", Endpoint::ControlPlane),
    ("GetEnvironment", Endpoint::ControlPlane),
    ("InvokeRestApi", Endpoint::EnvironmentData),
    ("ListEnvironments", Endpoint::ControlPlane),
    ("ListTagsForResource", Endpoint::ControlPlane),
    ("PublishMetrics", Endpoint::Operations),
    ("TagResource", Endpoint::ControlPlane),
    ("UntagResource", Endpoint::ControlPlane),
    ("UpdateEnvironment", Endpoint::ControlPlane),
];

/// The endpoint the model hosts `operation` on, or `None` when this service's model binds no
/// operation of that name.
#[must_use]
pub fn endpoint_of(operation: &str) -> Option<Endpoint> {
    OPERATION_ENDPOINTS
        .iter()
        .find(|(name, _)| *name == operation)
        .map(|(_, endpoint)| *endpoint)
}

/// Whether `operation` is one the model hosts on `endpoint`.
#[must_use]
pub fn hosted_on(operation: &str, endpoint: Endpoint) -> bool {
    endpoint_of(operation) == Some(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class the model declares is a class the census holds operations of, and a name
    /// the model does not bind resolves to no endpoint at all.
    #[test]
    fn the_census_classifies_every_operation_into_one_of_the_three_classes() {
        for endpoint in [
            Endpoint::ControlPlane,
            Endpoint::EnvironmentData,
            Endpoint::Operations,
        ] {
            let hosted: Vec<&str> = OPERATION_ENDPOINTS
                .iter()
                .filter(|(_, class)| *class == endpoint)
                .map(|(name, _)| *name)
                .collect();
            assert!(
                !hosted.is_empty(),
                "case {endpoint:?}: the model declares the host prefix {:?}, so the census \
                 must hold the operations hosted on it, got {hosted:?}",
                endpoint.host_prefix()
            );
        }
        assert_eq!(
            endpoint_of("NoSuchOperation"),
            None,
            "a name this model binds no operation of must resolve to no endpoint, got {:?}",
            endpoint_of("NoSuchOperation")
        );
        assert!(
            hosted_on("PublishMetrics", Endpoint::Operations)
                && !hosted_on("PublishMetrics", Endpoint::ControlPlane),
            "the internal operations endpoint hosts PublishMetrics and the control plane does \
             not, got {:?}",
            endpoint_of("PublishMetrics")
        );
    }
}
