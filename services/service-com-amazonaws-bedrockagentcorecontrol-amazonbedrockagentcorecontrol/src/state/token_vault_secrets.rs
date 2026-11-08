// SPDX-License-Identifier: CC0-1.0

//! Where the token vault's credential providers keep their credentials.
//!
//! The one store and the one resolution every provider plane of this vault
//! shares live with the API key provider's own record
//! ([`super::api_key_credential_providers`]): `store_managed_secret` writes the
//! secret this service manages for a provider, `secrets_manager_client` reaches
//! the service that owns it, and the payment provider plane beside it calls both
//! rather than keeping a second copy. This module holds no code of its own, so
//! no plane can grow one.
