// SPDX-License-Identifier: CC0-1.0

//! The PORTAL resource's own record: what a portal create resolved, how a reader projects
//! it onto the modelled `PortalSummary`, and the order `ListPortals` reports the account's
//! portals in.
//!
//! Split out of `state.rs` byte for byte, so the portal's record and the behaviour it
//! carries live in one file; nothing about it changed in the move.

use super::*;

/// A persisted `Portal` resource.
///
/// An account-level resource: a portal is not owned by an api, and the resources it
/// references (its portal products) are named by ARN rather than nested. The record
/// keeps what the create RESOLVED — the id and ARN it minted, the endpoint configuration
/// it derived from the request, the instant it was last modified — so every reader of
/// the portal reports one value, and [`Portal::to_wire`] projects it onto the modelled
/// `PortalSummary` shape every portal reader serializes from.
#[derive(Debug, Clone)]
pub struct Portal {
    pub portal_id: String,
    pub portal_arn: String,
    /// This portal's position in the order the account created its portals: the seed
    /// [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_id`] minted its id from,
    /// which counts the portals created before it. `ListPortals` reports the collection
    /// in this order (see [`CreatedInOrder`]), because a minted `PortalId` carries none.
    pub creation_order: u64,
    /// The authorization the create accepted: exactly one of its alternatives, see
    /// [`validate_exclusive_choice`].
    pub authorization: wire::Authorization,
    /// The endpoint configuration the create RESOLVED: the request names the domain
    /// (its own, or API Gateway's default) and the response reports the default domain
    /// name and hosted zone id the service generated for it.
    pub endpoint_configuration: wire::EndpointConfigurationResponse,
    pub included_portal_product_arns: Vec<String>,
    /// The logo image URI the caller supplied, if any. Not a member of any modelled
    /// portal RESPONSE shape — the logo is reported through
    /// `PortalContent$Theme$LogoLastUploaded` — so the record keeps it for the readers
    /// that serve the portal's content.
    pub logo_uri: Option<String>,
    pub portal_content: wire::PortalContent,
    pub rum_app_monitor_name: Option<String>,
    /// The TERMINAL `PublishStatus` this portal has settled in, out of
    /// `PUBLISH_STATUS_VALUES`: [`PUBLISH_STATUS_DISABLED`] for a portal that has never
    /// been published. An `..._IN_PROGRESS` status is never stored here — it belongs to
    /// a build still in flight, which [`Portal::transition`] holds and
    /// [`Portal::settle`] retires.
    pub publish_status: String,
    pub last_modified: SystemTime,
    /// Set only once the portal has been published; `None` until then, so nothing is
    /// reported for a portal that never was.
    pub last_published: Option<SystemTime>,
    pub last_published_description: Option<String>,
    /// "Error information for failed portal operations" — `None` while nothing has
    /// failed.
    pub status_exception: Option<wire::StatusException>,
    /// The publish or disable the service is building, or the last one it built; `None`
    /// until one is started. See [`Portal::settle`].
    pub transition: Option<PortalTransition>,
    /// The build this portal's domain is serving: `Some` once a publish has settled,
    /// `None` before the first one and after a disable has settled. What the domain
    /// serves is therefore a fact of the record rather than of whoever asks for it; see
    /// [`Portal::settle`].
    pub published_site: Option<PublishedSite>,
    /// When the preview this portal's last `PreviewPortal` started is due to be
    /// ready; `None` until one runs, so nothing is reported for a portal with no
    /// preview. The `PreviewStatus` is derived from it on every read, so no stored copy
    /// of it can go stale.
    pub preview_ready_at: Option<SystemTime>,
    /// When `DeletePortal` took this portal away, or `None` for a portal the account still
    /// holds.
    ///
    /// A deleted portal is a TOMBSTONE: it is gone for every reader and writer that
    /// resolves a `PortalId`
    /// ([`ComAmazonawsApigatewayv2Apigatewayv2State::live_portal`]) and its domain serves
    /// nothing, while `GetPortal` still reports its body with the terminal
    /// [`PUBLISH_STATUS_DELETED`]. See [`Portal::delete`].
    pub deleted_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl Portal {
    /// Whether `DeletePortal` has taken this portal away: a tombstone no reader but
    /// `GetPortal` resolves, and only until the removal comes due. See [`Portal::delete`]
    /// and [`Portal::is_gone`].
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Whether the removal this portal's `DeletePortal` started has come due as of `now`,
    /// after which the record names nothing at all — not even for `GetPortal`.
    ///
    /// Derived from the stored instant on every read ([`PORTAL_DELETION`]), so nothing
    /// sleeps for it and no reader has to write to make it true.
    pub fn is_gone(&self, now: SystemTime) -> bool {
        self.deleted_at
            .is_some_and(|deleted_at| now >= deleted_at + PORTAL_DELETION)
    }

    /// Take this portal away as of `now`: everything it was DOING stops, and what is left
    /// is the body `GetPortal` keeps reporting with the terminal
    /// [`PUBLISH_STATUS_DELETED`] until the removal comes due ([`Portal::is_gone`]).
    ///
    /// Stopping means: the build the service still owed it is dropped (nothing is
    /// published, disabled or previewed for a portal that is gone), its domain serves
    /// nothing, and the preview it had is unreachable because that preview was served on
    /// the same domain. Its configuration — the id and ARN, the endpoint configuration,
    /// the content, the inclusions, the tags and the instant it was last modified — is
    /// left exactly as it stood, because that is the body a reader confirming the deletion
    /// reads.
    pub fn delete(&mut self, now: SystemTime) {
        self.deleted_at = Some(now);
        self.transition = ::std::option::Option::None;
        self.published_site = ::std::option::Option::None;
        self.preview_ready_at = ::std::option::Option::None;
    }

    /// The `PublishStatus` this portal reports as of `now`.
    ///
    /// Derived rather than read off one field, so one read cannot leave a status behind for
    /// the next caller: a portal that is gone reports [`PUBLISH_STATUS_DELETED`], one with
    /// a build still in flight reports the status that build carries while it runs, one
    /// whose build the CLOCK has already made due reports the status that build settles it
    /// in, and otherwise the terminal status it is already in. Nothing is written — a
    /// caller that wants the due change materialised calls [`Portal::settle`].
    pub fn publish_status_at(&self, now: SystemTime) -> String {
        if self.is_deleted() {
            return PUBLISH_STATUS_DELETED.to_string();
        }
        match self.transition.as_ref() {
            Some(transition) if now < transition.due_at => transition.in_progress.to_string(),
            Some(transition) => transition.settled.to_string(),
            ::std::option::Option::None => self.publish_status.clone(),
        }
    }

    /// The change to this portal's PUBLICATION that is still in flight as of `now`,
    /// named by the `PublishStatus` it reports while it runs, or `None` when the
    /// publication is not moving.
    pub fn publication_change_in_flight(&self, now: SystemTime) -> Option<&'static str> {
        self.transition
            .as_ref()
            .filter(|transition| now < transition.due_at)
            .map(|transition| transition.in_progress)
    }

    /// The build of this portal's CONTENT that is still in flight as of `now`, named by
    /// the status it reports while it runs — the `PublishStatus` of a publish, which
    /// builds the content it serves, or the `PreviewStatus` of a preview — or `None` when
    /// nothing is being built.
    pub fn content_build_in_flight(&self, now: SystemTime) -> Option<&'static str> {
        let publishing = self
            .transition
            .as_ref()
            .filter(|transition| now < transition.due_at && transition.builds_content)
            .map(|transition| transition.in_progress);
        let previewing = self
            .preview_ready_at
            .filter(|ready_at| now < *ready_at)
            .map(|_| PREVIEW_STATUS_PREVIEW_IN_PROGRESS);
        publishing.or(previewing)
    }

    /// Materialise a publish or disable the CLOCK has already made due: the status it
    /// settles the portal in, and the publication it made, become the portal's own and
    /// the build is retired.
    ///
    /// Owed at the due instant whether or not anyone called, so the change is the
    /// clock's rather than this caller's. Readers never have to run it against the
    /// stored record — [`Portal::to_wire`] settles a copy — so reading a portal writes
    /// nothing.
    pub fn settle(&mut self, now: SystemTime) {
        if self
            .transition
            .as_ref()
            .is_none_or(|transition| now < transition.due_at)
        {
            return;
        }
        let Some(transition) = self.transition.take() else {
            return;
        };
        self.publish_status = transition.settled.to_string();
        match transition.published {
            Some(published) => {
                self.last_published = Some(published.at);
                self.last_published_description = published.description;
                // The build this publish made is what the portal's domain serves from
                // here on, in place of whatever an earlier publish put live.
                self.published_site = Some(published.site);
            }
            // A build that publishes nothing is a disable, which takes the publication
            // down: the portal's domain serves nothing until a later publish builds
            // another one.
            None => self.published_site = None,
        }
    }

    /// Start one server-managed build on this portal, due [`PORTAL_BUILD`] from `now`.
    ///
    /// A build starts only when what it needs is free: a publish and a disable each need
    /// the publication still, a publish and a preview each need the content build free,
    /// and a disable builds no content — so a preview and a disable run side by side,
    /// while a publish runs alone.
    ///
    /// `Err` carries the status of the build already in flight, for the caller's
    /// `ConflictException` — "the requested operation would cause a conflict with the
    /// current state of a service resource associated with the request" — and leaves the
    /// record exactly as it was. The refusal lifts of its own accord once the build this
    /// service already owes comes due.
    pub fn start_build(&mut self, now: SystemTime, build: PortalBuild) -> Result<(), &'static str> {
        self.settle(now);
        if build.needs_a_still_publication() {
            if let Some(in_flight) = self.publication_change_in_flight(now) {
                return Err(in_flight);
            }
        }
        if build.builds_content() {
            if let Some(in_flight) = self.content_build_in_flight(now) {
                return Err(in_flight);
            }
        }
        let builds_content = build.builds_content();
        let due_at = now + PORTAL_BUILD;
        match build {
            PortalBuild::Publish { description } => {
                self.transition = Some(PortalTransition {
                    in_progress: PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
                    settled: PUBLISH_STATUS_PUBLISHED,
                    due_at,
                    published: Some(PortalPublication {
                        at: due_at,
                        description,
                        // The content as it stands NOW is what this publish builds: the
                        // call that starts it is what names the version going live.
                        site: PublishedSite {
                            at: due_at,
                            content: self.portal_content.clone(),
                            included_portal_product_arns: self
                                .included_portal_product_arns
                                .clone(),
                            logo_uri: self.logo_uri.clone(),
                        },
                    }),
                    builds_content,
                });
            }
            PortalBuild::Disable => {
                // The take-down the service then carries out: the portal reports
                // `DISABLE_IN_PROGRESS` until it comes due. What the portal's domain
                // SERVES does not wait for that — see
                // [`PortalTransition::revokes_publication`].
                self.transition = Some(PortalTransition {
                    in_progress: PUBLISH_STATUS_DISABLE_IN_PROGRESS,
                    settled: PUBLISH_STATUS_DISABLED,
                    due_at,
                    published: ::std::option::Option::None,
                    builds_content,
                });
            }
            PortalBuild::Preview => self.preview_ready_at = Some(due_at),
        }
        Ok(())
    }

    /// Project the record onto the modelled `PortalSummary` shape as of `now` — the
    /// shape that carries every member the portal readers report, so each of them
    /// projects from this one value.
    pub fn to_wire(&self, now: SystemTime) -> wire::PortalSummary {
        // A read must not change what a later caller sees, so the projection settles a
        // COPY: a build the clock has already made due is reported as settled while the
        // stored record is left for the next writer to settle.
        let mut settled = self.clone();
        settled.settle(now);
        // Every build the clock owed is retired above, so a transition still held is one
        // still in flight and the portal reports the status it carries while it runs.
        let publish_status = settled.publish_status_at(now);
        // The portal body ALWAYS carries a `Preview`, and it carries NOTHING when no
        // preview has been built: a reader that asks whether a preview exists is answered
        // with the empty structure rather than with an absent member. `Preview$PreviewUrl`
        // and `Preview$PreviewStatus` describe a preview that was built, so neither is
        // reported until `PreviewPortal` has built one.
        let preview = match settled.preview_ready_at {
            Some(ready_at) => wire::Preview {
                preview_status: Some(
                    if now < ready_at {
                        PREVIEW_STATUS_PREVIEW_IN_PROGRESS
                    } else {
                        PREVIEW_STATUS_PREVIEW_READY
                    }
                    .to_string(),
                ),
                preview_url: Some(portal_preview_url(&settled.endpoint_configuration)),
                // Nothing failed, so there is no exception to explain.
                status_exception: ::std::option::Option::None,
            },
            ::std::option::Option::None => wire::Preview::default(),
        };
        wire::PortalSummary {
            authorization: settled.authorization.clone(),
            endpoint_configuration: settled.endpoint_configuration.clone(),
            included_portal_product_arns: settled.included_portal_product_arns.clone(),
            last_modified: to_timestamp(settled.last_modified),
            last_published: settled.last_published.map(to_timestamp),
            last_published_description: settled.last_published_description.clone(),
            portal_arn: settled.portal_arn.clone(),
            portal_content: settled.portal_content.clone(),
            portal_id: settled.portal_id.clone(),
            preview: Some(preview),
            publish_status: Some(publish_status),
            rum_app_monitor_name: settled.rum_app_monitor_name.clone(),
            status_exception: settled.status_exception.clone(),
            tags: Some(
                settled
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }

    /// The build this portal's domain is SERVING as of `now`, or `None` when it serves
    /// nothing: a portal nothing has published yet, and one whose publication a
    /// `DisablePortal` has taken down.
    ///
    /// Derived from the clock the way [`Portal::to_wire`] derives the status it reports, so
    /// asking what the domain serves writes nothing: a publish the clock has already made
    /// due is live here while the stored record is left for the next writer to settle. Only
    /// the publication is cloned — the answer itself — so one visit costs the bytes it
    /// serves rather than a copy of the whole record.
    ///
    /// The two builds that can be in flight are READ DIFFERENTLY, because one grants access
    /// and the other revokes it. A publish still being built has put nothing live yet, so
    /// what is served is whatever the last settled build left. A take-down is a revocation
    /// of public access, and access the service has been told to revoke is not still granted
    /// while the service finishes removing the publication: the domain stops serving from
    /// the instant `DisablePortal` is accepted, and the `PublishStatus` the portal reports
    /// until that build comes due says what the service has left to do rather than that the
    /// site is still up. So a caller that revokes a portal and probes it straight away
    /// cannot be served the build it just revoked.
    pub fn live_site(&self, now: SystemTime) -> Option<PublishedSite> {
        match self.transition.as_ref() {
            // A revocation, in flight or already due: nothing is served either way.
            Some(transition) if transition.revokes_publication() => None,
            // A build still in flight that makes a publication changes nothing yet: what
            // is live is whatever the last settled build left live.
            Some(transition) if now < transition.due_at => self.published_site.clone(),
            // A build the clock has made due: the site that build made.
            Some(transition) => transition
                .published
                .as_ref()
                .map(|published| published.site.clone()),
            None => self.published_site.clone(),
        }
    }

    /// Whether the preview this portal's last `PreviewPortal` built is ready as of `now` —
    /// the `PREVIEW_READY` of [`PREVIEW_STATUS_PREVIEW_READY`], derived from the same due
    /// instant [`Portal::to_wire`] reports the preview's status from.
    pub fn preview_is_ready(&self, now: SystemTime) -> bool {
        self.preview_ready_at
            .is_some_and(|ready_at| now >= ready_at)
    }
}

impl CreatedInOrder for Portal {
    fn creation_order(&self) -> u64 {
        self.creation_order
    }
}
