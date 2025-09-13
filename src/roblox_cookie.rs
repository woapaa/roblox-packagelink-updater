use anyhow::{Result, anyhow};
use base64::prelude::*;
use regex::Regex;
use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[cfg(windows)]
mod windows_crypto {
    use windows_sys::Win32::{
        Foundation::{HANDLE, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };
    pub fn dpapi_decrypt(encrypted_data: &[u8]) -> Result<Vec<u8>, String> {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: encrypted_data.len() as u32,
            pbData: encrypted_data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let result = unsafe {
            CryptUnprotectData(
                &mut in_blob,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };

        if result == 0 {
            return Err("CryptUnprotectData failed".to_string());
        }

        let decrypted_data = unsafe {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        };

        unsafe {
            LocalFree(out_blob.pbData as HANDLE);
        }

        Ok(decrypted_data)
    }
}

#[derive(Deserialize)]
struct CookiesFile {
    #[serde(rename = "CookiesData")]
    cookies_data: String,
}

fn clean_value(s: &str) -> String {
    s.trim()
        .trim_end_matches(';')
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

fn extract_roblosecurity(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\.ROBLOSECURITY\s+([^;\s#]+)").unwrap();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let v = clean_value(m.as_str());
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

// Gets your .ROBLOSECURITY
pub fn get_roblosecurity() -> Result<String> {
    let user_profile = env::var("USERPROFILE")?;
    let mut cookies_path = PathBuf::from(user_profile);
    cookies_path.push("AppData");
    cookies_path.push("Local");
    cookies_path.push("Roblox");
    cookies_path.push("LocalStorage");
    cookies_path.push("robloxcookies.dat");

    if !cookies_path.exists() {
        return Err(anyhow!(format!(
            "Cookies file not found at: {:?}",
            cookies_path
        )));
    }

    let temp_dir = env::var("TEMP")?;
    let mut destination_path = PathBuf::from(temp_dir);
    destination_path.push("RobloxCookies.dat");

    let final_destination_path = destination_path.clone();

    let result = (|| {
        fs::copy(&cookies_path, &final_destination_path)?;
        let file_content = fs::read_to_string(&final_destination_path)?;
        let parsed_file: CookiesFile = serde_json::from_str(&file_content)?;

        let encoded_cookies = parsed_file.cookies_data;
        if encoded_cookies.is_empty() {
            return Err(anyhow!("RobloxCookies.dat was found but is empty"));
        }
        let decoded_cookies = BASE64_STANDARD.decode(encoded_cookies)?;

        #[cfg(windows)]
        {
            let decrypted_bytes = windows_crypto::dpapi_decrypt(&decoded_cookies)
                .map_err(|e| anyhow!(format!("Error decrypting with DPAPI: {}", e)))?;

            let decrypted_string = String::from_utf8_lossy(&decrypted_bytes);
            let roblosecurity = extract_roblosecurity(&decrypted_string);
            if let Some(roblosecurity) = roblosecurity {
                return Ok(roblosecurity);
            }
        }

        #[cfg(not(windows))]
        {
            println!("DPAPI decryption is only available on Windows.");
        }

        return Err(anyhow!(format!(
            "Failed to parse cookies at: {:?}",
            cookies_path
        )));
    })();

    if final_destination_path.exists() {
        if let Err(e) = fs::remove_file(&final_destination_path) {
            eprintln!("Failed to delete temporary file: {}", e);
        }
    }

    result
}
