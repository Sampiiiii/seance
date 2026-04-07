// Owns the macOS keychain backend for biometric-protected device secrets.
use core_foundation::{
    base::{CFGetTypeID, CFRelease, CFType, CFTypeRef, OSStatus, TCFType},
    boolean::CFBoolean,
    data::{CFData, CFDataRef},
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use security_framework::{
    access_control::SecAccessControl, base::Error as SecurityError, passwords::AccessControlOptions,
};
use security_framework_sys::{
    base::{errSecAuthFailed, errSecItemNotFound, errSecSuccess},
    item::{
        kSecAttrAccessControl, kSecAttrAccount, kSecAttrService, kSecClass,
        kSecClassGenericPassword, kSecReturnData, kSecUseDataProtectionKeychain, kSecValueData,
    },
    keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete},
};

use super::{DeviceSecretError, DeviceSecretStore, KEYRING_SERVICE_NAME};

const ERR_SEC_SUCCESS: OSStatus = errSecSuccess;
const ERR_SEC_AUTH_FAILED: OSStatus = errSecAuthFailed;
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = errSecItemNotFound;
const ERR_SEC_MISSING_ENTITLEMENT: OSStatus = -34018;
const ERR_SEC_NOT_AVAILABLE: OSStatus = -25291;
const USER_CANCELED_STATUS: OSStatus = -128;
const INTERACTION_NOT_ALLOWED_STATUS: OSStatus = -25308;
const OPERATION_PROMPT: &str = "Unlock Seance vault";

unsafe extern "C" {
    static kSecUseOperationPrompt: CFStringRef;
}

#[derive(Clone, Copy)]
enum AccessPolicy {
    UserPresence {
        prompt: &'static str,
    },
    #[cfg(test)]
    Plaintext,
}

impl AccessPolicy {
    fn prompt(self) -> Option<&'static str> {
        match self {
            Self::UserPresence { prompt } => Some(prompt),
            #[cfg(test)]
            Self::Plaintext => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MacosBiometricDeviceSecretStore {
    access_policy: AccessPolicy,
}

impl Default for MacosBiometricDeviceSecretStore {
    fn default() -> Self {
        Self {
            access_policy: AccessPolicy::UserPresence {
                prompt: OPERATION_PROMPT,
            },
        }
    }
}

impl MacosBiometricDeviceSecretStore {
    #[cfg(test)]
    fn non_interactive_for_tests() -> Self {
        Self {
            access_policy: AccessPolicy::Plaintext,
        }
    }
}

impl DeviceSecretStore for MacosBiometricDeviceSecretStore {
    fn get_secret(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceSecretError> {
        get_secret_protected(account, self.access_policy.prompt())
    }

    fn set_secret(&self, account: &str, secret: &[u8]) -> Result<(), DeviceSecretError> {
        match self.access_policy {
            AccessPolicy::UserPresence { .. } => set_secret_protected(
                account,
                secret,
                Some(
                    SecAccessControl::create_with_flags(AccessControlOptions::USER_PRESENCE.bits())
                        .map_err(|err| DeviceSecretError::Backend(err.to_string()))?,
                ),
            ),
            #[cfg(test)]
            AccessPolicy::Plaintext => set_secret_protected(account, secret, None),
        }
    }
}

fn set_secret_protected(
    account: &str,
    secret: &[u8],
    access_control: Option<SecAccessControl>,
) -> Result<(), DeviceSecretError> {
    delete_secret_protected(account)?;
    delete_secret_legacy(account)?;

    let query = protected_write_query(account, secret, access_control);
    let status = unsafe { SecItemAdd(query.as_concrete_TypeRef(), std::ptr::null_mut()) };

    match status {
        ERR_SEC_SUCCESS => Ok(()),
        code if is_runtime_unavailable_status(code) => {
            Err(DeviceSecretError::UnavailableInThisBuild)
        }
        code => Err(security_error(code)),
    }
}

fn get_secret_protected(
    account: &str,
    prompt: Option<&'static str>,
) -> Result<Option<Vec<u8>>, DeviceSecretError> {
    let query = protected_read_query(account, prompt);
    let mut result: CFTypeRef = std::ptr::null_mut();
    let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) };

    match status {
        ERR_SEC_SUCCESS => read_secret_bytes(result).map(Some),
        ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        code if should_fallback_to_passphrase(code) => Ok(None),
        code if is_runtime_unavailable_status(code) => {
            Err(DeviceSecretError::UnavailableInThisBuild)
        }
        code => Err(security_error(code)),
    }
}

fn delete_secret_protected(account: &str) -> Result<(), DeviceSecretError> {
    delete_matching_item(protected_delete_query(account))
}

fn delete_secret_legacy(account: &str) -> Result<(), DeviceSecretError> {
    delete_matching_item(legacy_delete_query(account))
}

fn delete_matching_item(query: CFDictionary<CFString, CFType>) -> Result<(), DeviceSecretError> {
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };

    match status {
        ERR_SEC_SUCCESS | ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        code if is_runtime_unavailable_status(code) => {
            Err(DeviceSecretError::UnavailableInThisBuild)
        }
        code => Err(security_error(code)),
    }
}

fn base_password_query(account: &str) -> Vec<(CFString, CFType)> {
    vec![
        (
            unsafe { CFString::wrap_under_get_rule(kSecClass) },
            unsafe { CFString::wrap_under_get_rule(kSecClassGenericPassword) }.into_CFType(),
        ),
        (
            unsafe { CFString::wrap_under_get_rule(kSecAttrService) },
            CFString::from(KEYRING_SERVICE_NAME).into_CFType(),
        ),
        (
            unsafe { CFString::wrap_under_get_rule(kSecAttrAccount) },
            CFString::from(account).into_CFType(),
        ),
    ]
}

fn protected_write_query(
    account: &str,
    secret: &[u8],
    access_control: Option<SecAccessControl>,
) -> CFDictionary<CFString, CFType> {
    let mut query = base_password_query(account);
    query.push((
        unsafe { CFString::wrap_under_get_rule(kSecUseDataProtectionKeychain) },
        CFBoolean::true_value().into_CFType(),
    ));
    query.push((
        unsafe { CFString::wrap_under_get_rule(kSecValueData) },
        CFData::from_buffer(secret).into_CFType(),
    ));

    if let Some(access_control) = access_control {
        query.push((
            unsafe { CFString::wrap_under_get_rule(kSecAttrAccessControl) },
            access_control.into_CFType(),
        ));
    }

    CFDictionary::from_CFType_pairs(&query)
}

fn protected_read_query(
    account: &str,
    prompt: Option<&'static str>,
) -> CFDictionary<CFString, CFType> {
    let mut query = base_password_query(account);
    query.push((
        unsafe { CFString::wrap_under_get_rule(kSecUseDataProtectionKeychain) },
        CFBoolean::true_value().into_CFType(),
    ));
    query.push((
        unsafe { CFString::wrap_under_get_rule(kSecReturnData) },
        CFBoolean::true_value().into_CFType(),
    ));

    if let Some(prompt) = prompt {
        query.push((
            unsafe { CFString::wrap_under_get_rule(kSecUseOperationPrompt) },
            CFString::from(prompt).into_CFType(),
        ));
    }

    CFDictionary::from_CFType_pairs(&query)
}

fn protected_delete_query(account: &str) -> CFDictionary<CFString, CFType> {
    let mut query = base_password_query(account);
    query.push((
        unsafe { CFString::wrap_under_get_rule(kSecUseDataProtectionKeychain) },
        CFBoolean::true_value().into_CFType(),
    ));
    CFDictionary::from_CFType_pairs(&query)
}

fn legacy_delete_query(account: &str) -> CFDictionary<CFString, CFType> {
    CFDictionary::from_CFType_pairs(&base_password_query(account))
}

fn read_secret_bytes(result: CFTypeRef) -> Result<Vec<u8>, DeviceSecretError> {
    if result.is_null() {
        return Err(DeviceSecretError::Backend(
            "keychain lookup returned no secret bytes".into(),
        ));
    }

    if unsafe { CFGetTypeID(result) } != CFData::type_id() {
        unsafe { CFRelease(result) };
        return Err(DeviceSecretError::Backend(
            "keychain lookup returned an unexpected value type".into(),
        ));
    }

    let data = unsafe { CFData::wrap_under_create_rule(result as CFDataRef) };
    Ok(data.bytes().to_vec())
}

fn should_fallback_to_passphrase(status: OSStatus) -> bool {
    matches!(
        status,
        ERR_SEC_AUTH_FAILED | USER_CANCELED_STATUS | INTERACTION_NOT_ALLOWED_STATUS
    )
}

fn is_runtime_unavailable_status(status: OSStatus) -> bool {
    matches!(status, ERR_SEC_MISSING_ENTITLEMENT | ERR_SEC_NOT_AVAILABLE)
}

fn security_error(status: OSStatus) -> DeviceSecretError {
    DeviceSecretError::Backend(SecurityError::from_code(status).to_string())
}

#[cfg(test)]
fn dictionary_has_key(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> bool {
    dict.find(&unsafe { CFString::wrap_under_get_rule(key) })
        .is_some()
}

#[cfg(test)]
fn dictionary_bool_value(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<bool> {
    let value = dict.find(&unsafe { CFString::wrap_under_get_rule(key) })?;
    value.downcast::<CFBoolean>().map(bool::from)
}

#[cfg(test)]
fn dictionary_string_value(
    dict: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Option<String> {
    let value = dict.find(&unsafe { CFString::wrap_under_get_rule(key) })?;
    value
        .downcast::<CFString>()
        .map(|value: CFString| value.to_string())
}

#[cfg(test)]
fn dictionary_data_value(
    dict: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Option<Vec<u8>> {
    let value = dict.find(&unsafe { CFString::wrap_under_get_rule(key) })?;
    value
        .downcast::<CFData>()
        .map(|value: CFData| value.bytes().to_vec())
}

#[cfg(test)]
fn uses_protected_keychain(dict: &CFDictionary<CFString, CFType>) -> bool {
    dictionary_bool_value(dict, unsafe { kSecUseDataProtectionKeychain }).unwrap_or(false)
}

#[cfg(test)]
fn has_return_data(dict: &CFDictionary<CFString, CFType>) -> bool {
    dictionary_bool_value(dict, unsafe { kSecReturnData }).unwrap_or(false)
}

#[cfg(test)]
fn read_prompt(dict: &CFDictionary<CFString, CFType>) -> Option<String> {
    dictionary_string_value(dict, unsafe { kSecUseOperationPrompt })
}

#[cfg(test)]
fn account_and_secret_from_query(dict: &CFDictionary<CFString, CFType>) -> (String, Vec<u8>) {
    (
        dictionary_string_value(dict, unsafe { kSecAttrAccount }).unwrap(),
        dictionary_data_value(dict, unsafe { kSecValueData }).unwrap(),
    )
}

#[cfg(test)]
fn has_access_control(dict: &CFDictionary<CFString, CFType>) -> bool {
    dictionary_has_key(dict, unsafe { kSecAttrAccessControl })
}

#[cfg(test)]
#[derive(Default)]
struct RecordingStore {
    deleted: std::sync::Mutex<Vec<&'static str>>,
    writes: std::sync::Mutex<Vec<(String, Vec<u8>, bool)>>,
}

#[cfg(test)]
impl RecordingStore {
    fn delete_secret_protected(&self, account: &str) {
        let query = protected_delete_query(account);
        assert!(uses_protected_keychain(&query));
        assert_eq!(
            dictionary_string_value(&query, unsafe { kSecAttrAccount }).as_deref(),
            Some(account)
        );
        self.deleted.lock().unwrap().push("protected");
    }

    fn delete_secret_legacy(&self, account: &str) {
        let query = legacy_delete_query(account);
        assert!(!dictionary_has_key(&query, unsafe {
            kSecUseDataProtectionKeychain
        }));
        assert_eq!(
            dictionary_string_value(&query, unsafe { kSecAttrAccount }).as_deref(),
            Some(account)
        );
        self.deleted.lock().unwrap().push("legacy");
    }

    fn set_secret_protected(&self, account: &str, secret: &[u8], with_access_control: bool) {
        let query = protected_write_query(
            account,
            secret,
            if with_access_control {
                Some(
                    SecAccessControl::create_with_flags(AccessControlOptions::USER_PRESENCE.bits())
                        .unwrap(),
                )
            } else {
                None
            },
        );
        let (account, secret) = account_and_secret_from_query(&query);
        self.writes
            .lock()
            .unwrap()
            .push((account, secret, has_access_control(&query)));
    }
}

#[cfg(test)]
fn emulate_set_secret_sequence(store: &RecordingStore, account: &str, secret: &[u8]) {
    store.delete_secret_protected(account);
    store.delete_secret_legacy(account);
    store.set_secret_protected(account, secret, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected_keychain_unavailable(error: &DeviceSecretError) -> bool {
        match error {
            DeviceSecretError::UnavailableInThisBuild => true,
            DeviceSecretError::Backend(message) => {
                message.contains("required entitlement") || message.contains("not available")
            }
            DeviceSecretError::MissingSecret => false,
        }
    }

    fn cleanup(account: &str) {
        let _ = delete_secret_protected(account);
        let _ = delete_secret_legacy(account);
    }

    fn unique_account(label: &str) -> String {
        format!("seance-test:{label}:{}", crate::now_ts())
    }

    #[test]
    fn stores_and_reads_plaintext_secret_without_prompt() {
        let store = MacosBiometricDeviceSecretStore::non_interactive_for_tests();
        let account = unique_account("roundtrip");
        let secret = b"plain-device-secret".to_vec();
        cleanup(&account);

        match store.set_secret(&account, &secret) {
            Ok(()) => {}
            Err(err) if protected_keychain_unavailable(&err) => return,
            Err(err) => panic!("{err}"),
        }
        let loaded = store.get_secret(&account).unwrap().unwrap();

        assert_eq!(loaded, secret);
        cleanup(&account);
    }

    #[test]
    fn overwrites_plaintext_secret_without_duplicate_items() {
        let store = MacosBiometricDeviceSecretStore::non_interactive_for_tests();
        let account = unique_account("update");
        cleanup(&account);

        match store.set_secret(&account, b"first") {
            Ok(()) => {}
            Err(err) if protected_keychain_unavailable(&err) => return,
            Err(err) => panic!("{err}"),
        }
        match store.set_secret(&account, b"second") {
            Ok(()) => {}
            Err(err) if protected_keychain_unavailable(&err) => return,
            Err(err) => panic!("{err}"),
        }

        assert_eq!(store.get_secret(&account).unwrap().unwrap(), b"second");
        cleanup(&account);
    }

    #[test]
    fn protected_write_query_uses_data_protection_keychain() {
        let query = protected_write_query(
            "account",
            b"secret",
            Some(
                SecAccessControl::create_with_flags(AccessControlOptions::USER_PRESENCE.bits())
                    .unwrap(),
            ),
        );

        assert!(uses_protected_keychain(&query));
        assert!(has_access_control(&query));
        assert_eq!(
            dictionary_data_value(&query, unsafe { kSecValueData }).as_deref(),
            Some(b"secret".as_slice())
        );
    }

    #[test]
    fn protected_read_query_uses_data_protection_keychain_and_prompt() {
        let query = protected_read_query("account", Some(OPERATION_PROMPT));

        assert!(uses_protected_keychain(&query));
        assert!(has_return_data(&query));
        assert_eq!(read_prompt(&query).as_deref(), Some(OPERATION_PROMPT));
    }

    #[test]
    fn set_secret_sequence_deletes_protected_then_legacy_before_write() {
        let store = RecordingStore::default();

        emulate_set_secret_sequence(&store, "account", b"secret");

        assert_eq!(
            store.deleted.lock().unwrap().as_slice(),
            &["protected", "legacy"]
        );
        assert_eq!(
            store.writes.lock().unwrap().as_slice(),
            &[("account".into(), b"secret".to_vec(), true)]
        );
    }

    #[test]
    fn legacy_delete_query_does_not_use_data_protection_keychain() {
        let query = legacy_delete_query("account");

        assert!(!dictionary_has_key(&query, unsafe {
            kSecUseDataProtectionKeychain
        }));
    }

    #[test]
    fn treats_auth_failures_as_passphrase_fallback() {
        assert!(should_fallback_to_passphrase(ERR_SEC_AUTH_FAILED));
        assert!(should_fallback_to_passphrase(USER_CANCELED_STATUS));
        assert!(should_fallback_to_passphrase(
            INTERACTION_NOT_ALLOWED_STATUS
        ));
        assert!(!should_fallback_to_passphrase(ERR_SEC_ITEM_NOT_FOUND));
    }

    #[test]
    fn treats_missing_entitlement_as_build_unavailable() {
        assert!(is_runtime_unavailable_status(ERR_SEC_MISSING_ENTITLEMENT));
        assert!(is_runtime_unavailable_status(ERR_SEC_NOT_AVAILABLE));
        assert!(!is_runtime_unavailable_status(ERR_SEC_AUTH_FAILED));
    }
}
