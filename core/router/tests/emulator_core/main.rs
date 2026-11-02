// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The emulator's security properties, checked from outside the binary this build produced, each
//! test on its own emulator.

mod emulator_process;
mod https_exchange;
mod loopback_default;
mod published_files;
mod request_bodies;
mod request_signing;
mod required_signatures;
mod suite_support;
mod transport;
mod workspace_models;
