// SPDX-License-Identifier: CC0-1.0

//! The metadata records of this service's data-asset surface: form types, asset types,
//! assets, glossaries and glossary terms, with the refusals each miss answers with.

use super::*;

/// Whether an `UpdateWorkflow` request names a member of the workflow to change at all.
///
/// Every member of that request other than the `Name` addressing the workflow is optional,
/// so a request carrying none of them asks for no change: nothing is written, so the
/// workflow's `LastModifiedOn` stays where the last write that did change something put it
/// — the same way an `UpdateCrawler` naming no member leaves its crawler's `LastUpdated`
/// alone ([`crawler_update_names_a_change`]). Written as an exhaustive destructuring so a
/// member added to the request shape cannot be silently left out of the answer.
pub fn workflow_update_names_a_change(input: &UpdateWorkflowInput) -> bool {
    let UpdateWorkflowInput {
        name: _,
        description,
        default_run_properties,
        max_concurrent_runs,
    } = input;

    description.is_some() || default_run_properties.is_some() || max_concurrent_runs.is_some()
}

/// The message the catalog's metadata form type surface refuses an identifier it holds no
/// form type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// This surface's refusal text is itself a JSON document carrying the sentence under a
/// `Message` key, so that text — braces and all — is what the modelled member carries, and
/// a client reading `Message` reads the document rather than a bare sentence. It is NOT an
/// envelope this emulator adds around the member: the member's own value is this text, as
/// the surface words it. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a form type identifier
/// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn form_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Metadata form type {identifier} not found."))
}

/// `sentence` as the business-catalog surface puts a refusal on the wire: a JSON document
/// carrying it under a `Message` key.
///
/// One copy, shared by every message builder of that surface below, so the DOCUMENT shape
/// cannot drift between them while each keeps the sentence its own refusal words. Rendered
/// by `serde_json`, so a sentence carrying a quote or a backslash still yields a
/// well-formed document.
pub(super) fn catalog_refusal_document(sentence: &str) -> String {
    serde_json::json!({ "Message": sentence }).to_string()
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type one above does
/// ([`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the modelled
/// member carries, and the resource is named as the service names it, a business glossary.
/// `serde_json` renders it, so an identifier carrying a quote or a backslash still yields a
/// well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryId`
/// ([`ComAmazonawsGlueAwsglueState::glossary`]) — the glossary's own reader and the term
/// create that puts a term inside one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn glossary_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary {identifier} not found."))
}

/// The message every refusal of a glossary NAME this account already holds a glossary
/// under carries: the value of that refusal's `AlreadyExistsException$Message`.
///
/// Worded the way this surface words every refusal ([`glossary_not_found_message`],
/// [`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the
/// modelled member carries. The sentence names the glossary the write asked for, in
/// single quotes, which is its one variable part; `serde_json` renders it, so a name
/// carrying a quote or a backslash still yields a well-formed document.
///
/// One copy, shared by the create that would mint a second glossary under a stored name
/// and the update that would rename one onto it, so the two cannot disagree about how the
/// one collision is worded.
pub fn glossary_already_exists_message(name: &str) -> String {
    catalog_refusal_document(&format!("A glossary with name '{name}' already exists."))
}

/// A stored form type: the schema for structured metadata `PutFormType` registers in the
/// Data Catalog.
///
/// Every member of the modelled form type is held here — its id, its name and its schema
/// — and nothing else: the model gives a form type no status, owner or timestamp. The
/// name and the schema come from the request, and the id is derived from the name (see
/// [`Self::id`]).
#[derive(Debug, Clone)]
pub struct FormTypeEntry {
    /// `FormTypeId`: the identifier a client addresses this form type by, which is the
    /// form type's own NAME.
    ///
    /// Not an opaque per-record id: the catalog can word a refusal for a form type it no
    /// longer holds with that form type's name while all it was given is the identifier,
    /// so the identifier it hands out carries the name. `FormTypeName`'s `length 1..128`
    /// sits inside `FormTypeId`'s `length 1..256`, so every name is a usable identifier,
    /// and because the name is also the namespace's key, two form types never share one
    /// identifier and an update under a name keeps the identifier an earlier put reported.
    pub id: String,
    /// The form type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::form_types`].
    pub name: String,
    /// "The Smithy IDL schema definition for the form type", stored exactly as sent: the
    /// catalog reports back the schema text the client wrote, not a re-rendering of it.
    pub schema: String,
}

impl FormTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and schema, under a freshly minted id.
    pub fn new(name: String, schema: String) -> Self {
        Self {
            id: name.clone(),
            name,
            schema,
        }
    }
}

/// How many asset type ids this process has minted. A nonce source for
/// [`new_asset_type_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
pub(super) static MINTED_ASSET_TYPE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `AssetTypeId`: the opaque identifier the Data Catalog assigns an asset type
/// when `PutAssetType` first stores it under a name.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, the same shape a form type id has
/// and inside `AssetTypeId`'s `length 1..256`. The bits come from [`mint_hex_id`], so two
/// puts landing in the same nanosecond cannot be given the same id. It is minted ONCE,
/// when the name is first stored, and kept on the [`AssetTypeEntry`] through every later
/// update, so every read of that asset type reports the same id and two asset types never
/// share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_asset_type_id() -> String {
    mint_hex_id(&MINTED_ASSET_TYPE_IDS, CATALOG_ID_WORDS)
}

/// A stored asset type: the structure of an asset `PutAssetType` registers in the Data
/// Catalog, as the set of forms an asset of that type includes.
///
/// Every member of the modelled asset type is held here — its id, its name and its forms
/// — and nothing else: the model gives an asset type no status, owner or timestamp. The
/// name and the forms come from the request; the id is the one server-set member, minted
/// by [`new_asset_type_id`] when the name is first stored.
#[derive(Debug, Clone)]
pub struct AssetTypeEntry {
    /// `AssetTypeId`, minted by [`new_asset_type_id`] at the first put under this name and
    /// unchanged by every later one, so a caller that kept the id from an earlier put can
    /// still address the asset type after it has been updated.
    pub id: String,
    /// The asset type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::asset_types`].
    pub name: String,
    /// "The forms that make up the asset type, keyed by form name", each value the
    /// identifier of the form type that defines that form's schema, stored exactly as the
    /// put sent it (`AssetTypeFormReference$FormTypeIdentifier`, the reference's only
    /// member) so a read reports the identifier the client wrote. It resolved against this
    /// store's form types when it was stored
    /// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so it is either a
    /// minted `FormTypeId` or the `FormTypeName` the form type was registered under.
    /// Ordered by form name, so two reads of one asset type report the forms in one order.
    pub forms: BTreeMap<String, String>,
}

impl AssetTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and form references, under a freshly minted id.
    pub fn new(name: String, forms: BTreeMap<String, String>) -> Self {
        Self {
            id: new_asset_type_id(),
            name,
            forms,
        }
    }

    /// This record's forms as the modelled `AssetTypeFormsMap`, which is what every
    /// operation that reports an asset type answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetTypeFormReference> {
        self.forms
            .iter()
            .map(|(form_name, form_type_id)| {
                (
                    form_name.clone(),
                    AssetTypeFormReference {
                        form_type_identifier: form_type_id.clone(),
                    },
                )
            })
            .collect()
    }
}

/// The message the catalog's asset type surface refuses an identifier it holds no asset
/// type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type, asset and business
/// glossary ones do ([`form_type_not_found_message`]): the refusal text is itself a JSON
/// document carrying the sentence under a `Message` key, so that text — braces and all —
/// is what the modelled member carries, and a client reading `Message` reads the document
/// rather than a bare sentence. `serde_json` renders it, so an identifier carrying a quote
/// or a backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an asset type identifier
/// ([`ComAmazonawsGlueAwsglueState::asset_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn asset_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset type {identifier} not found."))
}

/// The message the catalog's asset surface refuses an identifier it holds no asset under
/// with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type and business
/// glossary ones above do ([`form_type_not_found_message`]): the refusal text is itself a
/// JSON document carrying the sentence under a `Message` key, so that text — braces and
/// all — is what the modelled member carries. This surface words the sentence the other
/// way round from those two, naming the resource kind and then the identifier that
/// resolved to nothing. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an `AssetId`
/// ([`ComAmazonawsGlueAwsglueState::asset`]) — the asset's own reader and the glossary
/// term association that addresses one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn asset_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset not found: {identifier}"))
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary TERM under with: the value of that refusal's
/// `EntityNotFoundException$Message`.
///
/// Worded the way the glossary itself words a miss ([`glossary_not_found_message`]) — the
/// refusal text is a JSON document carrying the sentence under a `Message` key — with the
/// resource named as the service names it, a business glossary term.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryTermId`
/// ([`ComAmazonawsGlueAwsglueState::glossary_term`]), so two of them cannot disagree about
/// how the one miss is worded.
pub fn glossary_term_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary term {identifier} not found."))
}

/// The message the catalog refuses a request that asks to associate a glossary term this
/// store holds none of with: the value of that refusal's `InvalidInputException$Message`.
///
/// The refusal is the ASSET surface's reading of a reference it cannot resolve, so it is
/// worded the way that surface words a miss ([`asset_not_found_message`]) rather than the
/// way the term's own reader words one: the text is a JSON document carrying the sentence
/// under a `Message` key, and the sentence names the kind of thing that was looked for and
/// then the identifier that resolved to nothing.
///
/// `identifier` is the text's one variable part and is the entry of the request's
/// identifier list that resolved to nothing, taken verbatim.
pub fn unassociable_glossary_term_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Glossary term not found: {identifier}"))
}

/// A stored asset: one catalog entity `PutAsset` registers as an instance of an asset
/// type, carrying that entity's metadata in forms keyed by form name.
///
/// Every member of the modelled asset is held here — its identifier, the asset type it is
/// an instance of, its name, its description, its forms and the two instants the SERVER
/// stamps on it — so a read reports the asset the last write stored and nothing invented on
/// the way out. The identifier, the asset type, the name, the description and the forms
/// come from the request; the two instants are the clock's.
#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// `AssetId`: the identifier the PUT's caller chose, which is also this record's key in
    /// [`ComAmazonawsGlueAwsglueState::assets`] and the address every later operation
    /// carries. Stored exactly as sent.
    pub id: String,
    /// "The identifier of the asset type for the asset" (`PutAssetRequest$AssetTypeId`),
    /// stored exactly as the put addressed it — either the `AssetTypeId` the catalog minted
    /// for that asset type or the `AssetTypeName` it was registered under, the two forms
    /// [`ComAmazonawsGlueAwsglueState::asset_type_by_reference`] resolves — because it
    /// resolved against this store when it was stored, so a read reports the identifier the
    /// client wrote.
    pub asset_type_id: String,
    /// "The name of the asset" (`PutAssetRequest$Name`), as the latest put sent it.
    pub name: String,
    /// "The description of the asset", or `None` when the latest put carried none: the
    /// member is optional and the model gives it no default.
    pub description: Option<String>,
    /// "The forms to set on the asset, keyed by form name", each entry the form type that
    /// defines the form's schema and the form's JSON content, stored exactly as the put
    /// sent them. A put SETS the whole map, so a form the latest put did not send is not
    /// held. Ordered by form name, so two reads of one asset report its forms in one order.
    pub forms: BTreeMap<String, AssetFormEntry>,
    /// "The timestamp at which the asset was created": the instant the put that first
    /// stored this identifier was served, kept by every later put under it so every read
    /// reports the same creation instant.
    pub created_at: SystemTime,
    /// "The timestamp at which the asset was last updated" (`GetAssetOutput$UpdatedAt`):
    /// the instant the latest put that changed this record was served, which for a record
    /// no put has updated yet is [`Self::created_at`].
    pub updated_at: SystemTime,
    /// The `ClientToken` of the put that last wrote this record, when it carried one.
    ///
    /// `PutAssetRequest$ClientToken` is "a unique, case-sensitive identifier that you
    /// provide to ensure the idempotency of the request", so a retry carrying the token of
    /// the put already stored is answered with that record rather than writing it again.
    pub client_token: Option<String>,
    /// The glossary terms associated with this asset, as the `GlossaryTermId` of each, in
    /// the order they were first associated.
    ///
    /// A SET, held in insertion order: associating a term already held changes nothing, so
    /// the same association sent twice leaves one entry, and the order two reads report is
    /// the order the associations happened in. The ids are stored exactly as the
    /// association sent them and are NOT resolved against the term namespace afterwards,
    /// because the association is to the id: a term deleted after being associated leaves
    /// its id on the asset, which is why this is not a list of references into
    /// [`ComAmazonawsGlueAwsglueState::glossary_terms`]. An asset no association has
    /// reached holds none, and the list never grows past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A put under an identifier already stored does not touch it: `PutAsset` sets an
    /// asset's forms and carries no glossary term member at all, so the association
    /// surface is the only writer.
    pub glossary_terms: Vec<String>,
}

impl AssetEntry {
    /// The record a put stores for an identifier this store does not hold yet: the
    /// request's members, stamped with the instant `now` the put was served at. A fresh
    /// asset carries no glossary term: the put models no member to send one with.
    pub fn new(
        id: String,
        asset_type_id: String,
        name: String,
        description: Option<String>,
        forms: BTreeMap<String, AssetFormEntry>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            asset_type_id,
            name,
            description,
            forms,
            created_at: now,
            updated_at: now,
            client_token,
            glossary_terms: Vec::new(),
        }
    }

    /// This record's glossary terms as the modelled `GlossaryTermIdList`, which is what
    /// every operation that reports an asset's associations answers with, or `None` when
    /// the asset carries none.
    ///
    /// An asset with no association reports no list rather than an empty one: the modelled
    /// list is "len 1..10", so an empty list is not a value this member can carry.
    pub fn glossary_terms_list(&self) -> Option<Vec<String>> {
        if self.glossary_terms.is_empty() {
            None
        } else {
            Some(self.glossary_terms.clone())
        }
    }

    /// Merge `identifiers` into this asset's glossary term set, reporting the terms the
    /// asset carries afterwards, or `None` when the merge would take it past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A union, not a replacement: an id the asset already holds keeps its place and is
    /// not repeated, and an id it does not hold is appended, so the terms an earlier
    /// association added survive a later one and an association sent twice is the same
    /// state as sending it once. The bound is checked against the UNION, so re-associating
    /// terms an asset already holds is served however full it is.
    ///
    /// The ceiling is also what bounds the WORK: a request can only carry as many new ids
    /// as the union admits, so the walk stops at the first one past it rather than merging
    /// the whole request first — one call cannot spend more comparisons or more memory than
    /// [`MAX_ASSET_GLOSSARY_TERMS`] allows, whatever it sends.
    pub fn associate_glossary_terms(&mut self, identifiers: &[String]) -> Option<Vec<String>> {
        let mut merged = self.glossary_terms.clone();
        for identifier in identifiers {
            if merged.iter().any(|held| held == identifier) {
                continue;
            }
            if merged.len() == MAX_ASSET_GLOSSARY_TERMS {
                return None;
            }
            merged.push(identifier.clone());
        }
        self.glossary_terms = merged;
        Some(self.glossary_terms.clone())
    }

    /// This record's forms as the modelled `AssetFormMap`, which is what every operation
    /// that reports an asset's forms answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetFormEntry> {
        self.forms
            .iter()
            .map(|(form_name, form)| (form_name.clone(), form.clone()))
            .collect()
    }
}

/// How many glossary ids this process has minted. A nonce source for
/// [`new_glossary_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
pub(super) static MINTED_GLOSSARY_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryId`: the opaque identifier the Data Catalog assigns a business
/// glossary when `CreateGlossary` stores it.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has; `GlossaryId` carries no length or pattern
/// constraint of its own. The bits come from [`mint_hex_id`], as a database id's do, so
/// two creates landing in the same nanosecond cannot be given the same id. It is minted
/// ONCE, at create, and kept on the [`GlossaryEntry`] through every later update — a
/// rename included — so every read of that glossary reports the same id and two
/// glossaries never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_id() -> String {
    mint_hex_id(&MINTED_GLOSSARY_IDS, CATALOG_ID_WORDS)
}

/// A stored business glossary: the container for glossary terms `CreateGlossary`
/// registers.
///
/// Every member of the modelled glossary is held here — its id, its name and its
/// description — and nothing else: the model gives a glossary no status, owner or
/// timestamp. The name and the description come from the request; the id is the one
/// server-set member, minted by [`new_glossary_id`] when the glossary is stored.
#[derive(Debug, Clone)]
pub struct GlossaryEntry {
    /// `GlossaryId`, minted by [`new_glossary_id`] at create and unchanged afterwards, so
    /// a caller that kept the id from the create can still address the glossary after it
    /// has been updated.
    pub id: String,
    /// "The name of the glossary" (`CreateGlossaryRequest$Name`), unique over this
    /// account's glossaries. Stored exactly as sent — the model documents no case fold
    /// for `GlossaryName`.
    pub name: String,
    /// "The description of the glossary", absent when the create carried none: the member
    /// is optional and the model gives it no default, so a glossary created without one
    /// reports none rather than an empty string.
    pub description: Option<String>,
    /// The `ClientToken` of the request that created this glossary, when it carried one.
    ///
    /// `CreateGlossaryRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryEntry {
    /// The record a create stores: the name and description it carried, under a freshly
    /// minted id, beside the idempotency token a retry of that create is recognised by.
    pub fn new(name: String, description: Option<String>, client_token: Option<String>) -> Self {
        Self {
            id: new_glossary_id(),
            name,
            description,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossary` request: every member comes from the request
    /// except the id, which the catalog assigns.
    pub fn from_create_input(input: &CreateGlossaryInput) -> Self {
        Self::new(
            input.name.clone(),
            input.description.clone(),
            input.client_token.clone(),
        )
    }

    /// Apply an `UpdateGlossary` request to this record.
    ///
    /// Every updatable member is optional on the request ("The updated name of the
    /// glossary", "The updated description of the glossary"), so the ones the request
    /// NAMES replace what is stored and the ones it omits are left where the last write
    /// that named them put them: an update naming one member changes that one alone.
    ///
    /// [`Self::id`] is not a member of the request and so cannot move, and
    /// [`Self::client_token`] keeps the CREATE's token, which is what a retry of that
    /// create is recognised by and not something an update of the glossary restates.
    pub fn apply_update(&mut self, input: &UpdateGlossaryInput) {
        if let Some(name) = input.name.clone() {
            self.name = name;
        }
        if let Some(description) = input.description.clone() {
            self.description = Some(description);
        }
    }
}

/// How many glossary term ids this process has minted. A nonce source for
/// [`new_glossary_term_id`], exactly as [`MINTED_GLOSSARY_IDS`] is for a glossary id.
pub(super) static MINTED_GLOSSARY_TERM_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryTermId`: the opaque identifier the Data Catalog assigns a glossary term
/// when `CreateGlossaryTerm` stores it.
///
/// The same shape as a glossary id ([`new_glossary_id`]) and for the same reason:
/// `GlossaryTermId` carries no length or pattern constraint of its own, and the bits come
/// from [`mint_hex_id`], so two creates landing in the same nanosecond cannot be given the
/// same id. It is minted ONCE, at create, and kept on the [`GlossaryTermEntry`] through
/// every later update — a rename included — so every read of that term reports the same id
/// and two terms never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_term_id() -> String {
    mint_hex_id(&MINTED_GLOSSARY_TERM_IDS, CATALOG_ID_WORDS)
}

/// A stored glossary term: one business concept `CreateGlossaryTerm` registers within a
/// glossary.
///
/// Every member of the modelled term is held here — its id, the glossary it belongs to, its
/// name and its two descriptions — and nothing else: the model gives a term no status,
/// owner or timestamp. The glossary, the name and the descriptions come from the request;
/// the id is the one server-set member, minted by [`new_glossary_term_id`] when the term is
/// stored.
#[derive(Debug, Clone)]
pub struct GlossaryTermEntry {
    /// `GlossaryTermId`, minted by [`new_glossary_term_id`] at create and unchanged
    /// afterwards, so a caller that kept the id from the create can still address the term
    /// after it has been updated.
    pub id: String,
    /// "The unique identifier of the glossary in which to create the term"
    /// (`CreateGlossaryTermRequest$GlossaryIdentifier`), recorded on the term because a
    /// term is addressed by its own id alone and the glossary it belongs to is reported by
    /// every read of it.
    pub glossary_id: String,
    /// "The name of the glossary term" (`CreateGlossaryTermRequest$Name`), unique within
    /// [`Self::glossary_id`]. Stored exactly as sent — the model documents no case fold for
    /// `GlossaryTermName`.
    pub name: String,
    /// "A short description of the glossary term", absent when the create carried none: the
    /// member is optional and the model gives it no default, so a term created without one
    /// reports none rather than an empty string.
    pub short_description: Option<String>,
    /// "A long description of the glossary term", absent for the same reason as
    /// [`Self::short_description`].
    pub long_description: Option<String>,
    /// The `ClientToken` of the request that created this term, when it carried one.
    ///
    /// `CreateGlossaryTermRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryTermEntry {
    /// The record a create stores: the glossary, name and descriptions it carried, under a
    /// freshly minted id, beside the idempotency token a retry of that create is recognised
    /// by.
    pub fn new(
        glossary_id: String,
        name: String,
        short_description: Option<String>,
        long_description: Option<String>,
        client_token: Option<String>,
    ) -> Self {
        Self {
            id: new_glossary_term_id(),
            glossary_id,
            name,
            short_description,
            long_description,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossaryTerm` request: every member comes from the
    /// request except the id, which the catalog assigns.
    pub fn from_create_input(input: &CreateGlossaryTermInput) -> Self {
        Self::new(
            input.glossary_identifier.clone(),
            input.name.clone(),
            input.short_description.clone(),
            input.long_description.clone(),
            input.client_token.clone(),
        )
    }
}
