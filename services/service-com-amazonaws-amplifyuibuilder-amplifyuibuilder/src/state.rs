// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplifyuibuilder-amplifyuibuilder service.
//!
//! The component is the primary resource of the Amplify UI Builder control
//! plane: `CreateComponent` mints one, `GetComponent` / `ListComponents` report
//! it, `UpdateComponent` merges into it and `DeleteComponent` removes it. The
//! record below is the one copy of a component every one of those handlers
//! reads and writes, plus the two projections the model asks for (`Component`
//! and `ComponentSummary`).

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    Component, ComponentBindingPropertiesValue, ComponentChild, ComponentDataConfiguration,
    ComponentEvent, ComponentProperty, ComponentSummary, ComponentVariant,
    ResourceNotFoundException,
};

/// The largest page `ListComponents` serves, and the page size it uses when the
/// request names no `maxResults`: that member targets `ListEntityLimit`, which
/// the model constrains `@range(min: 1, max: 100)`.
pub const COMPONENT_PAGE_LIMIT: i32 = 100;

/// How many components one `ExportComponents` page carries.
///
/// `ExportComponentsRequest` models no page-size member (only `nextToken`), so
/// the length of an export page is the service's own choice rather than
/// something a caller asks for. This emulator serves the same largest page its
/// component list does, so a caller walking either operation walks pages of one
/// bound.
pub const EXPORT_PAGE_LIMIT: usize = COMPONENT_PAGE_LIMIT as usize;

/// How many components the store holds before `CreateComponent` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no component quota this emulator can quote, so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is
/// released by `DeleteComponent`, which also forgets that component's client
/// tokens, so the token map can never outgrow this bound either.
pub const COMPONENT_QUOTA: usize = 1_000;

/// The map key a component's property maps may not carry.
///
/// `Component$properties`, `$overrides`, `$bindingProperties` and
/// `$collectionProperties` each document "You can't specify `tags` as a valid
/// property" for that map, so a request naming it is refused rather than stored.
pub const RESERVED_PROPERTY_NAME: &str = "tags";

/// How a `ListComponents` page token is spelled, so a token this operation never
/// issued can be told apart from one it did.
const PAGE_TOKEN_PREFIX: &str = "c-";

/// The width of the cursor a page token carries, in hex digits (a `u64`).
const PAGE_TOKEN_HEX_WIDTH: usize = 16;

/// The width of the app-environment digest a page token carries, in hex digits
/// (a `u32`).
const PAGE_TOKEN_SCOPE_WIDTH: usize = 8;

/// Which Amplify app and backend environment a component belongs to: the two
/// `@httpLabel` members that, with the id, name a component.
pub type EnvironmentKey = (String, String);

/// One Amplify UI Builder component, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredComponent {
    pub app_id: String,
    pub environment_name: String,
    /// `Component$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_component_id`].
    pub id: String,
    pub name: String,
    pub component_type: String,
    pub properties: HashMap<String, ComponentProperty>,
    pub children: Option<Vec<ComponentChild>>,
    pub variants: Vec<ComponentVariant>,
    pub overrides: HashMap<String, HashMap<String, String>>,
    pub binding_properties: HashMap<String, ComponentBindingPropertiesValue>,
    pub collection_properties: Option<HashMap<String, ComponentDataConfiguration>>,
    pub events: Option<HashMap<String, ComponentEvent>>,
    pub source_id: Option<String>,
    pub schema_version: Option<String>,
    /// `Component$tags` is reported for every component, so the tags a create
    /// supplied are held as a map rather than as an absence: a component created
    /// with no tags carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the component last changed. Creating it is the change that first set
    /// it, so it starts at [`StoredComponent::created_at`] and is moved by every
    /// `UpdateComponent` that merges into the record.
    pub modified_at: SystemTime,
    /// The position this component holds in its environment's creation order.
    ///
    /// It is what a `ListComponents` page token indexes into: it never changes
    /// and never repeats, so a page resumes at the same place whether or not
    /// components were created or deleted since the previous page.
    pub cursor: u64,
}

impl StoredComponent {
    /// The `Component` the create, get and update operations return.
    pub fn to_component(&self) -> Component {
        Component {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            source_id: self.source_id.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
            properties: self.properties.clone(),
            children: self.children.clone(),
            variants: self.variants.clone(),
            overrides: self.overrides.clone(),
            binding_properties: self.binding_properties.clone(),
            collection_properties: self.collection_properties.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            tags: Some(self.tags.clone()),
            events: self.events.clone(),
            schema_version: self.schema_version.clone(),
        }
    }

    /// The `ComponentSummary` projection `ListComponents` returns.
    pub fn to_summary(&self) -> ComponentSummary {
        ComponentSummary {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-amplifyuibuilder-amplifyuibuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// Every component, keyed by the (appId, environmentName) it belongs to and
    /// held in creation order within that environment — the stable total order
    /// `ListComponents` pages over.
    pub components: HashMap<EnvironmentKey, Vec<StoredComponent>>,
    /// A `CreateComponent` `clientToken` to the component that token created,
    /// so a retried create reports the first component instead of minting a
    /// second. The key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in, so
    /// reusing one against another app environment creates that environment's
    /// own component rather than reporting a component the request does not
    /// address.
    pub component_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The counter [`StoredComponent::cursor`] values are drawn from.
    pub cursor_seq: u64,
    /// The seed this store's component ids are minted from, drawn from the
    /// operating system the first time one is minted.
    id_seed: Option<u64>,
    /// The counter that keeps two ids of one run apart.
    id_seq: u64,
}

impl ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// The components stored for one app environment, in creation order.
    pub fn components_in(&self, app_id: &str, environment_name: &str) -> &[StoredComponent] {
        self.components
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The component `id` names in one app environment.
    pub fn component(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&StoredComponent> {
        self.components_in(app_id, environment_name)
            .iter()
            .find(|component| component.id == id)
    }

    /// The component `id` names in one app environment, for a caller that
    /// changes it.
    pub fn component_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredComponent> {
        self.components
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|component| component.id == id)
    }

    /// Store a component at the end of its environment's creation order.
    pub fn insert_component(&mut self, component: StoredComponent) {
        self.components
            .entry((
                component.app_id.clone(),
                component.environment_name.clone(),
            ))
            .or_default()
            .push(component);
    }

    /// Remove the component `id` names, and forget every client token that named
    /// it so the token map cannot outlive the record.
    pub fn remove_component(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<StoredComponent> {
        let key = (app_id.to_string(), environment_name.to_string());
        let components = self.components.get_mut(&key)?;
        let index = components
            .iter()
            .position(|component| component.id == id)?;
        let removed = components.remove(index);
        if components.is_empty() {
            self.components.remove(&key);
        }
        self.component_tokens
            .retain(|(token_key, _), token_id| !(*token_key == key && token_id == id));
        Some(removed)
    }

    /// The component one `clientToken` already created in the app environment
    /// the create addressed, if that token has been seen there.
    pub fn replayed_component(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredComponent> {
        let id = self
            .component_tokens
            .get(&(key.clone(), token.to_string()))?;
        self.component(&key.0, &key.1, id)
    }

    /// Remember that `token` created the component `id` in that app environment.
    pub fn remember_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.component_tokens.insert((key, token), id);
    }

    /// How many components the whole store holds — what [`COMPONENT_QUOTA`]
    /// bounds.
    pub fn total_components(&self) -> usize {
        self.components.values().map(Vec::len).sum()
    }

    /// The next cursor in this service's one creation order.
    ///
    /// Saturating rather than wrapping so the "never repeats" invariant above
    /// holds for every value it can actually reach: exhausting a `u64` would
    /// take 2^64 creates, which no emulator run performs.
    pub fn next_cursor(&mut self) -> u64 {
        self.cursor_seq = self.cursor_seq.saturating_add(1);
        self.cursor_seq
    }

    /// Mint an unused component id. `Component$id` targets `Uuid`, so the id is
    /// laid out as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_component_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_component_id();
            if self
                .components
                .get(key)
                .is_none_or(|components| components.iter().all(|c| c.id != id))
            {
                return id;
            }
        }
    }

    /// One candidate id: a version 4 UUID whose 122 free bits are the leading
    /// bits of a SHA-256 digest over this store's seed, a counter and the clock.
    ///
    /// The seed is drawn from the operating system once per store, so ids are
    /// not predictable from one run to the next; the counter and the clock make
    /// two ids of one run differ even on a seed collision. `sha2` does the
    /// hashing and `hex` the rendering — this only composes the input and sets
    /// the two nibbles RFC 4122 reserves.
    fn next_component_id(&mut self) -> String {
        use sha2::{Digest, Sha256};

        let seed = *self.id_seed.get_or_insert_with(os_seed);
        self.id_seq = self.id_seq.saturating_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());

        let mut digest = Sha256::new();
        digest.update(seed.to_le_bytes());
        digest.update(self.id_seq.to_le_bytes());
        digest.update(nanos.to_le_bytes());
        let digest = digest.finalize();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC 4122 §4.4: the high nibble of octet 6 is the version (4) and the
        // two high bits of octet 8 are the variant (10).
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;

        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }
}

/// A seed from the operating system, for the ids one store mints.
///
/// `RandomState` draws its hasher keys from the OS when it is constructed, so a
/// fresh instance's hash of the empty input is OS-drawn entropy — the standard
/// library's own source, with no further dependency.
fn os_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};

    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// True when a component property map carries the reserved
/// [`RESERVED_PROPERTY_NAME`] key.
///
/// Only the four maps the model documents the rule for are checked by the
/// handlers; `ComponentVariant$overrides` and `ComponentChild$properties` carry
/// no such statement, so a key named `tags` there is stored as given rather than
/// refused on a rule the model does not make.
pub fn carries_reserved_name<V>(map: &HashMap<String, V>) -> bool {
    map.contains_key(RESERVED_PROPERTY_NAME)
}

/// The `nextToken` a page of `key`'s components that ends before `cursor`
/// reports.
///
/// The token carries the app environment it was issued for as well as the
/// cursor, so a token cannot be replayed against another (appId,
/// environmentName): the pages a caller walks are the ones of the list it
/// started.
pub fn page_token(key: &EnvironmentKey, cursor: u64) -> String {
    format!(
        "{PAGE_TOKEN_PREFIX}{scope:0scope_width$x}{cursor:0cursor_width$x}",
        scope = token_scope(key),
        scope_width = PAGE_TOKEN_SCOPE_WIDTH,
        cursor_width = PAGE_TOKEN_HEX_WIDTH
    )
}

/// The cursor `token` resumes `key`'s list at, or `None` when it is not a token
/// `ListComponents` issued for that app environment.
///
/// A cursor beyond `issued` — the highest cursor the service has handed out — is
/// refused too, so a well-formed token nobody was ever given names no page.
pub fn parse_page_token(token: &str, key: &EnvironmentKey, issued: u64) -> Option<u64> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    // Both halves are hex digits, so refusing anything else here is what makes
    // the split below land on a character boundary: a request-supplied token is
    // arbitrary UTF-8, and splitting one at a byte offset inside a multi-byte
    // scalar would panic.
    if digits.len() != PAGE_TOKEN_SCOPE_WIDTH + PAGE_TOKEN_HEX_WIDTH
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let (scope, cursor) = digits.split_at(PAGE_TOKEN_SCOPE_WIDTH);
    if u32::from_str_radix(scope, 16).ok()? != token_scope(key) {
        return None;
    }
    let cursor = u64::from_str_radix(cursor, 16).ok()?;
    (cursor <= issued).then_some(cursor)
}

/// The app environment half of a page token: a digest of the (appId,
/// environmentName) the page was served for.
///
/// A token is only meaningful within the run that issued it, so the standard
/// hasher is enough; nothing persists a token across runs.
fn token_scope(key: &EnvironmentKey) -> u32 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.0.hash(&mut hasher);
    key.1.hash(&mut hasher);
    (hasher.finish() >> 32) as u32
}

/// The `ResourceNotFoundException` payload for an id that names no component in
/// the app environment the request addressed.
pub fn component_not_found(app_id: &str, environment_name: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "no component {id} in app {app_id} environment {environment_name}"
        )),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>)
}
