// SPDX-License-Identifier: CC0-1.0

//! What a SUB-RESOURCE write does to the content of the record it hangs off, driven over
//! HTTP through the same router the emulator boots and checked against the store the
//! handlers themselves write to.
//!
//! The rule under test, asserted over a TABLE of sub-resource writes rather than one
//! recorded request: the value that decides whether a write commits a new version of a
//! record is a function of that record's OWN members only, so a write to a sub-resource
//! hanging off it — one addressed by its own operations, versioned on its own and taking no
//! part in its parent's entity tag — changes no parent's content, while a write to a
//! record's own members does.
//!
//! The rows span every sub-resource of this service's hierarchy: the resource policy of a
//! channel and of an origin endpoint (attached, replaced by another document, replaced by
//! one carrying CDN authorization, detached, detached when none was ever attached), the
//! child channels of a group, the child endpoints of a channel, and the harvest jobs of an
//! endpoint. They differ in which record the sub-resource hangs off, in which sub-resource
//! is written, and in the kind of write (create, wholesale replace, delete, delete of
//! something absent).
//!
//! The decision itself is not projected by any response — AWS documents the ETag's PURPOSE
//! (it is what `x-amzn-update-if-match` is matched against) and not how it is computed — so
//! each row asserts it twice: on the persisted records' content fingerprints, and on the
//! commit bookkeeping (`ETag`, `ModifiedAt`) the matching reads report.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::VersionedRecord;

// The crate's shared test client, so this suite drives the emulator through one client
// rather than growing a second copy of the dispatch boilerplate, and the one
// `CreateHarvestJob` body its suites share.
use crate::channel_lifecycle::{minimal_endpoint, Emulator, REGION};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The header the model binds every `Update*`'s `ETag` member to.
const IF_MATCH: &str = "x-amzn-update-if-match";

/// The fixture chain every row hangs its sub-resources off. The names match the model's
/// `ResourceName` pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "scope-group";
const CHANNEL: &str = "scope-channel";
const ENDPOINT: &str = "scope-endpoint";

/// The one manifest the fixture endpoint publishes, so a harvest job created against it
/// names a manifest the endpoint really publishes (`minimal_endpoint` calls it `index`).
const MANIFEST: &str = "index";

/// The account every resource this emulator serves belongs to, which is therefore the
/// account a CDN authorization secret has to live in.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Two different policy documents. A resource policy is an IAM policy document — a JSON
/// object carrying a `Statement` — which is what the service's `INVALID_POLICY` validation
/// case is about.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"first"}]}"#;
const OTHER_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"second"}]}"#;

/// The `CdnAuthConfiguration` a row attaches with a document: `CdnIdentifierSecretArns` is
/// modelled `@length 1..100` and `SecretsRoleArn` `@length 20..2048`
/// (`smithy_query(query="shape", shape="CdnAuthConfiguration")`), so both are real ARNs.
fn cdn_auth() -> Value {
    json!({
        "CdnIdentifierSecretArns": [
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:cdn-auth"),
        ],
        "SecretsRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaPackageCdnAuth"),
    })
}

/// One sub-resource write: what it addresses and what it submits.
#[derive(Debug, Clone, Copy)]
enum SubResourceWrite {
    /// `PutChannelPolicy` on the fixture channel, with this document.
    PutChannelPolicy(&'static str),
    /// `DeleteChannelPolicy` on the fixture channel.
    DeleteChannelPolicy,
    /// `PutOriginEndpointPolicy` on the fixture endpoint: the document, plus whether CDN
    /// authorization travels with it.
    PutEndpointPolicy {
        document: &'static str,
        with_cdn_auth: bool,
    },
    /// `DeleteOriginEndpointPolicy` on the fixture endpoint.
    DeleteEndpointPolicy,
    /// `CreateChannel` — another channel beneath the fixture group.
    CreateSiblingChannel(&'static str),
    /// `CreateOriginEndpoint` — another endpoint beneath the fixture channel.
    CreateSiblingEndpoint(&'static str),
    /// `CreateHarvestJob` against the fixture endpoint.
    CreateHarvestJob(&'static str),
}

impl SubResourceWrite {
    /// Send this write, requiring the success the operation promises.
    async fn apply(self, emulator: &Emulator, case: &str) {
        let (status, body) = match self {
            Self::PutChannelPolicy(document) => {
                // `PutChannelPolicy` binds `PUT`, where `PutOriginEndpointPolicy` binds
                // `POST` (`smithy_query(query="operation",
                // operation="PutChannelPolicy,PutOriginEndpointPolicy")`).
                emulator
                    .call(
                        "PUT",
                        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/policy"),
                        Some(json!({"Policy": document})),
                    )
                    .await
            }
            Self::DeleteChannelPolicy => {
                emulator
                    .call(
                        "DELETE",
                        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/policy"),
                        None,
                    )
                    .await
            }
            Self::PutEndpointPolicy {
                document,
                with_cdn_auth,
            } => {
                let mut request = json!({"Policy": document});
                if with_cdn_auth {
                    request["CdnAuthConfiguration"] = cdn_auth();
                }
                emulator
                    .call("POST", &endpoint_policy_path(), Some(request))
                    .await
            }
            Self::DeleteEndpointPolicy => {
                emulator.call("DELETE", &endpoint_policy_path(), None).await
            }
            Self::CreateSiblingChannel(name) => {
                emulator.create_channel(GROUP, name, json!({})).await
            }
            Self::CreateSiblingEndpoint(name) => {
                emulator
                    .create_endpoint(GROUP, CHANNEL, name, minimal_endpoint("TS"))
                    .await
            }
            Self::CreateHarvestJob(name) => {
                // A window that opens in a minute and runs for an hour, so nothing the
                // clock owes the job ends it mid-test.
                let start = epoch_seconds_from_now(60.0);
                let mut request = harvest_body(start, start + 3600.0, &[MANIFEST]);
                request["HarvestJobName"] = json!(name);
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
                             /harvestJob"
                        ),
                        Some(request),
                    )
                    .await
            }
        };
        assert_eq!(
            status, 200,
            "case {case:?}: the sub-resource write {self:?} expected 200, got {status} {body}"
        );
    }
}

/// `PutOriginEndpointPolicy` / `GetOriginEndpointPolicy` / `DeleteOriginEndpointPolicy`
/// bind the fixture endpoint's `policy` sub-resource.
fn endpoint_policy_path() -> String {
    format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy")
}

/// The content fingerprint of each record of the fixture chain: the value
/// `VersionedRecord::commit_if_changed` decides a commit by, read straight out of the store
/// the handlers write to, since no response projects it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Content {
    group: String,
    channel: String,
    endpoint: String,
}

impl Content {
    /// The records whose content differs between two snapshots, each named with the
    /// fingerprint before and after — so a failing assertion reports the record that moved
    /// rather than all three.
    fn differences(&self, other: &Self) -> Vec<String> {
        [
            ("the channel group", &self.group, &other.group),
            ("the channel", &self.channel, &other.channel),
            ("the origin endpoint", &self.endpoint, &other.endpoint),
        ]
        .into_iter()
        .filter(|(_, before, after)| before != after)
        .map(|(record, before, after)| format!("{record}: {before} became {after}"))
        .collect()
    }
}

fn content(emulator: &Emulator, case: &str) -> Content {
    let store = emulator.store();
    let held = store.lock();
    let group = held
        .channel_group(REGION, GROUP)
        .unwrap_or_else(|| panic!("case {case:?}: the fixture group must be stored"));
    let channel = group
        .channels
        .get(CHANNEL)
        .unwrap_or_else(|| panic!("case {case:?}: the fixture channel must be stored"));
    let endpoint = channel
        .origin_endpoints
        .get(ENDPOINT)
        .unwrap_or_else(|| panic!("case {case:?}: the fixture endpoint must be stored"));
    Content {
        group: group.configuration_fingerprint(),
        channel: channel.configuration_fingerprint(),
        endpoint: endpoint.configuration_fingerprint(),
    }
}

/// The commit bookkeeping each record of the fixture chain reports: its entity tag and its
/// modification instant, the two values a commit moves.
#[derive(Debug, Clone, PartialEq)]
struct Bookkeeping {
    group: (Value, Value),
    channel: (Value, Value),
    endpoint: (Value, Value),
}

async fn bookkeeping(emulator: &Emulator, case: &str) -> Bookkeeping {
    async fn read(emulator: &Emulator, case: &str, what: &str, path: &str) -> (Value, Value) {
        let (status, body) = emulator.call("GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the {what} expected 200, got {status} {body}"
        );
        (body["ETag"].clone(), body["ModifiedAt"].clone())
    }
    Bookkeeping {
        group: read(emulator, case, "group", &format!("/channelGroup/{GROUP}")).await,
        channel: read(
            emulator,
            case,
            "channel",
            &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/"),
        )
        .await,
        endpoint: read(
            emulator,
            case,
            "endpoint",
            &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}"),
        )
        .await,
    }
}

/// Build the group -> channel -> endpoint chain every row hangs its sub-resources off.
async fn fixture(case: &str) -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(GROUP, CHANNEL, ENDPOINT, minimal_endpoint("TS"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateOriginEndpoint expected 200, got {status} {body}"
    );
    emulator
}

/// One row: the sub-resource writes that run before the snapshot, then the one whose effect
/// on its parents' content is under test.
struct ScopeCase {
    label: &'static str,
    prepare: &'static [SubResourceWrite],
    write: SubResourceWrite,
}

fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase {
            label: "attaching a policy to the origin endpoint",
            prepare: &[],
            write: SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: false,
            },
        },
        ScopeCase {
            label: "attaching a policy with CDN authorization to the origin endpoint",
            prepare: &[],
            write: SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: true,
            },
        },
        ScopeCase {
            label: "replacing the endpoint's policy with another document",
            prepare: &[SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: false,
            }],
            write: SubResourceWrite::PutEndpointPolicy {
                document: OTHER_DOCUMENT,
                with_cdn_auth: false,
            },
        },
        ScopeCase {
            label: "adding CDN authorization to the endpoint's stored policy",
            prepare: &[SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: false,
            }],
            write: SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: true,
            },
        },
        ScopeCase {
            label: "detaching the endpoint's policy",
            prepare: &[SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: true,
            }],
            write: SubResourceWrite::DeleteEndpointPolicy,
        },
        ScopeCase {
            label: "detaching a policy the endpoint never carried",
            prepare: &[],
            write: SubResourceWrite::DeleteEndpointPolicy,
        },
        ScopeCase {
            label: "attaching a policy to the channel",
            prepare: &[],
            write: SubResourceWrite::PutChannelPolicy(DOCUMENT),
        },
        ScopeCase {
            label: "replacing the channel's policy with another document",
            prepare: &[SubResourceWrite::PutChannelPolicy(DOCUMENT)],
            write: SubResourceWrite::PutChannelPolicy(OTHER_DOCUMENT),
        },
        ScopeCase {
            label: "detaching the channel's policy",
            prepare: &[SubResourceWrite::PutChannelPolicy(DOCUMENT)],
            write: SubResourceWrite::DeleteChannelPolicy,
        },
        ScopeCase {
            label: "creating another channel beneath the group",
            prepare: &[],
            write: SubResourceWrite::CreateSiblingChannel("scope-sibling-channel"),
        },
        ScopeCase {
            label: "creating another origin endpoint beneath the channel",
            prepare: &[],
            write: SubResourceWrite::CreateSiblingEndpoint("scope-sibling-endpoint"),
        },
        ScopeCase {
            label: "creating a harvest job against the endpoint",
            prepare: &[],
            write: SubResourceWrite::CreateHarvestJob("scope-job"),
        },
        ScopeCase {
            label: "creating a harvest job against an endpoint that carries a policy",
            prepare: &[SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: true,
            }],
            write: SubResourceWrite::CreateHarvestJob("scope-job"),
        },
    ]
}

/// THE RULE — a sub-resource write leaves the content of every record it hangs off alone,
/// so none of them commits, while a write to a record's OWN members does commit.
#[tokio::test]
async fn a_sub_resource_write_is_no_part_of_its_parents_content() {
    for case in scope_cases() {
        let label = case.label;
        let emulator = fixture(label).await;
        for prepared in case.prepare {
            prepared.apply(&emulator, label).await;
        }

        let before_content = content(&emulator, label);
        let before_bookkeeping = bookkeeping(&emulator, label).await;

        case.write.apply(&emulator, label).await;

        let after_content = content(&emulator, label);
        let moved = before_content.differences(&after_content);
        assert!(
            moved.is_empty(),
            "case {label:?}: a sub-resource write must leave the content of every record it \
             hangs off alone, but {}",
            moved.join("; ")
        );
        let after_bookkeeping = bookkeeping(&emulator, label).await;
        assert_eq!(
            after_bookkeeping, before_bookkeeping,
            "case {label:?}: a sub-resource write commits nothing, so no parent's ETag or \
             ModifiedAt may move, got {after_bookkeeping:?} where {before_bookkeeping:?} was \
             reported"
        );

        // The control: the endpoint's OWN content still decides its commit, with whatever
        // the row left hanging off it in place.
        let (status, committed) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                endpoint_with_description("described by the control"),
                &[(IF_MATCH, endpoint_tag(&before_bookkeeping, label))],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update carrying the current ETag expected 200, got {status} \
             {committed}"
        );
        let changed_content = content(&emulator, label);
        assert_ne!(
            changed_content.endpoint, before_content.endpoint,
            "case {label:?}: a write to the endpoint's own members must change its content, \
             got {} twice",
            changed_content.endpoint
        );
        let changed_bookkeeping = bookkeeping(&emulator, label).await;
        assert_ne!(
            changed_bookkeeping.endpoint, before_bookkeeping.endpoint,
            "case {label:?}: a write that changed the endpoint must commit, rotating its \
             ETag, got {:?} twice",
            changed_bookkeeping.endpoint
        );
        assert_eq!(
            changed_bookkeeping.group, before_bookkeeping.group,
            "case {label:?}: an endpoint's commit is no part of its group, got {:?} where \
             {:?} was reported",
            changed_bookkeeping.group, before_bookkeeping.group
        );
        assert_eq!(
            changed_bookkeeping.channel, before_bookkeeping.channel,
            "case {label:?}: an endpoint's commit is no part of its channel, got {:?} where \
             {:?} was reported",
            changed_bookkeeping.channel, before_bookkeeping.channel
        );

        // And the sub-resource the row wrote survived the commit: a write to the endpoint
        // neither reads nor replaces it.
        let (status, policy) = emulator.call("GET", &endpoint_policy_path(), None).await;
        let expected = stored_endpoint_policy(&case);
        match expected {
            Some((document, with_cdn_auth)) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: the attached policy must survive the endpoint's commit, \
                     got {status} {policy}"
                );
                assert_eq!(
                    policy["Policy"],
                    json!(document),
                    "case {label:?}: the stored document must be reported unchanged, got {policy}"
                );
                assert_eq!(
                    policy.get("CdnAuthConfiguration"),
                    with_cdn_auth.then(cdn_auth).as_ref(),
                    "case {label:?}: the stored CDN authorization must be reported unchanged, \
                     got {policy}"
                );
            }
            None => assert_eq!(
                status, 404,
                "case {label:?}: an endpoint carrying no policy must answer 404, got {status} \
                 {policy}"
            ),
        }
    }
}

/// The endpoint policy a row leaves attached: the document and whether CDN authorization
/// travels with it, or `None` when the row leaves the endpoint carrying none — a put
/// replaces the record wholesale, so the last put of the row decides.
fn stored_endpoint_policy(case: &ScopeCase) -> Option<(&'static str, bool)> {
    let mut stored = None;
    for write in case.prepare.iter().chain(std::iter::once(&case.write)) {
        match write {
            SubResourceWrite::PutEndpointPolicy {
                document,
                with_cdn_auth,
            } => stored = Some((*document, *with_cdn_auth)),
            SubResourceWrite::DeleteEndpointPolicy => stored = None,
            _ => {}
        }
    }
    stored
}

/// The entity tag the endpoint reported, as the `x-amzn-update-if-match` precondition.
fn endpoint_tag<'a>(bookkeeping: &'a Bookkeeping, case: &str) -> &'a str {
    bookkeeping
        .endpoint
        .0
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the endpoint read must report an ETag"))
}

/// The content the fixture endpoint was created with, plus a description: an update carries
/// the WHOLE configuration, so everything the create stored is re-submitted here.
fn endpoint_with_description(description: &str) -> Value {
    let mut body = minimal_endpoint("TS");
    body["Description"] = json!(description);
    body
}

/// THE SAME RULE, end to end: whatever sub-resource an endpoint carries, an update that
/// re-submits the content already stored commits nothing — so the tag the caller holds
/// stays usable — and the sub-resource is still there afterwards.
///
/// The rows are the policy states an endpoint can be in: none attached, a document, a
/// document with CDN authorization, and a document that has since been detached.
#[tokio::test]
async fn an_update_commits_by_the_endpoints_own_content_whatever_hangs_off_it() {
    let rows: [(&str, &[SubResourceWrite]); 4] = [
        ("an endpoint carrying no policy", &[]),
        (
            "an endpoint carrying a policy document",
            &[SubResourceWrite::PutEndpointPolicy {
                document: DOCUMENT,
                with_cdn_auth: false,
            }],
        ),
        (
            "an endpoint carrying a policy with CDN authorization",
            &[SubResourceWrite::PutEndpointPolicy {
                document: OTHER_DOCUMENT,
                with_cdn_auth: true,
            }],
        ),
        (
            "an endpoint whose policy was attached and then detached",
            &[
                SubResourceWrite::PutEndpointPolicy {
                    document: DOCUMENT,
                    with_cdn_auth: true,
                },
                SubResourceWrite::DeleteEndpointPolicy,
            ],
        ),
    ];

    for (label, writes) in rows {
        let emulator = fixture(label).await;
        for write in writes {
            write.apply(&emulator, label).await;
        }
        let stored = bookkeeping(&emulator, label).await;
        let tag = endpoint_tag(&stored, label).to_string();

        // Re-submitting the stored content, repeatedly and under the tag the caller already
        // holds: the invariant is idempotence, not a single stable answer.
        for attempt in 0..3 {
            let presented: Vec<(&str, &str)> = (attempt > 0)
                .then_some((IF_MATCH, tag.as_str()))
                .into_iter()
                .collect();
            let (status, answered) = emulator
                .update_endpoint(GROUP, CHANNEL, ENDPOINT, minimal_endpoint("TS"), &presented)
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}/{attempt}: re-submitting the stored content expected 200, got \
                 {status} {answered}"
            );
            assert_eq!(
                answered["ETag"],
                json!(tag),
                "case {label:?}/{attempt}: a write that stored nothing new must report the \
                 stored ETag, got {}",
                answered["ETag"]
            );
            let after = bookkeeping(&emulator, label).await;
            assert_eq!(
                after, stored,
                "case {label:?}/{attempt}: a write that stored nothing new commits nothing, \
                 got {after:?} where {stored:?} was reported"
            );
        }

        // A changed configuration commits exactly once, under the same held tag.
        let (status, committed) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                endpoint_with_description("described"),
                &[(IF_MATCH, tag.as_str())],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the changing update expected 200, got {status} {committed}"
        );
        assert_ne!(
            committed["ETag"],
            json!(tag),
            "case {label:?}: a write that changed the endpoint must rotate its ETag, got {} \
             twice",
            committed["ETag"]
        );
        assert_eq!(
            committed["Description"],
            json!("described"),
            "case {label:?}: the update must report what it stored, got {committed}"
        );
    }
}
