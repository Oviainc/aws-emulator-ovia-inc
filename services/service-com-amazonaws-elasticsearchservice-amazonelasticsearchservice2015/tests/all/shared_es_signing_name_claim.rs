// SPDX-License-Identifier: CC0-1.0

//! This service's half of the `es` claim rule: it claims a request for the shared `es`
//! routing key ONLY on the path space its own model publishes.
//!
//! `smithy_query(query="service_info")` gives this model sigv4Name `es`, endpointPrefix
//! `es`, arnNamespace `es`, protocol `restJson1`, API version `2015-01-01` — and the repo
//! carries a SECOND model of that same endpoint, the modern Amazon OpenSearch Service
//! (`services/service-com-amazonaws-opensearch-amazonopensearchservice`), whose
//! `service_info` differs only in `sdkId` and in declaring API version `2021-01-01`. AWS
//! serves both APIs on the one `es.<region>.amazonaws.com` endpoint, and the router
//! resolves a request's routing key from the SigV4 credential scope first, so both services
//! are handed the key `es`: claiming on the signing name ALONE made each claim the other's
//! traffic and the router refused every request to the endpoint `500 AmbiguousService`.
//!
//! The rule: a service under a shared signing name claims a request when the routing key
//! names the endpoint AND the request addresses a path space its own model publishes. Every
//! URI this model binds begins with the literal segment `2015-01-01`
//! (`smithy_query(query="digest")`), so that segment is this service's half of the
//! discriminator — the modern model's `/2021-01-01/…` space is NOT this service's, however
//! similar the path under it looks.
//!
//! The rows below span what the rule generalises over: the API version segment, the path
//! family under it (including the `tags` and `domain` families BOTH models bind, under
//! their own version each), and the three signals a routing key is resolved from. The
//! `core/router/tests/all/es_opensearch_and_elasticsearch_signing_name_routing.rs` suite
//! asserts the other half — that exactly one of the two services claims each request, with
//! both mounted.

use std::sync::Arc;

use service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService;
use smithy_server_runtime::Service;

/// The shared SigV4 signing name / endpoint prefix both `es` models declare.
const SHARED_KEY: &str = "es";

/// How a row addresses the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 `Authorization` credential scope naming this routing key.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming [`SHARED_KEY`].
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never read by a claim.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

#[test]
fn this_service_claims_only_the_path_space_its_own_model_publishes() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    /// A presigned credential scope naming the shared signing name, with its `/`
    /// separators percent-encoded as a presigned URL carries them.
    const PRESIGNED: &str =
        "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Fes%2Faws4_request";

    // (case, how it is addressed, Host, URI path, whether THIS service claims it)
    //
    // A claim reads the routing key and the URI only, never the method, so no row names
    // one.
    let rows: [(&str, Addressed, &str, &str, bool); 14] = [
        (
            "CreateElasticsearchDomain, under this model's version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/es/domain",
            true,
        ),
        (
            "one domain's config under this model's version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/es/domain/legacy-domain/config",
            true,
        ),
        (
            // The `tags` family is bound by both models; this is this model's.
            "the tags family at this model's version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/tags",
            true,
        ),
        (
            "the domain family at this model's version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/domain",
            true,
        ),
        (
            "the packages family at this model's version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/packages",
            true,
        ),
        (
            "a presigned request under this model's version",
            Addressed::Presigned,
            "es.eu-west-2.amazonaws.com",
            "/2015-01-01/es/domain",
            true,
        ),
        (
            "an anonymous request to the es host under this model's version",
            Addressed::Anonymous,
            "es.us-east-1.amazonaws.com",
            "/2015-01-01/es/domain",
            true,
        ),
        (
            // The modern model's space, which this service must leave alone — including
            // the request the AmbiguousService issue was filed on.
            "CreateApplication, at the modern version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/opensearch/application",
            false,
        ),
        (
            "the tags family at the modern version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/tags",
            false,
        ),
        (
            "the domain family at the modern version",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/domain",
            false,
        ),
        (
            "an anonymous request to the es host at the modern version",
            Addressed::Anonymous,
            "es.us-east-1.amazonaws.com",
            "/2021-01-01/opensearch/domain",
            false,
        ),
        (
            "an API version neither model declares",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/2099-01-01/es/domain",
            false,
        ),
        (
            // This model is REST: it binds no `POST /`.
            "the root path",
            Addressed::Signed(SHARED_KEY),
            "emulator.local",
            "/",
            false,
        ),
        (
            // The right path space, but the request names another endpoint entirely.
            "this model's path space signed for another service",
            Addressed::Signed("aoss"),
            "aoss.us-east-1.amazonaws.com",
            "/2015-01-01/es/domain",
            false,
        ),
    ];

    for (case, addressed, host, path, expected) in rows {
        let uri = match addressed {
            Addressed::Presigned => format!("{path}?{PRESIGNED}"),
            _ => path.to_string(),
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", host);
        if let Addressed::Signed(signing_name) = addressed {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, expected,
            "case {case:?} ({path} to host {host}, {addressed:?}): expected claimed \
             {expected}, got {claimed}"
        );
    }
}
