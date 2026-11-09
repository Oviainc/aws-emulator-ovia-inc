// SPDX-License-Identifier: CC0-1.0

//! The two DATA planes a MediaPackage channel publishes URLs for: the HLS INGEST endpoints
//! an encoder pushes to, and the PLAYBACK path an origin endpoint is watched at.
//!
//! The control plane hands both out — `CreateChannel` returns
//! `HlsIngest.IngestEndpoints[*].{Id,Url,Username,Password}` and `CreateOriginEndpoint`
//! returns the endpoint's `Url` — and a URL is a promise that it can be FOLLOWED. Neither
//! wire protocol is modelled in Smithy (the model describes the control plane only), so
//! what each listener answers is stated here, once, beside the store both planes read:
//! `crate::service` only turns an outcome below into an HTTP response.
//!
//! ## Addressing
//!
//! Both planes are addressed by the PATH of the URL the control plane published
//! ([`ingest_path`], [`super::playback_url_prefix`]), because that path is what a client
//! on this machine can carry: the published HOST names no service the emulator's front
//! door resolves, so a client reaching the emulator's own endpoint keeps the path and
//! nothing else. Each path carries the opaque label of the resource it belongs to, which
//! is derived from that resource's own identity ([`super::stable_label`]) — so a request is
//! claimed only when the label names a channel or an endpoint THIS store holds, and the
//! label resolves the Region too.
//!
//! ## The ingest plane
//!
//! An ingest endpoint is a WebDAV collection authenticated with HTTP Digest (RFC 2617)
//! over the credentials the control plane generated FOR THAT ENDPOINT:
//!
//! * a request presenting no credentials is challenged — 401 with
//!   `WWW-Authenticate: Digest realm="WebDAV", nonce=…, algorithm=MD5, qop="auth"`;
//! * a request presenting any OTHER authentication scheme is refused, bare, with 403 and
//!   no challenge — so a correct and an incorrect credential of the wrong scheme are
//!   indistinguishable, which is what the recorded listener answers;
//! * a Digest response that does not verify against that endpoint's stored `Username` and
//!   `Password` is challenged again (401);
//! * a verified request reaches the collection, where a `PUT` of a named object stores
//!   those bytes as the channel's ingested media and a `GET` reads them back.
//!
//! The media is the CHANNEL's: the two ingest endpoints of a channel are a redundant pair
//! an encoder pushes the same source to, so a push through either one feeds the one
//! channel, and what the credentials decide is only whether this push is accepted. What a
//! channel retains is a live window ([`MAX_INGESTED_OBJECTS`] named objects, oldest push
//! evicted first), which is all a live origin holds.
//!
//! ## The playback plane
//!
//! A playback request is the endpoint's own ACCESS CONTROL first and content second:
//!
//! * `Origination` is "control whether origination of video is permitted for this
//!   OriginEndpoint … can be helpful for temporarily disabling origination", so an endpoint
//!   stored `DENY` refuses playback with 403 while it is denied, and serves again when it is
//!   `ALLOW`;
//! * `Whitelist` is "a list of source IP CIDR blocks that will be allowed to access the
//!   OriginEndpoint", so a caller whose source address lies in none of them is refused with
//!   403, and an empty list restricts nobody.
//!
//! What is SERVED is the content the endpoint's channel ingested, and nothing else. This
//! origin does not transcode or repackage — no process here turns a pushed rendition into
//! another format — so the endpoint's manifest path answers with the manifest its channel
//! last ingested for that format and a segment path with the bytes pushed under that name;
//! a path the channel holds nothing for is 404. Synthesising a manifest that named
//! segments the channel never received would describe content that is not there, which is
//! the same reason a harvest's manifest names none ([`super::harvest_export`]).

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::harvest_export::{
    DASH_MANIFEST_MEDIA_TYPE, HLS_MANIFEST_MEDIA_TYPE, MSS_MANIFEST_MEDIA_TYPE,
};
use super::{
    channel_path_label, cmaf_manifest_path, endpoint_path_label, ChannelRecord,
    ComAmazonawsMediapackageMediapackageState, OriginEndpointRecord, PackagingKind, ResourceKey,
    DEFAULT_MANIFEST_NAME, DEFAULT_ORIGINATION, ORIGINATION_DENY,
};
use crate::ops::types::IngestEndpoint;

// ---------------------------------------------------------------------------
// How a request addresses one of the two planes
// ---------------------------------------------------------------------------

/// The route every ingest URL of every channel sits under.
///
/// The published URL is this route, the channel's own path label, the ingest endpoint's
/// system-generated id and [`INGEST_COLLECTION_SEGMENT`] ([`ingest_path`]); a push of a
/// named object addresses that path plus the object's name.
pub const INGEST_PATH_PREFIX: &str = "/in/v2/";

/// The last segment of an ingest URL: the channel's ingest collection, which the named
/// objects an encoder pushes sit inside.
pub const INGEST_COLLECTION_SEGMENT: &str = "channel";

/// The route every playback URL of every origin endpoint sits under
/// ([`super::playback_url_prefix`]).
pub const PLAYBACK_PATH_PREFIX: &str = "/out/v1/";

/// How many characters an opaque path label carries: the width [`super::stable_label`]
/// renders, which is also the width of a system-generated ingest endpoint id
/// ([`super::INGEST_GENERATED_BYTES`] bytes as hex).
const PATH_LABEL_CHARS: usize = 32;

/// The longest object name this plane keeps for one pushed object. A name is one path
/// segment of a URL an encoder chose, so its length is the caller's; the bound is this
/// emulator's own resource guard.
pub const MAX_INGEST_NAME_CHARS: usize = 128;

/// Whether `value` is one of this service's opaque path tokens: exactly
/// [`PATH_LABEL_CHARS`] lowercase hexadecimal characters.
fn is_path_label(value: &str) -> bool {
    value.len() == PATH_LABEL_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The path of ONE ingest endpoint's published URL, below the host.
///
/// The single statement of that path: the URL `CreateChannel` publishes is this, and
/// [`parse_ingest_path`] reads this, so the address a client is given and the address this
/// plane answers at cannot drift.
pub fn ingest_path(channel_label: &str, ingest_endpoint_id: &str) -> String {
    format!("{INGEST_PATH_PREFIX}{channel_label}/{ingest_endpoint_id}/{INGEST_COLLECTION_SEGMENT}")
}

/// What an ingest-plane request addresses: one channel's ingest collection, through one of
/// its ingest endpoints, and the object inside it when the request names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestTarget {
    /// The channel's own path label ([`channel_path_label`]), which names the channel and
    /// its Region together.
    pub channel_label: String,
    /// The system-generated id of the ingest endpoint the push presented itself at, whose
    /// credentials are the ones that have to verify.
    pub ingest_endpoint_id: String,
    /// The object the request addresses inside the collection — the last segment of the
    /// path below the published URL — or `None` for a request addressed at the collection
    /// itself.
    pub object_name: Option<String>,
}

/// The ingest collection `path` addresses, or `None` when it is not an ingest path at all.
///
/// Only the SHAPE is read here (the two opaque tokens and the collection segment); whether
/// the channel and the endpoint exist is the store's answer
/// ([`ComAmazonawsMediapackageMediapackageState::ingest_channel_key`]).
pub fn parse_ingest_path(path: &str) -> Option<IngestTarget> {
    let mut segments = path.strip_prefix(INGEST_PATH_PREFIX)?.split('/');
    let channel_label = segments.next()?;
    let ingest_endpoint_id = segments.next()?;
    if segments.next()? != INGEST_COLLECTION_SEGMENT {
        return None;
    }
    if !is_path_label(channel_label) || !is_path_label(ingest_endpoint_id) {
        return None;
    }
    // Everything below the collection names the object; an encoder pushes a playlist and
    // its segments by name, so the name is the last segment of whatever path it used.
    let object_name = segments
        .rfind(|segment| !segment.is_empty())
        .map(str::to_string);
    Some(IngestTarget {
        channel_label: channel_label.to_string(),
        ingest_endpoint_id: ingest_endpoint_id.to_string(),
        object_name,
    })
}

/// What a playback-plane request addresses: one origin endpoint, and the path it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackTarget {
    /// The endpoint's own path label ([`endpoint_path_label`]), which names the endpoint,
    /// its channel and its Region together.
    pub endpoint_label: String,
    /// The path below the endpoint's playback prefix: its own manifest, a child manifest of
    /// its CMAF package, or a segment name.
    pub path: String,
}

/// The origin endpoint `path` addresses, or `None` when it is not a playback path at all.
pub fn parse_playback_path(path: &str) -> Option<PlaybackTarget> {
    let rest = path.strip_prefix(PLAYBACK_PATH_PREFIX)?;
    let (endpoint_label, served) = rest.split_once('/').unwrap_or((rest, ""));
    is_path_label(endpoint_label).then(|| PlaybackTarget {
        endpoint_label: endpoint_label.to_string(),
        path: served.to_string(),
    })
}

/// One data-plane request, as both planes read it.
///
/// A plane answers from the request and the store alone, so everything either of them
/// reads off the wire is here: an ingest push needs the method, the credentials, the
/// request target the Digest response was computed over, the body and the clock the
/// challenge's nonce is measured against; a playback request needs the method and the
/// address it came from.
#[derive(Debug, Clone, Copy)]
pub struct PlaneRequest<'r> {
    /// The request method.
    pub method: &'r str,
    /// The request's own path, which the request target a Digest response was computed
    /// over has to address.
    pub path: &'r str,
    /// The `Authorization` header, when the request carries one.
    pub authorization: Option<&'r str>,
    /// The address this request came from, as the whitelist is matched against.
    pub source_address: &'r str,
    /// The body, when this process could read it as bytes.
    pub body: Option<&'r [u8]>,
    /// Now, as the challenge's nonce window is measured against.
    pub now: SystemTime,
}

// ---------------------------------------------------------------------------
// The ingest plane's authentication
// ---------------------------------------------------------------------------

/// The protection space the ingest challenge names, as the recorded listener states it.
pub const DIGEST_REALM: &str = "WebDAV";

/// The authentication scheme the ingest plane accepts: HTTP Digest (RFC 2617).
pub const DIGEST_SCHEME: &str = "Digest";

/// The digest algorithm the challenge advertises and the only one a response may be
/// computed with.
pub const DIGEST_ALGORITHM: &str = "MD5";

/// The quality of protection the challenge advertises: authentication of the request line,
/// which is the one RFC 2617 mode that does not digest the body.
pub const DIGEST_QOP: &str = "auth";

/// How long one challenge's nonce is accepted for.
///
/// AWS documents no window, so this span is the emulator's own: long enough for a client to
/// answer a challenge it was just given, short enough that a response captured from one
/// push cannot be replayed indefinitely. A nonce from the window before the current one is
/// still accepted, so a client that is answering a challenge issued just before a boundary
/// is not refused for it.
pub const NONCE_WINDOW: Duration = Duration::from_secs(300);

/// The nonce the challenge for `ingest_endpoint_id` carries in the window `at` falls in.
///
/// Derived from the endpoint and the window alone — a nonce is advertised in the challenge,
/// so it is public by construction and keeping it is not what makes it good: being bound to
/// ONE endpoint and to a window of the clock is. Nothing is stored for it, so a challenge
/// costs the store nothing and survives nothing being written.
fn nonce_at(ingest_endpoint_id: &str, at: SystemTime) -> String {
    let window = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .checked_div(NONCE_WINDOW.as_secs().max(1))
        .unwrap_or_default();
    super::stable_label(&["ingest-nonce", ingest_endpoint_id, &window.to_string()])
}

/// The `WWW-Authenticate` challenge a 401 of the ingest plane carries: Digest in the
/// WebDAV realm, with the nonce of this window, the algorithm and the quality of protection
/// a response has to be computed with.
pub fn digest_challenge(ingest_endpoint_id: &str, now: SystemTime) -> String {
    let nonce = nonce_at(ingest_endpoint_id, now);
    format!(
        "{DIGEST_SCHEME} realm=\"{DIGEST_REALM}\", nonce=\"{nonce}\", \
         algorithm={DIGEST_ALGORITHM}, qop=\"{DIGEST_QOP}\""
    )
}

/// Whether `nonce` is one the challenge for this endpoint carried in the window `now` falls
/// in, or in the one before it.
fn nonce_is_current(ingest_endpoint_id: &str, nonce: &str, now: SystemTime) -> bool {
    let previous = now.checked_sub(NONCE_WINDOW).unwrap_or(now);
    nonce == nonce_at(ingest_endpoint_id, now) || nonce == nonce_at(ingest_endpoint_id, previous)
}

/// What the credentials a request presented are worth to the ingest plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presented {
    /// No `Authorization` header at all: the request has not been challenged yet.
    Nothing,
    /// Credentials of some other scheme (HTTP Basic is the one encoders try), which this
    /// plane never accepts.
    OtherScheme,
    /// A Digest response that verifies against this endpoint's stored credentials.
    Verified,
    /// A Digest response that does not: a wrong password, a stale nonce, another realm or
    /// algorithm, or a response computed over another request.
    Unverified,
}

/// Read the credentials `request` presents and decide what they are worth for `endpoint`.
///
/// The SCHEME is read first, because it decides the refusal class on its own: this plane
/// accepts Digest and refuses every other scheme outright, whatever credentials that scheme
/// carried.
fn presented(request: &PlaneRequest<'_>, endpoint: &IngestEndpoint) -> Presented {
    let Some(value) = request
        .authorization
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Presented::Nothing;
    };
    let scheme = value
        .split_once(char::is_whitespace)
        .map_or(value, |(scheme, _parameters)| scheme);
    if !scheme.eq_ignore_ascii_case(DIGEST_SCHEME) {
        return Presented::OtherScheme;
    }
    let (Some(username), Some(password)) = (
        endpoint.username.as_deref(),
        endpoint.password.as_deref(),
    ) else {
        return Presented::Unverified;
    };
    if digest_verifies(
        &auth_parameters(value),
        request,
        endpoint.id.as_deref().unwrap_or_default(),
        username,
        password,
    ) {
        Presented::Verified
    } else {
        Presented::Unverified
    }
}

/// The `name=value` parameters of the Digest credentials `authorization` presents, each
/// name lowercased and each value unescaped.
///
/// Read by the maintained reader of RFC 7235's `credentials` grammar rather than by a
/// splitter of this module's own: the parameters are a quoted-string list a caller
/// controls, where a value may itself carry a comma (a `uri` with a query) or an escaped
/// quote. Anything that is not that grammar reads as no parameters at all, and so
/// verifies nothing.
fn auth_parameters(authorization: &str) -> Vec<(String, String)> {
    ::http_auth::parse_challenges(authorization)
        .ok()
        .into_iter()
        .flatten()
        .filter(|presented| presented.scheme.eq_ignore_ascii_case(DIGEST_SCHEME))
        .flat_map(|presented| {
            presented
                .params
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_unescaped()))
        })
        .collect()
}

/// The value of `name` among `parameters`.
fn parameter<'p>(parameters: &'p [(String, String)], name: &str) -> Option<&'p str> {
    parameters
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Whether the Digest response in `parameters` is the one this endpoint's credentials
/// compute for THIS request, as RFC 2617 §3.2.2 defines it.
///
/// Every part of the challenge the client is answering has to come back: the username it
/// claims is the endpoint's own, the realm and the algorithm are the ones advertised, the
/// nonce is one this endpoint's challenge carried in the current window, and the request
/// target the response was computed over addresses this request — so a response computed
/// for another endpoint, another realm, another target or an expired challenge does not
/// verify, whatever password went into it. Both forms RFC 2617 admits are computed: with
/// the quality of protection the challenge advertised, and the original RFC 2069 form for a
/// client that answers without one.
fn digest_verifies(
    parameters: &[(String, String)],
    request: &PlaneRequest<'_>,
    ingest_endpoint_id: &str,
    username: &str,
    password: &str,
) -> bool {
    let (Some(claimed), Some(nonce), Some(target), Some(response)) = (
        parameter(parameters, "username"),
        parameter(parameters, "nonce"),
        parameter(parameters, "uri"),
        parameter(parameters, "response"),
    ) else {
        return false;
    };
    if claimed != username {
        return false;
    }
    if parameter(parameters, "realm").is_none_or(|realm| realm != DIGEST_REALM) {
        return false;
    }
    if parameter(parameters, "algorithm")
        .is_some_and(|algorithm| !algorithm.eq_ignore_ascii_case(DIGEST_ALGORITHM))
    {
        return false;
    }
    if !nonce_is_current(ingest_endpoint_id, nonce, request.now) {
        return false;
    }
    if !addresses_request(target, request.path) {
        return false;
    }
    let secret = md5_hex(&format!("{username}:{DIGEST_REALM}:{password}"));
    let over_request = md5_hex(&format!("{}:{target}", request.method));
    let expected = match parameter(parameters, "qop") {
        None => md5_hex(&format!("{secret}:{nonce}:{over_request}")),
        Some(qop) if qop.eq_ignore_ascii_case(DIGEST_QOP) => {
            let (Some(count), Some(client_nonce)) =
                (parameter(parameters, "nc"), parameter(parameters, "cnonce"))
            else {
                return false;
            };
            md5_hex(&format!(
                "{secret}:{nonce}:{count}:{client_nonce}:{qop}:{over_request}"
            ))
        }
        // `auth-int` digests the body as well, which this challenge does not advertise.
        Some(_) => return false,
    };
    expected.eq_ignore_ascii_case(response)
}

/// Whether the request target a Digest response was computed over addresses THIS request:
/// the request's own path, with or without a query, or an absolute form ending in it — the
/// forms a client writes the `uri` parameter in.
fn addresses_request(target: &str, path: &str) -> bool {
    let target_path = target.split('?').next().unwrap_or(target);
    target_path == path || target_path.ends_with(path)
}

/// `value` digested with MD5, as the hexadecimal RFC 2617 writes every digest in.
fn md5_hex(value: &str) -> String {
    use ::md5::Digest as _;
    hex::encode(::md5::Md5::digest(value.as_bytes()))
}

// ---------------------------------------------------------------------------
// What a channel RETAINS of what was pushed into it
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS quota: how many named objects ONE channel's live
/// ingest window retains. A live origin holds a window rather than a history, so the
/// oldest push is evicted when a new one arrives past this count.
pub const MAX_INGESTED_OBJECTS: usize = 16;

/// Emulator resource guard, NOT an AWS quota: the most bytes ONE pushed object may carry.
pub const MAX_INGEST_OBJECT_BYTES: usize = 1024 * 1024;

/// Emulator resource guard, NOT an AWS quota: the most bytes the ingest windows of EVERY
/// channel together retain in this process, because the content is caller content held in
/// memory and how many channels a caller pushes to is the caller's own choice.
pub const MAX_INGESTED_BYTES: usize = 64 * 1024 * 1024;

/// One object an encoder pushed into a channel: the bytes, under the name it was pushed as.
#[derive(Debug, Clone, Default)]
pub struct IngestedObject {
    /// The name the object was pushed under, which is the name playback serves it by.
    pub name: String,
    /// The bytes of the last push of this name.
    pub content: Vec<u8>,
    /// Which push this was, counting from the channel's first: what orders the live window,
    /// so the oldest push is the one evicted and the newest manifest is the one served.
    pub pushed: u64,
}

/// The live window of media ONE channel has ingested.
///
/// Keyed by name, because that is how an HLS push addresses a playlist and its segments and
/// how a playback request asks for them; a second push of a name replaces its bytes, as an
/// origin's own copy of that name is replaced.
#[derive(Debug, Clone, Default)]
pub struct ChannelIngest {
    objects: BTreeMap<String, IngestedObject>,
    /// How many pushes this channel has accepted, which is where the next object's
    /// [`IngestedObject::pushed`] comes from.
    pushes: u64,
}

impl ChannelIngest {
    /// The object pushed under `name`, when the window still holds it.
    pub fn object(&self, name: &str) -> Option<&IngestedObject> {
        self.objects.get(name)
    }

    /// The most recently pushed object whose name ends in `.{extension}`: the manifest of
    /// that format the channel currently carries.
    pub fn manifest(&self, extension: &str) -> Option<&IngestedObject> {
        let suffix = format!(".{extension}");
        self.objects
            .values()
            .filter(|object| {
                object
                    .name
                    .to_ascii_lowercase()
                    .ends_with(&suffix.to_ascii_lowercase())
            })
            .max_by_key(|object| object.pushed)
    }

    /// How many bytes this window retains.
    pub fn retained_bytes(&self) -> usize {
        self.objects
            .values()
            .map(|object| object.content.len())
            .sum()
    }

    /// How many objects this window retains.
    pub fn retained_objects(&self) -> usize {
        self.objects.len()
    }

    /// Store `content` under `name`, evicting the oldest push when the window is already
    /// full.
    fn store(&mut self, name: &str, content: &[u8]) {
        self.pushes = self.pushes.saturating_add(1);
        self.objects.insert(
            name.to_string(),
            IngestedObject {
                name: name.to_string(),
                content: content.to_vec(),
                pushed: self.pushes,
            },
        );
        while self.objects.len() > MAX_INGESTED_OBJECTS {
            let Some(oldest) = self
                .objects
                .values()
                .min_by_key(|object| object.pushed)
                .map(|object| object.name.clone())
            else {
                return;
            };
            self.objects.remove(&oldest);
        }
    }
}

// ---------------------------------------------------------------------------
// Serving the ingest plane
// ---------------------------------------------------------------------------

/// What one ingest-plane request comes to. `crate::service` renders each of these as the
/// HTTP answer it names and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// The request presented no credentials, or credentials that did not verify: 401,
    /// carrying this challenge.
    Challenge(String),
    /// The request presented another authentication scheme: a bare 403, with no challenge.
    SchemeRefused,
    /// The ingest URL names no endpoint this service holds: 404.
    UnknownEndpoint,
    /// The push was accepted and the bytes are the channel's ingested media now: 200.
    Stored,
    /// The object pushed under this name, for a read of the collection: 200.
    Served {
        content: Vec<u8>,
        media_type: &'static str,
    },
    /// The collection holds no object of that name: 404.
    NoSuchObject,
    /// A method this collection does not serve, or a write addressed at the collection
    /// itself rather than at an object inside it: 405.
    MethodRefused,
    /// The request named no object this collection would keep: 400.
    NameRefused,
    /// The body could not be read as bytes: 400.
    Unreadable,
    /// One object larger than this plane keeps: 413.
    ObjectTooLarge,
    /// The ingest windows of this process are full: 507.
    StoreFull,
}

impl ComAmazonawsMediapackageMediapackageState {
    /// The key of the channel whose path label is `target`'s, when this store holds it and
    /// it carries the ingest endpoint the request presented itself at.
    ///
    /// The label is derived from the channel's id and Region, so resolving it resolves the
    /// Region as well — which is the only thing a data-plane request, carrying no SigV4
    /// credential scope, has to go on.
    pub fn ingest_channel_key(&self, target: &IngestTarget) -> Option<ResourceKey> {
        self.channels.iter().find_map(|(key, channel)| {
            let addressed = channel_path_label(&channel.id, &channel.region) == target.channel_label
                && channel
                    .ingest_endpoint_position(&target.ingest_endpoint_id)
                    .is_some();
            addressed.then(|| key.clone())
        })
    }

    /// The key of the origin endpoint whose path label is `label`, when this store holds it.
    pub fn playback_endpoint_key(&self, label: &str) -> Option<ResourceKey> {
        self.origin_endpoints.iter().find_map(|(key, endpoint)| {
            (endpoint_path_label(&endpoint.channel_id, &endpoint.id, &endpoint.region) == label)
                .then(|| key.clone())
        })
    }
}

/// Serve one ingest-plane request against `store`.
///
/// Authentication comes FIRST and decides on its own: a push presenting the wrong scheme is
/// refused the same way whatever it addressed or asked for, so nothing about the collection
/// — which objects it holds, whether a method would be served — is observable without the
/// endpoint's own credentials.
pub fn serve_ingest(
    store: &mut ComAmazonawsMediapackageMediapackageState,
    target: &IngestTarget,
    request: &PlaneRequest<'_>,
) -> IngestOutcome {
    let Some(key) = store.ingest_channel_key(target) else {
        return IngestOutcome::UnknownEndpoint;
    };
    let retained_elsewhere: usize = store
        .channels
        .iter()
        .filter(|(stored, _)| **stored != key)
        .map(|(_, channel)| channel.ingest.retained_bytes())
        .sum();
    let Some(channel) = store.channels.get_mut(&key) else {
        return IngestOutcome::UnknownEndpoint;
    };
    let Some(endpoint) = channel
        .ingest_endpoint_position(&target.ingest_endpoint_id)
        .and_then(|position| channel.ingest_endpoints.get(position))
    else {
        return IngestOutcome::UnknownEndpoint;
    };
    match presented(request, endpoint) {
        Presented::Nothing | Presented::Unverified => {
            return IngestOutcome::Challenge(digest_challenge(
                &target.ingest_endpoint_id,
                request.now,
            ));
        }
        Presented::OtherScheme => return IngestOutcome::SchemeRefused,
        Presented::Verified => {}
    }
    match request.method {
        "PUT" => store_push(channel, target, request, retained_elsewhere),
        "GET" | "HEAD" => match target
            .object_name
            .as_deref()
            .and_then(|name| channel.ingest.object(name))
        {
            Some(object) => IngestOutcome::Served {
                content: object.content.clone(),
                media_type: media_type_of(&object.name),
            },
            None => IngestOutcome::NoSuchObject,
        },
        _ => IngestOutcome::MethodRefused,
    }
}

/// Keep one accepted push as `channel`'s ingested media.
fn store_push(
    channel: &mut ChannelRecord,
    target: &IngestTarget,
    request: &PlaneRequest<'_>,
    retained_elsewhere: usize,
) -> IngestOutcome {
    // A write addressed at the collection itself names no object to store, which RFC 4918
    // answers for a PUT to a collection URI.
    let Some(name) = target.object_name.as_deref() else {
        return IngestOutcome::MethodRefused;
    };
    if !is_object_name(name) {
        return IngestOutcome::NameRefused;
    }
    // The body is what this plane KEEPS, so a request whose body this process cannot read
    // is refused rather than kept as an empty object: answering a success for a push that
    // stored nothing would tell an encoder its media had landed.
    let Some(body) = request.body else {
        return IngestOutcome::Unreadable;
    };
    if body.len() > MAX_INGEST_OBJECT_BYTES {
        return IngestOutcome::ObjectTooLarge;
    }
    let replaced = channel
        .ingest
        .object(name)
        .map_or(0, |object| object.content.len());
    let retained_here = channel.ingest.retained_bytes().saturating_sub(replaced);
    if retained_elsewhere
        .saturating_add(retained_here)
        .saturating_add(body.len())
        > MAX_INGESTED_BYTES
    {
        return IngestOutcome::StoreFull;
    }
    channel.ingest.store(name, body);
    IngestOutcome::Stored
}

/// Whether `name` is a name this collection keeps an object under: one non-empty path
/// segment of bounded length that names an object rather than a position in a hierarchy.
fn is_object_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_INGEST_NAME_CHARS
        && name != "."
        && name != ".."
        && !name.contains('\0')
}

// ---------------------------------------------------------------------------
// Serving the playback plane
// ---------------------------------------------------------------------------

/// What one playback-plane request comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackOutcome {
    /// The playback path names no endpoint this service holds: 404.
    UnknownEndpoint,
    /// The endpoint's own access control refuses this request — origination is denied, or
    /// the caller's address is outside its whitelist: a bare 403.
    Forbidden,
    /// The content this path names: 200.
    Served {
        content: Vec<u8>,
        media_type: &'static str,
    },
    /// The endpoint serves nothing at that path: 404.
    NoContent,
    /// A method a playback path does not serve: 405.
    MethodRefused,
}

/// Serve one playback-plane request against `store`.
///
/// A read: nothing about the endpoint, its channel or the media changes because somebody
/// watched.
pub fn serve_playback(
    store: &ComAmazonawsMediapackageMediapackageState,
    target: &PlaybackTarget,
    request: &PlaneRequest<'_>,
) -> PlaybackOutcome {
    let Some(key) = store.playback_endpoint_key(&target.endpoint_label) else {
        return PlaybackOutcome::UnknownEndpoint;
    };
    let Some(endpoint) = store.origin_endpoints.get(&key) else {
        return PlaybackOutcome::UnknownEndpoint;
    };
    // The endpoint's access control decides before anything about the content is read, so
    // a refused caller learns nothing about what the endpoint is carrying — and learns it
    // for every method, because origination and the whitelist govern the endpoint rather
    // than one request shape.
    if !permits_origination(endpoint) || !admits_source(&endpoint.whitelist, request.source_address)
    {
        return PlaybackOutcome::Forbidden;
    }
    if request.method != "GET" && request.method != "HEAD" {
        return PlaybackOutcome::MethodRefused;
    }
    let Some(channel) = store.channel(&endpoint.region, &endpoint.channel_id) else {
        return PlaybackOutcome::NoContent;
    };
    match playback_content(endpoint, channel, &target.path) {
        Some((content, media_type)) => PlaybackOutcome::Served {
            content,
            media_type,
        },
        None => PlaybackOutcome::NoContent,
    }
}

/// Whether this endpoint's stored `Origination` permits playback.
///
/// An endpoint stored `DENY` has origination switched off, which is what that member is
/// documented for; every other stored value — and an endpoint carrying none, which takes
/// the member's documented default — permits it.
pub fn permits_origination(endpoint: &OriginEndpointRecord) -> bool {
    endpoint
        .origination
        .as_deref()
        .unwrap_or(DEFAULT_ORIGINATION)
        != ORIGINATION_DENY
}

/// Whether `whitelist` admits a caller at `address`.
///
/// An EMPTY list restricts nobody: the member is "a list of source IP CIDR blocks that will
/// be allowed to access the OriginEndpoint", so an endpoint that lists none has no source
/// restriction at all. Otherwise the address has to lie in one of the blocks, read as
/// NETWORKS rather than as text (the workspace's IP-network parser), so a block admits
/// every address of its range and not only the one spelling of it. An address this emulator
/// cannot read lies in no block, so it is admitted by no non-empty list.
pub fn admits_source(whitelist: &[String], address: &str) -> bool {
    if whitelist.is_empty() {
        return true;
    }
    let Ok(address) = address.trim().parse::<IpAddr>() else {
        return false;
    };
    whitelist.iter().any(|entry| {
        entry
            .trim()
            .parse::<::ipnet::IpNet>()
            .is_ok_and(|block| block.contains(&address))
    })
}

/// The content an endpoint serves at `path`, with the media type it is served as.
///
/// Three paths an endpoint publishes resolve: its own manifest and each child manifest of
/// its CMAF package answer with the manifest its channel last ingested for the endpoint's
/// packaging format, and any other path with the object the channel holds under that name.
/// `None` is a path the channel holds nothing for — including every path of an endpoint
/// whose channel has been pushed nothing yet, which is a live origin with no input.
pub fn playback_content(
    endpoint: &OriginEndpointRecord,
    channel: &ChannelRecord,
    path: &str,
) -> Option<(Vec<u8>, &'static str)> {
    let kind = endpoint.packaging_kind()?;
    let published = published_manifest_paths(endpoint, kind);
    let object = if published.iter().any(|manifest| manifest == path) {
        channel.ingest.manifest(manifest_extension(kind))?
    } else {
        channel.ingest.object(path.rsplit('/').next()?)?
    };
    Some((object.content.clone(), media_type_of(&object.name)))
}

/// Every manifest path this endpoint publishes below its playback prefix: its own, and one
/// per child HLS manifest of its CMAF package.
///
/// Each is derived by the same function that built the URL the control plane published
/// ([`PackagingKind::manifest_path`], [`cmaf_manifest_path`]), so a URL a caller was handed
/// and a path this plane answers at cannot drift.
fn published_manifest_paths(endpoint: &OriginEndpointRecord, kind: PackagingKind) -> Vec<String> {
    let manifest_name = endpoint
        .manifest_name
        .as_deref()
        .unwrap_or(DEFAULT_MANIFEST_NAME);
    let mut paths = vec![kind.manifest_path(manifest_name)];
    for child in endpoint
        .cmaf_package
        .as_ref()
        .and_then(|package| package.hls_manifests.as_ref())
        .into_iter()
        .flatten()
    {
        paths.push(cmaf_manifest_path(
            &endpoint.channel_id,
            &endpoint.id,
            &endpoint.region,
            &child.id,
            child.manifest_name.as_deref().unwrap_or(manifest_name),
        ));
    }
    paths
}

/// The file extension the top-level manifest of a packaging format is written as — the
/// extension of the manifest path that format's playback URL ends in.
fn manifest_extension(kind: PackagingKind) -> &'static str {
    match kind {
        PackagingKind::Hls | PackagingKind::Cmaf => "m3u8",
        PackagingKind::Dash => "mpd",
        PackagingKind::Mss => "ism",
    }
}

/// The media type an object named `name` is served as.
///
/// The streaming types are the ones registered for each format — an HLS playlist, a DASH
/// MPD and a Microsoft Smooth server manifest (the three [`super::harvest_export`] already
/// writes a harvested manifest under), an MPEG-2 transport stream segment, a fragmented
/// MP4 segment and its audio-only sibling, and a WebVTT subtitle track. Anything else is
/// read by the workspace's one extension-to-media-type catalogue, which answers the generic
/// binary type for an extension nothing publishes.
pub fn media_type_of(name: &str) -> &'static str {
    let file_name = name.rsplit('/').next().unwrap_or(name);
    let extension = file_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "m3u8" => HLS_MANIFEST_MEDIA_TYPE,
        "mpd" => DASH_MANIFEST_MEDIA_TYPE,
        "ism" | "isml" => MSS_MANIFEST_MEDIA_TYPE,
        "ts" => "video/mp2t",
        "m4s" | "mp4" | "cmfv" => "video/mp4",
        "m4a" | "cmfa" => "audio/mp4",
        "aac" => "audio/aac",
        "vtt" => "text/vtt",
        _ => ::service_common::web_media_type::media_type_of_path(file_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two values this module compares an endpoint's `Origination` against are values
    /// the member admits, and they are different values.
    #[test]
    fn origination_is_decided_against_the_values_the_member_admits() {
        for value in [DEFAULT_ORIGINATION, ORIGINATION_DENY] {
            assert!(
                crate::ops::types::ORIGINATION_VALUES.contains(&value),
                "case {value:?}: Origination admits {:?}",
                crate::ops::types::ORIGINATION_VALUES
            );
        }
        assert_ne!(
            DEFAULT_ORIGINATION, ORIGINATION_DENY,
            "the default and the denied value must be different values"
        );
    }

    /// RULE: an ingest path is the published ingest URL's path, and what follows the
    /// collection names the object.
    ///
    /// The rows span what the reading generalises over: the published URL itself, a named
    /// object under it, a nested path, a trailing separator, and every shape that is not an
    /// ingest path at all.
    #[test]
    fn an_ingest_path_names_a_collection_and_at_most_one_object() {
        let label = super::super::stable_label(&["channel", "us-west-2"]);
        let endpoint = super::super::stable_label(&["ingest-endpoint", "1"]);
        let base = ingest_path(&label, &endpoint);
        let rows: &[(&str, String, Option<Option<&str>>)] = &[
            ("the published url", base.clone(), Some(None)),
            (
                "a playlist under it",
                format!("{base}/index.m3u8"),
                Some(Some("index.m3u8")),
            ),
            (
                "a segment under a nested path",
                format!("{base}/720p/seg1.ts"),
                Some(Some("seg1.ts")),
            ),
            ("a trailing separator", format!("{base}/"), Some(None)),
            ("the playback route", format!("{PLAYBACK_PATH_PREFIX}{label}/index.m3u8"), None),
            ("a modelled control-plane route", "/channels/c1".to_string(), None),
            (
                "a label that is not one of this service's",
                format!("{INGEST_PATH_PREFIX}not-a-label/{endpoint}/channel"),
                None,
            ),
            (
                "another last segment",
                format!("{INGEST_PATH_PREFIX}{label}/{endpoint}/stream"),
                None,
            ),
        ];
        for (case, path, expected) in rows {
            let parsed = parse_ingest_path(path);
            match expected {
                None => assert!(
                    parsed.is_none(),
                    "case {case:?}: {path:?} is not an ingest path, got {parsed:?}"
                ),
                Some(object) => {
                    let parsed = parsed
                        .unwrap_or_else(|| panic!("case {case:?}: {path:?} is an ingest path"));
                    assert_eq!(
                        (
                            parsed.channel_label.as_str(),
                            parsed.ingest_endpoint_id.as_str(),
                            parsed.object_name.as_deref()
                        ),
                        (label.as_str(), endpoint.as_str(), *object),
                        "case {case:?}: expected the object {object:?}, got {parsed:?}"
                    );
                }
            }
        }
    }

    /// RULE: a playback path is the endpoint's own label and the path it serves.
    #[test]
    fn a_playback_path_names_an_endpoint_and_what_it_serves() {
        let label = super::super::stable_label(&["c1", "oe1", "us-west-2"]);
        let rows: &[(&str, String, Option<&str>)] = &[
            (
                "the endpoint's own manifest",
                format!("{PLAYBACK_PATH_PREFIX}{label}/index.m3u8"),
                Some("index.m3u8"),
            ),
            (
                "a child manifest",
                format!("{PLAYBACK_PATH_PREFIX}{label}/{label}/master/index.m3u8"),
                Some(&format!("{label}/master/index.m3u8")),
            ),
            (
                "the prefix alone",
                format!("{PLAYBACK_PATH_PREFIX}{label}"),
                Some(""),
            ),
            ("the ingest route", format!("{INGEST_PATH_PREFIX}{label}/x"), None),
            ("a modelled control-plane route", "/origin_endpoints/oe1".to_string(), None),
        ];
        for (case, path, expected) in rows {
            let parsed = parse_playback_path(path);
            assert_eq!(
                parsed.as_ref().map(|target| target.path.as_str()),
                *expected,
                "case {case:?}: expected the served path {expected:?}, got {parsed:?}"
            );
        }
    }

    /// RULE: a whitelist admits exactly the addresses its blocks cover, and an empty one
    /// restricts nobody.
    ///
    /// The rows span what the rule generalises over: no list at all, a single-host block, a
    /// wider block, several blocks, both address families, a block written with its host
    /// bits set, and an address this emulator cannot read.
    #[test]
    fn a_whitelist_admits_the_addresses_its_blocks_cover() {
        let rows: &[(&str, &[&str], &str, bool)] = &[
            ("no list", &[], "203.0.113.9", true),
            ("the one host listed", &["198.51.100.7/32"], "198.51.100.7", true),
            ("another host", &["198.51.100.7/32"], "198.51.100.8", false),
            ("inside a /24", &["198.51.100.0/24"], "198.51.100.200", true),
            ("outside a /24", &["198.51.100.0/24"], "198.51.101.1", false),
            (
                "the second of several blocks",
                &["10.0.0.0/8", "127.0.0.0/8"],
                "127.0.0.1",
                true,
            ),
            ("a v6 block", &["2001:db8::/32"], "2001:db8::1", true),
            ("a v4 caller against a v6 block", &["2001:db8::/32"], "192.0.2.1", false),
            ("host bits set", &["10.1.2.3/8"], "10.9.9.9", true),
            ("an unreadable address", &["0.0.0.0/0"], "not-an-address", false),
        ];
        for (case, whitelist, address, expected) in rows {
            let entries: Vec<String> = whitelist.iter().map(|entry| (*entry).to_string()).collect();
            let admitted = admits_source(&entries, address);
            assert_eq!(
                admitted, *expected,
                "case {case:?}: expected admitted {expected}, got {admitted}"
            );
        }
    }

    /// RULE: a Digest response verifies only when it is the one this endpoint's credentials
    /// compute for this request, and the challenge states what has to go into it.
    ///
    /// The rows span what the rule generalises over: the correct response, a wrong
    /// password, a wrong username, another realm, a stale nonce, a response computed over
    /// another request target and another method, the RFC 2069 form without a quality of
    /// protection, and `auth-int`, which the challenge does not advertise.
    #[test]
    fn a_digest_response_verifies_only_against_the_endpoints_own_credentials() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let id = "7a1d4f0e9b2c3d4e5f60718293a4b5c6";
        let username = "9f8e7d6c5b4a39281706f5e4d3c2b1a0";
        let password = "1122334455667788990011223344aabb";
        let nonce = nonce_at(id, now);
        let target = "/in/v2/x/y/channel/index.m3u8";

        /// The `response` a client computes with `qop=auth`.
        fn response_with_qop(
            username: &str,
            password: &str,
            method: &str,
            target: &str,
            nonce: &str,
            realm: &str,
        ) -> String {
            let secret = md5_hex(&format!("{username}:{realm}:{password}"));
            let over_request = md5_hex(&format!("{method}:{target}"));
            md5_hex(&format!(
                "{secret}:{nonce}:00000001:abc123:{DIGEST_QOP}:{over_request}"
            ))
        }

        let correct = response_with_qop(username, password, "PUT", target, &nonce, DIGEST_REALM);
        let header = |fields: String| format!("{DIGEST_SCHEME} {fields}");
        let fields = |user: &str, realm: &str, nonce: &str, uri: &str, response: &str| {
            format!(
                "username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", \
                 algorithm=MD5, qop=auth, nc=00000001, cnonce=\"abc123\", \
                 response=\"{response}\""
            )
        };
        let rfc2069 = {
            let secret = md5_hex(&format!("{username}:{DIGEST_REALM}:{password}"));
            let over_request = md5_hex(&format!("PUT:{target}"));
            let response = md5_hex(&format!("{secret}:{nonce}:{over_request}"));
            header(format!(
                "username=\"{username}\", realm=\"{DIGEST_REALM}\", nonce=\"{nonce}\", \
                 uri=\"{target}\", response=\"{response}\""
            ))
        };
        let rows: &[(&str, String, &str, bool)] = &[
            (
                "the response this endpoint's credentials compute",
                header(fields(username, DIGEST_REALM, &nonce, target, &correct)),
                "PUT",
                true,
            ),
            (
                "the RFC 2069 form, with no quality of protection",
                rfc2069,
                "PUT",
                true,
            ),
            (
                "a wrong password",
                header(fields(
                    username,
                    DIGEST_REALM,
                    &nonce,
                    target,
                    &response_with_qop(username, "wrong", "PUT", target, &nonce, DIGEST_REALM),
                )),
                "PUT",
                false,
            ),
            (
                "another username",
                header(fields("someone-else", DIGEST_REALM, &nonce, target, &correct)),
                "PUT",
                false,
            ),
            (
                "another realm",
                header(fields(
                    username,
                    "Other",
                    &nonce,
                    target,
                    &response_with_qop(username, password, "PUT", target, &nonce, "Other"),
                )),
                "PUT",
                false,
            ),
            (
                "a nonce from outside the window",
                header(fields(
                    username,
                    DIGEST_REALM,
                    "00000000000000000000000000000000",
                    target,
                    &correct,
                )),
                "PUT",
                false,
            ),
            (
                "a response computed for another request target",
                header(fields(username, DIGEST_REALM, &nonce, "/in/v2/x/y/channel/other.ts", &correct)),
                "PUT",
                false,
            ),
            (
                "the same response replayed on another method",
                header(fields(username, DIGEST_REALM, &nonce, target, &correct)),
                "GET",
                false,
            ),
            (
                "auth-int, which the challenge does not advertise",
                header(
                    fields(username, DIGEST_REALM, &nonce, target, &correct)
                        .replace("qop=auth", "qop=auth-int"),
                ),
                "PUT",
                false,
            ),
            ("no parameters at all", header(String::new()), "PUT", false),
        ];
        let endpoint = IngestEndpoint {
            id: Some(id.to_string()),
            url: None,
            username: Some(username.to_string()),
            password: Some(password.to_string()),
        };
        for (case, authorization, method, expected) in rows {
            let request = PlaneRequest {
                method,
                path: target,
                authorization: Some(authorization),
                source_address: "127.0.0.1",
                body: Some(b""),
                now,
            };
            let verdict = presented(&request, &endpoint);
            let verified = verdict == Presented::Verified;
            assert_eq!(
                verified, *expected,
                "case {case:?}: expected verified {expected}, got {verdict:?}"
            );
        }
    }

    /// RULE: the scheme alone decides between a challenge and a bare refusal.
    #[test]
    fn the_scheme_presented_decides_the_refusal_class() {
        let endpoint = IngestEndpoint {
            id: Some("7a1d4f0e9b2c3d4e5f60718293a4b5c6".to_string()),
            url: None,
            username: Some("user".to_string()),
            password: Some("secret".to_string()),
        };
        let rows: &[(&str, Option<&str>, Presented)] = &[
            ("no header at all", None, Presented::Nothing),
            ("an empty header", Some("   "), Presented::Nothing),
            ("HTTP Basic", Some("Basic dXNlcjpzZWNyZXQ="), Presented::OtherScheme),
            (
                "HTTP Basic with the wrong password",
                Some("Basic dXNlcjp3cm9uZw=="),
                Presented::OtherScheme,
            ),
            ("a bearer token", Some("Bearer abc.def"), Presented::OtherScheme),
            ("Digest with nothing in it", Some("Digest"), Presented::Unverified),
        ];
        for (case, authorization, expected) in rows {
            let request = PlaneRequest {
                method: "PUT",
                path: "/in/v2/x/y/channel/index.m3u8",
                authorization: *authorization,
                source_address: "127.0.0.1",
                body: Some(b""),
                now: SystemTime::UNIX_EPOCH,
            };
            let verdict = presented(&request, &endpoint);
            assert_eq!(
                verdict, *expected,
                "case {case:?}: expected {expected:?}, got {verdict:?}"
            );
        }
    }

    /// RULE: the challenge advertises Digest in the WebDAV realm, with the algorithm and
    /// quality of protection a response has to be computed with, and a nonce of its own per
    /// endpoint.
    #[test]
    fn the_challenge_advertises_digest_in_the_webdav_realm() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let challenge = digest_challenge("7a1d4f0e9b2c3d4e5f60718293a4b5c6", now);
        for expected in [
            "Digest ",
            "realm=\"WebDAV\"",
            "algorithm=MD5",
            "qop=\"auth\"",
            "nonce=\"",
        ] {
            assert!(
                challenge.contains(expected),
                "case {expected:?}: the challenge must carry it, got {challenge:?}"
            );
        }
        assert_ne!(
            digest_challenge("7a1d4f0e9b2c3d4e5f60718293a4b5c6", now),
            digest_challenge("0000000000000000000000000000ffff", now),
            "two endpoints must be challenged with nonces of their own"
        );
    }

    /// RULE: a presented credential's auth-params read as its name/value pairs, whatever a
    /// value carries, and credentials of another scheme carry none.
    #[test]
    fn auth_parameters_read_as_name_value_pairs() {
        let parsed = auth_parameters(
            "Digest username=\"u\", uri=\"/p?a=1,2\", qop=auth, nc=00000001, response=\"abc\", \
             extra=\"a\\\"b\"",
        );
        let rows: &[(&str, &str)] = &[
            ("username", "u"),
            ("uri", "/p?a=1,2"),
            ("qop", "auth"),
            ("nc", "00000001"),
            ("response", "abc"),
            ("extra", "a\"b"),
        ];
        for (name, expected) in rows {
            let value = parameter(&parsed, name);
            assert_eq!(
                value,
                Some(*expected),
                "case {name:?}: expected {expected:?}, got {value:?}"
            );
        }
        assert!(
            auth_parameters("Basic dXNlcjpzZWNyZXQ=").is_empty(),
            "another scheme's credentials carry no Digest parameters"
        );
    }

    /// RULE: a channel retains a live window of what was pushed into it — the last push of
    /// a name, the newest manifest of a format, and at most [`MAX_INGESTED_OBJECTS`]
    /// objects, oldest evicted first.
    #[test]
    fn a_channels_ingest_window_keeps_the_last_push_of_each_name() {
        let mut window = ChannelIngest::default();
        window.store("index.m3u8", b"#EXTM3U\n");
        window.store("seg1.ts", b"first");
        window.store("seg1.ts", b"second");
        assert_eq!(
            window.object("seg1.ts").map(|object| object.content.clone()),
            Some(b"second".to_vec()),
            "a second push of a name replaces its bytes"
        );
        window.store("live.m3u8", b"#EXTM3U\n#EXT-X-VERSION:3\n");
        assert_eq!(
            window.manifest("m3u8").map(|object| object.name.clone()),
            Some("live.m3u8".to_string()),
            "the newest manifest of the format is the one served"
        );
        assert!(
            window.manifest("mpd").is_none(),
            "a format nothing was pushed for has no manifest"
        );
        for index in 0..MAX_INGESTED_OBJECTS * 2 {
            window.store(&format!("s{index}.ts"), b"x");
        }
        assert_eq!(
            window.retained_objects(),
            MAX_INGESTED_OBJECTS,
            "the window retains at most {MAX_INGESTED_OBJECTS} objects, got {}",
            window.retained_objects()
        );
        assert!(
            window.object("index.m3u8").is_none(),
            "the oldest push is the one evicted"
        );
    }

    /// RULE: an object's name decides the media type it is served as.
    #[test]
    fn a_media_objects_name_decides_its_media_type() {
        let rows: &[(&str, &str)] = &[
            ("index.m3u8", HLS_MANIFEST_MEDIA_TYPE),
            ("manifest.mpd", DASH_MANIFEST_MEDIA_TYPE),
            ("stream.ism", MSS_MANIFEST_MEDIA_TYPE),
            ("seg1.ts", "video/mp2t"),
            ("seg1.m4s", "video/mp4"),
            ("audio1.m4a", "audio/mp4"),
            ("subs.vtt", "text/vtt"),
            ("INDEX.M3U8", HLS_MANIFEST_MEDIA_TYPE),
            ("notes.txt", "text/plain"),
            ("blob", ::service_common::web_media_type::GENERIC_MEDIA_TYPE),
        ];
        for (name, expected) in rows {
            let media_type = media_type_of(name);
            assert_eq!(
                media_type, *expected,
                "case {name:?}: expected {expected:?}, got {media_type:?}"
            );
        }
    }
}
