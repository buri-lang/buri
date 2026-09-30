//! `core/crypto`'s sealing and signature checks, through `ring`.
//!
//! Four private intrinsics sit under `crypto.seal`, `crypto.open`,
//! `crypto.verifyEs256` and `crypto.verifyEd25519`. The Buri side frames the
//! sealed value and parses keys; these do the arithmetic. `runtime.js` answers
//! the same four keys in JavaScript, and the conformance corpus holds both to
//! the RFC 8439, RFC 8032 and RFC 7515 vectors.
//!
//! Behind the `crypto` feature, beside `entropy.rs`. `ring` is already in every
//! `net` archive as `rustls`'s provider, so this adds no crate.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::signature::{ECDSA_P256_SHA256_FIXED, ED25519, UnparsedPublicKey};

use crate::value::{list_of_bytes, BuriList};
use crate::BURI_OK;

/// Borrow a `[U8]` argument: `ptr` and `len`, per `lib.rs` §2 rule 1.
///
/// # Safety
/// `ptr` must be readable for `len` bytes, or null with a zero length.
unsafe fn octets<'a>(ptr: *const u8, len: u64) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
    // SAFETY: the caller promises `len` readable bytes.
    unsafe { std::slice::from_raw_parts(ptr, len as usize) }
}

fn sealing_key(key: &[u8]) -> Option<LessSafeKey> {
    UnboundKey::new(&CHACHA20_POLY1305, key).ok().map(LessSafeKey::new)
}

/// Zero a buffer that held plaintext, so a freed allocation keeps no copy.
fn wipe(buffer: &mut [u8]) {
    for b in buffer.iter_mut() {
        // SAFETY: `b` is a valid, aligned `u8` borrowed from `buffer`.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

/// `crypto.chacha20Poly1305Seal(ctx, key, nonce, plaintext, aad) -> [U8]` —
/// the ciphertext with its 16-byte tag appended.
///
/// A key that is not 32 bytes or a nonce that is not 12 aborts, with the
/// sentences `$crypto_chacha20Poly1305Seal` writes.
///
/// # Safety
/// Each `ptr`/`len` pair describes a readable range; `out` is writable and
/// aligned for a [`BuriList`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_crypto_chacha20_poly1305_seal(
    key_ptr: *const u8,
    key_len: u64,
    nonce_ptr: *const u8,
    nonce_len: u64,
    plaintext_ptr: *const u8,
    plaintext_len: u64,
    aad_ptr: *const u8,
    aad_len: u64,
    out: *mut BuriList,
) {
    // SAFETY: the caller promises every range.
    let (key, nonce, plaintext, aad) = unsafe {
        (
            octets(key_ptr, key_len),
            octets(nonce_ptr, nonce_len),
            octets(plaintext_ptr, plaintext_len),
            octets(aad_ptr, aad_len),
        )
    };
    let Some(key) = sealing_key(key) else { crate::abort::die(&[b"a sealing key is 32 bytes"]) };
    let Ok(nonce) = Nonce::try_assume_unique_for_key(nonce) else {
        crate::abort::die(&[b"a sealing nonce is 12 bytes"])
    };
    let mut buffer = plaintext.to_vec();
    if key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut buffer).is_err() {
        // Only a plaintext past ChaCha20's 256 GiB block counter gets here.
        crate::abort::die(&[b"a sealed value is at most 256 GiB"]);
    }
    // SAFETY: the caller promises a writable, aligned destination.
    unsafe { out.write(list_of_bytes(&buffer)) };
}

/// `crypto.chacha20Poly1305Open(ctx, key, nonce, sealed, aad) -> Option<[U8]>`
/// — `lib.rs` §2 rule 3.
///
/// `.None` for a wrong-sized key or nonce, a value shorter than a tag, and
/// every tag that does not match. `ring` checks the tag before it decrypts, and
/// nothing reaches `out` unless it matched.
///
/// # Safety
/// As [`buri_rt_crypto_chacha20_poly1305_seal`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn buri_rt_crypto_chacha20_poly1305_open(
    key_ptr: *const u8,
    key_len: u64,
    nonce_ptr: *const u8,
    nonce_len: u64,
    sealed_ptr: *const u8,
    sealed_len: u64,
    aad_ptr: *const u8,
    aad_len: u64,
    out: *mut BuriList,
) -> i32 {
    // SAFETY: the caller promises every range.
    let (key, nonce, sealed, aad) = unsafe {
        (
            octets(key_ptr, key_len),
            octets(nonce_ptr, nonce_len),
            octets(sealed_ptr, sealed_len),
            octets(aad_ptr, aad_len),
        )
    };
    let Some(key) = sealing_key(key) else { return 0 };
    let Ok(nonce) = Nonce::try_assume_unique_for_key(nonce) else { return 0 };
    let mut buffer = sealed.to_vec();
    let answer = match key.open_in_place(nonce, Aad::from(aad), &mut buffer) {
        Ok(plaintext) => {
            // SAFETY: the caller promises a writable, aligned destination.
            unsafe { out.write(list_of_bytes(plaintext)) };
            BURI_OK
        }
        Err(_) => 0,
    };
    wipe(&mut buffer);
    answer
}

/// `crypto.ecdsaP256Sha256Verify(key, message, signature) -> Bool` — `key` is
/// the 65-byte uncompressed point and `signature` is `r ++ s`, as JWS writes
/// it.
///
/// # Safety
/// Each `ptr`/`len` pair describes a readable range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_crypto_ecdsa_p256_sha256_verify(
    key_ptr: *const u8,
    key_len: u64,
    message_ptr: *const u8,
    message_len: u64,
    signature_ptr: *const u8,
    signature_len: u64,
) -> u8 {
    // SAFETY: the caller promises every range.
    let (key, message, signature) = unsafe {
        (
            octets(key_ptr, key_len),
            octets(message_ptr, message_len),
            octets(signature_ptr, signature_len),
        )
    };
    let key = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key);
    u8::from(key.verify(message, signature).is_ok())
}

/// `crypto.ed25519Verify(key, message, signature) -> Bool` — RFC 8032's
/// 32-byte key and 64-byte signature.
///
/// # Safety
/// As [`buri_rt_crypto_ecdsa_p256_sha256_verify`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buri_rt_crypto_ed25519_verify(
    key_ptr: *const u8,
    key_len: u64,
    message_ptr: *const u8,
    message_len: u64,
    signature_ptr: *const u8,
    signature_len: u64,
) -> u8 {
    // SAFETY: the caller promises every range.
    let (key, message, signature) = unsafe {
        (
            octets(key_ptr, key_len),
            octets(message_ptr, message_len),
            octets(signature_ptr, signature_len),
        )
    };
    let key = UnparsedPublicKey::new(&ED25519, key);
    u8::from(key.verify(message, signature).is_ok())
}
