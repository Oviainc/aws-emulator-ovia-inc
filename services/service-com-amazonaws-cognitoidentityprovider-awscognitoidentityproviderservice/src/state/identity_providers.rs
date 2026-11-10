// SPDX-License-Identifier: CC0-1.0

//! A pool's third-party identity providers and its managed login branding.

use super::*;

/// One stored third-party identity provider of a user pool: what
/// `CreateIdentityProvider` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct IdentityProvider {
    pub user_pool_id: String,
    pub provider_name: String,
    pub provider_type: String,
    /// "The scopes, URLs, and identifiers for your external identity provider"
    /// (`IdentityProviderType.ProviderDetails`): an opaque key/value map, kept
    /// exactly as it was submitted.
    pub provider_details: HashMap<String, String>,
    /// "A mapping of IdP attributes to standard and custom user pool attributes"
    /// (`IdentityProviderType.AttributeMapping`). A provider created with no
    /// mapping holds an empty one.
    pub attribute_mapping: HashMap<String, String>,
    /// The IdP identifiers of this provider, in the order they were submitted. A
    /// provider created with no identifiers holds none.
    pub idp_identifiers: Vec<String>,
    /// The ACR level mapping, which only an OIDC provider carries, so it stays
    /// absent on a provider that was created without one.
    pub acr_mapping: Option<HashMap<String, String>>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl IdentityProvider {
    /// Fill in, for this provider's `ProviderType`, every configuration entry
    /// Amazon Cognito resolves itself: the detail keys and the subject-claim
    /// mapping a create or update request does not have to state because the
    /// service knows them from the type of IdP.
    ///
    /// The model documents them per type as the difference between its
    /// `ProviderDetails` "create or update request" example and the "describe
    /// response" example beside it
    /// (`smithy_query(query="doc",shape="CreateIdentityProviderRequest",member="ProviderDetails")`),
    /// so a provider of a type whose endpoints the service knows renders them
    /// whether or not the request carried them. A stated key is never
    /// overwritten: what the request carries is this IdP's configuration, and
    /// the defaults only complete it.
    ///
    /// Seeded into the STORED provider, so every later read renders the same
    /// configuration as the create that stored it.
    pub fn seed_provider_type_defaults(&mut self) {
        for (key, value) in
            provider_type_detail_defaults(&self.provider_type, &self.provider_details)
        {
            self.provider_details
                .entry(key.to_string())
                .or_insert(value);
        }
        if let Some(claim) = provider_type_subject_claim(&self.provider_type) {
            self.attribute_mapping
                .entry(USERNAME_ATTRIBUTE.to_string())
                .or_insert_with(|| claim.to_string());
        }
    }

    /// The modelled view of this provider, rendered from what is stored.
    pub fn to_identity_provider_type(&self) -> IdentityProviderType {
        IdentityProviderType {
            user_pool_id: Some(self.user_pool_id.clone()),
            provider_name: Some(self.provider_name.clone()),
            provider_type: Some(self.provider_type.clone()),
            provider_details: Some(self.provider_details.clone()),
            attribute_mapping: Some(self.attribute_mapping.clone()),
            idp_identifiers: Some(self.idp_identifiers.clone()),
            acr_mapping: self.acr_mapping.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// The `IdentityProviderTypeType` values whose configuration Amazon Cognito
/// resolves itself — the endpoints the IdP is reached at and the claim its subject
/// arrives in — named one by one because that configuration is per type. They are
/// values of the catalog `crate::ops::types::IDENTITY_PROVIDER_TYPE_TYPE_VALUES`
/// carries; the one it carries beside them, a SAML IdP, resolves nothing from its
/// type (see [`provider_type_detail_defaults`]).
pub const PROVIDER_TYPE_FACEBOOK: &str = "Facebook";
/// A Google IdP, one of the values [`PROVIDER_TYPE_FACEBOOK`] documents.
pub const PROVIDER_TYPE_GOOGLE: &str = "Google";
/// A Login with Amazon IdP, one of the values [`PROVIDER_TYPE_FACEBOOK`] documents.
pub const PROVIDER_TYPE_LOGIN_WITH_AMAZON: &str = "LoginWithAmazon";
/// A Sign in with Apple IdP, one of the values [`PROVIDER_TYPE_FACEBOOK`] documents.
pub const PROVIDER_TYPE_SIGN_IN_WITH_APPLE: &str = "SignInWithApple";
/// An OpenID Connect IdP, one of the values [`PROVIDER_TYPE_FACEBOOK`] documents.
pub const PROVIDER_TYPE_OIDC: &str = "OIDC";

/// The user pool attribute a federated sign-in populates from the IdP's subject:
/// an `AttributeMappingType` key, "a user pool attribute as the key of the
/// key-value pair, and the IdP attribute claim name as the value"
/// (`CreateIdentityProvider.AttributeMapping` in this service's model).
pub(super) const USERNAME_ATTRIBUTE: &str = "username";

/// The `ProviderDetails` key stating which Facebook Graph API version an IdP of
/// that type is reached at (`ProviderDetails` in this service's model).
pub(super) const FACEBOOK_API_VERSION_KEY: &str = "api_version";

/// The IdP claim Amazon Cognito maps to the pool's [`USERNAME_ATTRIBUTE`] for a
/// provider of `provider_type` when the request maps nothing to it: that IdP's
/// own subject claim — `sub` for the OpenID Connect providers and the two social
/// providers that are OIDC ones, and the social provider's own user-id claim
/// otherwise.
///
/// `None` for a SAML IdP: a SAML assertion's subject arrives in whatever claim
/// the IdP's metadata names, so there is none to assume.
pub(super) fn provider_type_subject_claim(provider_type: &str) -> Option<&'static str> {
    match provider_type {
        PROVIDER_TYPE_FACEBOOK => Some("id"),
        PROVIDER_TYPE_LOGIN_WITH_AMAZON => Some("user_id"),
        PROVIDER_TYPE_GOOGLE | PROVIDER_TYPE_SIGN_IN_WITH_APPLE | PROVIDER_TYPE_OIDC => {
            Some("sub")
        }
        _ => None,
    }
}

/// Every `ProviderDetails` entry Amazon Cognito resolves for a provider of
/// `provider_type` itself, given the entries the request `stated`.
///
/// The model documents them per IdP type as the difference between its
/// `ProviderDetails` "create or update request" example and the "describe
/// response" example beside it: the endpoints of a named social IdP, whose
/// locators the service knows, and the `attributes_url_add_attributes` flag
/// saying whether the attribute request appends the attributes it asks for.
///
/// A SAML IdP resolves none here: the entries its describe response carries
/// beyond the request — the binding URIs and the active encryption certificate —
/// are read from the IdP's own metadata document, which is the IdP's to serve and
/// not this service's to invent.
pub(super) fn provider_type_detail_defaults(
    provider_type: &str,
    stated: &HashMap<String, String>,
) -> Vec<(&'static str, String)> {
    match provider_type {
        // Every Facebook Graph endpoint is versioned by the `api_version` the
        // request states, so the three locators are built from it.
        PROVIDER_TYPE_FACEBOOK => {
            let version = facebook_api_version_segment(stated);
            vec![
                (
                    "attributes_url",
                    format!("https://graph.facebook.com/{version}/me?fields="),
                ),
                ("attributes_url_add_attributes", "true".to_string()),
                (
                    "authorize_url",
                    format!("https://www.facebook.com/{version}/dialog/oauth"),
                ),
                ("token_request_method", "GET".to_string()),
                (
                    "token_url",
                    format!("https://graph.facebook.com/{version}/oauth/access_token"),
                ),
            ]
        }
        PROVIDER_TYPE_GOOGLE => owned_details(&[
            (
                "attributes_url",
                "https://people.googleapis.com/v1/people/me?personFields=",
            ),
            ("attributes_url_add_attributes", "true"),
            ("authorize_url", "https://accounts.google.com/o/oauth2/v2/auth"),
            ("oidc_issuer", "https://accounts.google.com"),
            ("token_request_method", "POST"),
            ("token_url", "https://www.googleapis.com/oauth2/v4/token"),
        ]),
        PROVIDER_TYPE_LOGIN_WITH_AMAZON => owned_details(&[
            ("attributes_url", "https://api.amazon.com/user/profile"),
            ("attributes_url_add_attributes", "false"),
            ("authorize_url", "https://www.amazon.com/ap/oa"),
            ("token_request_method", "POST"),
            ("token_url", "https://api.amazon.com/auth/o2/token"),
        ]),
        PROVIDER_TYPE_SIGN_IN_WITH_APPLE => owned_details(&[
            ("attributes_url_add_attributes", "false"),
            ("authorize_url", "https://appleid.apple.com/auth/authorize"),
            ("oidc_issuer", "https://appleid.apple.com"),
            ("token_request_method", "POST"),
            ("token_url", "https://appleid.apple.com/auth/token"),
        ]),
        PROVIDER_TYPE_OIDC => owned_details(&[("attributes_url_add_attributes", "false")]),
        _ => Vec::new(),
    }
}

/// Owned default values from borrowed ones, so a type's table reads as the model
/// documents it.
pub(super) fn owned_details(entries: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    entries
        .iter()
        .map(|(key, value)| (*key, (*value).to_string()))
        .collect()
}

/// The Facebook Graph API version an IdP of that type is reached at: the
/// `api_version` the request stated, or [`FACEBOOK_DEFAULT_API_VERSION`] when it
/// stated none. Every Graph locator carries it as the first segment of its path.
pub(super) fn facebook_api_version_segment(stated: &HashMap<String, String>) -> &str {
    stated_detail(stated, FACEBOOK_API_VERSION_KEY).unwrap_or(FACEBOOK_DEFAULT_API_VERSION)
}

/// The Facebook Graph API version an IdP of that type is reached at when the
/// request states no [`FACEBOOK_API_VERSION_KEY`] of its own: the version Amazon
/// Cognito substitutes for a Facebook IdP configured without one, which is a
/// version of Facebook's own API rather than anything this service's model fixes
/// (the model's `ProviderDetails` example states an `api_version` of its own, so
/// it documents no default). It therefore moves as that API advances — the value
/// here is the one Amazon Cognito substituted when this was written — while a
/// request that states a version of its own is always served that version.
pub(super) const FACEBOOK_DEFAULT_API_VERSION: &str = "v21.0";

/// The value a `ProviderDetails` map states for `key`, or `None` when it carries
/// no such key or carries it with nothing in it: a key with an empty value states
/// nothing.
pub(super) fn stated_detail<'details>(
    details: &'details HashMap<String, String>,
    key: &str,
) -> Option<&'details str> {
    details
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Every language `LinksType`'s key admits, in the order its target
/// `LanguageIdType` lists them
/// (`^cognito:(default|dutch|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional))$`
/// in this service's model). `cognito:default` is the one a language with no
/// entry of its own falls back to.
pub const LINK_LANGUAGES: [&str; 13] = [
    "cognito:default",
    "cognito:dutch",
    "cognito:english",
    "cognito:french",
    "cognito:spanish",
    "cognito:german",
    "cognito:bahasa-indonesia",
    "cognito:italian",
    "cognito:japanese",
    "cognito:korean",
    "cognito:portuguese-brazil",
    "cognito:chinese-simplified",
    "cognito:chinese-traditional",
];

/// The lowest-ordered key of a submitted `Links` map that is not one of
/// [`LINK_LANGUAGES`], or `None` when every key names a language managed login
/// localizes. Lowest-ordered so one request is refused over the same key every
/// time, whatever order the map arrived in.
pub fn unsupported_link_language<'links>(
    links: impl IntoIterator<Item = &'links String>,
) -> Option<&'links String> {
    links
        .into_iter()
        .filter(|language| !LINK_LANGUAGES.contains(&language.as_str()))
        .min()
}

/// One stored set of terms documents of a user pool: what `CreateTerms` stored
/// and every later read of it renders.
#[derive(Debug, Clone)]
pub struct Terms {
    pub terms_id: String,
    pub user_pool_id: String,
    pub client_id: String,
    pub terms_name: String,
    pub terms_source: String,
    pub enforcement: String,
    /// The language-to-URL map, ordered so a read of several languages is
    /// stable. `LinksType` admits 1..13 entries, so this map is never empty.
    pub links: BTreeMap<String, String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Terms {
    /// The modelled view of these terms documents, rendered from what is stored.
    pub fn to_terms_type(&self) -> TermsType {
        TermsType {
            terms_id: self.terms_id.clone(),
            user_pool_id: self.user_pool_id.clone(),
            client_id: self.client_id.clone(),
            terms_name: self.terms_name.clone(),
            terms_source: self.terms_source.clone(),
            enforcement: self.enforcement.clone(),
            links: self
                .links
                .iter()
                .map(|(language, url)| (language.clone(), url.clone()))
                .collect(),
            creation_date: to_date_time(self.creation_date),
            last_modified_date: to_date_time(self.last_modified_date),
        }
    }

    /// The summary view a listing renders of these terms documents: the subset
    /// of the stored record `TermsDescriptionType` declares, so a list and a
    /// describe of one document agree member for member on every member both
    /// carry.
    pub fn to_terms_description_type(&self) -> TermsDescriptionType {
        TermsDescriptionType {
            terms_id: self.terms_id.clone(),
            terms_name: self.terms_name.clone(),
            enforcement: self.enforcement.clone(),
            creation_date: to_date_time(self.creation_date),
            last_modified_date: to_date_time(self.last_modified_date),
        }
    }
}

/// One stored managed login branding style of a user pool: what
/// `CreateManagedLoginBranding` persisted and every later read of that style
/// renders.
///
/// A style belongs to exactly one app client of the pool — "Creates a new set of
/// branding settings for a user pool style and associates it with an app
/// client" — and keeps the customizations the request supplied: "Amazon Cognito
/// doesn't require that you pass all parameters in one request and preserves
/// existing style settings that you don't specify"
/// (`CreateManagedLoginBranding` in this service's model).
#[derive(Debug, Clone)]
pub struct ManagedLoginBranding {
    pub branding_id: String,
    pub user_pool_id: String,
    /// The app client the style is assigned to. `ManagedLoginBrandingType`
    /// models no `ClientId`, so this is not rendered; it is what makes the
    /// style reachable by app client and what the uniqueness rule keys on.
    pub client_id: String,
    /// Whether the style takes "the default branding style options" Amazon
    /// Cognito manages (`CreateManagedLoginBranding.UseCognitoProvidedValues`).
    pub use_cognito_provided_values: bool,
    /// The customized settings document, or `None` for a style that carries no
    /// customization of its own.
    pub settings: Option<::aws_smithy_types::Document>,
    /// The customized image files, in the order they were submitted.
    pub assets: Vec<crate::ops::types::AssetType>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl ManagedLoginBranding {
    /// The modelled view of this style, rendered from what is stored: "returns
    /// only values that you customized in your branding style"
    /// (`ReturnMergedResources` on the two readers of a style).
    ///
    /// `Assets` is always reported, as the list of image files the style holds:
    /// a style that holds none holds an EMPTY list of them, which is what a read
    /// of its image files answers.
    pub fn to_managed_login_branding_type(&self) -> crate::ops::types::ManagedLoginBrandingType {
        crate::ops::types::ManagedLoginBrandingType {
            managed_login_branding_id: Some(self.branding_id.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            use_cognito_provided_values: self.use_cognito_provided_values,
            settings: self.settings.clone(),
            assets: Some(self.assets.clone()),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }

    /// The MERGED view of this style: the customized view with every branding
    /// option and every image file this style leaves at the service's defaults
    /// filled in from [`default_branding_settings`] and
    /// [`default_branding_assets`].
    ///
    /// "When true, returns values for branding options that are unchanged from
    /// Amazon Cognito defaults. When false or when you omit this parameter,
    /// returns only values that you customized in your branding style"
    /// (`ReturnMergedResources` on the two readers of a style), so the merged
    /// answer reports the service's own value for every option the style did not
    /// change — its settings document under the style's settings, and its image
    /// file for every category and display mode the style supplied no file for.
    /// Every other member identifies the stored style, so the two views agree on
    /// those.
    pub fn to_merged_managed_login_branding_type(
        &self,
    ) -> crate::ops::types::ManagedLoginBrandingType {
        crate::ops::types::ManagedLoginBrandingType {
            settings: Some(merged_branding_settings(self.settings.as_ref())),
            assets: Some(self.merged_branding_assets()),
            ..self.to_managed_login_branding_type()
        }
    }

    /// The image files the MERGED view of this style reports: every file the
    /// style holds, and the service's own file for each role it supplied none
    /// for, as far as `AssetListType` admits.
    ///
    /// The style's own files come first and are never dropped: a style may hold
    /// a file for a display mode the service provides none for
    /// (`ColorSchemeModeType`'s `DYNAMIC`), so the two sets together can name
    /// more roles than the modelled maximum of 40 files, and what the merged view
    /// then leaves out is the service's own.
    fn merged_branding_assets(&self) -> Vec<crate::ops::types::AssetType> {
        let filled: ::std::collections::HashSet<(&str, &str)> = self
            .assets
            .iter()
            .map(|asset| (asset.category.as_str(), asset.color_mode.as_str()))
            .collect();
        let mut merged = self.assets.clone();
        for provided in default_branding_assets() {
            if merged.len() >= MAX_BRANDING_ASSETS {
                break;
            }
            if !filled.contains(&(provided.category.as_str(), provided.color_mode.as_str())) {
                merged.push(provided);
            }
        }
        merged
    }
}

/// The most image files a branding style's `Assets` may carry: `AssetListType`'s
/// modelled length maximum
/// (`smithy_query(query="operation",operation="CreateManagedLoginBranding")`).
pub const MAX_BRANDING_ASSETS: usize = 40;

/// The branding options this service manages for a style that customizes none
/// of them: the baseline the MERGED view of a style reports.
///
/// The option names are the ones this API documents — `Settings` names
/// `signUp`, `instructions`, `sessionTimerDisplay` and `languageSelector` among
/// the components a style carries (this service's model), and the branding
/// editor groups them under `categories`, `componentClasses` and `components` —
/// while the VALUES are this emulator's defaults: the managed defaults are not
/// in the model, so a style that overrides none of them reads back this
/// document. Rendered fresh per read, so no caller can change what another
/// caller's merged read reports.
pub fn default_branding_settings() -> ::aws_smithy_types::Document {
    document_from_json(&::serde_json::json!({
        "categories": {
            "auth": {
                "authMethodOrder": [
                    [
                        { "display": "BUTTON", "type": "FEDERATED" },
                        { "display": "INPUT", "type": "USERNAME_PASSWORD" }
                    ]
                ],
                "federation": { "interfaceStyle": "BUTTON_LIST", "order": [] }
            },
            "form": {
                "displayGraphics": true,
                "instructions": { "enabled": false },
                "languageSelector": { "enabled": false },
                "location": { "horizontal": "CENTER", "vertical": "CENTER" },
                "sessionTimerDisplay": "NONE"
            },
            "global": {
                "colorSchemeMode": "LIGHT",
                "pageFooter": { "enabled": false },
                "pageHeader": { "enabled": false },
                "spacingDensity": "REGULAR"
            },
            "signUp": { "acceptanceElements": [{ "enforcement": "NONE", "textKey": "en" }] }
        },
        "componentClasses": {
            "buttons": { "borderRadius": 8 },
            "divider": { "darkMode": { "borderColor": "232b37ff" }, "lightMode": { "borderColor": "ebebf0ff" } },
            "dropDown": {
                "borderRadius": 8,
                "darkMode": {
                    "defaults": { "itemBackgroundColor": "192534ff" },
                    "hover": {
                        "itemBackgroundColor": "081120ff",
                        "itemBorderColor": "5f6b7aff",
                        "itemTextColor": "e9ebedff"
                    },
                    "match": { "itemBackgroundColor": "d1d5dbff", "itemTextColor": "539fe5ff" }
                },
                "lightMode": {
                    "defaults": { "itemBackgroundColor": "ffffffff" },
                    "hover": {
                        "itemBackgroundColor": "f4f4f4ff",
                        "itemBorderColor": "7d8998ff",
                        "itemTextColor": "000716ff"
                    },
                    "match": { "itemBackgroundColor": "414d5cff", "itemTextColor": "0972d3ff" }
                }
            },
            "focusState": { "darkMode": { "borderColor": "539fe5ff" }, "lightMode": { "borderColor": "0972d3ff" } },
            "idpButtons": { "icons": { "enabled": true } },
            "input": {
                "borderRadius": 8,
                "darkMode": {
                    "defaults": { "backgroundColor": "0f1b2aff", "borderColor": "5f6b7aff" },
                    "placeholderColor": "8d99a8ff"
                },
                "lightMode": {
                    "defaults": { "backgroundColor": "ffffffff", "borderColor": "7d8998ff" },
                    "placeholderColor": "5f6b7aff"
                }
            },
            "inputDescription": {
                "darkMode": { "textColor": "8d99a8ff" },
                "lightMode": { "textColor": "5f6b7aff" }
            },
            "inputLabel": {
                "darkMode": { "textColor": "d1d5dbff" },
                "lightMode": { "textColor": "000716ff" }
            },
            "link": {
                "darkMode": {
                    "defaults": { "textColor": "539fe5ff" },
                    "hover": { "textColor": "89bdeeff" }
                },
                "lightMode": {
                    "defaults": { "textColor": "0972d3ff" },
                    "hover": { "textColor": "033160ff" }
                }
            },
            "optionControls": {
                "darkMode": {
                    "defaults": { "backgroundColor": "0f1b2aff", "borderColor": "7d8998ff" },
                    "selected": { "backgroundColor": "539fe5ff", "foregroundColor": "000716ff" }
                },
                "lightMode": {
                    "defaults": { "backgroundColor": "ffffffff", "borderColor": "7d8998ff" },
                    "selected": { "backgroundColor": "0972d3ff", "foregroundColor": "ffffffff" }
                }
            },
            "statusIndicator": {
                "darkMode": {
                    "error": {
                        "backgroundColor": "1a0000ff",
                        "borderColor": "eb6f6fff",
                        "indicatorColor": "eb6f6fff"
                    },
                    "pending": { "indicatorColor": "aeb6c0ff" },
                    "success": {
                        "backgroundColor": "001a02ff",
                        "borderColor": "29ad32ff",
                        "indicatorColor": "29ad32ff"
                    },
                    "warning": {
                        "backgroundColor": "1d1906ff",
                        "borderColor": "e0ca57ff",
                        "indicatorColor": "e0ca57ff"
                    }
                },
                "lightMode": {
                    "error": {
                        "backgroundColor": "fff7f7ff",
                        "borderColor": "d91515ff",
                        "indicatorColor": "d91515ff"
                    },
                    "pending": { "indicatorColor": "5f6b7aff" },
                    "success": {
                        "backgroundColor": "f2fcf3ff",
                        "borderColor": "037f0cff",
                        "indicatorColor": "037f0cff"
                    },
                    "warning": {
                        "backgroundColor": "fffce9ff",
                        "borderColor": "8d6605ff",
                        "indicatorColor": "8d6605ff"
                    }
                }
            }
        },
        "components": {
            "alert": {
                "borderRadius": 12,
                "darkMode": {
                    "error": { "backgroundColor": "1a0000ff", "borderColor": "eb6f6fff" },
                    "info": { "backgroundColor": "001129ff", "borderColor": "539fe5ff" },
                    "success": { "backgroundColor": "001a02ff", "borderColor": "29ad32ff" },
                    "warning": { "backgroundColor": "1d1906ff", "borderColor": "e0ca57ff" }
                },
                "lightMode": {
                    "error": { "backgroundColor": "fff7f7ff", "borderColor": "d91515ff" },
                    "info": { "backgroundColor": "f2f8fdff", "borderColor": "0972d3ff" },
                    "success": { "backgroundColor": "f2fcf3ff", "borderColor": "037f0cff" },
                    "warning": { "backgroundColor": "fffce9ff", "borderColor": "8d6605ff" }
                }
            },
            "favicon": { "enabledTypes": ["ICO", "SVG"] },
            "form": {
                "backgroundImage": { "enabled": false },
                "borderRadius": 8,
                "borderWidth": 1,
                "darkMode": { "backgroundColor": "0f1b2aff", "borderColor": "424650ff" },
                "lightMode": { "backgroundColor": "ffffffff", "borderColor": "c6c6cdff" },
                "logo": {
                    "enabled": false,
                    "formInclusion": "IN",
                    "location": "CENTER",
                    "position": "TOP"
                }
            },
            "idpButton": {
                "custom": {},
                "standard": {
                    "darkMode": {
                        "active": {
                            "backgroundColor": "354150ff",
                            "borderColor": "89bdeeff",
                            "textColor": "89bdeeff"
                        },
                        "defaults": {
                            "backgroundColor": "0f1b2aff",
                            "borderColor": "539fe5ff",
                            "textColor": "539fe5ff"
                        },
                        "hover": {
                            "backgroundColor": "192534ff",
                            "borderColor": "89bdeeff",
                            "textColor": "89bdeeff"
                        }
                    },
                    "lightMode": {
                        "active": {
                            "backgroundColor": "d3e7f9ff",
                            "borderColor": "033160ff",
                            "textColor": "033160ff"
                        },
                        "defaults": {
                            "backgroundColor": "ffffffff",
                            "borderColor": "0972d3ff",
                            "textColor": "0972d3ff"
                        },
                        "hover": {
                            "backgroundColor": "f2f8fdff",
                            "borderColor": "033160ff",
                            "textColor": "033160ff"
                        }
                    }
                }
            },
            "pageBackground": {
                "darkMode": { "color": "0f1b2aff" },
                "image": { "enabled": true },
                "lightMode": { "color": "ffffffff" }
            },
            "pageFooter": {
                "backgroundImage": { "enabled": false },
                "darkMode": { "background": { "color": "0f1b2aff" }, "borderColor": "424650ff" },
                "lightMode": { "background": { "color": "fafafaff" }, "borderColor": "d1d5dbff" },
                "logo": { "enabled": false, "location": "START" }
            },
            "pageHeader": {
                "backgroundImage": { "enabled": false },
                "darkMode": { "background": { "color": "0f1b2aff" }, "borderColor": "424650ff" },
                "lightMode": { "background": { "color": "fafafaff" }, "borderColor": "d1d5dbff" },
                "logo": { "enabled": false, "location": "START" }
            },
            "pageText": {
                "darkMode": {
                    "bodyColor": "b6bec9ff",
                    "descriptionColor": "b6bec9ff",
                    "headingColor": "d1d5dbff"
                },
                "lightMode": {
                    "bodyColor": "414d5cff",
                    "descriptionColor": "414d5cff",
                    "headingColor": "000716ff"
                }
            },
            "phoneNumberSelector": { "displayType": "TEXT" },
            "primaryButton": {
                "darkMode": {
                    "active": { "backgroundColor": "539fe5ff", "textColor": "000716ff" },
                    "defaults": { "backgroundColor": "539fe5ff", "textColor": "000716ff" },
                    "disabled": { "backgroundColor": "5f6b7aff", "textColor": "000716ff" },
                    "hover": { "backgroundColor": "89bdeeff", "textColor": "000716ff" }
                },
                "lightMode": {
                    "active": { "backgroundColor": "033160ff", "textColor": "ffffffff" },
                    "defaults": { "backgroundColor": "0972d3ff", "textColor": "ffffffff" },
                    "disabled": { "backgroundColor": "d1d5dbff", "textColor": "ffffffff" },
                    "hover": { "backgroundColor": "065299ff", "textColor": "ffffffff" }
                }
            },
            "secondaryButton": {
                "darkMode": {
                    "active": {
                        "backgroundColor": "354150ff",
                        "borderColor": "89bdeeff",
                        "textColor": "89bdeeff"
                    },
                    "defaults": {
                        "backgroundColor": "0f1b2aff",
                        "borderColor": "539fe5ff",
                        "textColor": "539fe5ff"
                    },
                    "hover": {
                        "backgroundColor": "192534ff",
                        "borderColor": "89bdeeff",
                        "textColor": "89bdeeff"
                    }
                },
                "lightMode": {
                    "active": {
                        "backgroundColor": "d3e7f9ff",
                        "borderColor": "033160ff",
                        "textColor": "033160ff"
                    },
                    "defaults": {
                        "backgroundColor": "ffffffff",
                        "borderColor": "0972d3ff",
                        "textColor": "0972d3ff"
                    },
                    "hover": {
                        "backgroundColor": "f2f8fdff",
                        "borderColor": "033160ff",
                        "textColor": "033160ff"
                    }
                }
            }
        }
    }))
}

/// The image files this service provides for the roles a style supplies none of
/// its own for: the baseline the MERGED view of a style reports, one file per
/// role the style did not fill itself.
///
/// One file for each display mode of every category the service provides
/// artwork for — every value of `AssetCategoryType` — in `LIGHT` and in `DARK`,
/// the two display modes an image is served for (`ColorSchemeModeType`'s third
/// value, `DYNAMIC`, is the mode that follows the browser rather than a third
/// image to serve). Every file is an `SVG`, the one file type of
/// `AssetExtensionType` that scales to each of these roles, and the set stays
/// inside `AssetListType`'s modelled maximum of 40 files. Categories, display
/// modes and file types from
/// `smithy_query(query="shape",shape="AssetCategoryType,ColorSchemeModeType,AssetExtensionType")`.
///
/// Each file names the role it fills and the display mode it is for, and carries
/// no `Bytes`: "The image file, in Base64-encoded binary" (`AssetType.Bytes`) is
/// the file a CALLER uploaded, while the artwork for these roles is the
/// service's own and is served from the managed login pages rather than handed
/// back through this API. Rendered fresh per read, so no caller can change what
/// another caller reads.
pub fn default_branding_assets() -> Vec<crate::ops::types::AssetType> {
    crate::ops::types::ASSET_CATEGORY_TYPE_VALUES
        .iter()
        .flat_map(|category| {
            PROVIDED_ASSET_MODES
                .iter()
                .map(move |color_mode| crate::ops::types::AssetType {
                    category: (*category).to_string(),
                    color_mode: (*color_mode).to_string(),
                    extension: PROVIDED_ASSET_EXTENSION.to_string(),
                    bytes: None,
                    // "The ID of the asset" (`AssetType.ResourceId`) names an
                    // uploaded file; a file the service provides is not one a
                    // caller uploaded, so it carries none.
                    resource_id: None,
                })
        })
        .collect()
}

/// The file type of every file [`default_branding_assets`] provides: one of the
/// modelled `AssetExtensionType` values.
const PROVIDED_ASSET_EXTENSION: &str = "SVG";

/// The display modes [`default_branding_assets`] provides a file for: the two
/// modelled `ColorSchemeModeType` values an image is served for.
const PROVIDED_ASSET_MODES: [&str; 2] = ["LIGHT", "DARK"];

/// [`default_branding_settings`] with `customized` laid over it: every option a
/// style customizes is the style's, and every option it leaves alone is the
/// default. A style that customizes nothing reads back the defaults unchanged.
///
/// An option the style supplies REPLACES the default at that name, whatever
/// either side holds — a merged read reports the style's own value, not a value
/// nothing configured. Only where BOTH sides hold a JSON object are the two
/// walked into.
pub fn merged_branding_settings(
    customized: Option<&::aws_smithy_types::Document>,
) -> ::aws_smithy_types::Document {
    let defaults = default_branding_settings();
    match customized {
        None => defaults,
        Some(customized) => overlay_documents(defaults, customized),
    }
}

/// `customized` laid over `base`, member by member: a member only `base` holds
/// stays, a member only `customized` holds is added, and a member both hold is
/// the customized one — except where both hold a JSON object, which is where the
/// two are walked into.
///
/// Walked ITERATIVELY over an explicit worklist of the objects still being
/// merged: BOTH sides can be a document a caller supplied (a stored style's
/// settings under an update's), so the nesting depth is the caller's to choose
/// and a recursive walk of it would put that depth on the stack.
pub(super) fn overlay_documents(
    base: ::aws_smithy_types::Document,
    customized: &::aws_smithy_types::Document,
) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::Document;
    // The two objects to merge, or nothing to walk at all: anything else the
    // customization names replaces what `base` held.
    let (base, customized) = match (base, customized) {
        (Document::Object(base), Document::Object(customized)) => (base, customized),
        (_, customized) => return customized.clone(),
    };

    /// One object being merged: the members merged so far (starting as the
    /// base's own), the customized members still to lay over them, and the name
    /// this object is held under in the object one level up.
    struct Pending<'a> {
        name: Option<&'a str>,
        merged: HashMap<String, ::aws_smithy_types::Document>,
        remaining: Vec<(&'a String, &'a ::aws_smithy_types::Document)>,
    }

    let mut worklist = vec![Pending {
        name: None,
        merged: base,
        remaining: customized.iter().collect(),
    }];
    loop {
        let Some(current) = worklist.last_mut() else {
            // Unreachable: the worklist starts non-empty and the last frame
            // finished is returned rather than popped into nothing.
            return Document::Object(HashMap::new());
        };
        match current.remaining.pop() {
            // A member both sides hold as an object: merged one level down.
            Some((name, Document::Object(customized))) => {
                match current.merged.remove(name.as_str()) {
                    Some(Document::Object(base)) => worklist.push(Pending {
                        name: Some(name),
                        merged: base,
                        remaining: customized.iter().collect(),
                    }),
                    _ => {
                        current
                            .merged
                            .insert(name.clone(), Document::Object(customized.clone()));
                    }
                }
            }
            // Every other member the customization names replaces what the base
            // held under that name.
            Some((name, value)) => {
                current.merged.insert(name.clone(), value.clone());
            }
            // This object is merged: it belongs to the object above it, and the
            // outermost one is the answer.
            None => {
                let finished = worklist.pop().expect("the frame just inspected");
                let merged = Document::Object(finished.merged);
                match (worklist.last_mut(), finished.name) {
                    (Some(parent), Some(name)) => {
                        parent.merged.insert(name.to_string(), merged);
                    }
                    _ => return merged,
                }
            }
        }
    }
}

/// The `aws_smithy_types::Document` a JSON value is, so a document this service
/// builds from a JSON literal travels as the modelled `Document` type.
///
/// Called only on a literal this crate wrote, never on caller input: the walk is
/// one level of recursion per nesting level, and the documents it is given nest
/// as deeply as the source they are written in.
pub fn document_from_json(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::{Document, Number};
    match value {
        ::serde_json::Value::Null => Document::Null,
        ::serde_json::Value::Bool(value) => Document::Bool(*value),
        ::serde_json::Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else {
                Document::Number(Number::Float(value.as_f64().unwrap_or_default()))
            }
        }
        ::serde_json::Value::String(value) => Document::String(value.clone()),
        ::serde_json::Value::Array(items) => {
            Document::Array(items.iter().map(document_from_json).collect())
        }
        ::serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), document_from_json(member)))
                .collect(),
        ),
    }
}

/// `customized` laid over a style's STORED settings: "Amazon Cognito doesn't
/// require that you pass all parameters in one request and preserves existing
/// style settings that you don't specify", and a style larger than one request
/// may carry is submitted as "multiple requests, each with a size smaller than
/// the limit" (`UpdateManagedLoginBranding` in this service's model) — which
/// only builds up one style if each request's options are laid over the stored
/// ones instead of replacing them.
///
/// `None` for `customized` names no setting at all, so the stored document
/// stands.
pub fn updated_branding_settings(
    stored: Option<&::aws_smithy_types::Document>,
    customized: Option<&::aws_smithy_types::Document>,
) -> Option<::aws_smithy_types::Document> {
    match (stored, customized) {
        (stored, None) => stored.cloned(),
        (None, Some(customized)) => Some(customized.clone()),
        (Some(stored), Some(customized)) => Some(overlay_documents(stored.clone(), customized)),
    }
}

/// `customized` laid over a style's STORED image files, under the same
/// preserve-what-you-did-not-specify rule as [`updated_branding_settings`].
///
/// An image file is the one the style shows for its category in its display
/// mode — "The category that the image corresponds to in your managed login
/// configuration" and "The display-mode target of the asset: light, dark, or
/// browser-adaptive" (`AssetType` in this service's model) — so a submitted
/// file REPLACES the stored file of that category and display mode and leaves
/// every other one alone. The stored order is kept and a file for a pair the
/// style did not hold is appended, so a read of several is stable.
pub fn updated_branding_assets(
    stored: &[crate::ops::types::AssetType],
    customized: &[crate::ops::types::AssetType],
) -> Vec<crate::ops::types::AssetType> {
    let mut merged: Vec<crate::ops::types::AssetType> = stored.to_vec();
    // Where each slot already sits, INDEXED rather than searched for: both lists
    // are as long as the requests that built them, so a scan per submitted file
    // would do work in the product of the two lengths. One lookup per submitted
    // file keeps the merge linear in them, and the key is built once per file
    // rather than once per comparison. The first file of a slot is the one a
    // later request replaces, so a style that somehow holds two of one slot
    // keeps its later one untouched.
    let mut slot_positions: ::std::collections::HashMap<(String, String), usize> =
        ::std::collections::HashMap::with_capacity(merged.len());
    for (position, asset) in merged.iter().enumerate() {
        slot_positions
            .entry((asset.category.clone(), asset.color_mode.clone()))
            .or_insert(position);
    }
    for asset in customized {
        let slot = (asset.category.clone(), asset.color_mode.clone());
        match slot_positions.get(&slot) {
            Some(&position) => merged[position] = asset.clone(),
            None => {
                slot_positions.insert(slot, merged.len());
                merged.push(asset.clone());
            }
        }
    }
    merged
}
