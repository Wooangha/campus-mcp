//! The cryptography of the SSO handoff: ECDH on P-256 to agree a secret, and
//! SEED-CBC under a key derived from it to seal the credentials.

use openssl::provider::Provider;
use openssl::symm::{Cipher, Crypter, Mode};
use p256::{
    AffinePoint, EncodedPoint, ProjectivePoint, SecretKey,
    elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint},
};

use crate::services::lms::plms::PlmsError;

pub(crate) fn shared_point(secret: &SecretKey, qx: &str, qy: &str) -> Result<Vec<u8>, PlmsError> {
    let x = hex::decode(qx)?;
    let y = hex::decode(qy)?;
    if x.len() != 32 || y.len() != 32 {
        return Err(PlmsError::InvalidSsoResponse);
    }
    let encoded =
        EncodedPoint::from_affine_coordinates(x.as_slice().into(), y.as_slice().into(), false);
    let point = Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded))
        .ok_or(PlmsError::InvalidSsoResponse)?;
    let scalar = *secret.to_nonzero_scalar().as_ref();
    Ok((ProjectivePoint::from(point) * scalar)
        .to_affine()
        .to_encoded_point(false)
        .as_bytes()[1..]
        .to_vec())
}

/// SEED key and IV, taken from the shared point the way the SSO login script
/// derives them: the first half of X, then the first half of Y.
pub(crate) fn seed_key_iv(shared: &[u8]) -> Result<(&[u8], &[u8]), PlmsError> {
    let (x, y) = shared
        .split_at_checked(32)
        .ok_or(PlmsError::InvalidSsoResponse)?;
    let key = x.get(..16).ok_or(PlmsError::InvalidSsoResponse)?;
    let iv = y.get(..16).ok_or(PlmsError::InvalidSsoResponse)?;
    Ok((key, iv))
}

/// SEED-CBC with the padding the SSO login script uses: zero bytes followed by
/// the pad length, which is not PKCS#7. Confirmed against the live SSO endpoint.
pub(crate) fn seed_encrypt(plain: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, PlmsError> {
    let _legacy = Provider::load(None, "legacy")?;
    let padding = 16 - plain.len() % 16;
    let mut input = plain.to_vec();
    input.resize(input.len() + padding - 1, 0);
    input.push(padding as u8);
    let cipher = Cipher::seed_cbc();
    let mut crypter = Crypter::new(cipher, Mode::Encrypt, key, Some(iv))?;
    crypter.pad(false);
    let mut output = vec![0; input.len() + cipher.block_size()];
    let length =
        crypter.update(&input, &mut output)? + crypter.finalize(&mut output[input.len()..])?;
    output.truncate(length);
    Ok(output)
}
