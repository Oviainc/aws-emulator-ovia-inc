// SPDX-License-Identifier: CC0-1.0

//! The REGION a record belongs to, and every lookup scoped by it.
//!
//! Every resource this service mints is a regional typed resource: `CreateMicrovmImage`
//! and `RunMicrovm` stamp the region the request was signed for into the ARN they mint
//! (`crate::state::microvm_image_arn`, `crate::state::microvm_arn`), and that region is
//! the one endpoint the record can be addressed through. A client pinned to another
//! region reaches another endpoint of the service, which holds its own records and knows
//! nothing of this one — so an identifier minted here is answered EXACTLY as an
//! identifier nobody ever created there: refused, and absent from that region's
//! listings.
//!
//! The store is therefore PARTITIONED: each region has its own [`RegionalStore`], and
//! every read, list and mutation goes through one of the accessors below with the region
//! [`crate::state::request_region`] derived from the request. A region nothing was ever
//! created in holds no records, which is what makes the first call for it an absence
//! rather than a hit on somebody else's record.
//!
//! The one lookup that is NOT scoped is the MicroVM endpoint's
//! ([`ComAmazonawsLambdamicrovmsLambdamicrovmsState::microvm_anywhere`]): that address is
//! the MicroVM's own data plane rather than this service's regional control plane, and a
//! caller reaches it by the URL `RunMicrovm` published, which carries the MicroVM's id
//! and no region at all.

use std::collections::BTreeMap;
use std::time::Instant;

use super::*;

/// The records of ONE region: everything a request signed for that region can address.
///
/// Each map is bounded exactly as the store as a whole documents
/// ([`ComAmazonawsLambdamicrovmsLambdamicrovmsState`]), and the bounds are per region
/// because the quotas they stand for are: AWS publishes a service quota per account AND
/// region.
#[derive(Debug, Default)]
pub struct RegionalStore {
    /// Images by `name` (`ImageName` is unique within the account and region).
    pub images: BTreeMap<String, MicrovmImage>,
    /// `imageArn` -> image `name`, so an identifier resolves by ARN or by name.
    pub arn_index: BTreeMap<String, String>,
    /// MicroVMs by `microvmId`. The ids sort in creation order, which is the order
    /// `ListMicrovms` pages through.
    pub microvms: BTreeMap<String, Microvm>,
    /// MicroVM ARN -> `microvmId`, so an identifier resolves by ARN or by id.
    pub microvm_arn_index: BTreeMap<String, String>,
    /// `clientToken` -> the resource that token already minted. A `clientToken` names a
    /// request made against one regional endpoint, so a replay of it is answered only
    /// there.
    pub idempotency: BTreeMap<String, IdempotencyRecord>,
}

impl ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// The records `region` holds, or `None` when nothing was ever created there.
    ///
    /// A read answers `None` rather than creating a partition, so a reporting operation
    /// for a region nothing has used writes nothing at all.
    pub fn in_region(&self, region: &str) -> Option<&RegionalStore> {
        self.regions.get(region)
    }

    /// The records `region` holds, for mutation — created empty on the first write that
    /// region takes.
    ///
    /// A write that would open a region this store holds none is refused BEFORE it gets
    /// here, so the partitions stay bounded; see [`Self::would_open_region`].
    pub fn in_region_mut(&mut self, region: &str) -> &mut RegionalStore {
        self.regions.entry(region.to_string()).or_default()
    }

    /// Whether storing a record for `region` would open a partition this store does not
    /// hold while it is already at [`MAX_REGIONS`] — the one caller-driven dimension of
    /// the store, so an operation that would open one answers its modelled quota refusal
    /// instead of growing the store ([`too_many_regions_message`]).
    pub fn would_open_region(&self, region: &str) -> bool {
        !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS
    }

    /// The images `region` holds, in `name` order — the order a listing pages them in.
    pub fn images_in(&self, region: &str) -> impl Iterator<Item = &MicrovmImage> {
        self.in_region(region)
            .into_iter()
            .flat_map(|regional| regional.images.values())
    }

    /// How many images `region` holds — what [`MAX_MICROVM_IMAGES`] caps.
    pub fn image_count(&self, region: &str) -> usize {
        self.in_region(region)
            .map_or(0, |regional| regional.images.len())
    }

    /// The image `region` holds under `name`.
    pub fn image_named(&self, region: &str, name: &str) -> Option<&MicrovmImage> {
        self.in_region(region)?.images.get(name)
    }

    /// The image `region` holds under `name`, for mutation.
    pub fn image_named_mut(&mut self, region: &str, name: &str) -> Option<&mut MicrovmImage> {
        self.regions.get_mut(region)?.images.get_mut(name)
    }

    /// Persist `image` in `region`, with the ARN index that resolves its identifier.
    pub fn insert_image(&mut self, region: &str, image: MicrovmImage) {
        let regional = self.in_region_mut(region);
        regional
            .arn_index
            .insert(image.image_arn.clone(), image.name.clone());
        regional.images.insert(image.name.clone(), image);
    }

    /// The name of the image `identifier` names in `region`, resolving either form
    /// `MicrovmImageIdentifier` documents ("The ARN or ID of the MicroVmImage").
    ///
    /// An ARN resolves ONLY through [`RegionalStore::arn_index`], so it names the image
    /// whose stored `imageArn` it IS. Matching just its resource segment would resolve
    /// another partition's, region's or account's ARN to a local image of the same name.
    pub fn resolve_image_name(&self, region: &str, identifier: &str) -> Option<String> {
        let regional = self.in_region(region)?;
        if identifier.starts_with("arn:") {
            return regional.arn_index.get(identifier).cloned();
        }
        regional
            .images
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// What `identifier` resolves to in `region`, which is what says whether the caller
    /// gets the image, a not-found or a refusal.
    ///
    /// `MicrovmImageIdentifier` is "The ARN or ID of the MicroVmImage", so the
    /// identifier is resolved as an id this region holds FIRST and read as an ARN only
    /// when it names no held image: a value that is neither — a bare string that is no
    /// id of this region and no ARN — addresses nothing this API can name, so it is
    /// [`ImageReference::Malformed`] rather than absent.
    pub fn resolve_image_reference(&self, region: &str, identifier: &str) -> ImageReference {
        if let Some(name) = self.resolve_image_name(region, identifier) {
            return ImageReference::Resolved(name);
        }
        if ::service_common::arn::parse(identifier).is_some() {
            return ImageReference::Absent;
        }
        ImageReference::Malformed
    }

    /// Whether the calling account is granted the MicroVM image `identifier` names, in
    /// the region the request was signed for.
    ///
    /// This is the grant a call on a `MicrovmImageIdentifier` is judged against, and it
    /// is an ACCOUNT-level question: the images an account may address are the ones of
    /// its OWN image namespace ([`is_own_microvm_image_arn`]), whether or not a record
    /// exists under the identifier right now and whichever region minted it — so a
    /// record the account created and then deleted, and one it created through another
    /// region's endpoint, are both identifiers it is permitted to ask about. An
    /// identifier of another namespace — AWS's own
    /// ([`is_managed_microvm_image_arn`]), another account's, another partition's,
    /// another vendor's, or a value spelling this service's resource in a form
    /// `MicrovmImageArn` does not admit — names nothing this account was ever granted.
    ///
    /// WHERE the record is then looked for is this region's own store, which is what
    /// makes an image of another region an ABSENCE here rather than a hit.
    pub fn grants_microvm_image(&self, region: &str, identifier: &str) -> bool {
        self.resolve_image_name(region, identifier).is_some() || is_own_microvm_image_arn(identifier)
    }

    /// What a customer-facing call addressed at the `MicrovmImageIdentifier`
    /// `identifier` answers in `region`, AUTHORIZATION first.
    ///
    /// The service decides access before it looks anything up, so it never reports the
    /// absence of an identifier the caller is not permitted to read: an identifier
    /// outside the calling account's own image namespace
    /// ([`Self::grants_microvm_image`]) is [`ImageAccess::Denied`] whether or not it
    /// names a resource, and only an identifier of that namespace gets an answer about
    /// existence IN THIS REGION — [`ImageAccess::Granted`] while this region holds the
    /// record and [`ImageAccess::Absent`] when it does not, which is the one answer an
    /// identifier nobody created here and one minted through another region's endpoint
    /// both get. A value that is no identifier of this API at all
    /// ([`ImageReference::Malformed`]) is a bad REQUEST, which is decided on the input
    /// rather than on any grant.
    pub fn resolve_image_access(&self, region: &str, identifier: &str) -> ImageAccess {
        match self.resolve_image_reference(region, identifier) {
            ImageReference::Malformed => ImageAccess::Malformed,
            reference => match (self.grants_microvm_image(region, identifier), reference) {
                (false, _) => ImageAccess::Denied,
                (true, ImageReference::Resolved(name)) => ImageAccess::Granted(name),
                (true, _) => ImageAccess::Absent,
            },
        }
    }

    /// The image `identifier` names in `region`, by ARN or by name.
    pub fn image(&self, region: &str, identifier: &str) -> Option<&MicrovmImage> {
        let name = self.resolve_image_name(region, identifier)?;
        self.image_named(region, &name)
    }

    /// The image `identifier` names in `region`, for mutation.
    pub fn image_mut(&mut self, region: &str, identifier: &str) -> Option<&mut MicrovmImage> {
        let name = self.resolve_image_name(region, identifier)?;
        self.image_named_mut(region, &name)
    }

    /// The version of the base image `base_image_arn` names, for a request that named
    /// none.
    ///
    /// `baseImageVersion` is "the specific version of the base MicroVM image to use",
    /// so the service resolves one rather than recording none: the base image's own
    /// active version when this region holds it, and otherwise the initial version,
    /// [`FIRST_IMAGE_VERSION`].
    ///
    /// A reference into the AWS-managed catalogue takes that same initial version rather
    /// than the newest GENERATION the catalogue publishes for it
    /// ([`latest_managed_base_image_version`]), so the catalogue is deliberately NOT
    /// consulted here: the recorded create answers the `major.minor` initial version for
    /// an unpinned build on a managed base image, while a build that pins a generation
    /// records the generation it pinned.
    pub fn resolve_base_image_version(&self, region: &str, base_image_arn: &str) -> String {
        if let Some(active) = self
            .image(region, base_image_arn)
            .and_then(|image| image.latest_active_image_version.clone())
        {
            return active;
        }
        FIRST_IMAGE_VERSION.to_string()
    }

    /// The `microvmId` the `identifier` names in `region`, resolving either form
    /// `MicrovmIdentifier` documents ("The ARN or ID of the MicroVm").
    ///
    /// An ARN resolves ONLY through [`RegionalStore::microvm_arn_index`], so it names
    /// the MicroVM whose stored ARN it IS. Matching just its resource segment would
    /// resolve another partition's, region's or account's ARN to a local MicroVM of the
    /// same id.
    pub fn resolve_microvm_id(&self, region: &str, identifier: &str) -> Option<String> {
        let regional = self.in_region(region)?;
        if identifier.starts_with("arn:") {
            return regional.microvm_arn_index.get(identifier).cloned();
        }
        regional
            .microvms
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// The MicroVM `identifier` names in `region`, by ARN or by id.
    pub fn microvm(&self, region: &str, identifier: &str) -> Option<&Microvm> {
        let id = self.resolve_microvm_id(region, identifier)?;
        self.in_region(region)?.microvms.get(&id)
    }

    /// The MicroVM `identifier` names in `region`, for mutation.
    pub fn microvm_mut(&mut self, region: &str, identifier: &str) -> Option<&mut Microvm> {
        let id = self.resolve_microvm_id(region, identifier)?;
        self.regions.get_mut(region)?.microvms.get_mut(&id)
    }

    /// The MicroVMs `region` holds, in `microvmId` order — creation order, which is the
    /// order `ListMicrovms` pages through.
    pub fn microvms_in(&self, region: &str) -> impl Iterator<Item = &Microvm> {
        self.in_region(region)
            .into_iter()
            .flat_map(|regional| regional.microvms.values())
    }

    /// Persist `vm` in `region`, with the ARN index that resolves its identifier.
    pub fn insert_microvm(&mut self, region: &str, vm: Microvm) {
        let regional = self.in_region_mut(region);
        regional
            .microvm_arn_index
            .insert(vm.microvm_arn.clone(), vm.microvm_id.clone());
        regional.microvms.insert(vm.microvm_id.clone(), vm);
    }

    /// How many MicroVMs of `region` are not terminated — what [`MAX_ACTIVE_MICROVMS`]
    /// caps.
    pub fn active_microvm_count(&self, region: &str) -> usize {
        self.microvms_in(region)
            .filter(|vm| vm.state != MICROVM_STATE_TERMINATED)
            .count()
    }

    /// What `client_token` already minted on `operation` in `region`, if anything.
    pub fn minted_by(
        &self,
        region: &str,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<&MintedResource> {
        let key = idempotency_key(operation, client_token?);
        Some(&self.in_region(region)?.idempotency.get(&key)?.minted)
    }

    /// Record what `client_token` minted on `operation` in `region`, evicting the record
    /// closest to expiry when that region is at [`MAX_IDEMPOTENCY_RECORDS`].
    pub fn record_minted(
        &mut self,
        region: &str,
        operation: &str,
        client_token: &str,
        minted: MintedResource,
        now: Instant,
    ) {
        let key = idempotency_key(operation, client_token);
        let regional = self.in_region_mut(region);
        if !regional.idempotency.contains_key(&key)
            && regional.idempotency.len() >= MAX_IDEMPOTENCY_RECORDS
        {
            let evict = regional
                .idempotency
                .iter()
                .min_by_key(|(_, record)| record.expires_at)
                .map(|(token, _)| token.clone());
            if let Some(token) = evict {
                regional.idempotency.remove(&token);
            }
        }
        regional.idempotency.insert(
            key,
            IdempotencyRecord {
                minted,
                expires_at: now + IDEMPOTENCY_RETENTION,
            },
        );
    }

    /// The MicroVM `identifier` names in WHATEVER region holds it, with that region.
    ///
    /// Only the MicroVM's own published endpoint resolves an identifier this way, and
    /// only because that address carries no region (see this module's header); every
    /// modelled operation of the control plane goes through [`Self::microvm`] instead.
    pub(crate) fn microvm_anywhere(&self, identifier: &str) -> Option<(&str, &Microvm)> {
        self.regions.keys().find_map(|region| {
            let vm = self.microvm(region, identifier)?;
            Some((region.as_str(), vm))
        })
    }
}
