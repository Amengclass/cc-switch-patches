//! 远程主机密码的加密存储。
//!
//! 用 Windows DPAPI(`CryptProtectData`)加密后，base64 存到
//! `~/.cc-switch/remote_passwords.json`。
//!
//! 原方案用 `keyring`(Windows 凭据管理器)：其在**非域机器**上使用
//! `CRED_PERSIST_ENTERPRISE` 只把凭据写进当前登录会话，换进程即丢失
//! （`set_password` 返回 Ok 但新进程读不到）。DPAPI 绑定当前用户账户，
//! 跨进程 / 重启稳定，是标准做法。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// 保护并发读写。
static STORE: Mutex<()> = Mutex::new(());

fn store_path() -> Result<PathBuf, String> {
    Ok(crate::config::get_app_config_dir().join("remote_passwords.json"))
}

fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    }
}

/// DPAPI 加密（绑定当前用户账户）。
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let in_blob = blob(data);
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &in_blob,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 {
        return Err("DPAPI 加密失败".to_string());
    }
    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }.to_vec();
    unsafe { LocalFree(out_blob.pbData as _) };
    Ok(out)
}

/// DPAPI 解密。
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    let in_blob = blob(data);
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 {
        return Err("DPAPI 解密失败".to_string());
    }
    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }.to_vec();
    unsafe { LocalFree(out_blob.pbData as _) };
    Ok(out)
}

/// 保存密码。
pub fn save_password(host_id: &str, password: &str) -> Result<(), String> {
    let _guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_map()?;
    let encrypted = protect(password.as_bytes())?;
    map.insert(
        host_id.to_string(),
        base64::engine::general_purpose::STANDARD.encode(encrypted),
    );
    write_map(&map)
}

/// 读取密码；未保存时返回 None。
pub fn get_password(host_id: &str) -> Result<Option<String>, String> {
    let _guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let map = load_map()?;
    match map.get(host_id) {
        Some(b64) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("base64 解码失败: {e}"))?;
            let decrypted = unprotect(&bytes)?;
            Ok(Some(
                String::from_utf8(decrypted).map_err(|e| format!("密码解码失败: {e}"))?,
            ))
        }
        None => Ok(None),
    }
}

/// 删除密码；未保存时静默成功。
pub fn delete_password(host_id: &str) -> Result<(), String> {
    let _guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_map()?;
    if map.remove(host_id).is_some() {
        write_map(&map)?;
    }
    Ok(())
}

fn load_map() -> Result<HashMap<String, String>, String> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取密码文件失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析密码文件失败: {e}"))
}

fn write_map(map: &HashMap<String, String>) -> Result<(), String> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(map).map_err(|e| format!("序列化密码文件失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入密码文件失败: {e}"))
}
