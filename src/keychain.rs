use std::ffi::c_void;
use std::ptr;

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

// Security.framework FFI
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecItemCopyMatching(query: *const c_void, result: *mut *const c_void) -> i32;
}

// CoreFoundation types needed for the keychain query
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreateMutable(
        allocator: *const c_void,
        capacity: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *mut c_void;
    fn CFDictionarySetValue(dict: *mut c_void, key: *const c_void, value: *const c_void);
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        is_external: bool,
    ) -> *const c_void;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFRelease(cf: *const c_void);

    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: *const c_void;

    static kSecClass: *const c_void;
    static kSecClassGenericPassword: *const c_void;
    static kSecAttrService: *const c_void;
    static kSecReturnData: *const c_void;
    static kSecMatchLimit: *const c_void;
    static kSecMatchLimitOne: *const c_void;
}

const K_UTF8_ENCODING: u32 = 0x08000100;

/// Create a CFString from a Rust &str. Caller must CFRelease.
unsafe fn cfstring(s: &str) -> *const c_void {
    unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            K_UTF8_ENCODING,
            false,
        )
    }
}

/// Read Claude Code OAuth credentials from macOS Keychain using Security.framework.
pub fn read_credentials() -> Option<serde_json::Value> {
    unsafe {
        let query = CFDictionaryCreateMutable(
            ptr::null(),
            5,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        );

        CFDictionarySetValue(query, kSecClass, kSecClassGenericPassword);

        let service = cfstring(KEYCHAIN_SERVICE);
        CFDictionarySetValue(query, kSecAttrService, service);
        CFDictionarySetValue(query, kSecReturnData, kCFBooleanTrue);
        CFDictionarySetValue(query, kSecMatchLimit, kSecMatchLimitOne);

        let mut result: *const c_void = ptr::null();
        let status = SecItemCopyMatching(query, &mut result);

        CFRelease(service);
        CFRelease(query as *const c_void);

        if status != 0 {
            eprintln!("Keychain read failed (OSStatus={status})");
            return None;
        }

        if result.is_null() {
            eprintln!("Keychain returned null data");
            return None;
        }

        let bytes = CFDataGetBytePtr(result);
        let len = CFDataGetLength(result) as usize;
        let slice = std::slice::from_raw_parts(bytes, len);
        let raw = std::str::from_utf8(slice).ok()?;

        let parsed = match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Keychain JSON parse error: {e}");
                CFRelease(result);
                return None;
            }
        };

        CFRelease(result);

        let mut data = parsed;
        if data.get("claudeAiOauth").is_some() {
            data = data["claudeAiOauth"].take();
        }

        Some(data)
    }
}
