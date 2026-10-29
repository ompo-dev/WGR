use anyhow::Result;

/// Secrets deliberately have no Debug or Serialize implementation.
pub struct Secret(pub String);
pub trait CredentialStore {
    fn put(&self, key: &str, secret: &Secret) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<Secret>>;
    fn delete(&self, key: &str) -> Result<()>;
}
pub struct WindowsVault;

#[cfg(windows)]
mod platform {
    use super::*;
    use windows::{
        Win32::{Foundation::ERROR_NOT_FOUND, Security::Credentials::*},
        core::PCWSTR,
    };
    fn target(key: &str) -> Vec<u16> {
        format!("WGR/{key}").encode_utf16().chain(Some(0)).collect()
    }
    impl CredentialStore for WindowsVault {
        fn put(&self, key: &str, secret: &Secret) -> Result<()> {
            anyhow::ensure!(secret.0.len() <= 2400, "Credencial muito longa");
            let name = target(key);
            let credential = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: windows::core::PWSTR(name.as_ptr().cast_mut()),
                CredentialBlobSize: secret.0.len() as u32,
                CredentialBlob: secret.0.as_ptr().cast_mut(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..Default::default()
            };
            unsafe { CredWriteW(&credential, 0)? };
            Ok(())
        }
        fn get(&self, key: &str) -> Result<Option<Secret>> {
            let name = target(key);
            let mut pointer = std::ptr::null_mut();
            match unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut pointer) }
            {
                Ok(()) => {
                    let credential = unsafe { &*pointer };
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            credential.CredentialBlob,
                            credential.CredentialBlobSize as usize,
                        )
                    };
                    let result = String::from_utf8(bytes.to_vec());
                    unsafe { CredFree(pointer.cast()) };
                    Ok(Some(Secret(result?)))
                }
                Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
        fn delete(&self, key: &str) -> Result<()> {
            let name = target(key);
            match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}
#[cfg(not(windows))]
impl CredentialStore for WindowsVault {
    fn put(&self, _: &str, _: &Secret) -> Result<()> {
        anyhow::bail!("Cofre disponível apenas no Windows")
    }
    fn get(&self, _: &str) -> Result<Option<Secret>> {
        anyhow::bail!("Cofre disponível apenas no Windows")
    }
    fn delete(&self, _: &str) -> Result<()> {
        anyhow::bail!("Cofre disponível apenas no Windows")
    }
}
