//! Monta o prompt (system + tamanho em palavras + anexos) e faz streaming da resposta do modelo.

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;

use crate::AppState;

pub const DEFAULT_SYSTEM: &str = "Você é um escritor criativo profissional e parceiro de escrita. \
Escreva prosa vívida, específica e com voz própria: mostre em vez de explicar, use detalhes sensoriais concretos, \
diálogos naturais e ritmo variado. Evite clichês, frases genéricas e construções repetitivas. \
Mantenha continuidade com tudo o que já foi escrito na conversa (personagens, tom, tempo verbal, ponto de vista, mundo). \
Responda no idioma do usuário. Entregue apenas o texto pedido, sem preâmbulos, avisos ou comentários meta, \
a menos que o usuário peça explicitamente sugestões ou feedback.";

#[derive(Deserialize)]
pub struct Attach {
    pub name: String,
    pub kind: String,
    pub text: Option<String>,
    pub data_url: Option<String>,
}

#[derive(Deserialize)]
pub struct InMsg {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attach>,
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub system: Option<String>,
    pub messages: Vec<InMsg>,
    /// Alvo de palavras da resposta. `None`/0 = automático.
    pub words: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    /// Liga o modo de raciocínio (<think>) do Qwen3.5.
    #[serde(default)]
    pub thinking: bool,
    pub max_tokens: Option<u32>,
}

/// Instrução de tamanho em palavras (faixa ±15%).
pub fn length_directive(words: u32) -> String {
    let lo = (words as f32 * 0.85).round() as u32;
    let hi = (words as f32 * 1.15).round() as u32;
    format!(
        "A resposta deve ter aproximadamente {words} palavras (entre {lo} e {hi}). \
Planeje o arco do texto para caber nesse tamanho e termine de forma natural, sem cortar no meio e sem mencionar a contagem de palavras."
    )
}

/// Tokens máximos a partir do alvo de palavras (pt-BR ≈ 1,6–2 tokens/palavra; com folga).
pub fn token_budget(words: u32, thinking: bool) -> u32 {
    let base = (words as f32 * 2.4).ceil() as u32 + 96;
    if thinking { base + 3072 } else { base }
}

fn build_user_content(m: &InMsg, trailer: Option<&str>) -> Value {
    let mut text = String::new();
    for a in m.attachments.iter().filter(|a| a.kind == "text") {
        if let Some(t) = &a.text {
            text.push_str(&format!("<anexo nome=\"{}\">\n{}\n</anexo>\n\n", a.name.replace('"', "'"), t));
        }
    }
    text.push_str(&m.content);
    if let Some(t) = trailer {
        text.push_str("\n\n");
        text.push_str(t);
    }
    let images: Vec<&Attach> = m.attachments.iter().filter(|a| a.kind == "image" && a.data_url.is_some()).collect();
    if images.is_empty() {
        return Value::String(text);
    }
    let mut parts = vec![json!({ "type": "text", "text": text })];
    for a in images {
        parts.push(json!({ "type": "image_url", "image_url": { "url": a.data_url } }));
    }
    Value::Array(parts)
}

pub fn build_payload(req: &ChatRequest, default_model: &str) -> Value {
    let words = req.words.filter(|w| *w > 0);
    let mut system = req
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SYSTEM)
        .to_string();
    let directive = words.map(length_directive);
    if let Some(d) = &directive {
        system.push_str("\n\n");
        system.push_str(d);
    }

    let last_user = req.messages.iter().rposition(|m| m.role == "user");
    let mut msgs = vec![json!({ "role": "system", "content": system })];
    for (i, m) in req.messages.iter().enumerate() {
        match m.role.as_str() {
            "user" => {
                // Lembrete de tamanho junto da última mensagem do usuário: modelos pequenos seguem melhor.
                let trailer = (Some(i) == last_user)
                    .then(|| words.map(|w| format!("[Tamanho desejado da resposta: ~{w} palavras]")))
                    .flatten();
                msgs.push(json!({ "role": "user", "content": build_user_content(m, trailer.as_deref()) }));
            }
            "assistant" => msgs.push(json!({ "role": "assistant", "content": m.content })),
            _ => {}
        }
    }

    let mut p = json!({
        "model": req.model.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(default_model),
        "messages": msgs,
        "stream": true,
        "stream_options": { "include_usage": true },
        "temperature": req.temperature.unwrap_or(0.9),
        "top_p": req.top_p.unwrap_or(0.95),
        "chat_template_kwargs": { "enable_thinking": req.thinking },
    });
    let o = p.as_object_mut().unwrap();
    if let Some(v) = req.top_k { o.insert("top_k".into(), json!(v)); }
    if let Some(v) = req.min_p { o.insert("min_p".into(), json!(v)); }
    if let Some(v) = req.repeat_penalty { o.insert("repeat_penalty".into(), json!(v)); }
    let max = req.max_tokens.or_else(|| words.map(|w| token_budget(w, req.thinking)));
    if let Some(m) = max { o.insert("max_tokens".into(), json!(m)); }
    p
}

fn ev(v: Value) -> Result<Event, Infallible> {
    Ok(Event::default().data(v.to_string()))
}

pub async fn chat(
    State(st): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> impl axum::response::IntoResponse {
    let payload = build_payload(&req, &st.cfg.model);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        let url = format!("{}/v1/chat/completions", st.cfg.backend_url);
        let mut rb = st.http.post(&url).json(&payload);
        if let Some(k) = &st.cfg.api_key {
            rb = rb.bearer_auth(k);
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(ev(json!({ "type": "error", "message": format!(
                    "Não consegui falar com o servidor do modelo em {}: {e}. Ele está rodando? (veja o README)", st.cfg.backend_url
                ) }))).await;
                return;
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(str::to_string))
                .unwrap_or(body);
            let _ = tx.send(ev(json!({ "type": "error", "message": format!("Servidor do modelo respondeu {status}: {msg}") }))).await;
            return;
        }

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut finish: Option<String> = None;
        let mut usage: Option<Value> = None;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                let _ = tx.send(ev(json!({ "type": "error", "message": "Conexão com o modelo interrompida." }))).await;
                return;
            };
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim().strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" || data.is_empty() { continue; }
                let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) { usage = Some(u.clone()); }
                let choice = &v["choices"][0];
                if let Some(f) = choice["finish_reason"].as_str() { finish = Some(f.to_string()); }
                let delta = &choice["delta"];
                if let Some(r) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).filter(|s| !s.is_empty()) {
                    if tx.send(ev(json!({ "type": "reasoning", "text": r }))).await.is_err() { return; }
                }
                if let Some(c) = delta["content"].as_str().filter(|s| !s.is_empty()) {
                    // Receptor fechou (usuário clicou em "parar") → descartar o stream cancela a geração.
                    if tx.send(ev(json!({ "type": "content", "text": c }))).await.is_err() { return; }
                }
            }
        }
        let _ = tx.send(ev(json!({ "type": "done", "finish_reason": finish, "usage": usage }))).await;
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(words: Option<u32>, thinking: bool) -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                { "role": "user", "content": "Comece uma história", "attachments": [
                    { "name": "ref.txt", "kind": "text", "text": "Mundo sombrio" },
                    { "name": "i.png", "kind": "image", "data_url": "data:image/png;base64,AA==" } ] },
                { "role": "assistant", "content": "Era uma vez..." },
                { "role": "user", "content": "Continue" }
            ],
            "words": words, "thinking": thinking
        })).unwrap()
    }

    #[test]
    fn payload_has_length_and_attachments() {
        let p = build_payload(&req(Some(500), false), "m");
        assert_eq!(p["model"], "m");
        assert!(p["messages"][0]["content"].as_str().unwrap().contains("aproximadamente 500 palavras"));
        // anexos entram no primeiro user; lembrete só no último
        let first = &p["messages"][1]["content"];
        assert!(first[0]["text"].as_str().unwrap().contains("<anexo nome=\"ref.txt\">"));
        assert_eq!(first[1]["type"], "image_url");
        assert!(!first[0]["text"].as_str().unwrap().contains("Tamanho desejado"));
        assert!(p["messages"][3]["content"].as_str().unwrap().contains("~500 palavras"));
        assert_eq!(p["max_tokens"], 1296);
        assert_eq!(p["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn auto_length_has_no_cap() {
        let p = build_payload(&req(None, true), "m");
        assert!(p.get("max_tokens").is_none());
        assert!(!p["messages"][0]["content"].as_str().unwrap().contains("palavras (entre"));
        assert_eq!(p["chat_template_kwargs"]["enable_thinking"], true);
    }

    #[test]
    fn thinking_adds_budget() {
        assert!(token_budget(500, true) > token_budget(500, false));
    }
}
