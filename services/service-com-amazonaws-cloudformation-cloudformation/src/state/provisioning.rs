// SPDX-License-Identifier: CC0-1.0

//! Creating a stack's declared resources in the services that own them, and deleting them
//! when the stack is deleted.
//!
//! ## The rule this module implements
//!
//! A stack is not just a record of a template. When CloudFormation creates a stack it
//! calls the OWNING service's own create API once per resource the template's `Resources`
//! section declares, and it deletes those same resources when the stack is deleted. What
//! each resource IS — the physical id a `Ref` resolves to and the attributes an
//! `Fn::GetAtt` resolves to — is computed by [`super::template`]; what this module adds is
//! the call that makes the resource exist.
//!
//! The call is made in-process through the handler's own callback into this emulator
//! ([`smithy_server_runtime::OperationContext::callback`]), signed with the emulator's
//! credentials: the resource is created by the SAME code path a client's own
//! `CreateDeliveryStream` goes through, so a stack cannot create a resource that differs
//! from one the service would have created itself, and this crate names no other service's
//! crate.
//!
//! The call is written on the owning service's WIRE PROTOCOL rather than through its AWS
//! SDK client (the shape
//! `services/service-com-amazonaws-scheduler-awschronosservice/src/state.rs` uses to ask
//! IAM a question) because of what a stack's resource properties are: CloudFormation's
//! `AWS::KinesisFirehose::DeliveryStream` properties ARE the `CreateDeliveryStream`
//! request's own members, so the evaluated property DOCUMENT is passed through verbatim.
//! A typed SDK builder cannot carry a document through — it would need every member of
//! every destination configuration mapped by hand here, which is precisely the mapping
//! that would drift from the model as the service grows. The one thing CloudFormation
//! itself decides, the resource's name, is the one member set on the way out.
//!
//! ## What a resource type needs before a stack can create it
//!
//! [`provisioner_for`] maps a template resource type to the operations of the service that
//! owns it. The type names that service itself: its middle segment is the service model's
//! own `awsService.cloudFormationName` (Amazon Data Firehose's model carries
//! `cloudFormationName: KinesisFirehose`, so its streams are
//! `AWS::KinesisFirehose::DeliveryStream`).
//!
//! A type with no entry there is left DECLARED-ONLY: [`StoredResource::materialised`]
//! stays `false` and nothing is created in any service. Such a resource still carries the
//! physical id and the attributes CloudFormation DOCUMENTS for its type, because those are
//! computed from the template and the stack's own identity; that is what lets a stack's
//! `Outputs` name the bucket the template asked for while `s3`'s own `CreateBucket` is a
//! 501 stub (`services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket.rs`). The
//! per-resource reads (`DescribeStackResources` and its siblings) report it as the stack
//! DECLARED it, with the status of the deployment that recorded it — this emulator's
//! inherited scope for a resource type no service here creates yet, not a claim that the
//! resource exists in that service. Adding a provisioner for another type is a local
//! change: one arm in [`provisioner_for`] and the create/delete pair it names, and that
//! type's resources then really are created and deleted.
//!
//! A type that DOES have an entry is never passed off as created: whatever its owning
//! service answers — a modelled refusal, or the 501 of an operation this emulator has not
//! implemented — leaves the stack `CREATE_FAILED` with that answer as the reason.
//!
//! ## Identity, and the isolation these calls can and cannot carry
//!
//! The call is signed with the EMULATOR's own credentials, because that is what they are
//! for: "The emulator's own (root) credentials, `ctx.callback().credentials()` and
//! `ctx.callback().aws_credentials_provider()`, only sign this operation's in-process
//! calls to other services" (`AGENTS.md`). The caller's own signature cannot be reused —
//! the emulator never holds a client's secret (its `CredentialStore` holds only
//! credentials an operation minted), so no signature of the caller's could be produced
//! here.
//!
//! Nor is there state for a credential to select. Every service of this emulator reaches
//! its store by TYPE out of the ONE `InMemoryEngine` the router builds and hands to all of
//! them (`AGENTS.md`: "`engine` is deliberately ONE crate shared by every service crate
//! and the router"; `::engine::InMemoryEngine::state::<T>()` takes no identity), and every
//! ARN it mints carries the single canonical account id
//! (`::emulator_server::EMULATOR_ACCOUNT_ID`). A stack's stream therefore lands in exactly
//! the store a client's own `CreateDeliveryStream` writes to — which is the point of
//! provisioning it — and the account half of a credential scope has one value, so the
//! REGION is the only part there is to propagate: these calls carry the stack's own
//! Region, the one the call that created it was signed for, on both the create and the
//! delete.
//!
//! What this module does keep apart is one STACK's resources from another's: a stack's
//! deletion deletes the resources ITS OWN records say it created
//! ([`StoredResource::materialised`]), never everything an owning service happens to hold,
//! and a resource another stack (or a client) already created is not adopted — the owning
//! service answers the create with its own uniqueness rule (Firehose: "This name must be
//! unique per Amazon Web Services account in the same Amazon Web Services Region"), which
//! leaves the second stack `CREATE_FAILED` with that refusal and the existing resource
//! untouched. Both are held to that by
//! `core/router/tests/all/cloudformation_provisions_stack_resources.rs`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{Map, Value};

use super::template::FIREHOSE_DELIVERY_STREAM;
use super::StoredResource;

// The stack whose resources are being created or deleted is identified by its Region
// alone — what the credential scope of every call this module makes names.

/// What deploying a stack's template left its resources as.
#[derive(Debug, Clone, Default)]
pub struct Materialised {
    /// The stack's resources, in the order the template declares them: each one as
    /// [`super::template::provision`] computed it, with `materialised` set and any
    /// attribute the owning service reported for it applied.
    pub resources: Vec<StoredResource>,
    /// The reason the stack cannot reach its completed status: an owning service refused
    /// to create one of the resources, or to delete one the deployment replaced. `None`
    /// when every resource this emulator provisions was created.
    pub failure: Option<String>,
}

/// Reconcile what a stack's newly deployed template declares with what the stack already
/// has: create what is new in the service that owns it, carry forward what is unchanged,
/// and delete what the deployment replaced or removed.
///
/// `region` is the stack's own Region: the resources live there, and it is what the
/// credential scope of every call this module makes names. `previous` is the set the
/// stack's last completed deployment left (empty for a create), which is what makes a
/// re-deploy of the same template a no-op rather than a second create — and what keeps a
/// resource another deployment created from being forgotten, so the stack's deletion still
/// deletes it.
///
/// The resources are created in the order the template declares them. The first refusal
/// from an owning service stops the walk and is reported as [`Materialised::failure`]: the
/// resources created before it are still reported (with their `materialised` flag set), so
/// the stack's deletion can clean them up, and the ones after it are reported as the
/// declaration left them.
pub async fn reconcile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    declared: Vec<StoredResource>,
    previous: &[StoredResource],
) -> Materialised {
    let mut outcome = Materialised::default();
    let mut declared = declared.into_iter();
    for mut resource in declared.by_ref() {
        if let Some(existing) = previous
            .iter()
            .find(|existing| existing.materialised && is_same_resource(existing, &resource))
        {
            // The owning service already created exactly this resource and the deployment
            // did not change its identity, so nothing is created again; what that service
            // reported about it, and the configuration it really has, are carried forward.
            let changed = resource.properties != existing.properties;
            carry_forward(existing, &mut resource);
            if changed {
                // ...but this deployment asks for a DIFFERENT configuration of a resource
                // that already exists, and changing one in place is not something this
                // emulator does: the owning service's update API is not called here, so
                // nothing would have changed there. Reported as a failure of the
                // deployment rather than a success that dropped the caller's change.
                outcome.failure = Some(format!(
                    "The following resource(s) failed to update: [{}]. NotImplemented: this \
                     emulator does not change the properties of an existing {} in place; the \
                     resource was left as it is.",
                    resource.logical_id, resource.resource_type
                ));
                outcome.resources.push(resource);
                break;
            }
            outcome.resources.push(resource);
            continue;
        }
        let created = match provisioner_for(&resource.resource_type) {
            Some(Provisioner::FirehoseDeliveryStream) => {
                create_delivery_stream(ctx, region, &resource).await
            }
            None => Ok(BTreeMap::new()),
        };
        match created {
            Ok(reported) => {
                if provisioner_for(&resource.resource_type).is_some() {
                    // What the owning service answered wins over the computed value: it
                    // is the resource's real identity.
                    resource.attributes.extend(reported);
                    resource.materialised = true;
                }
            }
            Err(refusal) => {
                outcome.failure = Some(format!(
                    "The following resource(s) failed to create: [{}]. {refusal}",
                    resource.logical_id
                ));
                outcome.resources.push(resource);
                break;
            }
        }
        outcome.resources.push(resource);
    }
    // Whatever the failure stopped: nothing was created for it, so it is recorded as the
    // declaration left it — except where the stack ALREADY had that resource, which still
    // exists in its owning service and keeps the record that says so, so the stack's
    // deletion still deletes it.
    for mut resource in declared {
        if let Some(existing) = previous
            .iter()
            .find(|existing| existing.materialised && is_same_resource(existing, &resource))
        {
            carry_forward(existing, &mut resource);
        }
        outcome.resources.push(resource);
    }

    // The resources this deployment replaced or removed exist in their owning services
    // and the stack no longer records them, so the deployment deletes them — a resource
    // the stack stops recording must not be left behind. Judged against what the stack
    // now records (not against the walk above, which a failed create may have stopped
    // early), so one logical resource is never recorded twice.
    let replaced: Vec<StoredResource> = previous
        .iter()
        .filter(|existing| {
            existing.materialised
                && !outcome
                    .resources
                    .iter()
                    .any(|recorded| is_same_resource(existing, recorded))
        })
        .cloned()
        .collect();
    if replaced.is_empty() {
        return outcome;
    }
    if outcome.failure.is_some() {
        // A create failed, so this deployment is not the one that replaces anything: the
        // resources it would have deleted are still there, and the stack keeps their
        // records so its own deletion still deletes them.
        outcome.resources.extend(replaced);
        return outcome;
    }
    let deleted = delete(ctx, region, &replaced).await;
    // A resource an owning service refused to delete still exists there, so the stack
    // keeps its record: the deployment failed, and a later deletion is what has to delete
    // it. The ones that were deleted carry `materialised = false` and are not kept.
    outcome.resources.extend(
        deleted
            .resources
            .into_iter()
            .filter(|resource| resource.materialised),
    );
    outcome.failure = deleted.failure;
    outcome
}

/// Record what an owning service already created: the resource exists there with the
/// identity and the configuration it was created with, so the new record carries both
/// rather than the deployment's own idea of them.
fn carry_forward(existing: &StoredResource, resource: &mut StoredResource) {
    resource.materialised = true;
    for (name, value) in &existing.attributes {
        resource.attributes.insert(name.clone(), value.clone());
    }
    // The properties the resource really has are the ones it was created with. For a
    // resource this deployment keeps unchanged they are the same document; where they
    // differ, this is what keeps the stack from recording a configuration nothing applied.
    resource.properties = existing.properties.clone();
}

/// Whether two records are the same resource: the same logical id, of the same type, with
/// the same physical id — the three things that decide whether a deployment keeps a
/// resource or replaces it. The one place that rule is written, so the record a re-deploy
/// carries forward ([`super::StoredStack::deploy_template`]) and the resource
/// [`reconcile`] leaves alone cannot drift apart.
pub fn is_same_resource(left: &StoredResource, right: &StoredResource) -> bool {
    left.logical_id == right.logical_id
        && left.resource_type == right.resource_type
        && left.physical_resource_id == right.physical_resource_id
}

/// What deleting a stack's resources left.
#[derive(Debug, Clone, Default)]
pub struct Deleted {
    /// The stack's resources as the deletion left them: one whose deletion the owning
    /// service carried out no longer carries `materialised`, so a retried delete does not
    /// ask for it again.
    pub resources: Vec<StoredResource>,
    /// The reason the stack's deletion cannot be reported as complete: an owning service
    /// refused to delete one of the resources.
    pub failure: Option<String>,
}

/// Delete the resources a stack created, newest first — the reverse of the order they were
/// created in, so a resource is deleted before whatever it depends on.
///
/// [`Deleted::failure`] names the first resource whose owning service refused to delete
/// it; the stack's deletion cannot be reported as complete then, and the resources it had
/// already deleted are reported as deleted so a retry does not ask for them again. A
/// resource the service no longer has is a success: the request asks for a state the
/// service is already in.
pub async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
) -> Deleted {
    let mut outcome = Deleted {
        resources: resources.to_vec(),
        failure: None,
    };
    // Newest first, over the indices of the set being reported back, so each resource's
    // record is updated in place as its deletion succeeds.
    for index in (0..outcome.resources.len()).rev() {
        // Nothing was created for a declared-only resource, so there is nothing to
        // delete.
        if !outcome.resources[index].materialised {
            continue;
        }
        let resource = &outcome.resources[index];
        let deleted = match provisioner_for(&resource.resource_type) {
            Some(Provisioner::FirehoseDeliveryStream) => {
                delete_delivery_stream(ctx, region, &resource.physical_resource_id).await
            }
            None => Ok(()),
        };
        match deleted {
            Ok(()) => outcome.resources[index].materialised = false,
            Err(refusal) => {
                outcome.failure = Some(format!(
                    "The following resource(s) failed to delete: [{}]. {refusal}",
                    outcome.resources[index].logical_id
                ));
                return outcome;
            }
        }
    }
    outcome
}

/// The services whose resources this emulator creates. One arm per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provisioner {
    FirehoseDeliveryStream,
}

fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The owning services' own APIs, called in-process through the handler's callback.
// ---------------------------------------------------------------------------

/// Amazon Data Firehose's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: firehose`, service shape `Firehose_20150804`).
const FIREHOSE_SIGNING_NAME: &str = "firehose";
const FIREHOSE_TARGET_PREFIX: &str = "Firehose_20150804";

/// Create the stream an `AWS::KinesisFirehose::DeliveryStream` declares.
///
/// The type's properties are the `CreateDeliveryStream` request's own members — the
/// destination configurations, the source configuration, `DeliveryStreamType`,
/// `DeliveryStreamEncryptionConfigurationInput` and `Tags` are named identically on both
/// sides — so the resolved properties ARE the request, with the one member CloudFormation
/// decides (the stream's name, which is this resource's physical id) set to it.
async fn create_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert(
        "DeliveryStreamName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let reported = call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        &format!("{FIREHOSE_TARGET_PREFIX}.CreateDeliveryStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("DeliveryStreamARN").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(attributes)
}

/// Delete the stream a stack created. A stream Firehose no longer has
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), String> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        &format!("{FIREHOSE_TARGET_PREFIX}.DeleteDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        // The stream is already in the state the delete asks for. Judged on the error
        // CODE, never on the message, which carries the request's own resource name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal.to_string()),
    }
}

/// A refusal another service answered with: its error CODE and its message, kept apart so
/// a caller branches on the code (as a client does) and never on text a request can
/// influence.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl ::std::fmt::Display for Refusal {
    /// One line, as the stack's status reason carries it.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        if self.message.is_empty() {
            write!(formatter, "{}", self.code)
        } else {
            write!(formatter, "{}: {}", self.code, self.message)
        }
    }
}

/// The error code a service answers for a resource it does not have, which a delete asking
/// for exactly that state treats as success.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Call one `awsJson1_1` operation of another emulated service in-process and read its
/// answer document. `Err(refusal)` is the service's own refusal: its code, and the message
/// it worded.
///
/// A 501 — the owning operation is still one of this emulator's stubs — is a refusal like
/// any other, so a resource of a type this module DOES provision is never passed off as
/// created when nothing created it; the type's own entry in [`provisioner_for`] is what
/// decides whether a create is attempted at all.
async fn call_json_rpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    target: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    let body = serde_json::to_vec(request).map_err(|error| Refusal {
        code: "SerializationException".to_string(),
        message: error.to_string(),
    })?;
    let headers = vec![
        ("content-type", "application/x-amz-json-1.1".to_string()),
        ("x-amz-target", target.to_string()),
    ];
    let (status, payload) = dispatch(ctx, signing_name, region, headers, body)
        .await
        .map_err(|message| Refusal {
            code: "InternalFailure".to_string(),
            message,
        })?;
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of(status, &document))
}

/// A service's error answer as the code it carries and the message it worded. The code is
/// the last segment of the awsJson `__type`
/// (`com.amazonaws.firehose#ResourceInUseException` -> `ResourceInUseException`), which is
/// what a client branches on.
fn refusal_of(status: u16, document: &Value) -> Refusal {
    let code = document
        .get("__type")
        .or_else(|| document.get("code"))
        .or_else(|| document.get("Code"))
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let message = document
        .get("message")
        .or_else(|| document.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Refusal { code, message }
}

/// Sign a request with the emulator's own credentials and dispatch it in-process to the
/// service its credential scope names.
///
/// The request is signed because the emulator verifies every request signature when
/// authentication is on; the credential scope is also what routes it, so the service that
/// owns the resource is the one that serves the call.
async fn dispatch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>), String> {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host);
    for (name, value) in &headers {
        request = request.header(*name, value);
    }
    let mut request = request.body(body).map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = callback_credentials(ctx).await?;
    sign_callback_request(
        &mut request,
        signing_name,
        region,
        &access_key_id,
        &secret_access_key,
    )?;

    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| error.to_string())?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &ctx.callback().http_connector(),
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok((status, payload))
}

/// SigV4-sign one in-process request to another emulated service, in place.
///
/// The settings are the ones the emulator's own verifier re-signs with
/// (`vendor/server/src/request_authentication.rs`), so the signature this produces is the
/// one that verifier computes over the same request — which is what lets a stack create a
/// resource with request authentication ON. Public so a test can hold it to exactly that
/// (`tests/all/callback_signature.rs`).
pub fn sign_callback_request(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Result<(), String> {
    use ::aws_sigv4::http_request::{
        sign, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
    };

    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name("aws-emulator-cloudformation")
            .access_key_id(access_key_id)
            .secret_access_key(secret_access_key)
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let signable_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        signable_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}

/// The credentials a callback signs with: the emulator's own, through the provider the
/// framework hands every handler — which falls back to its placeholder pair when the
/// router vends none (request authentication off, where nothing verifies a signature).
async fn callback_credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = ctx
        .callback()
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        credentials.access_key_id().to_string(),
        credentials.secret_access_key().to_string(),
    ))
}
