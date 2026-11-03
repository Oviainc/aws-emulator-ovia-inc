// SPDX-License-Identifier: CC0-1.0

//! How a response headers policy's `SecurityHeadersConfig` reaches the wire.
//!
//! `ResponseHeadersPolicySecurityHeadersConfig` declares one member per security header a
//! policy can configure, and every one of them is optional
//! (`smithy_query(query="shape",shape="ResponseHeadersPolicySecurityHeadersConfig")`).
//! CloudFront serializes the structure WHOLE: each member occupies an element of every
//! serialized policy, and a header the configuration does not set is the member's EMPTY
//! element rather than an absent one. A client that reads the structure to decide which
//! security headers a policy configures therefore reads the same set of elements for every
//! policy, and tells "not configured" from "not serialized" — which is impossible when the
//! unset members are simply left out.
//!
//! The generated serializer cannot express that on its own: it writes an `Option` member
//! only when it holds a value, and the member's target carries `@required` members of its
//! own, so a stand-in value would serialize the header's directives rather than an empty
//! element. [`SecurityHeadersConfigProjection`] is therefore what the member of
//! [`ResponseHeadersPolicyConfig`] holds, and it writes the shape's members itself.
//!
//! Because the stored configuration carries the projection, the ONE serializer here is
//! what the create, both reads of a policy, the config-only read, the update and every
//! listed summary answer through — a policy cannot be serialized one way by one operation
//! and another way by the next. Nothing else about the shape changes: the schema stays the
//! generated, model-derived one, the projection reports exactly the headers the
//! configuration set (an unset member is still `None` in state), and reading a policy back
//! deserializes exactly as before.

use super::*;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use ::aws_smithy_schema_server::DeserializableStruct;

/// The security headers a response headers policy configures, as CloudFront serializes
/// them: every member of the shape, with an empty element where the configuration set no
/// header.
///
/// A thin wrapper around the modelled [`ResponseHeadersPolicySecurityHeadersConfig`] —
/// which it derefs to, so a reader of one header reads the modelled member — that changes
/// how the configuration is WRITTEN and nothing about what it holds.
#[derive(Debug, Clone, Default)]
pub struct SecurityHeadersConfigProjection(ResponseHeadersPolicySecurityHeadersConfig);

impl SecurityHeadersConfigProjection {
    /// The shape this projects is still
    /// `com.amazonaws.cloudfront#ResponseHeadersPolicySecurityHeadersConfig`: the
    /// projection changes how a configuration is written, never which shape it is.
    pub const SCHEMA: ::aws_smithy_schema::Schema =
        ResponseHeadersPolicySecurityHeadersConfig::SCHEMA;

    /// The configuration `configured` describes, as it reaches the wire.
    pub fn new(configured: ResponseHeadersPolicySecurityHeadersConfig) -> Self {
        Self(configured)
    }
}

impl From<ResponseHeadersPolicySecurityHeadersConfig> for SecurityHeadersConfigProjection {
    fn from(configured: ResponseHeadersPolicySecurityHeadersConfig) -> Self {
        Self::new(configured)
    }
}

impl ::std::ops::Deref for SecurityHeadersConfigProjection {
    type Target = ResponseHeadersPolicySecurityHeadersConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SerializableStruct for SecurityHeadersConfigProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        // Every member of the shape, in the order the model declares them — the member
        // list is the generated schema's own, so the set cannot drift from the model.
        // A member the configuration set is written as that configuration; one it did
        // not is written as the empty element.
        for member in RESPONSE_HEADERS_POLICY_SECURITY_HEADERS_CONFIG_SCHEMA.members() {
            let configured = match member.member_name() {
                Some("XSSProtection") => as_serializable(&self.0.xss_protection),
                Some("FrameOptions") => as_serializable(&self.0.frame_options),
                Some("ReferrerPolicy") => as_serializable(&self.0.referrer_policy),
                Some("ContentSecurityPolicy") => as_serializable(&self.0.content_security_policy),
                Some("ContentTypeOptions") => as_serializable(&self.0.content_type_options),
                Some("StrictTransportSecurity") => {
                    as_serializable(&self.0.strict_transport_security)
                }
                // A member this file does not name is serialized as the empty element,
                // which is this shape's rule for a header nothing configured. The suite
                // pins the member list against the schema, so a member the model adds
                // fails there rather than reaching the wire unnoticed.
                _ => None,
            };
            serializer.write_struct(member, configured.unwrap_or(&UNCONFIGURED))?;
        }
        Ok(())
    }
}

impl DeserializableStruct for SecurityHeadersConfigProjection {
    fn deserialize(
        deserializer: &mut dyn ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self(
            <ResponseHeadersPolicySecurityHeadersConfig as DeserializableStruct>::deserialize(
                deserializer,
            )?,
        ))
    }
}

/// One member's configured header, as the serializer takes it.
fn as_serializable<T: SerializableStruct>(
    configured: &Option<T>,
) -> Option<&dyn SerializableStruct> {
    configured
        .as_ref()
        .map(|header| header as &dyn SerializableStruct)
}

/// What a member whose header the configuration does not set is written as.
static UNCONFIGURED: UnconfiguredHeader = UnconfiguredHeader;

/// A header no configuration set: it writes no member, so its element opens and closes
/// with nothing between — the empty element CloudFront answers for an unset security
/// header.
#[derive(Debug)]
struct UnconfiguredHeader;

impl SerializableStruct for UnconfiguredHeader {
    fn serialize_members(
        &self,
        _serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        Ok(())
    }
}
