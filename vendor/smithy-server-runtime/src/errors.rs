// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Per-operation error trait.
//!
//! Operation error enums implement [`OperationError`] so the framework can
//! find the schema and shape name of the active variant at runtime.

use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;

/// Trait for operation-error enums. The dispatch layer uses this to obtain
/// the schema and `__type` for the active variant.
pub trait OperationError: std::error::Error + Send + Sync + 'static {
    /// Returns a `&dyn SerializableStruct` view of the active variant.
    fn as_serializable(&self) -> &dyn SerializableStruct;
    /// Returns the schema of the active variant.
    fn variant_schema(&self) -> &'static Schema;
    /// Returns the `__type` value for the active variant — its Smithy
    /// shape name without the namespace.
    fn shape_name(&self) -> &'static str;
    /// HTTP status code for this error variant (400 for client, 500 for server).
    fn http_status(&self) -> u16;
    /// The error code the query protocols name this variant by, when the model
    /// gives one, which is often not the shape name (`NoSuchEntityException` is
    /// `NoSuchEntity`, `CredentialReportExpiredException` is `ReportExpired`).
    ///
    /// Two protocols read it, for different places. A native query service puts
    /// it in the body's `<Code>` element, because that is the only code its
    /// clients ever see. An `awsQueryCompatible` awsJson service keeps the
    /// awsJson body and carries this in the `x-amzn-query-error` header instead,
    /// for a client that asked for the legacy code.
    fn query_error_code(&self) -> Option<&'static str> {
        None
    }
}
