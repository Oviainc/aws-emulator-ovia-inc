// SPDX-License-Identifier: CC0-1.0

//! What a reset of ONE origin endpoint reaches, asserted end-to-end across the control
//! plane and both halves of the data plane.
//!
//! The rule under test: a reset records on the ADDRESSED resource the instant from which
//! it no longer offers what it offered before, and removes nothing any other caller can
//! observe — the content a PARENT resource retains stays retained, the sibling resources
//! beneath that parent keep their own reset state, and the addressed resource's own
//! configuration and data-plane answers are what they were. The availability of an origin
//! endpoint's content is derived, not stored: the only content this service retains is
//! what a channel's ingest data plane accepted, so one endpoint's reset is not what
//! removes it.
//!
//! `origin_endpoint_reset.rs` covers the recorded instant itself (that it is reported,
//! read back, repeatable, and refused through an unresolved chain), and
//! `channel_reset_removes_ingested_content.rs` covers what the CHANNEL's reset removes.
//! This suite is the part neither pins: what a reset of one endpoint does NOT reach, over
//! rows that differ along the channel's input type, the endpoint's container type and
//! manifest family, how many objects the channel ingested before the reset, how many
//! sibling endpoints hang beside the target, and how many times the target is reset.
//!
//! The client, the policy writer, the URL splitter and the two published-URL helpers are
//! the ones `rejection_rule_and_negative_paths` and `published_data_plane_urls` already
//! own, reached through `pub(crate)`, so there is no second copy of them here.

use serde_json::{json, Value};

use crate::published_data_plane_urls::{manifest_url, sibling_object};
use crate::rejection_rule_and_negative_paths::{grant_policy, split_url, Answer, Emulator};

/// The methods the ingest data plane serves: a contributing encoder writes a segment with
/// either of them.
const INGEST_METHODS: [&str; 2] = ["PUT", "POST"];

/// A cap on the fill loop below, so a changed per-channel ingest ceiling cannot make this
/// suite run forever. It is only a safety net: the loop stops at the first refusal.
const FILL_ATTEMPT_CAP: usize = 1024;

/// One origin endpoint of a row: its name, the container type it is created with, and the
/// manifest family (with the suffix that family's URL ends in) it publishes.
///
/// The container/family pairings come from the ones the model's `ValidationExceptionType`
/// leaves legal — `TS_CONTAINER_TYPE_WITH_DASH_MANIFEST`,
/// `TS_CONTAINER_TYPE_WITH_MSS_MANIFEST`, `CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST`,
/// `ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST`,
/// `ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST` — over the `ContainerType` enum (CMAF | ISM |
/// TS) and the `CreateOriginEndpoint` manifest members (`HlsManifests`,
/// `LowLatencyHlsManifests`, `DashManifests`, `MssManifests`).
struct EndpointRow {
    name: &'static str,
    container: &'static str,
    family: &'static str,
    manifest: &'static str,
    suffix: &'static str,
}

/// One row: a channel and the endpoints beneath it, the first of which is the one that
/// gets reset.
struct Row {
    label: &'static str,
    group: &'static str,
    channel: &'static str,
    /// The channel's `InputType`, or `None` to leave the member off the create — which
    /// the member's own doc defaults to `HLS`.
    input_type: Option<&'static str>,
    /// The row's endpoints, the first entry being the reset target and the rest its
    /// untouched siblings.
    endpoints: &'static [EndpointRow],
    /// How many objects the channel's ingest data plane accepts before the reset.
    ingested: usize,
    /// How many times the target endpoint is reset.
    resets: usize,
}

/// The rows. Each hangs under its own channel group, so one row's content cannot be
/// mistaken for another's.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a lone TS/HLS endpoint on a channel nothing has pushed to",
            group: "scope-default",
            channel: "c-default",
            input_type: None,
            endpoints: &[EndpointRow {
                name: "ep-hls",
                container: "TS",
                family: "HlsManifests",
                manifest: "index",
                suffix: ".m3u8",
            }],
            ingested: 0,
            resets: 1,
        },
        Row {
            label: "a CMAF/DASH endpoint beside two siblings, one object ingested",
            group: "scope-cmaf",
            channel: "c-cmaf",
            input_type: Some("CMAF"),
            endpoints: &[
                EndpointRow {
                    name: "ep-dash",
                    container: "CMAF",
                    family: "DashManifests",
                    manifest: "dash",
                    suffix: ".mpd",
                },
                EndpointRow {
                    name: "ep-llhls",
                    container: "CMAF",
                    family: "LowLatencyHlsManifests",
                    manifest: "low",
                    suffix: ".m3u8",
                },
                EndpointRow {
                    name: "ep-hls-too",
                    container: "CMAF",
                    family: "HlsManifests",
                    manifest: "index",
                    suffix: ".m3u8",
                },
            ],
            ingested: 1,
            resets: 1,
        },
        Row {
            label: "an ISM/MSS endpoint beside a sibling, five objects ingested, reset twice",
            group: "scope-ism",
            channel: "c-ism",
            input_type: Some("CMAF"),
            endpoints: &[
                EndpointRow {
                    name: "ep-mss",
                    container: "ISM",
                    family: "MssManifests",
                    manifest: "smooth",
                    suffix: ".ism/Manifest",
                },
                EndpointRow {
                    name: "ep-mss-other",
                    container: "ISM",
                    family: "MssManifests",
                    manifest: "other",
                    suffix: ".ism/Manifest",
                },
            ],
            ingested: 5,
            resets: 2,
        },
        Row {
            label: "an HLS-input channel whose endpoint is reset three times",
            group: "scope-hls",
            channel: "c-hls",
            input_type: Some("HLS"),
            endpoints: &[
                EndpointRow {
                    name: "ep-ts",
                    container: "TS",
                    family: "HlsManifests",
                    manifest: "index",
                    suffix: ".m3u8",
                },
                EndpointRow {
                    name: "ep-ts-other",
                    container: "TS",
                    family: "HlsManifests",
                    manifest: "spare",
                    suffix: ".m3u8",
                },
            ],
            ingested: 3,
            resets: 3,
        },
    ]
}

/// `ResetOriginEndpointState`: a POST of the endpoint's `/reset` sub-resource, per the
/// operation's http binding.
async fn reset_endpoint(emulator: &Emulator, group: &str, channel: &str, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}/reset"),
            None,
        )
        .await
}

/// `GetOriginEndpoint`.
async fn get_endpoint(emulator: &Emulator, group: &str, channel: &str, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}"),
            None,
        )
        .await
}

/// `GetChannel` — the trailing slash is the operation's http binding.
async fn get_channel(emulator: &Emulator, group: &str, channel: &str) -> Answer {
    emulator
        .call("GET", &format!("/channelGroup/{group}/channel/{channel}/"), None)
        .await
}

/// `PutOriginEndpointPolicy`, so an egress fetch of this endpoint gets past
/// authorization and reaches the content lookup.
async fn grant_on_endpoint(
    emulator: &Emulator,
    group: &str,
    channel: &str,
    name: &str,
    action: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}/policy"),
            Some(json!({"Policy": grant_policy(action)})),
        )
        .await
}

/// One credentialed push to a published ingest URL.
async fn push(emulator: &Emulator, host: &str, path: &str, method: &str) -> Answer {
    emulator
        .send(
            method,
            path,
            Some(json!({"segment": "bytes"})),
            host,
            true,
            &[],
        )
        .await
}

/// One anonymous fetch of a published egress URL — what a player sends.
async fn fetch(emulator: &Emulator, host: &str, path: &str, method: &str) -> Answer {
    emulator.send(method, path, None, host, false, &[]).await
}

/// The URLs a channel response publishes for a contributing encoder to push to.
fn ingest_urls(channel: &Value) -> Vec<String> {
    channel
        .get("IngestEndpoints")
        .and_then(Value::as_array)
        .map(|endpoints| {
            endpoints
                .iter()
                .filter_map(|endpoint| endpoint.get("Url").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One egress probe: a label, the method, the host, the path, and the status the rule
/// requires of it.
struct Probe {
    label: String,
    method: &'static str,
    host: String,
    path: String,
    expected: u16,
}

/// Run every probe and report the statuses, in order.
async fn probe_statuses(emulator: &Emulator, probes: &[Probe], when: &str) -> Vec<u16> {
    let mut statuses = Vec::with_capacity(probes.len());
    for probe in probes {
        let answer = fetch(emulator, &probe.host, &probe.path, probe.method).await;
        assert_eq!(
            answer.status, probe.expected,
            "{when}: probe {:?} ({} {}) expected HTTP {}, got {} with body {}",
            probe.label, probe.method, probe.path, probe.expected, answer.status, answer.body
        );
        statuses.push(answer.status);
    }
    statuses
}

/// Build one row's hierarchy, push its content in, and report what the row needs
/// afterwards: the create-time read of every endpoint and the egress probes to compare
/// across the reset.
struct Built {
    /// The pre-reset `GetOriginEndpoint` body of every endpoint, in the row's order.
    endpoints: Vec<Value>,
    /// The pre-reset `GetChannel` body.
    channel: Value,
    /// The ingest host and the path of the channel's first published ingest endpoint.
    ingest: Option<(String, String)>,
    probes: Vec<Probe>,
}

async fn build(emulator: &Emulator, row: &Row) -> Built {
    let label = row.label;
    let group = emulator.create_group(row.group, json!({})).await;
    assert_eq!(
        group.status, 200,
        "case {label:?}: CreateChannelGroup expected 200, got {} with body {}",
        group.status, group.body
    );
    let egress_host = group.string("EgressDomain").to_string();

    let mut channel_body = json!({});
    if let Some(input_type) = row.input_type {
        channel_body["InputType"] = json!(input_type);
    }
    let channel = emulator
        .create_channel(row.group, row.channel, channel_body)
        .await;
    assert_eq!(
        channel.status, 200,
        "case {label:?}: CreateChannel expected 200, got {} with body {}",
        channel.status, channel.body
    );
    let published = ingest_urls(&channel.json);
    assert!(
        published.len() >= 2,
        "case {label:?}: a channel that receives ingest publishes a redundant pair of ingest \
         endpoints, got {published:?}"
    );

    // The content the channel retains before the reset: spread over both methods an
    // encoder may use and both endpoints of the channel's redundant pair, so the rule is
    // not pinned to one push shape.
    for object in 0..row.ingested {
        let url = &published[object % published.len()];
        let (host, path) = split_url(url);
        let method = INGEST_METHODS[object % INGEST_METHODS.len()];
        let answer = push(emulator, &host, &format!("{path}/{object}.mp4"), method).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: pushing object {object} with {method} expected 200, got {} with \
             body {}",
            answer.status, answer.body
        );
    }

    let mut endpoints = Vec::new();
    let mut probes = Vec::new();
    for (position, endpoint) in row.endpoints.iter().enumerate() {
        let mut body = json!({"ContainerType": endpoint.container});
        // The family is a row's value, so the member it names is set on the body rather
        // than written as a literal key.
        body[endpoint.family] = json!([{"ManifestName": endpoint.manifest}]);
        let created = emulator
            .create_endpoint(row.group, row.channel, endpoint.name, body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateOriginEndpoint {} expected 200, got {} with body {}",
            endpoint.name, created.status, created.body
        );
        let url = manifest_url(&created.json, endpoint.family);
        let (host, manifest_path) = split_url(&url);
        assert_eq!(
            host, egress_host,
            "case {label:?}: endpoint {} must publish on the group's egress domain, got {url}",
            endpoint.name
        );
        // The probes below address a path the SERVICE published, so the path itself is
        // pinned to the one the endpoint's manifest family builds.
        assert_eq!(
            manifest_path,
            format!(
                "/out/v1/{}/{}/{}/{}{}",
                row.group, row.channel, endpoint.name, endpoint.manifest, endpoint.suffix
            ),
            "case {label:?}: endpoint {} must publish its {} manifest under its own path, got \
             {url}",
            endpoint.name,
            endpoint.family
        );

        if position == 0 {
            // The reset target is granted the action a `GET` needs, so its fetches reach
            // the content lookup rather than stopping at authorization: the lookup is what
            // a reset could change the answer of.
            let granted = grant_on_endpoint(
                emulator,
                row.group,
                row.channel,
                endpoint.name,
                "mediapackagev2:GetObject",
            )
            .await;
            assert_eq!(
                granted.status, 200,
                "case {label:?}: granting GetObject on {} expected 200, got {} with body {}",
                endpoint.name, granted.status, granted.body
            );
            // Nothing is packaged for an endpoint, so an authorized fetch resolves no
            // object: 404 both before and after a reset.
            probes.push(Probe {
                label: format!("the granted manifest of {}", endpoint.name),
                method: "GET",
                host: host.clone(),
                path: manifest_path.clone(),
                expected: 404,
            });
            probes.push(Probe {
                label: format!("a granted segment beside the manifest of {}", endpoint.name),
                method: "GET",
                host: host.clone(),
                path: sibling_object(&manifest_path),
                expected: 404,
            });
            // `HEAD` needs its own action, which this policy does not grant: still an
            // authorization refusal, which a reset does not change either.
            probes.push(Probe {
                label: format!("an ungranted HEAD of {}", endpoint.name),
                method: "HEAD",
                host: host.clone(),
                path: manifest_path.clone(),
                expected: 403,
            });
        } else {
            // A sibling carries no policy at all, so its fetches are refused — and a reset
            // of the target must not make them anything else.
            probes.push(Probe {
                label: format!("the ungated manifest of the sibling {}", endpoint.name),
                method: "GET",
                host: host.clone(),
                path: manifest_path.clone(),
                expected: 403,
            });
        }

        let read = get_endpoint(emulator, row.group, row.channel, endpoint.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetOriginEndpoint {} expected 200, got {} with body {}",
            endpoint.name, read.status, read.body
        );
        assert!(
            read.json["ResetAt"].is_null(),
            "case {label:?}: endpoint {} was never reset, so ResetAt must be absent, got {}",
            endpoint.name,
            read.body
        );
        endpoints.push(read.json);
    }

    let channel_read = get_channel(emulator, row.group, row.channel).await;
    assert_eq!(
        channel_read.status, 200,
        "case {label:?}: GetChannel expected 200, got {} with body {}",
        channel_read.status, channel_read.body
    );

    let ingest = published.first().map(|url| split_url(url));
    Built {
        endpoints,
        channel: channel_read.json,
        ingest,
        probes,
    }
}

/// The rule, over the whole table: resetting one endpoint records its instant there and
/// changes nothing else a caller can read — not the channel, not the siblings, not the
/// endpoint's own configuration, and not what either half of the data plane answers.
#[tokio::test]
async fn resetting_one_endpoint_changes_nothing_any_other_caller_can_observe() {
    let emulator = Emulator::new();

    for row in rows() {
        let label = row.label;
        let built = build(&emulator, &row).await;
        let target = row.endpoints[0].name;
        let before_probes = probe_statuses(&emulator, &built.probes, &format!("case {label:?} before the reset")).await;

        let mut previous = f64::NEG_INFINITY;
        for attempt in 1..=row.resets {
            let answer = reset_endpoint(&emulator, row.group, row.channel, target).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: reset #{attempt} expected 200, got {} with body {}",
                answer.status, answer.body
            );
            let reset_at = answer.json["ResetAt"].as_f64().unwrap_or_else(|| {
                panic!(
                    "case {label:?}: reset #{attempt} must report ResetAt as epoch seconds, got {}",
                    answer.body
                )
            });
            assert!(
                reset_at >= previous,
                "case {label:?}: reset #{attempt} reported ResetAt {reset_at}, which precedes the \
                 previous {previous}"
            );
            previous = reset_at;
        }

        // The addressed endpoint: ResetAt is the one member that moved.
        let after = get_endpoint(&emulator, row.group, row.channel, target).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: GetOriginEndpoint after the reset expected 200, got {} with body {}",
            after.status, after.body
        );
        assert_eq!(
            after.json["ResetAt"].as_f64(),
            Some(previous),
            "case {label:?}: the read must report the last reset's instant {previous}, got {}",
            after.body
        );
        for (member, value) in built.endpoints[0]
            .as_object()
            .expect("GetOriginEndpoint answers an object")
        {
            // `ResetAt` is the member the reset moved, asserted on its own above; every
            // other member the pre-reset read reported must come back unchanged.
            if member == "ResetAt" {
                continue;
            }
            assert_eq!(
                &after.json[member], value,
                "case {label:?}: a reset must not change {member} of the endpoint it reset, got {} \
                 after {value}",
                after.json[member]
            );
        }

        // The siblings beneath the same channel: untouched, reset state included.
        for (position, sibling) in row.endpoints.iter().enumerate().skip(1) {
            let read = get_endpoint(&emulator, row.group, row.channel, sibling.name).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: GetOriginEndpoint {} expected 200, got {} with body {}",
                sibling.name, read.status, read.body
            );
            assert_eq!(
                read.json, built.endpoints[position],
                "case {label:?}: resetting {target} must leave the sibling {} exactly as it was, \
                 got {}",
                sibling.name, read.body
            );
        }

        // The channel above it: untouched, its own reset state included.
        let channel = get_channel(&emulator, row.group, row.channel).await;
        assert_eq!(
            channel.status, 200,
            "case {label:?}: GetChannel expected 200, got {} with body {}",
            channel.status, channel.body
        );
        assert_eq!(
            channel.json, built.channel,
            "case {label:?}: resetting {target} must leave its channel exactly as it was, got {}",
            channel.body
        );

        // The egress half: a reset neither reveals content the endpoint was not serving
        // nor changes an authorization decision.
        let after_probes = probe_statuses(&emulator, &built.probes, &format!("case {label:?} after the reset")).await;
        assert_eq!(
            after_probes, before_probes,
            "case {label:?}: the egress answers after the reset must be the ones before it"
        );

        // The ingest half: the channel still accepts content after one of its endpoints
        // was reset.
        if let Some((host, path)) = &built.ingest {
            let answer = push(&emulator, host, &format!("{path}/after-reset.mp4"), "PUT").await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: a push after the reset expected 200, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

/// The retention half of the same rule, at the one boundary where retained content is
/// observable: the channel's ingest bounds. Resetting an endpoint beneath the channel frees
/// none of what the channel retains, whichever endpoint is reset — while resetting the
/// channel, the resource that content belongs to, does.
#[tokio::test]
async fn resetting_an_endpoint_frees_none_of_the_content_its_channel_retains() {
    let emulator = Emulator::new();
    let group = emulator.create_group("retain-g", json!({})).await;
    assert_eq!(
        group.status, 200,
        "CreateChannelGroup expected 200, got {} with body {}",
        group.status, group.body
    );
    let channel = emulator
        .create_channel("retain-g", "retain-c", json!({"InputType": "HLS"}))
        .await;
    assert_eq!(
        channel.status, 200,
        "CreateChannel expected 200, got {} with body {}",
        channel.status, channel.body
    );
    let published = ingest_urls(&channel.json);
    let url = published
        .first()
        .unwrap_or_else(|| panic!("an HLS channel publishes ingest URLs, got {}", channel.body));
    let (host, path) = split_url(url);

    let endpoints = ["retain-e", "retain-e-other"];
    for name in endpoints {
        let created = emulator
            .create_endpoint(
                "retain-g",
                "retain-c",
                name,
                json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateOriginEndpoint {name} expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    // Fill the channel to its ingest bounds: push distinct objects until one is refused.
    // What the bounds ARE is this emulator's own choice, so they are discovered rather than
    // written here — the rule is about which operation moves them.
    let mut accepted = 0usize;
    let mut refusal = None;
    for object in 0..FILL_ATTEMPT_CAP {
        let answer = push(&emulator, &host, &format!("{path}/fill-{object}.mp4"), "PUT").await;
        if answer.status == 200 {
            accepted += 1;
            continue;
        }
        refusal = Some((answer.status, answer.body));
        break;
    }
    let (refused_status, refused_body) = refusal.unwrap_or_else(|| {
        panic!("the channel must reach its ingest bounds within {FILL_ATTEMPT_CAP} pushes")
    });
    assert!(
        accepted > 0,
        "the channel must accept content before its bounds are reached, accepted {accepted}"
    );
    assert_eq!(
        refused_status, 507,
        "a push past the channel's ingest bounds must answer 507, got {refused_status} with \
         body {refused_body}"
    );

    // Each endpoint's reset: recorded on that endpoint, and the channel's retained content
    // is still retained — the next new object is refused exactly as before.
    for (position, name) in endpoints.iter().enumerate() {
        let reset = reset_endpoint(&emulator, "retain-g", "retain-c", name).await;
        assert_eq!(
            reset.status, 200,
            "resetting {name} expected 200, got {} with body {}",
            reset.status, reset.body
        );
        let read = get_endpoint(&emulator, "retain-g", "retain-c", name).await;
        assert_eq!(
            read.json["ResetAt"], reset.json["ResetAt"],
            "case {name:?}: the read must report the instant the reset answered with, got {}",
            read.body
        );

        let after = push(
            &emulator,
            &host,
            &format!("{path}/after-reset-{position}.mp4"),
            "PUT",
        )
        .await;
        assert_eq!(
            after.status, refused_status,
            "case {name:?}: resetting an origin endpoint frees none of the content its channel \
             retains, so a new object must still answer {refused_status}, got {} with body {}",
            after.status, after.body
        );
        assert_eq!(
            after.body, refused_body,
            "case {name:?}: the refusal must be the one the bounds already answered, got {}",
            after.body
        );
    }

    // And the content is still THERE rather than merely counted: an object the channel
    // already holds is still writable at the bounds, which only a stored object is.
    let replaced = push(&emulator, &host, &format!("{path}/fill-0.mp4"), "PUT").await;
    assert_eq!(
        replaced.status, 200,
        "replacing an object the channel already holds must be accepted at its ingest bounds, \
         got {} with body {}",
        replaced.status, replaced.body
    );

    // The other side of the same rule, and what makes the answers above a SCOPE rather than
    // an inability to remove anything: resetting the resource the content BELONGS to frees
    // it, so the next new object is accepted.
    let channel_reset = emulator
        .call("POST", "/channelGroup/retain-g/channel/retain-c/reset", None)
        .await;
    assert_eq!(
        channel_reset.status, 200,
        "ResetChannelState expected 200, got {} with body {}",
        channel_reset.status, channel_reset.body
    );
    let after_channel_reset = push(
        &emulator,
        &host,
        &format!("{path}/after-channel-reset.mp4"),
        "PUT",
    )
    .await;
    assert_eq!(
        after_channel_reset.status, 200,
        "resetting the channel the content belongs to frees it, so a new object must be \
         accepted, got {} with body {}",
        after_channel_reset.status, after_channel_reset.body
    );
}
