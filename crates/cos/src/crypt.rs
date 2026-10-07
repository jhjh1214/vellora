//! The Standard Security Handler (ISO 32000-2 §7.6; revisions 2–4 are ISO 32000-1 §7.6).
//!
//! [`Encryption::from_dict`] reads an `/Encrypt` dictionary, [`Encryption::authenticate`] checks a
//! password (the user password first, then the owner password) and gives a [`Decryptor`] that
//! decrypts strings and stream data per object. [`crate::ObjectStore`] drives all of this: it tries
//! the empty password on open and decrypts strings and streams when they are read. The writers
//! ([`crate::write`]) use [`Decryptor::encrypt_string`] and [`Decryptor::encrypt_stream`] to
//! encrypt what they write with the same key.
//!
//! # Supported
//!
//! | Revision | `/V` | Cipher | Key |
//! |---|---|---|---|
//! | 2 | 1 | RC4 | 40 bit |
//! | 3 | 2 | RC4 | 40–128 bit |
//! | 4 | 4 | crypt filters: RC4 or AES-128 (`/V2`, `/AESV2`) | 128 bit |
//! | 5 (deprecated Adobe extension) | 5 | AES-256 (`/AESV3`) | 256 bit |
//! | 6 | 5 | AES-256 (`/AESV3`) | 256 bit |
//!
//! Public-key handlers (`/Filter` other than `/Standard`) and `/V 3` are
//! [`EncryptionError::UnsupportedHandler`] / [`EncryptionError::UnsupportedVersion`].
//!
//! # Choices and tolerances
//!
//! - **Passwords are bytes.** Revisions 2–4 take them as `PDFDocEncoding` bytes (padded or cut to 32),
//!   revisions 5–6 as UTF-8 cut to 127 bytes. `SASLprep` (ISO 32000-2 §7.6.4.3.3) is the caller's
//!   job; `cos` does not normalise.
//! - **AES input is read leniently.** The IV is the first 16 bytes; a trailing partial block is
//!   dropped; if the PKCS#5 padding is not valid the plain text is kept as it is rather than
//!   failing, because one damaged string should not make an object unreadable. A body shorter than
//!   the IV decrypts to nothing.
//! - **`/Perms` is not verified** for revisions 5 and 6 (it only detects tampering with `/P`, and
//!   real files get it wrong).
//! - **Crypt filters:** `/StmF` and `/StrF` choose the method; a stream whose first filter is
//!   `/Crypt` uses the filter named in its `/DecodeParms /Name` (`/Identity` if absent, which
//!   leaves the data alone). `/EFF` (a separate filter for embedded files) is ignored: embedded
//!   files use `/StmF`. Cross-reference streams are never encrypted, and metadata streams are not
//!   when `/EncryptMetadata false` (§7.6.3.2).
//! - Output is never longer than the input, so decryption needs no extra limit.
//!
//! RC4 is implemented here (about 20 lines, checked against RFC 6229): the `rc4` crate fixes the
//! key length at compile time and keys here are 5–16 bytes.

use std::borrow::Cow;

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use aes::{Aes128, Aes256};
use md5::{Digest as _, Md5};
use sha2::{Sha256, Sha384, Sha512};

use crate::error::{EncryptionError, Error, Result};
use crate::object::{Dict, ObjRef, Object, ObjectKind};

/// ISO 32000-2 Table 22 / Algorithm 2 step (a): the password padding string.
const PASSWORD_PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// Revision 5 and 6 passwords are cut to this many UTF-8 bytes (§7.6.4.3.3).
const MAX_PASSWORD_R56: usize = 127;

/// How data is encrypted under one crypt filter (`/CFM`, §7.6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CryptMethod {
    /// Not encrypted (`/Identity`, or `/CFM /None`).
    Identity,
    /// RC4 with a per-object key (`/V2`, and every revision 2 and 3 file).
    Rc4,
    /// AES-128 in CBC mode with a per-object key (`/AESV2`).
    AesV2,
    /// AES-256 in CBC mode with the file key itself (`/AESV3`).
    AesV3,
}

/// Which password unlocked the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordRole {
    /// The user password (possibly empty).
    User,
    /// The owner password.
    Owner,
}

/// What the `/Encrypt` dictionary says, for display (`vellora inspect`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionInfo {
    /// The `/V` entry: the algorithm version.
    pub version: u32,
    /// The `/R` entry: the security handler revision.
    pub revision: u32,
    /// Length of the file encryption key in bits.
    pub key_bits: u32,
    /// The `/P` entry as a signed 32-bit permission mask (bit 3 is print, and so on).
    pub permissions: i32,
    /// Method for streams (`/StmF`).
    pub stream_method: CryptMethod,
    /// Method for strings (`/StrF`).
    pub string_method: CryptMethod,
    /// Whether the document metadata is encrypted (`/EncryptMetadata`, only meaningful from
    /// revision 4 on; always `true` before).
    pub encrypt_metadata: bool,
}

/// A parsed `/Encrypt` dictionary of the Standard Security Handler. It holds no key; see
/// [`authenticate`](Self::authenticate).
#[derive(Debug, Clone)]
pub struct Encryption {
    version: u32,
    revision: u32,
    key_bytes: usize,
    permissions: i32,
    owner: Vec<u8>,
    user: Vec<u8>,
    owner_key: Vec<u8>,
    user_key: Vec<u8>,
    encrypt_metadata: bool,
    id0: Vec<u8>,
    /// Named crypt filters from `/CF`.
    filters: Vec<(Vec<u8>, CryptMethod)>,
    stream_method: CryptMethod,
    string_method: CryptMethod,
}

fn malformed(what: &'static str) -> Error {
    EncryptionError::Malformed(what).into()
}

fn integer(dict: &Dict<'_>, key: &[u8]) -> Option<i64> {
    match dict.get(key)?.kind {
        ObjectKind::Integer(n) => Some(n),
        _ => None,
    }
}

fn name<'d>(dict: &'d Dict<'_>, key: &[u8]) -> Option<&'d [u8]> {
    match &dict.get(key)?.kind {
        ObjectKind::Name(n) => Some(n.as_ref()),
        _ => None,
    }
}

fn string<'d>(dict: &'d Dict<'_>, key: &[u8]) -> Option<&'d [u8]> {
    match &dict.get(key)?.kind {
        ObjectKind::String(s) => Some(s.as_ref()),
        _ => None,
    }
}

/// A string of at least `min` bytes, or an error.
fn required_string(dict: &Dict<'_>, key: &[u8], min: usize, what: &'static str) -> Result<Vec<u8>> {
    match string(dict, key) {
        Some(s) if s.len() >= min => Ok(s.to_vec()),
        _ => Err(malformed(what)),
    }
}

fn prefix(bytes: &[u8], n: usize) -> &[u8] {
    bytes.get(..n).unwrap_or(bytes)
}

fn xor_each(key: &[u8], with: u8) -> Vec<u8> {
    key.iter().map(|b| b ^ with).collect()
}

impl Encryption {
    /// Reads an `/Encrypt` dictionary. `id0` is the first string of the trailer's `/ID` (empty if
    /// there is none). Indirect values inside `dict` must already be resolved: an unresolved
    /// reference counts as a missing entry.
    ///
    /// # Errors
    /// [`Error::Encryption`] when the handler, version or revision is not supported or the
    /// dictionary is not usable.
    pub fn from_dict(dict: &Dict<'_>, id0: &[u8]) -> Result<Self> {
        if name(dict, b"Filter") != Some(b"Standard") {
            return Err(EncryptionError::UnsupportedHandler.into());
        }
        let version = integer(dict, b"V").unwrap_or(0);
        let revision = integer(dict, b"R").ok_or_else(|| malformed("/R is missing"))?;
        if !matches!(version, 1 | 2 | 4 | 5) {
            return Err(EncryptionError::UnsupportedVersion { version }.into());
        }
        if !(2..=6).contains(&revision) {
            return Err(EncryptionError::UnsupportedRevision { revision }.into());
        }
        // Revisions 5 and 6 are the AES-256 handler and only it.
        if (revision >= 5) != (version == 5) {
            return Err(malformed("/V and /R do not belong together"));
        }
        let long_revision = revision >= 5;
        let (min_owner, min_key) = if long_revision { (48, 32) } else { (32, 0) };
        let owner = required_string(dict, b"O", min_owner, "/O is missing or too short")?;
        let user = required_string(dict, b"U", min_owner, "/U is missing or too short")?;
        let (owner_key, user_key) = if long_revision {
            (
                required_string(dict, b"OE", min_key, "/OE is missing or too short")?,
                required_string(dict, b"UE", min_key, "/UE is missing or too short")?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        // /P is a 32-bit value; some writers store it unsigned.
        let permissions = integer(dict, b"P").ok_or_else(|| malformed("/P is missing"))?;
        let permissions = u32::try_from(permissions & 0xFFFF_FFFF)
            .map_or(0, |p| i32::from_ne_bytes(p.to_ne_bytes()));
        let encrypt_metadata = revision < 4
            || !matches!(
                dict.get(b"EncryptMetadata").map(|o| &o.kind),
                Some(ObjectKind::Bool(false))
            );

        let filters = crypt_filters(dict)?;
        let (stream_filter, string_filter) = if version >= 4 {
            (
                name(dict, b"StmF").unwrap_or(b"Identity").to_vec(),
                name(dict, b"StrF").unwrap_or(b"Identity").to_vec(),
            )
        } else {
            (b"Identity".to_vec(), b"Identity".to_vec())
        };
        let method_of = |filter: &[u8]| -> Result<CryptMethod> {
            if version < 4 {
                return Ok(CryptMethod::Rc4);
            }
            if filter == b"Identity" {
                return Ok(CryptMethod::Identity);
            }
            filters
                .iter()
                .find(|(n, ..)| n == filter)
                .map(|&(_, m, _)| m)
                .ok_or_else(|| EncryptionError::UnknownCryptFilter.into())
        };
        let stream_method = method_of(&stream_filter)?;
        let string_method = method_of(&string_filter)?;

        let key_bytes = key_length(
            dict,
            version,
            revision,
            &filters,
            &stream_filter,
            stream_method,
        )?;
        Ok(Self {
            version: u32::try_from(version).unwrap_or(0),
            revision: u32::try_from(revision).unwrap_or(0),
            key_bytes,
            permissions,
            owner,
            user,
            owner_key,
            user_key,
            encrypt_metadata,
            id0: id0.to_vec(),
            filters: filters.into_iter().map(|f| (f.0, f.1)).collect(),
            stream_method,
            string_method,
        })
    }

    /// The facts about the encryption a UI or `inspect` shows.
    #[must_use]
    pub fn info(&self) -> EncryptionInfo {
        EncryptionInfo {
            version: self.version,
            revision: self.revision,
            key_bits: u32::try_from(self.key_bytes * 8).unwrap_or(0),
            permissions: self.permissions,
            stream_method: self.stream_method,
            string_method: self.string_method,
            encrypt_metadata: self.encrypt_metadata,
        }
    }

    /// Checks `password` as the user password, then as the owner password, and returns the
    /// decryptor for the file encryption key it unlocks. The empty password is the usual first try.
    ///
    /// # Errors
    /// [`EncryptionError::IncorrectPassword`] if neither password matches.
    pub fn authenticate(&self, password: &[u8]) -> Result<Decryptor> {
        let found = if self.revision >= 5 {
            self.authenticate_long(password)
        } else {
            self.authenticate_short(password)
        };
        let (key, role) = found.ok_or(EncryptionError::IncorrectPassword)?;
        Ok(Decryptor {
            encryption: self.clone(),
            key,
            role,
        })
    }

    /// Revisions 2–4 (Algorithms 2, 4–7).
    fn authenticate_short(&self, password: &[u8]) -> Option<(Vec<u8>, PasswordRole)> {
        if let Some(key) = self.user_key_short(&pad_password(password)) {
            return Some((key, PasswordRole::User));
        }
        // Algorithm 7: the owner password decrypts /O into the padded user password.
        let mut digest = Md5::digest(pad_password(password));
        if self.revision >= 3 {
            for _ in 0..50 {
                digest = Md5::digest(digest);
            }
        }
        let rc4_key = prefix(&digest, self.key_bytes);
        let mut user_password = prefix(&self.owner, 32).to_vec();
        if self.revision == 2 {
            rc4(rc4_key, &mut user_password);
        } else {
            for round in (0..=19u8).rev() {
                rc4(&xor_each(rc4_key, round), &mut user_password);
            }
        }
        let key = self.user_key_short(&pad_password(&user_password))?;
        Some((key, PasswordRole::Owner))
    }

    /// The file key for a padded user password if it matches `/U` (Algorithms 2 and 4/5).
    fn user_key_short(&self, padded: &[u8; 32]) -> Option<Vec<u8>> {
        // Algorithm 2.
        let mut hash = Md5::new();
        hash.update(padded);
        hash.update(prefix(&self.owner, 32));
        hash.update(self.permissions.to_le_bytes());
        hash.update(&self.id0);
        if self.revision >= 4 && !self.encrypt_metadata {
            hash.update([0xFF; 4]);
        }
        let mut digest = hash.finalize();
        if self.revision >= 3 {
            for _ in 0..50 {
                digest = Md5::digest(prefix(&digest, self.key_bytes));
            }
        }
        let key = prefix(&digest, self.key_bytes).to_vec();

        // Algorithm 4 (revision 2) or 5 (revision 3 and later).
        let matches = if self.revision == 2 {
            let mut check = PASSWORD_PAD.to_vec();
            rc4(&key, &mut check);
            check == prefix(&self.user, 32)
        } else {
            let mut hash = Md5::new();
            hash.update(PASSWORD_PAD);
            hash.update(&self.id0);
            let mut check = hash.finalize().to_vec();
            rc4(&key, &mut check);
            for round in 1..=19u8 {
                rc4(&xor_each(&key, round), &mut check);
            }
            // Only the first 16 bytes are significant; the rest is arbitrary padding.
            prefix(&check, 16) == prefix(&self.user, 16)
        };
        matches.then_some(key)
    }

    /// Revisions 5 and 6 (ISO 32000-2 Algorithms 2.A, 11, 12). `/U` and `/O` are validation hash
    /// (32 bytes), validation salt (8) and key salt (8).
    fn authenticate_long(&self, password: &[u8]) -> Option<(Vec<u8>, PasswordRole)> {
        let password = prefix(password, MAX_PASSWORD_R56);
        let user = prefix(&self.user, 48);
        let (hash, salts) = (user.get(..32)?, user.get(32..48)?);
        if self.hash_long(password, salts.get(..8)?, &[]) == hash {
            let intermediate = self.hash_long(password, salts.get(8..)?, &[]);
            return Some((
                unwrap_key(&intermediate, &self.user_key)?,
                PasswordRole::User,
            ));
        }
        let owner = prefix(&self.owner, 48);
        let (hash, salts) = (owner.get(..32)?, owner.get(32..48)?);
        if self.hash_long(password, salts.get(..8)?, user) == hash {
            let intermediate = self.hash_long(password, salts.get(8..)?, user);
            return Some((
                unwrap_key(&intermediate, &self.owner_key)?,
                PasswordRole::Owner,
            ));
        }
        None
    }

    /// Revision 5 hashes with plain SHA-256; revision 6 with Algorithm 2.B.
    fn hash_long(&self, password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
        if self.revision == 5 {
            let mut hash = Sha256::new();
            hash.update(password);
            hash.update(salt);
            hash.update(udata);
            hash.finalize().to_vec()
        } else {
            hash_2b(password, salt, udata)
        }
    }
}

/// A crypt filter from `/CF`: name, method and `/Length` (bytes, or bits in some files).
type CryptFilter = (Vec<u8>, CryptMethod, Option<i64>);

/// `/CF`: name to method. A filter with an unknown `/CFM` is an error; a missing one is `None`.
fn crypt_filters(dict: &Dict<'_>) -> Result<Vec<CryptFilter>> {
    let Some(ObjectKind::Dict(cf)) = dict.get(b"CF").map(|o| &o.kind) else {
        return Ok(Vec::new());
    };
    let mut filters = Vec::new();
    for entry in &cf.entries {
        let Some(filter) = entry.value.as_dict() else {
            continue;
        };
        let method = match name(filter, b"CFM") {
            None | Some(b"None") => CryptMethod::Identity,
            Some(b"V2") => CryptMethod::Rc4,
            Some(b"AESV2") => CryptMethod::AesV2,
            Some(b"AESV3") => CryptMethod::AesV3,
            Some(_) => return Err(EncryptionError::UnsupportedCryptFilter.into()),
        };
        filters.push((entry.key.to_vec(), method, integer(filter, b"Length")));
    }
    Ok(filters)
}

/// The file key length in bytes. For `/V 4` it comes from the stream filter's `/Length` (bytes, but
/// some writers give bits), else the dictionary's `/Length` (bits).
fn key_length(
    dict: &Dict<'_>,
    version: i64,
    revision: i64,
    filters: &[CryptFilter],
    stream_filter: &[u8],
    stream_method: CryptMethod,
) -> Result<usize> {
    if version == 5 {
        return Ok(32);
    }
    if version == 1 || revision == 2 {
        return Ok(5);
    }
    if stream_method == CryptMethod::AesV2 {
        return Ok(16);
    }
    let mut bits = integer(dict, b"Length").unwrap_or(if version == 4 { 128 } else { 40 });
    if version == 4
        && let Some(&(_, _, Some(length))) = filters.iter().find(|(n, ..)| n == stream_filter)
    {
        bits = if length < 40 {
            length.saturating_mul(8)
        } else {
            length
        };
    }
    match usize::try_from(bits) {
        Ok(bits) if (40..=128).contains(&bits) && bits % 8 == 0 => Ok(bits / 8),
        _ => Err(malformed("/Length is not a key length of 40 to 128 bits")),
    }
}

/// Algorithm 2.A: the file key is the AES-256 decryption (zero IV, no padding) of `/UE` or `/OE`
/// under the intermediate key.
fn unwrap_key(intermediate: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    let mut buffer = prefix(wrapped, 32).to_vec();
    cbc::Decryptor::<Aes256>::new_from_slices(intermediate, &[0; 16])
        .ok()?
        .decrypt_padded_mut::<NoPadding>(&mut buffer)
        .ok()?;
    Some(buffer)
}

/// Algorithm 2.B: the revision 6 hash (ISO 32000-2 §7.6.4.3.4). At most 287 rounds, because the
/// loop ends once the round count passes 63 and the last byte (at most 255) stops being larger
/// than `round - 32`.
fn hash_2b(password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(password);
    hash.update(salt);
    hash.update(udata);
    let mut k = hash.finalize().to_vec();
    let mut round = 0usize;
    loop {
        let mut repeated = Vec::with_capacity((password.len() + k.len() + udata.len()) * 64);
        for _ in 0..64 {
            repeated.extend_from_slice(password);
            repeated.extend_from_slice(&k);
            repeated.extend_from_slice(udata);
        }
        // `k` is 32, 48 or 64 bytes: key = first 16, IV = next 16.
        let encrypted = match (k.get(..16), k.get(16..32)) {
            (Some(key), Some(iv)) => cbc::Encryptor::<Aes128>::new_from_slices(key, iv)
                .ok()
                .and_then(|cipher| {
                    let length = repeated.len();
                    cipher
                        .encrypt_padded_mut::<NoPadding>(&mut repeated, length)
                        .ok()
                        .map(<[u8]>::to_vec)
                }),
            _ => None,
        }
        .unwrap_or_default();
        // The first 16 bytes read as a big-endian number mod 3: 256 is 1 mod 3, so it is the byte sum.
        let selector = encrypted
            .iter()
            .take(16)
            .map(|&b| u32::from(b))
            .sum::<u32>()
            % 3;
        k = match selector {
            0 => Sha256::digest(&encrypted).to_vec(),
            1 => Sha384::digest(&encrypted).to_vec(),
            _ => Sha512::digest(&encrypted).to_vec(),
        };
        round += 1;
        let last = encrypted.last().copied().unwrap_or(0);
        if round >= 64 && usize::from(last) <= round - 32 {
            break;
        }
    }
    k.truncate(32);
    k
}

/// Algorithm 2 step (a): the password cut or padded to 32 bytes.
fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut padded = [0u8; 32];
    for (slot, byte) in padded
        .iter_mut()
        .zip(password.iter().chain(PASSWORD_PAD.iter()))
    {
        *slot = *byte;
    }
    padded
}

/// RC4: `data` is combined with the key stream by XOR (so it encrypts and decrypts). An empty key leaves the data alone.
fn rc4(key: &[u8], data: &mut [u8]) {
    if key.is_empty() {
        return;
    }
    let mut state: [u8; 256] = std::array::from_fn(|i| u8::try_from(i).unwrap_or(0));
    let mut j = 0u8;
    for (i, &k) in (0..256usize).zip(key.iter().cycle()) {
        j = j.wrapping_add(state[i]).wrapping_add(k);
        state.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0u8, 0u8);
    for byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[usize::from(i)]);
        state.swap(usize::from(i), usize::from(j));
        let k = state[usize::from(state[usize::from(i)].wrapping_add(state[usize::from(j)]))];
        *byte ^= k;
    }
}

/// AES-CBC decryption of `IV || blocks` with PKCS#5 padding; see the module notes for how bad
/// input is treated. `key` must be 16 or 32 bytes.
fn aes_cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let Some((iv, body)) = data.split_at_checked(16) else {
        return Vec::new();
    };
    let mut plain = prefix(body, body.len() / 16 * 16).to_vec();
    let decrypted = match key.len() {
        16 => cbc::Decryptor::<Aes128>::new_from_slices(key, iv)
            .map(|c| c.decrypt_padded_mut::<NoPadding>(&mut plain).is_ok()),
        32 => cbc::Decryptor::<Aes256>::new_from_slices(key, iv)
            .map(|c| c.decrypt_padded_mut::<NoPadding>(&mut plain).is_ok()),
        _ => return Vec::new(),
    };
    if decrypted != Ok(true) {
        return Vec::new();
    }
    if let Some(&pad) = plain.last()
        && (1..=16).contains(&pad)
        && let Some(start) = plain.len().checked_sub(usize::from(pad))
        && plain
            .get(start..)
            .is_some_and(|tail| tail.iter().all(|&b| b == pad))
    {
        plain.truncate(start);
    }
    plain
}

/// AES-CBC encryption with PKCS#5 padding; the output is `IV || blocks`. `key` must be 16 or 32
/// bytes.
fn aes_cbc_encrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let pad = 16 - data.len() % 16;
    let mut buffer = data.to_vec();
    buffer.resize(data.len() + pad, u8::try_from(pad).unwrap_or(16));
    let length = buffer.len();
    let encrypted = match key.len() {
        16 => cbc::Encryptor::<Aes128>::new_from_slices(key, iv).map(|c| {
            c.encrypt_padded_mut::<NoPadding>(&mut buffer, length)
                .is_ok()
        }),
        32 => cbc::Encryptor::<Aes256>::new_from_slices(key, iv).map(|c| {
            c.encrypt_padded_mut::<NoPadding>(&mut buffer, length)
                .is_ok()
        }),
        // Unreachable for keys made by `authenticate`; the data would otherwise go out in clear.
        _ => return Vec::new(),
    };
    if encrypted != Ok(true) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(16 + buffer.len());
    out.extend_from_slice(iv);
    out.extend_from_slice(&buffer);
    out
}

/// Holds the file encryption key of an authenticated document and decrypts with it.
#[derive(Debug, Clone)]
pub struct Decryptor {
    encryption: Encryption,
    key: Vec<u8>,
    role: PasswordRole,
}

impl Decryptor {
    /// Which password was accepted.
    #[must_use]
    pub fn role(&self) -> PasswordRole {
        self.role
    }

    /// The dictionary's facts (see [`Encryption::info`]).
    #[must_use]
    pub fn info(&self) -> EncryptionInfo {
        self.encryption.info()
    }

    /// Decrypts a string of object `id` (§7.6.2).
    #[must_use]
    pub fn decrypt_string<'d>(&self, id: ObjRef, data: &'d [u8]) -> Cow<'d, [u8]> {
        self.decrypt(self.encryption.string_method, id, data)
    }

    /// Decrypts the raw bytes of the stream object `id` with dictionary `dict`. The result is
    /// still encoded (filters are applied afterwards). Streams that are never encrypted are
    /// returned as they are: cross-reference streams, streams under the `/Identity` crypt filter,
    /// and metadata when `/EncryptMetadata` is false.
    ///
    /// # Errors
    /// [`EncryptionError::UnknownCryptFilter`] if the stream names a crypt filter that `/CF` does
    /// not define.
    pub fn decrypt_stream<'d>(
        &self,
        id: ObjRef,
        dict: &Dict<'_>,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>> {
        let method = self.stream_method(dict)?;
        Ok(self.decrypt(method, id, data))
    }

    /// The method that applies to the stream with dictionary `dict`; see
    /// [`decrypt_stream`](Self::decrypt_stream) for the streams that are never encrypted.
    fn stream_method(&self, dict: &Dict<'_>) -> Result<CryptMethod> {
        if name(dict, b"Type") == Some(b"XRef") {
            return Ok(CryptMethod::Identity);
        }
        Ok(match crypt_filter_name(dict) {
            Some(filter) => self.method_named(filter)?,
            None if !self.encryption.encrypt_metadata
                && name(dict, b"Type") == Some(b"Metadata") =>
            {
                CryptMethod::Identity
            }
            None => self.encryption.stream_method,
        })
    }

    /// Encrypts a string of object `id`: the inverse of [`decrypt_string`](Self::decrypt_string),
    /// with the same file key, for writers that append to an encrypted document.
    ///
    /// AES needs an initialisation vector. `cos` has no source of randomness, so the IV is
    /// derived from the key, the object and the plain text (SHA-256, first 16 bytes): output is
    /// deterministic, and, because the IV depends on the plain text, equal inputs are the only
    /// way to get equal IVs.
    #[must_use]
    pub fn encrypt_string<'d>(&self, id: ObjRef, data: &'d [u8]) -> Cow<'d, [u8]> {
        self.encrypt(self.encryption.string_method, id, data)
    }

    /// Encrypts the (already encoded) data of the stream `id` with dictionary `dict`, using the
    /// same rules as [`decrypt_stream`](Self::decrypt_stream) about which streams are left alone.
    ///
    /// # Errors
    /// [`EncryptionError::UnknownCryptFilter`] if the stream names a crypt filter that `/CF` does
    /// not define.
    pub fn encrypt_stream<'d>(
        &self,
        id: ObjRef,
        dict: &Dict<'_>,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>> {
        let method = self.stream_method(dict)?;
        Ok(self.encrypt(method, id, data))
    }

    fn encrypt<'d>(&self, method: CryptMethod, id: ObjRef, data: &'d [u8]) -> Cow<'d, [u8]> {
        match method {
            CryptMethod::Identity => Cow::Borrowed(data),
            CryptMethod::Rc4 => {
                let mut out = data.to_vec();
                rc4(&self.object_key(id, false), &mut out);
                Cow::Owned(out)
            }
            CryptMethod::AesV2 => Cow::Owned(aes_cbc_encrypt(
                &self.object_key(id, true),
                &self.synthetic_iv(id, data),
                data,
            )),
            CryptMethod::AesV3 => Cow::Owned(aes_cbc_encrypt(
                &self.key,
                &self.synthetic_iv(id, data),
                data,
            )),
        }
    }

    fn synthetic_iv(&self, id: ObjRef, data: &[u8]) -> [u8; 16] {
        let mut hash = Sha256::new();
        hash.update(b"vellora-aes-iv");
        hash.update(&self.key);
        hash.update(id.num.to_le_bytes());
        hash.update(id.generation.to_le_bytes());
        hash.update(data);
        let digest = hash.finalize();
        let mut iv = [0u8; 16];
        iv.copy_from_slice(prefix(&digest, 16));
        iv
    }

    fn method_named(&self, filter: &[u8]) -> Result<CryptMethod> {
        if filter == b"Identity" {
            return Ok(CryptMethod::Identity);
        }
        self.encryption
            .filters
            .iter()
            .find(|(n, _)| n == filter)
            .map(|&(_, m)| m)
            .ok_or_else(|| EncryptionError::UnknownCryptFilter.into())
    }

    fn decrypt<'d>(&self, method: CryptMethod, id: ObjRef, data: &'d [u8]) -> Cow<'d, [u8]> {
        match method {
            CryptMethod::Identity => Cow::Borrowed(data),
            CryptMethod::AesV3 => Cow::Owned(aes_cbc_decrypt(&self.key, data)),
            CryptMethod::Rc4 => {
                let mut out = data.to_vec();
                rc4(&self.object_key(id, false), &mut out);
                Cow::Owned(out)
            }
            CryptMethod::AesV2 => Cow::Owned(aes_cbc_decrypt(&self.object_key(id, true), data)),
        }
    }

    /// Algorithm 1: the key for one object, `MD5(key || num[3] || gen[2] || "sAlT"?)` cut to
    /// `min(key length + 5, 16)` bytes.
    fn object_key(&self, id: ObjRef, aes: bool) -> Vec<u8> {
        let mut hash = Md5::new();
        hash.update(&self.key);
        hash.update(id.num.to_le_bytes().get(..3).unwrap_or_default());
        hash.update(id.generation.to_le_bytes());
        if aes {
            hash.update(b"sAlT");
        }
        let digest = hash.finalize();
        prefix(&digest, (self.key.len() + 5).min(16)).to_vec()
    }
}

/// The crypt filter a stream asks for: its first `/Filter` is `/Crypt`, and `/DecodeParms /Name`
/// names the filter (`Identity` when it does not). `None` if the stream does not use `/Crypt`.
fn crypt_filter_name<'d>(dict: &'d Dict<'_>) -> Option<&'d [u8]> {
    let (first, parms) = match &dict.get(b"Filter")?.kind {
        ObjectKind::Name(n) => (n.as_ref(), dict.get(b"DecodeParms")),
        ObjectKind::Array(filters) => match &filters.first()?.kind {
            ObjectKind::Name(n) => (n.as_ref(), dict.get(b"DecodeParms")),
            _ => return None,
        },
        _ => return None,
    };
    if first != b"Crypt" {
        return None;
    }
    // With a filter array, /DecodeParms is an array too, and the first entry is this filter's.
    let parms = match parms.map(|p| &p.kind) {
        Some(ObjectKind::Array(all)) => all.first(),
        _ => parms,
    };
    Some(
        parms
            .and_then(Object::as_dict)
            .and_then(|p| name(p, b"Name"))
            .unwrap_or(b"Identity"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;
    use crate::parser::Parser;

    fn dict(source: &str) -> Dict<'static> {
        let limits = Limits::default();
        let object = Parser::new(source.as_bytes(), &limits)
            .parse_object()
            .expect("test dictionary parses");
        object.as_dict().expect("a dictionary").clone().into_owned()
    }

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut text, b| {
            write!(text, "{b:02x}").expect("writing to a String");
            text
        })
    }

    #[test]
    fn rc4_matches_rfc_6229_key_stream() {
        // RFC 6229, 40-bit key 0102030405, offset 0.
        let mut stream = [0u8; 16];
        rc4(&[1, 2, 3, 4, 5], &mut stream);
        assert_eq!(hex(&stream), "b2396305f03dc027ccc3524a0a1118a8");
        // And 128-bit key 0102..10.
        let key: Vec<u8> = (1..=16).collect();
        let mut stream = [0u8; 16];
        rc4(&key, &mut stream);
        assert_eq!(hex(&stream), "9ac7cc9a609d1ef7b2932899cde41b97");
        // Encrypting twice gives the plain text back; an empty key does nothing.
        let mut text = *b"hello";
        rc4(b"k", &mut text);
        assert_ne!(&text, b"hello");
        rc4(b"k", &mut text);
        assert_eq!(&text, b"hello");
        rc4(&[], &mut text);
        assert_eq!(&text, b"hello");
    }

    #[test]
    fn aes_cbc_decrypts_the_sp800_38a_vector() {
        // NIST SP 800-38A F.2.1, first block, with the IV in front.
        let key = hex_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let mut data = hex_bytes("000102030405060708090a0b0c0d0e0f");
        data.extend(hex_bytes("7649abac8119b246cee98e9b12e9197d"));
        // The plain text ends in 0x2a, which is not valid padding, so it is kept whole.
        assert_eq!(
            hex(&aes_cbc_decrypt(&key, &data)),
            "6bc1bee22e409f96e93d7e117393172a"
        );
    }

    fn hex_bytes(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// Test-only encryption: PKCS#5 padding, random-looking fixed IV.
    fn aes_cbc_encrypt(key: &[u8], plain: &[u8]) -> Vec<u8> {
        let iv = [7u8; 16];
        let pad = 16 - plain.len() % 16;
        let mut buffer = plain.to_vec();
        buffer.extend(std::iter::repeat_n(u8::try_from(pad).expect("pad"), pad));
        let length = buffer.len();
        let cipher = match key.len() {
            16 => cbc::Encryptor::<Aes128>::new_from_slices(key, &iv)
                .expect("key")
                .encrypt_padded_mut::<NoPadding>(&mut buffer, length)
                .expect("block aligned")
                .to_vec(),
            _ => cbc::Encryptor::<Aes256>::new_from_slices(key, &iv)
                .expect("key")
                .encrypt_padded_mut::<NoPadding>(&mut buffer, length)
                .expect("block aligned")
                .to_vec(),
        };
        [iv.to_vec(), cipher].concat()
    }

    #[test]
    fn aes_cbc_round_trips_and_strips_padding() {
        for key in [vec![9u8; 16], vec![5u8; 32]] {
            for len in [0usize, 1, 15, 16, 17, 100] {
                let plain: Vec<u8> = (0..len)
                    .map(|i| u8::try_from(i % 251).expect("byte"))
                    .collect();
                assert_eq!(
                    aes_cbc_decrypt(&key, &aes_cbc_encrypt(&key, &plain)),
                    plain,
                    "{len}"
                );
            }
        }
    }

    #[test]
    fn aes_cbc_tolerates_short_and_ragged_input() {
        let key = [1u8; 16];
        assert_eq!(aes_cbc_decrypt(&key, &[]), Vec::<u8>::new());
        assert_eq!(aes_cbc_decrypt(&key, &[0; 15]), Vec::<u8>::new());
        assert!(
            aes_cbc_decrypt(&key, &[0; 16]).is_empty(),
            "an IV alone is empty"
        );
        // A partial trailing block is dropped.
        let mut ragged = aes_cbc_encrypt(&key, b"0123456789abcdef");
        let whole = aes_cbc_decrypt(&key, &ragged);
        ragged.extend([1, 2, 3]);
        assert_eq!(aes_cbc_decrypt(&key, &ragged), whole);
        // A key of the wrong size cannot decrypt anything.
        assert_eq!(aes_cbc_decrypt(&[1; 7], &[0; 48]), Vec::<u8>::new());
    }

    #[test]
    fn pad_password_cuts_and_pads() {
        assert_eq!(pad_password(b""), PASSWORD_PAD);
        let padded = pad_password(b"abc");
        assert_eq!(&padded[..3], b"abc");
        assert_eq!(&padded[3..], &PASSWORD_PAD[..29]);
        let long = [b'x'; 40];
        assert_eq!(pad_password(&long), [b'x'; 32]);
    }

    #[test]
    fn dictionaries_that_cannot_be_used_are_typed_errors() {
        let kind = |source: &str| match Encryption::from_dict(&dict(source), b"id") {
            Err(Error::Encryption { kind }) => kind,
            other => panic!("expected an encryption error, got {other:?}"),
        };
        let o32 = "(".to_owned() + &"o".repeat(32) + ")";
        let base = format!("/Filter /Standard /O {o32} /U {o32} /P -4");
        assert_eq!(
            kind("<< /Filter /Adobe.PubSec /V 4 /R 4 >>"),
            EncryptionError::UnsupportedHandler
        );
        assert_eq!(
            kind(&format!("<< {base} /V 3 /R 3 >>")),
            EncryptionError::UnsupportedVersion { version: 3 }
        );
        assert_eq!(
            kind(&format!("<< {base} /V 2 /R 7 >>")),
            EncryptionError::UnsupportedRevision { revision: 7 }
        );
        assert!(matches!(
            kind(&format!("<< {base} /V 2 >>")),
            EncryptionError::Malformed(_)
        ));
        assert!(matches!(
            kind(&format!("<< {base} /V 5 /R 3 >>")),
            EncryptionError::Malformed(_)
        ));
        assert!(matches!(
            kind("<< /Filter /Standard /V 2 /R 3 /O (short) /U (short) /P -4 >>"),
            EncryptionError::Malformed(_)
        ));
        assert!(matches!(
            kind(&format!("<< {base} /V 2 /R 3 /Length 100 >>")),
            EncryptionError::Malformed(_)
        ));
        assert_eq!(
            kind(&format!(
                "<< {base} /V 4 /R 4 /CF << /A << /CFM /Weird >> >> >>"
            )),
            EncryptionError::UnsupportedCryptFilter
        );
        assert_eq!(
            kind(&format!("<< {base} /V 4 /R 4 /StmF /Missing >>")),
            EncryptionError::UnknownCryptFilter
        );
    }

    #[test]
    fn info_reports_methods_key_length_and_unsigned_permissions() {
        let o32 = "(".to_owned() + &"o".repeat(32) + ")";
        let source = format!(
            "<< /Filter /Standard /V 4 /R 4 /O {o32} /U {o32} /P 4294967292 /EncryptMetadata false \
             /CF << /StdCF << /CFM /AESV2 /Length 16 >> >> /StmF /StdCF /StrF /Identity >>"
        );
        let info = Encryption::from_dict(&dict(&source), b"")
            .expect("parses")
            .info();
        assert_eq!(info.version, 4);
        assert_eq!(info.revision, 4);
        assert_eq!(info.key_bits, 128);
        assert_eq!(info.permissions, -4);
        assert_eq!(info.stream_method, CryptMethod::AesV2);
        assert_eq!(info.string_method, CryptMethod::Identity);
        assert!(!info.encrypt_metadata);
    }

    #[test]
    fn crypt_filter_lengths_may_be_bytes_or_bits() {
        let o32 = "(".to_owned() + &"o".repeat(32) + ")";
        for (length, bits) in [(16, 128), (128, 128), (5, 40)] {
            let source = format!(
                "<< /Filter /Standard /V 4 /R 4 /O {o32} /U {o32} /P -4 \
                 /CF << /C << /CFM /V2 /Length {length} >> >> /StmF /C /StrF /C >>"
            );
            let info = Encryption::from_dict(&dict(&source), b"")
                .expect("parses")
                .info();
            assert_eq!(info.key_bits, bits, "/Length {length}");
        }
    }

    #[test]
    fn crypt_filter_name_reads_the_first_filter_only() {
        assert_eq!(crypt_filter_name(&dict("<< /Filter /FlateDecode >>")), None);
        assert_eq!(
            crypt_filter_name(&dict("<< /Filter /Crypt /DecodeParms << /Name /Std >> >>")),
            Some(&b"Std"[..])
        );
        assert_eq!(
            crypt_filter_name(&dict(
                "<< /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Std >> null] >>"
            )),
            Some(&b"Std"[..])
        );
        assert_eq!(
            crypt_filter_name(&dict("<< /Filter [/Crypt /FlateDecode] >>")),
            Some(&b"Identity"[..])
        );
        assert_eq!(
            crypt_filter_name(&dict("<< /Filter [/FlateDecode /Crypt] >>")),
            None
        );
        assert_eq!(crypt_filter_name(&dict("<< >>")), None);
    }
}
