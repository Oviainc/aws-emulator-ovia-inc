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
    let host = host_of(&origin.domain_name);
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

/// Read the policy of the bucket an `s3` origin names, through that bucket's own
/// `GetBucketPolicy` (`GET /{Bucket}?policy`, answered with the document as the payload).
///
/// Addressed virtual-hosted-style at the origin's OWN host, so the document read is the one
/// held by the bucket the distribution's configuration names, and signed the way the fetch
/// itself is — the read is part of serving this viewer request, so it is bounded by the same
/// origin read timeout.
async fn bucket_policy(
    ctx: &OperationContext<'_>,
    access: &OriginAccess,
    read_timeout: Duration,
) -> PolicyRead {
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
}
