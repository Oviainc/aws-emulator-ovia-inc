// SPDX-License-Identifier: CC0-1.0

//! The wire harness the AppIntegrations suites in this crate drive the service
//! through: `restJson1` over the same multi-service router the emulator boots, with
//! only the HTTP status, the `x-amzn-errortype` header and the JSON body inspected.
//!
//! It holds ONLY the plumbing — build a router, send a request, read a member. Every
//! assertion lives in the suite that makes it.
//!
//! KMS is mounted beside AppIntegrations on the SAME engine because
//! `CreateDataIntegration` resolves its required `KmsKey` by calling KMS in process,
//! exactly as the assembled emulator does.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_appintegrations_amazonappintegrationservice::state::{
    application_association_arn, ApplicationAssociationRecord,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region a case signs for unless it is exercising the region rule itself.
pub(crate) const REGION: &str = "us-east-1";

/// A `@httpLabel` carries ONE path segment, so the `:` and `/` inside an ARN have to
/// travel as `%3A` and `%2F` or the request addresses a different route.
pub(crate) fn encode_label(identifier: &str) -> String {
    identifier
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Reply {
    /// A present, non-null member of the response body.
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    /// A present, non-null STRING member of the response body.
    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name).and_then(Value::as_str).map(str::to_string)
    }

    /// The `message` an error body carries, whichever casing the envelope used.
    pub(crate) fn message(&self) -> Option<String> {
        self.string("message").or_else(|| self.string("Message"))
    }
}

/// An emulator with AppIntegrations and KMS mounted on one shared engine, driven
/// in-process through the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::build(true)
    }

    /// An emulator with AppIntegrations mounted ALONE, so a handler's in-process call
    /// to KMS reaches no KMS at all: the lookup answers nothing about the key it was
    /// asked about.
    pub(crate) fn without_kms() -> Self {
        Self::build(false)
    }

    fn build(with_kms: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where the ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_appintegrations_amazonappintegrationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        if with_kms {
            router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// The AppIntegrations store this emulator serves from.
    ///
    /// Only a suite covering a resource NO operation of this API creates needs it: an
    /// application association comes into being on the side of the client that
    /// associates the application, so a case that needs one stored puts it here. Every
    /// response is still read off the wire.
    pub(crate) fn appintegrations_state(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_appintegrations_amazonappintegrationservice::state::ComAmazonawsAppintegrationsAmazonappintegrationserviceState,
    > {
        service_com_amazonaws_appintegrations_amazonappintegrationservice::state::service_state(
            &*self.engine,
        )
        .expect("the AppIntegrations store is reachable from the engine the router runs on")
    }

    /// Store one application association per entry of `client_ids` for the application
    /// `(application_id, application_arn)`, answering the records as the store keeps
    /// them.
    ///
    /// Plumbing rather than an assertion: AppIntegrations publishes no create for an
    /// application association, so a suite that needs one stored puts it in the store
    /// the router serves from and then reads every assertion off the wire. The
    /// association ids are hex-ordered and continue after whatever is already stored, so
    /// the store's order is the order they were added in and two applications never
    /// share one.
    pub(crate) fn seed_application_associations<S: AsRef<str>>(
        &self,
        application_id: &str,
        application_arn: &str,
        client_ids: &[S],
    ) -> Vec<ApplicationAssociationRecord> {
        let state = self.appintegrations_state();
        let mut guard = state.lock();
        let first = guard.application_associations.len();
        let mut seeded = Vec::new();
        for (offset, client_id) in client_ids.iter().enumerate() {
            let id = format!("{:08x}-0000-4000-8000-000000000000", first + offset);
            let record = ApplicationAssociationRecord {
                arn: application_association_arn(REGION, application_id, &id),
                id: id.clone(),
                application_id: application_id.to_string(),
                application_arn: application_arn.to_string(),
                client_id: client_id.as_ref().to_string(),
            };
            guard.application_associations.insert(id, record.clone());
            seeded.push(record);
        }
        seeded
    }

    /// Send one AppIntegrations request, signed for `region`.
    pub(crate) async fn send(
        &self,
        method: &str,
        uri: &str,
        region: &str,
        body: Option<Value>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/app-integrations/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("app-integrations.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("the request body serializes"),
                ))
                .expect("the request builds"),
            None => builder.body(Bytes::new()).expect("the request builds"),
        };
        self.dispatch(request).await
    }

    /// Mint a KMS key in `region` through the mounted KMS service and answer its ARN,
    /// so a `KmsKey` a case means to be REAL is one KMS can resolve.
    pub(crate) async fn create_kms_key(&self, region: &str) -> String {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kms/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://kms.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", "TrentService.CreateKey")
            .header("authorization", authorization)
            .body(Bytes::from_static(b"{}"))
            .expect("the request builds");
        let reply = self.dispatch(request).await;
        assert_eq!(
            reply.status, 200,
            "CreateKey must succeed so a case has a real key, got {} / {}",
            reply.status, reply.body
        );
        reply.body["KeyMetadata"]["Arn"]
            .as_str()
            .expect("CreateKey reports the new key's ARN")
            .to_string()
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> Reply {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}
