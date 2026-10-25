// SPDX-License-Identifier: CC0-1.0

//! What makes a string an address of a CHAIN this service indexes.
//!
//! `ChainAddress` is one Smithy type for every chain this service indexes — its
//! `^[-A-Za-z0-9]{13,74}$` admits an EVM address, a base58 Bitcoin address and a bech32
//! one alike — so which strings are addresses AT ALL, and of which chain, is a rule only
//! the service can apply: it checks a member against the encodings the chain the request
//! names spells an address in ([`is_address_of_network`]), and a member whose operation
//! fixes the chain itself (`ListFilteredTransactionEvents` "is only supported on the
//! Bitcoin networks") against that chain's ([`is_bitcoin_address`]).
//!
//! # Bitcoin
//!
//! Each of the chain's two encodings fixes the PREFIX, the ALPHABET and the LENGTH a
//! string may have:
//!
//! * base58 (BIP-13): a leading character that encodes the version byte — `1` (0x00,
//!   mainnet pay-to-public-key-hash), `3` (0x05, mainnet pay-to-script-hash), `m` or `n`
//!   (0x6f, the testnet P2PKH forms) and `2` (0xc4, testnet P2SH) — over the base58
//!   alphabet, which drops `0`, `O`, `I` and `l` precisely so characters cannot be
//!   confused, in the 26..=35 characters a version byte plus a 20-byte hash plus a
//!   four-byte checksum encodes to;
//! * bech32 and bech32m (BIP-173, BIP-350): the human-readable part `bc` on the mainnet
//!   or `tb` on testnet, the separator `1`, and a data part over bech32's own
//!   32-character alphabet, which drops `1`, `b`, `i` and `o`.
//!
//! This decides the SHAPE, not the checksum each encoding also carries: a string shaped
//! like an address of this chain but mistyped is accepted here and then answered with the
//! modelled empty page, because the index holds nothing under it — the same answer any
//! address it does not hold gets. The distinction that has to be drawn is between an
//! address of THIS chain and an address of another chain that `ChainAddress` equally
//! admits, and that one the alphabet and the prefix draw: an EVM address's `0x` is outside
//! the base58 alphabet, and `x` is outside bech32's.
//!
//! # EVM
//!
//! An account on an Ethereum network is the 20 bytes of its address, written as 40 hex
//! digits with or without the `0x` prefix — both spellings name the same account, which
//! is why [`super::normalize_contract_address`] keys the ledger by the digits alone. The
//! letter case of those digits carries no information about the account either (EIP-55
//! spends it on a checksum over the same 20 bytes), so any case is an address and
//! [`super::derived::normalize_address`] is what folds the spellings onto one account.
//!
//! Nothing here reads the service's state or a recorded value.

/// The base58 alphabet (BIP-13), in value order: the ASCII alphanumerics without `0`,
/// `O`, `I` and `l`.
const BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// The leading characters a base58 Bitcoin address can have, one per version byte of the
/// two address forms on each of the model's two Bitcoin networks: `1` (mainnet P2PKH),
/// `3` (mainnet P2SH), `m`/`n` (testnet P2PKH) and `2` (testnet P2SH).
const BASE58_PREFIXES: [char; 5] = ['1', '3', 'm', 'n', '2'];

/// The characters a base58 Bitcoin address can be long: a one-byte version, a 20-byte
/// hash and a four-byte checksum encode to 34 characters, and a payload whose leading
/// bytes are zero encodes to fewer — the published range is 26 to 35.
const BASE58_LENGTH: std::ops::RangeInclusive<usize> = 26..=35;

/// The human-readable parts of the bech32 address forms (BIP-173): `bc` on the Bitcoin
/// mainnet and `tb` on testnet, each followed by the `1` separator.
const BECH32_PREFIXES: [&str; 2] = ["bc1", "tb1"];

/// bech32's data alphabet (BIP-173): the ASCII alphanumerics without `1`, `b`, `i` and
/// `o`. Case is not significant, and a mixed-case string is invalid, so the comparison
/// below is made on one case.
const BECH32_ALPHABET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// The characters a bech32 Bitcoin address can be long: BIP-173 bounds the whole string
/// at 90, and `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` bounds it tighter still; the
/// shortest form is the 3-character prefix, a witness version, a 2-byte program and the
/// six-character checksum.
const BECH32_LENGTH: std::ops::RangeInclusive<usize> = 14..=74;

/// The prefix an EVM address is usually written with, and the hex digits that carry the
/// address itself: 20 bytes of account, so 40 digits.
///
/// The prefix is PRESENTATION rather than identity — the same 20 bytes with it and
/// without it name one account, which is why [`super::normalize_contract_address`] keys
/// the ledger by the digits alone — so a string is read with it or without it.
const EVM_PREFIX: &str = "0x";

/// See [`EVM_PREFIX`]: the digit count of a 20-byte address.
const EVM_DIGITS: usize = 40;

/// What this service answers a member that does not carry an address of the chain the
/// request named, one message per chain.
///
/// Request-independent, like every other message this service sends
/// ([`super::INVALID_PARAMETER_VALUES_MESSAGE`]): the member a rule was violated on is
/// `ValidationExceptionField$name` and the value that violated it is the caller's own, so
/// neither is interpolated into the sentence. Only the CHAIN varies, because only the
/// chain changes what the member had to carry.
pub const INVALID_BITCOIN_ADDRESS_MESSAGE: &str = "Please provide a valid bitcoin address.";

/// See [`INVALID_BITCOIN_ADDRESS_MESSAGE`].
pub const INVALID_ETHEREUM_ADDRESS_MESSAGE: &str = "Please provide a valid ethereum address.";

/// Whether `value` is an address of the chain `network` runs — the question an
/// address-typed member of a request that also names the network has to answer, since
/// `ChainAddress` admits the other chains' addresses too.
///
/// `network` is a `QueryNetwork` value, so it has already been held to the model's value
/// set by the time this is asked: the two Bitcoin values name the Bitcoin chain and the
/// rest name an EVM chain.
pub fn is_address_of_network(network: &str, value: &str) -> bool {
    if super::is_bitcoin(network) {
        is_bitcoin_address(value)
    } else {
        is_evm_address(value)
    }
}

/// What a refusal of an address member says on `network`; see
/// [`INVALID_BITCOIN_ADDRESS_MESSAGE`].
pub fn invalid_address_message(network: &str) -> &'static str {
    if super::is_bitcoin(network) {
        INVALID_BITCOIN_ADDRESS_MESSAGE
    } else {
        INVALID_ETHEREUM_ADDRESS_MESSAGE
    }
}

/// Whether `value` is an address of an EVM chain: the [`EVM_DIGITS`] hex digits of a
/// 20-byte account, in any letter case (EIP-55 spends the case on a checksum over those
/// same bytes) and with or without [`EVM_PREFIX`].
pub fn is_evm_address(value: &str) -> bool {
    let body = value
        .strip_prefix(EVM_PREFIX)
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    body.len() == EVM_DIGITS && body.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `value` is an address of the Bitcoin chain — in either of the encodings this
/// module's documentation lists, on either of the networks the model names.
///
/// The NETWORK is deliberately not part of the question: which network an address belongs
/// to is carried by its version byte or its human-readable part, and an address of the
/// other Bitcoin network is a well-formed Bitcoin address that this network's index
/// simply holds nothing under — the modelled empty page, not a refusal.
pub fn is_bitcoin_address(value: &str) -> bool {
    is_base58_address(value) || is_bech32_address(value)
}

/// The base58 half of [`is_bitcoin_address`].
fn is_base58_address(value: &str) -> bool {
    BASE58_LENGTH.contains(&value.chars().count())
        && value
            .chars()
            .next()
            .is_some_and(|first| BASE58_PREFIXES.contains(&first))
        && value
            .chars()
            .all(|character| BASE58_ALPHABET.contains(character))
}

/// The bech32 half of [`is_bitcoin_address`]. Case is not significant in bech32, but a
/// MIXED-case string is invalid there, so a string is read in whichever single case it is
/// written in.
fn is_bech32_address(value: &str) -> bool {
    let uniform_case = value == value.to_ascii_lowercase() || value == value.to_ascii_uppercase();
    if !uniform_case || !BECH32_LENGTH.contains(&value.chars().count()) {
        return false;
    }
    let lowercase = value.to_ascii_lowercase();
    let Some(data) = BECH32_PREFIXES
        .iter()
        .find_map(|prefix| lowercase.strip_prefix(prefix))
    else {
        return false;
    };
    !data.is_empty()
        && data
            .chars()
            .all(|character| BECH32_ALPHABET.contains(character))
}
