use std::ops::Deref;

use data_encoding::BASE64URL_NOPAD;
use jsonwebkey::JsonWebKey;
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private, Public},
    rsa::Padding,
    sha::Sha256,
    sign,
};

pub trait KeyManagerAccess<KeyManager>
where
    KeyManager: self::KeyManager,
{
    fn get_key_manager(&self) -> &KeyManager;
}

pub trait KeyManager {
    fn bundler_address(&self) -> &str; // FIXME: replace with Address
    fn validator_address(&self) -> &str; // FIXME: replace with Address
    fn validator_sign(&self, data: &[u8]) -> Vec<u8>;
    // FIXME: return Result
    fn verify_bundler_signature(&self, data: &[u8], sig: &[u8]) -> bool;
    // FIXME: return Result
    fn verify_validator_signature(&self, data: &[u8], sig: &[u8]) -> bool;
}

impl<T, K> KeyManager for T
where
    K: KeyManager + 'static,
    T: Deref<Target = K>,
{
    fn bundler_address(&self) -> &str {
        self.deref().bundler_address()
    }

    fn validator_address(&self) -> &str {
        self.deref().validator_address()
    }

    fn validator_sign(&self, data: &[u8]) -> Vec<u8> {
        self.deref().validator_sign(data)
    }

    fn verify_bundler_signature(&self, data: &[u8], sig: &[u8]) -> bool {
        self.deref().verify_bundler_signature(data, sig)
    }

    fn verify_validator_signature(&self, data: &[u8], sig: &[u8]) -> bool {
        self.deref().verify_validator_signature(data, sig)
    }
}

pub fn split_jwk(jwk: &JsonWebKey) -> (PKey<Private>, PKey<Public>, String) {
    let priv_key = {
        let der = jwk.key.try_to_der().unwrap();
        PKey::private_key_from_der(der.as_slice()).unwrap()
    };
    let pub_key = {
        let pub_key_part = jwk.key.to_public().unwrap();
        let der = pub_key_part.try_to_der().unwrap();
        PKey::public_key_from_der(der.as_slice()).unwrap()
    };
    let mut hasher = Sha256::new();
    hasher.update(&pub_key.rsa().unwrap().n().to_vec());
    let hash = hasher.finish();
    let address = BASE64URL_NOPAD.encode(&hash);
    (priv_key, pub_key, address)
}

pub fn split_public_only_jwk(jwk: &JsonWebKey) -> (PKey<Public>, String) {
    let der = if jwk.key.is_private() {
        let pub_key = jwk.key.to_public().unwrap();
        pub_key.try_to_der().unwrap()
    } else {
        jwk.key.try_to_der().unwrap()
    };
    let pub_key = PKey::public_key_from_der(der.as_slice()).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&pub_key.rsa().unwrap().n().to_vec());
    let hash = hasher.finish();
    let address = BASE64URL_NOPAD.encode(&hash);
    (pub_key, address)
}

pub trait InMemoryKeyManagerConfig {
    fn bundler_jwk(&self) -> &JsonWebKey;
    fn validator_jwk(&self) -> &JsonWebKey;
}

pub struct InMemoryKeyManager {
    bundler_address: String,
    bundler_public: PKey<Public>,
    validator_address: String,
    validator_public: PKey<Public>,
    validator_private: PKey<Private>,
}

impl InMemoryKeyManager {
    pub fn new<Config>(config: &Config) -> Self
    where
        Config: InMemoryKeyManagerConfig,
    {
        let bundler_jwk = config.bundler_jwk();
        let validator_jwk = config.validator_jwk();

        let (bundler_public, bundler_address) = split_public_only_jwk(bundler_jwk);
        let (validator_private, validator_public, validator_address) = split_jwk(validator_jwk);

        Self {
            bundler_address,
            bundler_public,
            validator_address,
            validator_private,
            validator_public,
        }
    }
}

impl KeyManager for InMemoryKeyManager {
    fn bundler_address(&self) -> &str {
        &self.bundler_address
    }

    fn validator_address(&self) -> &str {
        &self.validator_address
    }

    // TODO: should this return Result?
    // When returning Result, caller can decide what needs to be done if
    // this call fails, instead of panicking here.
    fn validator_sign(&self, data: &[u8]) -> Vec<u8> {
        let mut signer =
            sign::Signer::new(MessageDigest::sha256(), &self.validator_private).unwrap();
        signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        signer.update(data).unwrap();
        signer.sign_to_vec().unwrap()
    }

    fn verify_bundler_signature(&self, data: &[u8], sig: &[u8]) -> bool {
        let mut verifier =
            sign::Verifier::new(MessageDigest::sha256(), &self.bundler_public).unwrap();
        verifier.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        verifier.update(data).unwrap();
        // TODO: we shouldn't probably hide errors here, at least we should log them
        verifier.verify(sig).unwrap_or(false)
    }

    fn verify_validator_signature(&self, data: &[u8], sig: &[u8]) -> bool {
        let mut verifier =
            sign::Verifier::new(MessageDigest::sha256(), &self.validator_public).unwrap();
        verifier.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        verifier.update(data).unwrap();
        // TODO: we shouldn't probably hide errors here, at least we should log them
        verifier.verify(sig).unwrap_or(false)
    }
}

#[cfg(test)]
pub mod test_utils {
    use data_encoding::BASE64URL_NOPAD;
    use jsonwebkey::{JsonWebKey, Key, PublicExponent, RsaPrivate, RsaPublic};
    use openssl::pkey::{PKey, Private, Public};
    use openssl::rsa::Rsa;
    use openssl::sha::Sha256;

    use super::{split_jwk, split_public_only_jwk, InMemoryKeyManager};

    pub fn test_keys() -> (InMemoryKeyManager, PKey<Private>) {
        let (bundler_jwk, bundler_private) = bundler_key();
        let validator_jwk = validator_key();

        let (bundler_public, bundler_address) = split_public_only_jwk(&bundler_jwk);
        let (validator_private, validator_public, validator_address) = split_jwk(&validator_jwk);

        (
            InMemoryKeyManager {
                bundler_address,
                bundler_public,
                validator_address,
                validator_private,
                validator_public,
            },
            bundler_private,
        )
    }

    pub fn bundler_key() -> (JsonWebKey, PKey<Private>) {
        let rsa = Rsa::generate(2048).unwrap();
        let n = rsa.n().to_vec().into();

        let pkey = PKey::from_rsa(rsa).unwrap();
        let private_der = pkey.private_key_to_der().unwrap();

        (
            JsonWebKey::new(Key::RSA {
                public: RsaPublic {
                    e: PublicExponent,
                    n,
                },
                private: None,
            }),
            PKey::private_key_from_der(&private_der.as_slice()).unwrap(),
        )
    }

    pub fn validator_key() -> JsonWebKey {
        let rsa = Rsa::generate(2048).unwrap();

        JsonWebKey::new(Key::RSA {
            public: RsaPublic {
                e: PublicExponent,
                n: rsa.n().to_vec().into(),
            },
            private: Some(RsaPrivate {
                d: rsa.d().to_vec().into(),
                p: rsa.p().map(|v| v.to_vec().into()),
                q: rsa.q().map(|v| v.to_vec().into()),
                dp: rsa.dmp1().map(|v| v.to_vec().into()),
                dq: rsa.dmq1().map(|v| v.to_vec().into()),
                qi: rsa.iqmp().map(|v| v.to_vec().into()),
            }),
        })
    }

    pub fn to_private_key(key: &JsonWebKey) -> Result<PKey<Private>, ()> {
        let der: Vec<u8> = key.key.try_to_der().map_err(|err| {
            eprintln!("Failed to extract der: {:?}", err);
            ()
        })?;
        PKey::private_key_from_der(der.as_slice()).map_err(|err| {
            eprintln!("Failed to extract public key from der: {:?}", err);
            ()
        })
    }

    pub fn to_public_key(jwk: &JsonWebKey) -> Result<PKey<Public>, ()> {
        let der = if jwk.key.is_private() {
            let pub_key = jwk.key.to_public().ok_or_else(|| {
                eprintln!("Key has no public part");
            })?;
            pub_key.try_to_der().map_err(|err| {
                eprintln!("Failed to extract der: {:?}", err);
                ()
            })?
        } else {
            jwk.key.try_to_der().map_err(|err| {
                eprintln!("Failed to extract der: {:?}", err);
                ()
            })?
        };
        PKey::public_key_from_der(der.as_slice()).map_err(|err| {
            eprintln!("Failed to extract public key from der: {:?}", err);
            ()
        })
    }

    pub fn to_address(key: &JsonWebKey) -> Result<String, ()> {
        let pub_key: PKey<Public> = to_public_key(key)?;
        let mut hasher = Sha256::new();
        hasher.update(&pub_key.rsa().unwrap().n().to_vec());
        let hash = hasher.finish();
        Ok(BASE64URL_NOPAD.encode(&hash))
    }
}

#[cfg(test)]
mod tests {
    use openssl::hash::MessageDigest;
    use openssl::rsa::Padding;
    use openssl::sign::{Signer, Verifier};

    use super::test_utils::{
        bundler_key, to_address, to_private_key, to_public_key, validator_key,
    };

    #[test]
    fn extract_address_from_public_key_only_jwk() {
        let (jwk, _) = bundler_key();

        let address = to_address(&jwk);
        assert!(address.is_ok());
    }

    #[test]
    fn extract_address_from_private_key_containing_jwk() {
        let jwk = validator_key();

        let address = to_address(&jwk);
        assert!(address.is_ok());
    }

    #[test]
    fn get_public_key_from_public_key_only_jwk() {
        let (jwk, _) = bundler_key();

        let pub_key = to_public_key(&jwk);
        assert!(pub_key.is_ok());
    }

    #[test]
    fn get_public_key_from_private_key_containing_jwk() {
        let jwk = validator_key();

        let pub_key = to_public_key(&jwk);
        assert!(pub_key.is_ok());
    }

    #[test]
    fn get_private_key_from_private_key_containing_jwk() {
        let jwk = validator_key();

        let key = to_private_key(&jwk);
        assert!(key.is_ok());
    }

    #[test]
    fn get_private_key_fails_for_public_key_only_jwk() {
        let (jwk, _) = bundler_key();

        let key = to_private_key(&jwk);
        assert!(key.is_err());
    }

    #[test]
    fn test_signing_with_bundler_key() {
        let (jwk, signing_key) = bundler_key();

        let data = b"hello, world!";

        // Sign the data
        let mut signer = Signer::new(MessageDigest::sha256(), &signing_key).unwrap();
        signer.update(data).unwrap();
        let signature = signer.sign_to_vec().unwrap();

        // Verify the data
        let pub_key = to_public_key(&jwk).unwrap();
        let mut verifier = Verifier::new(MessageDigest::sha256(), &pub_key).unwrap();
        verifier.update(data).unwrap();
        assert!(verifier.verify(&signature).unwrap());
    }

    #[test]
    fn test_signing_with_validator_key() {
        let jwk = validator_key();

        let data = b"hello, world!";

        // Sign the data
        let priv_key = to_private_key(&jwk).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &priv_key).unwrap();
        signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        signer.update(data).unwrap();
        let signature = signer.sign_to_vec().unwrap();

        // Verify the data
        let pub_key = to_public_key(&jwk).unwrap();
        let mut verifier = Verifier::new(MessageDigest::sha256(), &pub_key).unwrap();
        verifier.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        verifier.update(data).unwrap();
        assert!(verifier.verify(&signature).unwrap());
    }
}
