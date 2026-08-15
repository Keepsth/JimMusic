//! 生成 Ed25519 密钥对并对消息签名（插件发布工作流工具）。
//!
//! 用法：
//! ```text
//! cargo run -p plugin-manager --example sign_gen -- print-keypair
//! cargo run -p plugin-manager --example sign_gen -- sign <hex-private-key> <message-hex-or-ascii>
//! ```
//!
//! 输出 JSON：`{ "public_key": "...", "signature": "..." }`。安装时把
//! public_key 与 signature（对插件内容 SHA-256 摘要的签名）一并传给
//! `POST /plugins/install` 即可启用防篡改校验。

use ed25519_dalek::{Signer, SigningKey};
use getrandom::{rand_core::UnwrapErr, SysRng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("print-keypair") => print_keypair(),
        Some("sign") => sign(&args),
        _ => {
            eprintln!("usage: sign_gen print-keypair | sign <hex-private-key> <message-ascii>");
            std::process::exit(1);
        }
    }
}

fn print_keypair() {
    let mut csprng = UnwrapErr(SysRng);
    let signing_key = SigningKey::generate(&mut csprng);
    let vk = signing_key.verifying_key();
    let json = serde_json::json!({
        "private_key": hex::encode(signing_key.to_bytes()),
        "public_key": hex::encode(vk.to_bytes()),
    });
    println!("{json}");
}

fn sign(args: &[String]) {
    let sk_hex = args.get(2).expect("missing private key hex");
    let message = args.get(3).expect("missing message").as_bytes();

    let sk_bytes: [u8; 32] = hex::decode(sk_hex)
        .expect("invalid private key hex")
        .try_into()
        .expect("private key must be 32 bytes");
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let vk = signing_key.verifying_key();
    let sig = signing_key.sign(message);

    let json = serde_json::json!({
        "public_key": hex::encode(vk.to_bytes()),
        "signature": hex::encode(sig.to_bytes()),
    });
    println!("{json}");
}
