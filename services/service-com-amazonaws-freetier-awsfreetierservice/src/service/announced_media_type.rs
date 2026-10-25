// SPDX-License-Identifier: CC0-1.0

//! The media type this service announces on every response.
//!
//! # What the wire shows
//!
//! The model declares `aws.protocols#awsJson1_0`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-freetier-awsfreetierservice/smithy-model/com-amazonaws-freetier-awsfreetierservice.json")`),
//! while every recorded response of this service — HTTP 200 and HTTP 400 alike — carries
//! `content-type: application/x-amz-json-1.1`. The request side is untouched and matches
//! throughout: requests still route by `X-Amz-Target: AWSFreeTierService.<Operation>`, and
//! the awsJson `1.1` codec this service answers in is the SAME `JsonCodec` over the same
//! body, so a request sent under either media type still reads byte for byte. Only the
//! media type the RESPONSE announces differs from what the protocol trait implies, and that
//! is what a client keys content negotiation and protocol assertion off.
//!
//! # Where the rule lives
//!
//! Not here. The rule, the one function that applies it and the operation-seam wrapper that
//! reaches the framework's refusal tail with it are
//! [`service_common::response_media_type`], the tree's one statement of them. This module
//! holds only what is this SERVICE's: the pair its responses are re-announced across, named
//! once so every seam in [`crate::service`] reads the same two values. Which seams those
//! are, and the one response no author seam reaches, are in that file's header.

use service_common::response_media_type::{ResponseMediaType, AWS_JSON_1_0, AWS_JSON_1_1};

/// The pair this service's responses are re-announced across: the media type its model's
/// protocol trait implies, and the one its recorded wire announces.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1);
