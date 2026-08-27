//! API key 加密存储（ADR-0011 R5）：AES-256-GCM，主密钥在服务端 `.master_key` 文件
//! （gitignored，首次使用自动生成 32 字节 hex；**备份 DB 必须连同备份该文件**，否则密钥不可解）。
//! 库中格式：`enc:v1:<base64(nonce)>:<base64(ciphertext+tag)>`；绝不明文、绝不简单编码。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::AppError;

const PREFIX: &str = "enc:v1:";
const NONCE_LEN: usize = 12;

static MASTER_KEY: OnceLock<Vec<u8>> = OnceLock::new();

/// 主密钥（32 字节）：`.master_key` 文件存在则读，不存在则生成并落盘（0600）。
fn master_key() -> Result<&'static Vec<u8>, AppError> {
    if let Some(k) = MASTER_KEY.get() {
        return Ok(k);
    }
    let path = key_file_path();
    let key_bytes: [u8; 32] = match std::fs::read_to_string(&path) {
        Ok(s) => {
            let hex = s.trim();
            decode_hex_32(hex).map_err(|e| {
                AppError::BadRequest(format!("主密钥文件格式错误（{path:?}）: {e}"))
            })?
        }
        Err(_) => {
            // 首次生成：OsRng 32 字节 -> hex 落盘
            use rand::RngCore;
            let mut raw = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut raw);
            let hex = hex::encode(raw);
            std::fs::write(&path, &hex).map_err(|e| {
                AppError::BadRequest(format!("主密钥文件写入失败（{path:?}）: {e}"))
            })?;
            #[cfg(unix)]
            {
                let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
            }
            raw
        }
    };
    let _ = MASTER_KEY.set(key_bytes.to_vec());
    Ok(MASTER_KEY.get().unwrap())
}

fn key_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".master_key")
}

fn decode_hex_32(hex: &str) -> Result<[u8; 32], String> {
    if hex.len() != 64 {
        return Err(format!("期望 64 位 hex，实际 {} 位", hex.len()));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// 是否已是密文格式
pub fn is_encrypted(s: &str) -> bool {
    s.starts_with(PREFIX)
}

/// 明文 -> `enc:v1:nonce:ct`（已加密的输入原样返回，幂等）
pub fn encrypt(plain: &str) -> Result<String, AppError> {
    if is_encrypted(plain) {
        return Ok(plain.to_string());
    }
    let key = master_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plain.as_bytes())
        .map_err(|_| AppError::BadRequest("加密失败".to_string()))?;
    let engine = base64::engine::general_purpose::STANDARD;
    Ok(format!(
        "{PREFIX}{}:{}",
        engine.encode(nonce_bytes),
        engine.encode(ct)
    ))
}

/// `enc:v1:nonce:ct` -> 明文（非密文格式的输入按明文原样返回，供懒迁移读取）
pub fn decrypt(stored: &str) -> Result<String, AppError> {
    if !is_encrypted(stored) {
        return Ok(stored.to_string());
    }
    let body = &stored[PREFIX.len()..];
    let (nonce_b64, ct_b64) = body.split_once(':').ok_or_else(|| {
        AppError::BadRequest("密文格式错误".to_string())
    })?;
    let engine = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = engine
        .decode(nonce_b64)
        .map_err(|_| AppError::BadRequest("密文 nonce 解码失败".to_string()))?;
    let ct = engine
        .decode(ct_b64)
        .map_err(|_| AppError::BadRequest("密文解码失败".to_string()))?;
    let key = master_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ct.as_ref())
        .map_err(|_| AppError::BadRequest("解密失败（主密钥不匹配或数据损坏）".to_string()))?;
    String::from_utf8(plain).map_err(|_| AppError::BadRequest("解密结果非 UTF-8".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_format() {
        let secret = "sk-test-abc123";
        let enc = encrypt(secret).unwrap();
        assert!(enc.starts_with("enc:v1:"), "应为密文格式");
        assert!(!enc.contains(secret), "密文不得包含明文片段");
        assert!(is_encrypted(&enc));
        assert_eq!(decrypt(&enc).unwrap(), secret);
    }

    #[test]
    fn encrypt_is_idempotent_on_ciphertext() {
        let once = encrypt("hello").unwrap();
        let twice = encrypt(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn plaintext_passthrough_on_decrypt() {
        assert_eq!(decrypt("plain-key").unwrap(), "plain-key");
    }

    #[test]
    fn unique_nonce_each_call() {
        assert_ne!(encrypt("same").unwrap(), encrypt("same").unwrap());
    }
}
