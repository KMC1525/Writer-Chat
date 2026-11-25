use std::{env, net::SocketAddr, path::PathBuf};

/// Modelo padrão do Writer-Chat.
pub const DEFAULT_MODEL: &str = "DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic";
/// Quantização padrão (apenas informativa na UI; o servidor de inferência decide o que carrega).
pub const DEFAULT_QUANT: &str = "Q4_K_M";

#[derive(Clone, Debug)]
pub struct Config {
    /// Endereço onde o Writer-Chat escuta.
    pub addr: SocketAddr,
    /// URL base do servidor compatível com a API da OpenAI (llama.cpp, Ollama, LM Studio, vLLM…).
    pub backend_url: String,
    /// Chave de API opcional enviada ao servidor de inferência.
    pub api_key: Option<String>,
    pub model: String,
    pub quant: String,
    pub data_dir: PathBuf,
    /// Máximo de caracteres de texto extraído por anexo.
    pub max_attach_chars: usize,
}

impl Config {
    pub fn from_env() -> Self {
        let get = |k: &str| env::var(k).ok().filter(|v| !v.trim().is_empty());
        let backend = get("WRITER_BACKEND_URL").unwrap_or_else(|| "http://127.0.0.1:8080".into());
        let backend = backend.trim_end_matches('/');
        let backend = backend.strip_suffix("/v1").unwrap_or(backend).to_string();
        Self {
            addr: get("WRITER_ADDR")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| "127.0.0.1:3000".parse().unwrap()),
            backend_url: backend,
            api_key: get("WRITER_API_KEY"),
            model: get("WRITER_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into()),
            quant: get("WRITER_QUANT").unwrap_or_else(|| DEFAULT_QUANT.into()),
            data_dir: get("WRITER_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| "data".into()),
            max_attach_chars: get("WRITER_MAX_ATTACH_CHARS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(120_000),
        }
    }
}
