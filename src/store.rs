//! Persistência simples de conversas: um arquivo JSON por conversa em `data/conversations/`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::AppState;

type ApiErr = (StatusCode, Json<Value>);

fn err(code: StatusCode, msg: &str) -> ApiErr {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn dir(st: &AppState) -> PathBuf {
    st.cfg.data_dir.join("conversations")
}

fn path(st: &AppState, id: &str) -> Result<PathBuf, ApiErr> {
    if !valid_id(id) {
        return Err(err(StatusCode::BAD_REQUEST, "id inválido"));
    }
    Ok(dir(st).join(format!("{id}.json")))
}

#[derive(Serialize)]
pub struct Summary {
    id: String,
    title: String,
    updated_at: i64,
    messages: usize,
}

pub async fn list(State(st): State<AppState>) -> Json<Vec<Summary>> {
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir(&st)).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = tokio::fs::read(&p).await else { continue };
            let Ok(v) = serde_json::from_slice::<Value>(&raw) else { continue };
            let Some(id) = v["id"].as_str() else { continue };
            out.push(Summary {
                id: id.to_string(),
                title: v["title"].as_str().unwrap_or("Sem título").to_string(),
                updated_at: v["updated_at"].as_i64().unwrap_or(0),
                messages: v["messages"].as_array().map_or(0, |m| m.len()),
            });
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    Json(out)
}

pub async fn get(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiErr> {
    let p = path(&st, &id)?;
    let raw = tokio::fs::read(&p).await.map_err(|_| err(StatusCode::NOT_FOUND, "conversa não encontrada"))?;
    serde_json::from_slice(&raw)
        .map(Json)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "arquivo de conversa corrompido"))
}

pub async fn put(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<StatusCode, ApiErr> {
    let p = path(&st, &id)?;
    let obj = body.as_object_mut().ok_or_else(|| err(StatusCode::BAD_REQUEST, "corpo deve ser um objeto"))?;
    obj.insert("id".into(), Value::String(id));
    tokio::fs::create_dir_all(dir(&st))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "não foi possível criar o diretório de dados"))?;
    // Escrita atômica: grava em .tmp e renomeia.
    let tmp = p.with_extension("json.tmp");
    let data = serde_json::to_vec(&body).map_err(|_| err(StatusCode::BAD_REQUEST, "JSON inválido"))?;
    tokio::fs::write(&tmp, data)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "falha ao gravar"))?;
    tokio::fs::rename(&tmp, &p)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "falha ao gravar"))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiErr> {
    let p = path(&st, &id)?;
    match tokio::fs::remove_file(&p).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "falha ao apagar")),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_id;

    #[test]
    fn ids() {
        assert!(valid_id("abc-123_X"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
    }
}
