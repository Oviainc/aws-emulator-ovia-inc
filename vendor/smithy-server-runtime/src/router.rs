// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
// Re-export types needed for in-process HTTP client and macro-generated code
pub use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorSettings, SharedHttpConnector,
};
pub use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
pub use aws_smithy_runtime_api::http::{Request as HttpRequest, Response as HttpResponse};

/// Handler function type
type HandlerFn<I, O, E> = std::sync::Arc<
    dyn Fn(
            I,
            &http::request::Parts,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<O, E>> + Send>>
        + Send
        + Sync,
>;

/// Wrapper for handler functions that implements Debug
pub struct Handler<I, O, E> {
    inner: HandlerFn<I, O, E>,
}

impl<I, O, E> Handler<I, O, E> {
    pub fn new(f: HandlerFn<I, O, E>) -> Self {
        Self { inner: f }
    }

    pub fn call(
        &self,
        input: I,
        parts: &http::request::Parts,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<O, E>> + Send>> {
        (self.inner)(input, parts)
    }
}

impl<I, O, E> Clone for Handler<I, O, E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<I, O, E> std::fmt::Debug for Handler<I, O, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler").finish_non_exhaustive()
    }
}
