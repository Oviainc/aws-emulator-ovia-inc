// SPDX-License-Identifier: CC0-1.0

//! ORIGIN ACCESS CONTROL on the data plane: how the edge makes an origin request ON BEHALF
//! OF the distribution that configured it.
//!
//! An `Origin` may name an `OriginAccessControlId`, and the control it names says what
//! CloudFront does to every request it sends that origin: `SigningBehavior` `always` —
//! "CloudFront signs all origin requests, overwriting the `Authorization` header from the
//! viewer request if one exists" — `no-override`, which signs only "if the viewer request
//! doesn't contain the `Authorization` header", and `never`, which "turns off origin access
//! control for all origins in all distributions that use this origin access control". That
//! signature is the whole point of the control: it is what lets a PRIVATE origin be reached
//! through the distribution and through nothing else, because the origin's own
//! resource-based policy grants the read to the [`SERVICE_PRINCIPAL`] service principal and
//! scopes that grant, by `aws:SourceArn`, to the one distribution it was written for.
//!
//! So one origin fetch under an origin access control is two questions, in this order:
//!
//! 1. the DECISION the ORIGIN's own resource policy states about it — may
//!    [`SERVICE_PRINCIPAL`], acting for THIS distribution, perform the action the viewer's
//!    method names on the object the viewer asked for? The document is read from the origin's
//!    own service (for an `s3` origin type, through that bucket's `GetBucketPolicy`), so what
//!    decides a fetch is the document a client's last write left on the bucket: a policy just
//!    written grants the next fetch and one just cleared stops granting it. The decision
//!    itself is the tree's ONE evaluation of IAM policy documents
//!    (`service_common::iam_authorization`), carrying the keys such a grant is scoped by
//!    ([`ORIGIN_REQUEST_CONDITION_KEYS`]), so a grant written for one distribution authorizes
//!    that distribution's fetches alone.
//! 2. the SIGNATURE, which is what the origin serves the request for at all.
//!
//! The signature this emulator applies is made with the credential
//! `OperationContext::callback` hands every handler — the only identity an in-process request
//! here can be signed as — and it is applied ONLY to a fetch the origin's own document grants
//! the CloudFront principal. A fetch that document does NOT grant travels exactly as it would
//! with no origin access control at all: unsigned, so the origin answers it as it answers any
//! caller that presents no credential, in its own vocabulary and with its own status, rather
//! than CloudFront inventing a refusal on the origin's behalf. A document this emulator could
//! not read decides nothing here, so the control's own instruction stands and the request is
//! signed; whatever the origin then answers is what the edge serves.
//!
//! One identity the fetch cannot carry on the credential every in-process call shares it
//! carries on a credential of its own. A decision the ORIGIN's service makes ITSELF about the
//! fetch (the KMS key policy of the key an SSE-KMS origin object's bytes are encrypted under
//! is the one here) has to be made for the CloudFront service principal and this
//! distribution's ARN, not for the account. So a granted fetch is signed with a credential
//! minted for that one fetch and RECORDED, under it, as the call it is
//! ([`sign_as_the_service_principal`]); the serving service reads that record for the access
//! key id the signature was verified against. The resource-policy half is decided here
//! ([`admits`]) and is not decided again from the record.
//!
//! Two limits worth stating. A viewer's own credential is never relayed to an origin
//! ([`super::edge`]'s `HEADERS_NEVER_FORWARDED` says why), so under `no-override` a request
//! whose viewer presented one travels with neither that header nor a signature. And the
//! resource policy of an origin type other than `s3` is read through that origin's own
//! service: those origins (a Lambda function URL, a MediaStore container, a MediaPackage
//! channel) are served by nothing in this emulator, so there is no fetch of them to authorize
//! — the control's signing instruction is still honoured for them, and the fetch reports the
//! origin it could not reach.

use ::std::time::Duration;

use ::aws_smithy_runtime_api::client::http::HttpConnector;
use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::OperationContext;

use crate::ops::types::Origin;
use crate::state::{ComAmazonawsCloudfrontCloudfront20200531State, StoredDistribution};

/// The service principal an origin request CloudFront makes is made as, and so the principal
/// an origin's resource policy grants the read to.
pub const SERVICE_PRINCIPAL: &str = "cloudfront.amazonaws.com";

/// The `SigningBehavior` values under which CloudFront signs EVERY origin request: `always`
/// ("CloudFront signs all origin requests") and `always-amz-auth` ("CloudFront signs all
/// origin requests with Amazon authentication headers"), which differ only in whether the
/// viewer's own `Authorization` is forwarded beside the signature — a header this emulator
/// never forwards.
const SIGNS_EVERY_REQUEST: [&str; 2] = ["always", "always-amz-auth"];

/// The `SigningBehavior` under which CloudFront signs an origin request only "if the viewer
/// request doesn't contain the `Authorization` header".
const SIGNS_UNLESS_THE_VIEWER_PRESENTED_ONE: &str = "no-override";

/// The signing name of the service each `OriginAccessControlOriginType` names
/// (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`: `s3`, `mediastore`,
/// `mediapackagev2`, `lambda`) — the service the origin belongs to, which is the name the
/// origin request's own credential scope carries.
const ORIGIN_TYPE_SIGNING_NAMES: [(&str, &str); 4] = [
    ("s3", "s3"),
    ("mediastore", "mediastore"),
    ("mediapackagev2", "mediapackagev2"),
    ("lambda", "lambda"),
];

/// The `OriginAccessControlOriginType` whose origin is an Amazon S3 bucket, and so the one
/// whose resource policy is a BUCKET policy.
const S3_ORIGIN_TYPE: &str = "s3";

/// The condition key naming the resource a cross-service call was made ON BEHALF OF — here
/// the distribution the origin request is being made for, which is what an origin's own
/// policy scopes its grant to.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition key naming the account that resource belongs to.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// Every condition key ONE origin request carries a value for.
///
/// They are DECLARED on the request
/// (`service_common::iam_authorization::Request::with_published_keys`), because this fetch
/// knows each of them exactly: a statement asking whether one is PRESENT — rather than what
/// it equals — is therefore decided rather than left undecided.
const ORIGIN_REQUEST_CONDITION_KEYS: [&str; 2] =
    [SOURCE_ARN_CONDITION_KEY, SOURCE_ACCOUNT_CONDITION_KEY];

/// The Amazon S3 action a viewer METHOD asks of an S3 origin, from the S3 User Guide's
/// *Actions, resources, and condition keys for Amazon S3*: a read of the object's bytes or
/// its metadata is `s3:GetObject`, a write of it is `s3:PutObject`, and a removal is
/// `s3:DeleteObject`. The action is what an origin's policy states its grant over, so it
/// follows the request rather than being fixed to the read.
const S3_OBJECT_ACTIONS: [(&str, &str); 6] = [
    ("GET", "s3:GetObject"),
    ("HEAD", "s3:GetObject"),
    ("OPTIONS", "s3:GetObject"),
    ("PUT", "s3:PutObject"),
    ("POST", "s3:PutObject"),
    ("DELETE", "s3:DeleteObject"),
];

/// The Region an endpoint host that names none addresses: the global Amazon S3 endpoint
/// (`<bucket>.s3.amazonaws.com`) is served from US East (N. Virginia).
const DEFAULT_REGION: &str = "us-east-1";

/// The label an S3 WEBSITE endpoint host carries before its Region
/// (`<bucket>.s3-website-<region>.amazonaws.com`), which is not part of the Region name.
const WEBSITE_LABEL_PREFIX: &str = "website-";

/// The zone labels that are the endpoint's own rather than a Region or a bucket
/// (`s3.amazonaws.com`, `s3.api.aws`, `<id>.lambda-url.<region>.on.aws`): the trailing run of
/// them is what the Region and the endpoint label are anchored against ([`addressing_labels`]).
const ZONE_LABELS: [&str; 6] = ["amazonaws", "api", "aws", "com", "cn", "on"];

/// The query a `GetBucketPolicy` names the policy sub-resource with
/// (`GET /{Bucket}?policy`).
const BUCKET_POLICY_QUERY: &str = "policy";

/// The S3 error code for a bucket that carries no policy at all — an ANSWER ("this bucket has
/// granted nothing"), not a failure to read one.
const NO_SUCH_BUCKET_POLICY: &str = "NoSuchBucketPolicy";

/// What one origin request is made under, when the origin it addresses carries an origin
/// access control that signs it.
///
/// Owned, and computed under the store's lock, so the decision and the signing happen after
/// that lock is released.
#[derive(Debug, Clone)]
pub struct OriginAccess {
    /// The signing name of the service the origin belongs to, from the control's own
    /// `OriginAccessControlOriginType` ([`ORIGIN_TYPE_SIGNING_NAMES`]).
    signing_name: &'static str,
    /// The Region the origin's host addresses ([`region_of_host`]).
    region: String,
    /// The host the origin request is addressed to, which is also where its resource policy
    /// is read from.
    host: String,
    /// The ARN of the distribution the request is made on behalf of: the value of
    /// [`SOURCE_ARN_CONDITION_KEY`] a grant may be scoped by.
    source_arn: String,
    /// The object an `s3` origin's bucket policy is evaluated over, for an origin whose host
    /// names a bucket; `None` for an origin whose resource policy is not one this emulator
    /// reads.
    s3_object: Option<S3Object>,
}

/// The Amazon S3 object one origin request addresses, as a bucket policy names it.
#[derive(Debug, Clone)]
struct S3Object {
    /// The action the viewer's method asks ([`S3_OBJECT_ACTIONS`]).
    action: &'static str,
    /// The object's ARN, which is what a statement's `Resource` element is matched against.
    resource: String,
}

/// What the origin access control of `origin` makes of one fetch, or `None` when no request
/// to that origin is signed at all: an origin naming no control, a control this account no
/// longer holds, one whose `SigningBehavior` is `never`, one whose behaviour leaves a viewer
/// that presented its own credential alone, and one whose origin type names no service.
pub fn of_fetch(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    distribution: &StoredDistribution,
    origin: &Origin,
    method: &str,
    object_path: &str,
    viewer_presented_a_credential: bool,
) -> Option<OriginAccess> {
    let id = origin.origin_access_control_id.as_deref()?;
    let config = &store.origin_access_controls.get(id)?.config;
    if !signs(&config.signing_behavior, viewer_presented_a_credential) {
        return None;
    }
    let signing_name = signing_name(&config.origin_access_control_origin_type)?;
    // `Origin$DomainName` is caller-supplied, so what this reads it as is a HOSTNAME or
    // nothing: an origin whose domain name is not one names no endpoint of any service, and a
    // control over it signs no request rather than composing one from whatever it says.
    let host = dns_host(&host_of(&origin.domain_name))?;
    let s3_object = (config.origin_access_control_origin_type == S3_ORIGIN_TYPE)
        .then(|| s3_object(&host, method, object_path))
        .flatten();
    Some(OriginAccess {
        signing_name,
        region: region_of_host(&host, signing_name),
        host,
        source_arn: distribution.arn.clone(),
        s3_object,
    })
}

/// Whether a control with this `SigningBehavior` signs the request at hand.
///
/// `always` and `always-amz-auth` sign every origin request; `no-override` signs one whose
/// viewer presented no `Authorization`; `never`, and a behaviour this emulator does not know,
/// sign none.
fn signs(signing_behavior: &str, viewer_presented_a_credential: bool) -> bool {
    if SIGNS_EVERY_REQUEST.contains(&signing_behavior) {
        return true;
    }
    signing_behavior == SIGNS_UNLESS_THE_VIEWER_PRESENTED_ONE && !viewer_presented_a_credential
}

/// The signing name of the service an `OriginAccessControlOriginType` names, or `None` for a
/// type this emulator does not know ([`ORIGIN_TYPE_SIGNING_NAMES`]).
fn signing_name(origin_type: &str) -> Option<&'static str> {
    ORIGIN_TYPE_SIGNING_NAMES
        .iter()
        .find(|(known, _)| *known == origin_type)
        .map(|(_, signing_name)| *signing_name)
}

/// The bucket, the action and the object ARN one fetch of an S3 origin is judged over, or
/// `None` when the origin's host names no bucket or the viewer's method asks no S3 action
/// this maps.
fn s3_object(host: &str, method: &str, object_path: &str) -> Option<S3Object> {
    let bucket = s3_bucket_of_host(host)?;
    let action = S3_OBJECT_ACTIONS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(method))
        .map(|(_, action)| *action)?;
    let key = object_path.trim_start_matches('/');
    Some(S3Object {
        // An S3 object's ARN leaves the Region and the account fields empty, because a
        // bucket name is globally unique: `arn:aws:s3:::<bucket>/<key>`, which is how a
        // bucket policy names an object.
        resource: format!("arn:aws:s3:::{bucket}/{key}"),
        action,
    })
}

/// The bucket an Amazon S3 endpoint host names, or `None` for a host that is not one.
///
/// A CloudFront S3 origin is addressed virtual-hosted-style, so the bucket is every label
/// BEFORE the one naming the S3 endpoint — which keeps a bucket whose own name carries dots
/// whole. The endpoint label is anchored from the RIGHT ([`endpoint_label`]), so a bucket
/// whose own name is spelled like that endpoint (`s3-backups.s3.<region>.amazonaws.com`) is
/// read as the bucket it is rather than as the endpoint. The labels have to spell a name S3
/// holds a bucket under (`service_common::s3_bucket_name`), so a host that does not name one
/// names no bucket — which is also what a PATH-style endpoint host
/// (`s3.<region>.amazonaws.com`) names.
fn s3_bucket_of_host(host: &str) -> Option<String> {
    let labels = addressing_labels(host);
    let (mark, _region) = endpoint_label(&labels, S3_ORIGIN_TYPE)?;
    let bucket = labels.get(..mark).filter(|labels| !labels.is_empty())?.join(".");
    ::service_common::s3_bucket_name::is_well_formed(&bucket).then_some(bucket)
}

/// The labels of `host` with its trailing ZONE labels ([`ZONE_LABELS`]) removed, which is
/// what leaves the endpoint label and the Region at the END of the list and so anchors both
/// from the right.
fn addressing_labels(host: &str) -> Vec<&str> {
    let mut labels: Vec<&str> = host.split('.').collect();
    while labels.len() > 1 && labels.last().is_some_and(|label| ZONE_LABELS.contains(label)) {
        labels.pop();
    }
    labels
}

/// Which label of `labels` names the endpoint of the service `signing_name`, and the Region
/// that label itself carries — `None` for a host that names no such endpoint.
///
/// Read from the RIGHT, because only the right end is anchored: an AWS endpoint host ends
/// with the service's own label (`<bucket>.s3`, `<bucket>.s3-<region>`) or with that label
/// followed by the Region (`<bucket>.s3.<region>`, `<id>.lambda-url.<region>`), while
/// everything to its left is the resource's own name and may be spelled like anything —
/// including like the endpoint.
fn endpoint_label(labels: &[&str], signing_name: &str) -> Option<(usize, Option<String>)> {
    let last = labels.len().checked_sub(1)?;
    let last_label = labels.get(last)?;
    if names_endpoint(last_label, signing_name) {
        return Some((last, region_in_label(last_label, signing_name)));
    }
    let before = last.checked_sub(1)?;
    names_endpoint(labels.get(before)?, signing_name)
        .then(|| (before, Some((*last_label).to_string())))
}

/// Whether one host label names the endpoint of the service `signing_name`: the signing name
/// itself, or the dash form that carries the Region (and the S3 website form) inside the
/// label.
fn names_endpoint(label: &str, signing_name: &str) -> bool {
    label == signing_name
        || label
            .strip_prefix(signing_name)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// The Region an endpoint label carries inside itself
/// (`s3-<region>`, and the website form which carries [`WEBSITE_LABEL_PREFIX`] before it),
/// or `None` for the bare form, which names its Region in the next label or not at all.
fn region_in_label(label: &str, signing_name: &str) -> Option<String> {
    let rest = label.strip_prefix(signing_name)?.strip_prefix('-')?;
    let region = rest.strip_prefix(WEBSITE_LABEL_PREFIX).unwrap_or(rest);
    (!region.is_empty()).then(|| region.to_string())
}

/// The Region the endpoint host `host` of the service `signing_name` addresses.
///
/// An AWS endpoint host names its Region either in the label after the service's
/// (`<bucket>.s3.<region>.amazonaws.com`) or inside that label after a dash
/// (`<bucket>.s3-<region>.amazonaws.com`), both read from the right
/// ([`endpoint_label`]). A host that names none — the global endpoint
/// `<bucket>.s3.amazonaws.com`, whose own label is the last one left — addresses
/// [`DEFAULT_REGION`].
fn region_of_host(host: &str, signing_name: &str) -> String {
    let labels = addressing_labels(host);
    endpoint_label(&labels, signing_name)
        .and_then(|(_mark, region)| region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A host without its port, which is what a credential scope's host and a bucket's own name
/// are read from.
fn host_of(domain_name: &str) -> String {
    domain_name
        .split(':')
        .next()
        .unwrap_or(domain_name)
        .to_ascii_lowercase()
}

/// `host` when it is a well-formed DNS NAME, and `None` when it is not.
///
/// An `Origin$DomainName` is caller-supplied, and everything this module does with it — the
/// bucket it names, the Region it addresses, the resource-policy read it composes itself —
/// treats it as a hostname. So it is read as one HERE, once, and a value that is not one names
/// no origin at all: a name is at most 253 characters of 1-to-63-character labels, each of
/// letters, digits and hyphens alone (RFC 1035's preferred syntax, as every AWS endpoint host
/// and every S3 bucket host is spelled). That rejects everything that is not a host and would
/// otherwise travel in a request this module builds — a path or query separator, userinfo, an
/// address literal, a space, an empty label — so a later test over the name's trailing labels
/// (`AWS_ENDPOINT_ZONES`) is a test on a label boundary and cannot be fooled by a name whose
/// authority ends somewhere else.
fn dns_host(host: &str) -> Option<String> {
    let well_formed = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    well_formed.then(|| host.to_string())
}

/// Whether the ORIGIN's own resource policy leaves this fetch signed: it grants
/// [`SERVICE_PRINCIPAL`], acting for this distribution, the action on the object — or it is
/// a document this emulator cannot read, which decides nothing and leaves the control's own
/// instruction standing.
///
/// `false` only for a document that WAS read and does not grant the fetch, which includes a
/// bucket carrying no policy at all: such an origin has granted CloudFront nothing, so the
/// request travels unsigned and the origin answers it itself.
pub async fn grants_origin_request(
    ctx: &OperationContext<'_>,
    access: &OriginAccess,
    read_timeout: Duration,
) -> bool {
    let Some(object) = &access.s3_object else {
        return true;
    };
    match bucket_policy(ctx, access, read_timeout).await {
        PolicyRead::Document(document) => {
            let granted = admits(&document, access, object);
            if !granted {
                ::tracing::debug!(
                    origin = %access.host,
                    action = %object.action,
                    "the origin's own policy does not grant this distribution the fetch, so \
                     the origin request is not signed for it"
                );
            }
            granted
        }
        PolicyRead::Absent => {
            ::tracing::debug!(
                origin = %access.host,
                "the origin carries no resource policy, so it has granted this distribution \
                 nothing and the origin request is not signed for it"
            );
            false
        }
        PolicyRead::Unreadable => true,
    }
}

/// Whether `document` grants [`SERVICE_PRINCIPAL`], acting for the distribution
/// `access.source_arn` names, the action `object.action` on `object.resource`.
///
/// A service principal holds no identity-based policies, so the ORIGIN's document is the only
/// one in force over this fetch and it alone decides it: in IAM's own order an explicit `Deny`
/// about the fetch refuses it whatever a grant says, and otherwise the fetch needs an `Allow`
/// about it — a document whose statements are about something else has granted nothing, which
/// is IAM's implicit deny.
fn admits(document: &str, access: &OriginAccess, object: &S3Object) -> bool {
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let account = ::service_common::arn::parse(&access.source_arn)
        .map(|arn| arn.account.to_string())
        .unwrap_or_default();
    let context = [
        (
            SOURCE_ARN_CONDITION_KEY.to_string(),
            access.source_arn.clone(),
        ),
        (SOURCE_ACCOUNT_CONDITION_KEY.to_string(), account),
    ];
    let request =
        ::service_common::iam_authorization::Request::new(object.action, &object.resource, &principals)
            .with_context(&context)
            .with_published_keys(&ORIGIN_REQUEST_CONDITION_KEYS);
    ::service_common::iam_authorization::resource_decision(document, &request)
        == ::service_common::iam_authorization::Decision::Allowed
}

/// What the origin answered when its own resource policy was read.
#[derive(Debug)]
enum PolicyRead {
    /// The document the origin carries.
    Document(String),
    /// The origin answered that it carries no policy at all, which is a grant of nothing.
    Absent,
    /// Nothing is known: the origin did not answer the read, or answered something that is
    /// no policy and no absence.
    Unreadable,
}

/// The zones AWS serves its own endpoints in, and so the only zones an origin whose resource
/// policy this module READS can be under: every emulated service's endpoint host is in one of
/// them, and an origin host outside them names no endpoint of the service the control's
/// `OriginAccessControlOriginType` says this origin belongs to.
///
/// `Origin$DomainName` is caller-supplied, so this is what keeps the policy read — the one
/// request this module composes itself, rather than relaying — to the hosts this emulator
/// serves. The value tested is already a well-formed DNS name ([`dns_host`], which the access
/// is built through), and each zone below begins at a label boundary, so the labels this
/// matches are the trailing labels of the authority the request is actually addressed to.
/// (It is dispatched over the in-process transport `OperationContext::callback` hands a
/// handler, which opens no socket and reaches nothing but this emulator; the zone test is the
/// second half, so the request is not even composed for a host that names no AWS endpoint.)
const AWS_ENDPOINT_ZONES: [&str; 4] =
    [".amazonaws.com", ".amazonaws.com.cn", ".api.aws", ".on.aws"];

/// The most bytes of an origin's resource policy this module reads.
///
/// An EMULATOR ceiling: the document is answered by another service of this emulator over the
/// in-process transport, and a document longer than this is not read into an authorization
/// decision at all. AWS's own bucket-policy limit is 20 KB
/// ("Bucket policies are limited to 20 KB in size", Amazon S3 User Guide), so a document this
/// refuses is one no origin could be carrying.
const MAX_POLICY_DOCUMENT_BYTES: usize = 64 * 1024;

/// Whether `host` is a name under one of [`AWS_ENDPOINT_ZONES`].
///
/// `host` is a DNS name ([`dns_host`]) and each zone begins with its separating dot, so the
/// test is on a label boundary: a name merely ENDING in those characters inside a longer label
/// (`not-amazonaws.com`) is not under the zone and is not read.
fn under_an_aws_endpoint_zone(host: &str) -> bool {
    dns_host(host).is_some_and(|host| {
        AWS_ENDPOINT_ZONES
            .iter()
            .any(|zone| host.ends_with(zone) && host.len() > zone.len())
    })
}

/// Read the policy of the bucket an `s3` origin names, through that bucket's own
/// `GetBucketPolicy` (`GET /{Bucket}?policy`, answered with the document as the payload).
///
/// Addressed virtual-hosted-style at the origin's OWN host, so the document read is the one
/// held by the bucket the distribution's configuration names, and signed the way the fetch
/// itself is — the read is part of serving this viewer request, so it is bounded by the same
/// origin read timeout.
///
/// Two bounds on a read composed from a caller-supplied `DomainName`: the host — a DNS name
/// by the time an access carries it ([`dns_host`]) — must be under one of the zones AWS
/// serves its endpoints in ([`AWS_ENDPOINT_ZONES`]), and a document longer than
/// [`MAX_POLICY_DOCUMENT_BYTES`] is not read. Either way nothing is known about what the
/// origin grants, which leaves the control's own instruction standing.
async fn bucket_policy(
    ctx: &OperationContext<'_>,
    access: &OriginAccess,
    read_timeout: Duration,
) -> PolicyRead {
    if !under_an_aws_endpoint_zone(&access.host) {
        ::tracing::debug!(
            origin = %access.host,
            "this origin's host names no AWS endpoint, so no resource policy of it is read"
        );
        return PolicyRead::Unreadable;
    }
    let uri = format!("https://{}/?{BUCKET_POLICY_QUERY}", access.host);
    let built = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", &access.host)
        .body(Vec::new());
    let Ok(mut request) = built else {
        return PolicyRead::Unreadable;
    };
    if sign(&mut request, access, ctx).await.is_err() {
        return PolicyRead::Unreadable;
    }
    let Ok((status, body)) = call(ctx, request, read_timeout).await else {
        return PolicyRead::Unreadable;
    };
    if body.len() > MAX_POLICY_DOCUMENT_BYTES {
        ::tracing::debug!(
            origin = %access.host,
            bytes = body.len(),
            "the origin answered more than this emulator reads of a resource policy, so \
             nothing it states is known for this origin request"
        );
        return PolicyRead::Unreadable;
    }
    let text = String::from_utf8_lossy(&body).to_string();
    if status == 200 {
        return PolicyRead::Document(text);
    }
    if text.contains(NO_SUCH_BUCKET_POLICY) {
        return PolicyRead::Absent;
    }
    ::tracing::debug!(
        origin = %access.host,
        status,
        "the origin did not answer a read of its own resource policy, so nothing it states \
         is known for this origin request"
    );
    PolicyRead::Unreadable
}

/// Dispatch one request through the in-process transport `OperationContext::callback` hands
/// this handler, bounded by `read_timeout`: the status and the bytes it answered.
async fn call(
    ctx: &OperationContext<'_>,
    request: ::http::Request<Vec<u8>>,
    read_timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(parts, SdkBody::from(::bytes::Bytes::from(body)));
    let request = RtRequest::try_from(request).map_err(|error| error.to_string())?;
    let transport = ctx.callback().http_connector();
    let answered = ::tokio::time::timeout(read_timeout, transport.call(request))
        .await
        .map_err(|elapsed| elapsed.to_string())?
        .map_err(|error| error.to_string())?;
    let status = answered.status().as_u16();
    let body = answered
        .into_body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    Ok((status, body))
}

/// How long past the fetch's own deadline an in-flight record stays readable.
///
/// What ENDS a record is the guard being dropped, which happens on every path out of the
/// fetch; the deadline is the floor that bounds a record nothing dropped (see
/// [`::service_common::service_principal_calls`]). It sits beyond the fetch's deadline so a
/// record can never expire while the ORIGIN's own handler is still deciding the call — a read
/// that found no record would be decided against the credential instead, which is the one
/// thing this design must not let happen.
const IN_FLIGHT_RECORD_SLACK: Duration = Duration::from_secs(60);

/// Sign one origin request AS [`SERVICE_PRINCIPAL`], acting for the distribution `access` was
/// built for, and record that call for as long as it is in flight.
///
/// The CREDENTIAL is what says whose call this is. This mints one in the emulator's own
/// credential store for this one fetch, signs the fetch with it, and records under its access
/// key id the principal the call is made as and the resource it is made for
/// ([`::service_common::service_principal_calls`]). The service SERVING the fetch — the KMS
/// key policy of the key an origin object's bytes are encrypted under is the decision that
/// needs this — resolves the record from the access key id the signature was verified
/// against, and so decides the fetch for the CloudFront service principal and this
/// distribution's ARN, exactly as the bucket-policy half is decided here ([`admits`]).
///
/// The credential and the record live only as long as the call: the returned value deletes
/// both when it is dropped, which is every path out of the fetch, and the record carries the
/// fetch's own read timeout as its deadline so neither outlives the request it describes.
///
/// A fetch that cannot be made AS that principal is not made as the ACCOUNT instead: on an
/// engine carrying no such directory, or when the directory already holds as many in-flight
/// calls as it records, the request is left UNSIGNED (`Ok(None)`), so the origin answers it as
/// it answers a caller presenting no credential — the same travel a fetch the origin's own
/// document does not grant makes. Signing it with the credential every in-process call
/// carries would decide the origin's own checks for the ACCOUNT, which a default key policy
/// permits and AWS would refuse. `Err` carries what went wrong as text, with the request left
/// unsigned for the caller to report.
pub async fn sign_as_the_service_principal<'e, Eng: Send + Sync + 'static>(
    engine: &'e Eng,
    request: &mut ::http::Request<Vec<u8>>,
    access: &OriginAccess,
    ctx: &'e OperationContext<'_>,
    read_timeout: Duration,
) -> Result<Option<InFlightOriginRequest<'e, Eng>>, String> {
    let Some(directory) = in_flight_calls(engine) else {
        // No directory to record the call in, so there is no way for the origin's service to
        // decide it as the CloudFront principal: the request travels UNSIGNED rather than as
        // the account, and the origin answers it as it answers a credential-less caller.
        ::tracing::warn!(
            origin = %access.host,
            "this emulator holds no directory of service-principal calls, so this origin \
             request is made carrying no credential"
        );
        return Ok(None);
    };
    let minted = ctx
        .credential_store()
        .create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let Some(secret) = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string)
    else {
        let _ = ctx.credential_store().delete(&minted.id);
        return Err("the credential store minted no secret for this origin request".to_string());
    };
    let now = ::std::time::SystemTime::now();
    let Some(source_account) = source_account(access) else {
        // The distribution's own ARN names no account, so no record could carry the
        // `aws:SourceAccount` a reader matches on: the request is made carrying no credential
        // rather than as the account, the same narrow answer the cap path gives.
        ::tracing::warn!(
            origin = %access.host,
            distribution = %access.source_arn,
            "this distribution's ARN names no account, so this origin request cannot be made \
             as the CloudFront service principal and is made carrying no credential"
        );
        let _ = ctx.credential_store().delete(&minted.id);
        return Ok(None);
    };
    let recorded = directory.lock().remember(
        minted.id.clone(),
        ::service_common::service_principal_calls::ServicePrincipalCall {
            principal: SERVICE_PRINCIPAL.to_string(),
            source_arn: access.source_arn.clone(),
            source_account,
            expires_at: now + read_timeout + IN_FLIGHT_RECORD_SLACK,
        },
        now,
    );
    if !recorded {
        // The same narrow answer: an unrecorded call is made carrying no credential, never as
        // the account.
        ::tracing::warn!(
            origin = %access.host,
            "this emulator is already making as many service-principal calls as it records \
             at once, so this origin request is made carrying no credential"
        );
        let _ = ctx.credential_store().delete(&minted.id);
        return Ok(None);
    }
    let in_flight = InFlightOriginRequest {
        engine,
        credentials: ctx.credential_store(),
        access_key_id: minted.id.clone(),
    };
    ::service_common::in_process_sigv4::sign_in_process_request_as(
        request,
        access.signing_name,
        &access.region,
        &minted.id,
        &secret,
    )
    .await?;
    Ok(Some(in_flight))
}

/// The account the distribution an origin request is made for belongs to, read out of its own
/// ARN through the tree's one ARN parse: the value of `aws:SourceAccount`, and what scopes the
/// record to this account.
///
/// `None` for an ARN that names no account, which no record can be written for: a record
/// carrying no account matches no reader, and a call that cannot be recorded is made carrying
/// no credential rather than as the account.
fn source_account(access: &OriginAccess) -> Option<String> {
    ::service_common::arn::parse(&access.source_arn)
        .map(|arn| arn.account.to_string())
        .filter(|account| !account.is_empty())
}

/// The shared directory of in-flight service-principal calls, in the one shared engine:
/// written here and read by the service serving the origin request.
fn in_flight_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}

/// One origin request's own credential and in-flight record, held for as long as that request
/// is being made.
///
/// Dropping it forgets the record and DELETES the credential, so the identity this call was
/// made under exists only while the edge is waiting for it: a credential nothing holds can
/// sign nothing, and a record nothing holds decides nothing.
pub struct InFlightOriginRequest<'e, Eng: Send + Sync + 'static> {
    engine: &'e Eng,
    credentials: &'e ::emulator_server::CredentialStore,
    access_key_id: String,
}

/// Prints the access key id of the credential, never a secret: the store never hands one back
/// after creating it, so there is none here to print.
impl<Eng: Send + Sync + 'static> ::std::fmt::Debug for InFlightOriginRequest<'_, Eng> {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("InFlightOriginRequest")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

impl<Eng: Send + Sync + 'static> Drop for InFlightOriginRequest<'_, Eng> {
    fn drop(&mut self) {
        if let Some(directory) = in_flight_calls(self.engine) {
            directory.lock().forget(&self.access_key_id);
        }
        let _ = self.credentials.delete(&self.access_key_id);
    }
}

/// SigV4-sign one origin request in place, for the service and Region the origin addresses.
///
/// The identity is the emulator's own credential — the one `OperationContext::callback` hands
/// every handler — because that is the only identity an in-process request here can be signed
/// as; [`grants_origin_request`] is what keeps the signature to a fetch the origin's own
/// document grants the CloudFront principal. The signature itself is the tree's one
/// in-process signer (`service_common::in_process_sigv4`), so it is the signature the
/// emulator's own verifier computes over the same request — which is what lets an origin
/// fetch be served with request authentication on.
///
/// `SigningProtocol` is not read here: `sigv4` and `sigv4a` both say the request is signed,
/// and the emulator verifies one credential-scope form, so a control naming either is signed
/// in the form its origin can verify.
pub async fn sign(
    request: &mut ::http::Request<Vec<u8>>,
    access: &OriginAccess,
    ctx: &OperationContext<'_>,
) -> Result<(), String> {
    ::service_common::in_process_sigv4::sign_in_process_request(
        request,
        access.signing_name,
        &access.region,
        ctx,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distribution ARN every row below is made on behalf of.
    const DISTRIBUTION_ARN: &str = "arn:aws:cloudfront::000000000000:distribution/E1EDGEEXAMPLE";

    /// One origin access this module's own rules are read over.
    fn access(host: &str, action: &'static str, key: &str) -> OriginAccess {
        OriginAccess {
            signing_name: "s3",
            region: region_of_host(host, "s3"),
            host: host.to_string(),
            source_arn: DISTRIBUTION_ARN.to_string(),
            s3_object: Some(S3Object {
                action,
                resource: format!(
                    "arn:aws:s3:::{}/{key}",
                    s3_bucket_of_host(host).unwrap_or_default()
                ),
            }),
        }
    }

    /// A bucket policy granting `action` on `resource` to a `Service` principal, optionally
    /// scoped by a condition clause.
    fn policy(principal: &str, action: &str, resource: &str, condition: Option<&str>) -> String {
        let condition = condition
            .map(|clause| format!(",\"Condition\":{clause}"))
            .unwrap_or_default();
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"{action}\",\
             \"Resource\":\"{resource}\"{condition}}}]}}"
        )
    }

    /// `SigningBehavior` decides which requests are signed; the rows are the enum's own
    /// values (`smithy_query(query="shape",shape="OriginAccessControlSigningBehaviors")`),
    /// each with and without a viewer credential.
    #[test]
    fn the_signing_behavior_decides_which_requests_carry_a_signature() {
        let rows: [(&str, bool, bool); 8] = [
            ("always", false, true),
            ("always", true, true),
            ("always-amz-auth", false, true),
            ("always-amz-auth", true, true),
            ("no-override", false, true),
            ("no-override", true, false),
            ("never", false, false),
            ("never", true, false),
        ];
        for (behavior, viewer_credential, expected) in rows {
            let signed = signs(behavior, viewer_credential);
            assert_eq!(
                signed, expected,
                "case {behavior:?} with viewer credential {viewer_credential}: expected \
                 signed={expected}, got {signed}"
            );
        }
    }

    /// The grant is scoped to the distribution it names: the rows differ along the principal
    /// the statement names, the action, the resource pattern and the `aws:SourceArn` clause.
    #[test]
    fn only_a_grant_to_this_distribution_leaves_the_fetch_signed() {
        let host = "private-origin.s3.eu-west-1.amazonaws.com";
        let subject = access(host, "s3:GetObject", "reports/held.csv");
        let another = "arn:aws:cloudfront::000000000000:distribution/E2OTHEREXAMPLE";
        let rows: [(&str, String, bool); 8] = [
            (
                "a grant scoped to this distribution",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{DISTRIBUTION_ARN}\"}}}}"
                    )),
                ),
                true,
            ),
            (
                "a grant scoped to another distribution",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"aws:SourceArn\":\"{another}\"}}}}"
                    )),
                ),
                false,
            ),
            (
                "a grant scoped by an ArnLike clause this distribution matches",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/*",
                    Some("{\"ArnLike\":{\"aws:SourceArn\":\"arn:aws:cloudfront::*:distribution/*\"}}"),
                ),
                true,
            ),
            (
                "an unscoped grant to the CloudFront principal",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/*",
                    None,
                ),
                true,
            ),
            (
                "a grant to another service principal",
                policy(
                    "lambda.amazonaws.com",
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/*",
                    None,
                ),
                false,
            ),
            (
                "a grant of another action",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:PutObject",
                    "arn:aws:s3:::private-origin/*",
                    None,
                ),
                false,
            ),
            (
                "a grant over another prefix of the same bucket",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/public/*",
                    None,
                ),
                false,
            ),
            (
                "a grant over this object exactly",
                policy(
                    SERVICE_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::private-origin/reports/held.csv",
                    None,
                ),
                true,
            ),
        ];
        for (case, document, expected) in rows {
            let object = subject
                .s3_object
                .as_ref()
                .expect("the subject origin names an object");
            let granted = admits(&document, &subject, object);
            assert_eq!(
                granted, expected,
                "case {case:?}: expected granted={expected}, got {granted}"
            );
        }
    }

    /// The bucket and the Region come from the origin's own host: the rows span the endpoint
    /// forms an S3 origin is addressed at, and a host that is no S3 endpoint at all.
    #[test]
    fn an_s3_origin_host_names_its_bucket_and_its_region() {
        let rows: [(&str, Option<&str>, &str); 11] = [
            (
                "reports.s3.us-west-2.amazonaws.com",
                Some("reports"),
                "us-west-2",
            ),
            ("reports.s3.amazonaws.com", Some("reports"), DEFAULT_REGION),
            (
                "reports.s3-eu-central-1.amazonaws.com",
                Some("reports"),
                "eu-central-1",
            ),
            (
                "reports.s3-website-ap-south-1.amazonaws.com",
                Some("reports"),
                "ap-south-1",
            ),
            (
                "reports.s3.cn-north-1.amazonaws.com.cn",
                Some("reports"),
                "cn-north-1",
            ),
            (
                "my.reports.s3.us-east-1.amazonaws.com",
                Some("my.reports"),
                "us-east-1",
            ),
            ("edge.example.com", None, DEFAULT_REGION),
            (
                "UPPERCASE.s3.us-east-2.amazonaws.com",
                None,
                "us-east-2",
            ),
            // A bucket whose own name is spelled like the S3 endpoint: the endpoint label is
            // anchored from the right, so this is the bucket it is.
            (
                "s3-backups.s3.us-east-1.amazonaws.com",
                Some("s3-backups"),
                "us-east-1",
            ),
            (
                "s3-website.s3-website-us-east-1.amazonaws.com",
                Some("s3-website"),
                "us-east-1",
            ),
            // A PATH-style endpoint host names no bucket at all.
            ("s3.eu-west-2.amazonaws.com", None, "eu-west-2"),
        ];
        for (host, bucket, region) in rows {
            let read = s3_bucket_of_host(host);
            assert_eq!(
                read.as_deref(),
                bucket,
                "case {host:?}: expected bucket {bucket:?}, got {read:?}"
            );
            let read = region_of_host(host, "s3");
            assert_eq!(
                read, region,
                "case {host:?}: expected region {region:?}, got {read:?}"
            );
        }
    }

    /// The action a fetch is judged under follows the viewer's method
    /// ([`S3_OBJECT_ACTIONS`]), and the object ARN is the bucket's with the viewer path as
    /// the key.
    #[test]
    fn the_viewer_method_names_the_action_the_origin_policy_is_read_for() {
        let host = "reports.s3.us-east-1.amazonaws.com";
        let rows: [(&str, &str, Option<&str>); 7] = [
            ("GET", "/reports/held.csv", Some("s3:GetObject")),
            ("head", "/reports/held.csv", Some("s3:GetObject")),
            ("OPTIONS", "/reports/held.csv", Some("s3:GetObject")),
            ("PUT", "/reports/held.csv", Some("s3:PutObject")),
            ("POST", "/reports/held.csv", Some("s3:PutObject")),
            ("DELETE", "/reports/held.csv", Some("s3:DeleteObject")),
            ("PATCH", "/reports/held.csv", None),
        ];
        for (method, path, action) in rows {
            let read = s3_object(host, method, path);
            assert_eq!(
                read.as_ref().map(|object| object.action),
                action,
                "case {method:?}: expected action {action:?}, got {:?}",
                read.as_ref().map(|object| object.action)
            );
            if let Some(object) = read {
                assert_eq!(
                    object.resource, "arn:aws:s3:::reports/reports/held.csv",
                    "case {method:?}: expected the object's own ARN, got {:?}",
                    object.resource
                );
            }
        }
    }

    /// An `Origin$DomainName` is read as a HOSTNAME or as nothing, and only a name under one
    /// of the zones AWS serves its endpoints in has a resource policy read from it.
    ///
    /// The rows span what the reading has to separate: the endpoint hosts an S3 origin takes
    /// in each AWS zone, a customer domain, and every shape a caller-supplied value takes that
    /// is not a hostname at all — a path or query separator, userinfo, a backslash, a space,
    /// an empty label, an over-long label — including the ones that would end in an AWS zone
    /// while addressing something else, and a name whose last labels merely spell a zone
    /// inside a longer label.
    #[test]
    fn a_domain_name_is_read_as_a_host_and_only_an_aws_endpoint_host_is_read_for_a_policy() {
        let longest_label = "a".repeat(63);
        let over_long_label = "a".repeat(64);
        let rows: [(&str, bool, bool); 16] = [
            ("reports.s3.us-east-1.amazonaws.com", true, true),
            ("reports.s3.amazonaws.com", true, true),
            ("reports.s3-website-us-east-1.amazonaws.com", true, true),
            ("reports.s3.cn-north-1.amazonaws.com.cn", true, true),
            ("abc123.lambda-url.us-east-1.on.aws", true, true),
            ("reports.s3.us-east-1.api.aws", true, true),
            (&format!("{longest_label}.s3.us-east-1.amazonaws.com"), true, true),
            ("content.example.com", true, false),
            ("not-amazonaws.com", true, false),
            ("elsewhere.example.com/.amazonaws.com", false, false),
            ("elsewhere.example.com?.amazonaws.com", false, false),
            ("user@elsewhere.example.com", false, false),
            ("elsewhere.example.com\\.amazonaws.com", false, false),
            ("a space.amazonaws.com", false, false),
            ("reports..s3.us-east-1.amazonaws.com", false, false),
            (&format!("{over_long_label}.s3.amazonaws.com"), false, false),
        ];
        for (domain_name, a_host, read_for_a_policy) in rows {
            let read = dns_host(&host_of(domain_name));
            assert_eq!(
                read.is_some(),
                a_host,
                "case {domain_name:?}: expected a host {a_host}, got {read:?}"
            );
            let under_a_zone = under_an_aws_endpoint_zone(&host_of(domain_name));
            assert_eq!(
                under_a_zone, read_for_a_policy,
                "case {domain_name:?}: expected a policy read {read_for_a_policy}, got \
                 {under_a_zone}"
            );
        }
    }
}
