// SPDX-License-Identifier: CC0-1.0

//! How a request's REGION is read from the endpoint it arrived at: a DynamoDB table
//! name is unique within one region, so the region is part of every lookup key.

/// The region a request that names no region at all is served: the region a generated
/// emulator defaults to, which is also the region `::engine::emulator_arn` mints an ARN
/// for when nothing says otherwise.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most bytes a region name this module reads out of a request may carry — in a
/// credential scope or in a host label. A region code is far shorter ("us-gov-east-1" is
/// 13); this keeps the scan over a caller-supplied `Host` bounded by a constant, and
/// bounds the length of a region that becomes a store key and an ARN segment.
pub(super) const MAX_REGION_LABEL_BYTES: usize = 64;

/// The region this request addresses: the region its tables are stored in and read
/// from, and the region the ARNs it mints carry.
///
/// A request names its endpoint, and a REGION and a service name are two fields of that
/// one endpoint, so the region is resolved in the SAME tiered order the framework
/// resolves the service from (`::emulator_server::resolve_route_key`, which is what
/// routed this request here): the SigV4 credential scope in the `Authorization` header,
/// then the credential scope a PRESIGNED request carries in its `X-Amz-Credential`
/// query parameter, then the region label of the `Host`/`:authority` endpoint it was
/// sent to. Whichever of those a request carries, it is addressing one regional
/// endpoint, and the store it reaches has to be that region's.
///
/// Reading only the first tier is what made a table invisible to a later reader: a
/// request that named its region in the presigned scope or in the endpoint host was
/// served [`DEFAULT_REGION`]'s store instead of the one it addressed, so a `CreateTable`
/// that arrived through one tier and a `DescribeTable` that arrived through another
/// disagreed about which region's tables they were talking about.
///
/// A request that names no region anywhere, or whose credential scope names something
/// that is not a region NAME ([`is_region_shaped`]), is served [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    signed_region(&parts.headers)
        .or_else(|| presigned_region(&parts.uri))
        .or_else(|| endpoint_region(parts))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region of the SigV4 credential scope in the `Authorization` header — the signal
/// every signed AWS SDK request carries, and the first tier
/// [`::emulator_server::resolve_route_key`] reads.
pub(super) fn signed_region(headers: &::http::HeaderMap) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
}

/// The region of the SigV4 credential scope a PRESIGNED request carries in its
/// `X-Amz-Credential` query parameter, where no `Authorization` header holds one.
///
/// The value is the same `<access key id>/<date>/<region>/<service>/aws4_request` scope,
/// percent-encoded (the `/` separators arrive as `%2F`), read here exactly as
/// `::emulator_server::service_from_sigv4_query` reads the SERVICE name out of it — the
/// 3rd field rather than the 4th.
pub(super) fn presigned_region(uri: &::http::Uri) -> Option<String> {
    let credential = uri.query()?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "X-Amz-Credential").then(|| value.to_string())
    })?;
    let decoded = credential.replace("%2F", "/").replace("%2f", "/");
    let mut fields = decoded.split('/');
    let (Some(_access_key_id), Some(_date), Some(region), Some(_service), Some("aws4_request")) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    is_region_shaped(region).then(|| region.to_string())
}

/// Whether a region a CREDENTIAL SCOPE states may be used as it stands.
///
/// That field IS the region whatever region exists by that name, so it is not held to
/// the three-group shape a host label is ([`is_region_code`]) — a credential scope may
/// legitimately name a region this emulator has never heard of. It IS held to a region
/// name's CHARACTER SET and a bounded length, because the value becomes a store key and
/// a segment of every ARN this service mints: without that, a caller's own credential
/// scope could put an ARN delimiter (`:` or `/`) into an ARN, or an unbounded string into
/// a map key. A value outside it names no region, so the caller falls back to
/// [`DEFAULT_REGION`].
pub(super) fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_LABEL_BYTES
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region of the endpoint host the request was sent to:
/// `dynamodb.<region>.amazonaws.com`, `dynamodb-fips.<region>.amazonaws.com` and
/// `dynamodb.<region>.api.aws` all name their region in a LABEL of the host, which is
/// the last tier [`::emulator_server::resolve_route_key`] reads a request's endpoint
/// from (there as the FIRST label, which names the service).
///
/// The service label is therefore skipped and the first following label that is shaped
/// like a region code ([`is_region_code`]) is the region. A host that names none — a
/// loopback address, or the legacy global `dynamodb.amazonaws.com` — yields `None`, and
/// the caller falls back to [`DEFAULT_REGION`], which is the region that global endpoint
/// resolves to anyway.
pub(super) fn endpoint_region(parts: &::http::request::Parts) -> Option<String> {
    // The `Host` label first and the URI authority second, which is the order the
    // framework reads a host-style endpoint in (`service_from_host` then
    // `service_from_authority`), so a region-less `Host` beside a regional `:authority`
    // still names its region.
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        parts.uri.host(),
    ]
    .into_iter()
    .flatten()
    .find_map(region_of_host)
}

/// The region label of one endpoint host, with any `user@` and `:port` removed: the
/// first label after the service's own that is shaped like a region code.
pub(super) fn region_of_host(host: &str) -> Option<String> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let hostname = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    hostname
        .split('.')
        .skip(1)
        .find(|label| is_region_code(label))
        .map(ToString::to_string)
}

/// Whether `label` is shaped like an AWS region code: lowercase hyphen-joined words
/// ending in a number, at least three groups long — `us-east-1`, `ap-southeast-4`,
/// `us-gov-west-1`.
///
/// This is the test that tells a region label apart from the other labels of an endpoint
/// host (`amazonaws`, `com`, `api`, `aws`); the region a credential scope states is held
/// to the looser [`is_region_shaped`] instead, because that field IS the region and may
/// name a region this emulator has never heard of.
pub(super) fn is_region_code(label: &str) -> bool {
    if label.len() > MAX_REGION_LABEL_BYTES {
        return false;
    }
    // The last hyphen group is the region's number and every group before it is a word,
    // walked without collecting the caller-supplied label into a Vec.
    let Some((words, number)) = label.rsplit_once('-') else {
        return false;
    };
    let word_count = words.split('-').count();
    word_count >= 2
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && words
            .split('-')
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}
