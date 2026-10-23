// SPDX-License-Identifier: CC0-1.0

//! What makes a string an address of the BITCOIN chain.
//!
//! `ChainAddress` is one Smithy type for every chain this service indexes — its
//! `^[-A-Za-z0-9]{13,74}$` admits an EVM address, a base58 Bitcoin address and a bech32
//! one alike — so a member that must carry a Bitcoin address in particular
//! (`ListFilteredTransactionEvents` "is only supported on the Bitcoin networks") can only
//! be checked by the operation that requires it, against the two encodings Bitcoin itself
//! spells an address in. Each encoding fixes the PREFIX, the ALPHABET and the LENGTH a
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
//! address it does not hold gets. The distinction this operation actually has to draw is
//! between an address of THIS chain and an address of another chain that `ChainAddress`
//! equally admits, and that one the alphabet and the prefix draw: an EVM address's `0x`
//! is outside the base58 alphabet, and `x` is outside bech32's.
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
