// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Wire format conversion traits for types that diverge from their Smithy representation.

/// Convert from a Smithy string representation to a Rust type.
pub trait FromSmithyString: Sized {
    type Err: std::error::Error + Send + Sync + 'static;
    fn from_smithy_string(s: &str) -> Result<Self, Self::Err>;
}

/// Identity implementation for String — allows using String fields where the model
/// declares an enum type (useful when the handler treats the value as an opaque string).
impl FromSmithyString for String {
    type Err = std::convert::Infallible;
    fn from_smithy_string(s: &str) -> Result<Self, Self::Err> {
        Ok(s.to_string())
    }
}
