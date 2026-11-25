//! Upload de anexos: extrai texto de .txt/.md/.pdf/.docx/etc. e converte imagens em data URLs.

use axum::{
    extract::{Multipart, State},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::io::Read;

use crate::AppState;

const MAX_IMAGE_BYTES: usize = 15 * 1024 * 1024;

#[derive(Serialize, Default)]
pub struct Attachment {
    pub name: String,
    /// "text" ou "image"
    pub kind: &'static str,
    pub mime: String,
    pub size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_url: Option<String>,
    pub chars: usize,
    pub words: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn upload(State(st): State<AppState>, mut mp: Multipart) -> Json<Vec<Attachment>> {
    let mut out = Vec::new();
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.file_name().map(sanitize_name).unwrap_or_else(|| "anexo".into());
        let ctype = field.content_type().unwrap_or("").to_string();
        match field.bytes().await {
            Ok(bytes) => {
                let max = st.cfg.max_attach_chars;
                let n = name.clone();
                // Extração em thread bloqueante: PDFs grandes são CPU-bound e a lib pode dar panic.
                let res = tokio::task::spawn_blocking(move || process(&n, &ctype, &bytes, max)).await;
                out.push(match res {
                    Ok(Ok(a)) => a,
                    Ok(Err(e)) => failed(name, e),
                    Err(_) => failed(name, "falha ao processar o arquivo".into()),
                });
            }
            Err(e) => out.push(failed(name, format!("erro no upload: {e}"))),
        }
    }
    Json(out)
}

fn failed(name: String, error: String) -> Attachment {
    Attachment { name, kind: "text", error: Some(error), ..Default::default() }
}

fn sanitize_name(n: &str) -> String {
    let base = n.rsplit(['/', '\\']).next().unwrap_or(n);
    let s: String = base.chars().filter(|c| !c.is_control()).take(120).collect();
    if s.is_empty() { "anexo".into() } else { s }
}

fn ext(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn process(name: &str, ctype: &str, bytes: &[u8], max_chars: usize) -> Result<Attachment, String> {
    if bytes.is_empty() {
        return Err("arquivo vazio".into());
    }
    let e = ext(name);

    let image_mime = match e.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    };
    if let Some(mime) = image_mime {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("imagem maior que 15 MB".into());
        }
        return Ok(Attachment {
            name: name.into(),
            kind: "image",
            mime: mime.into(),
            size: bytes.len(),
            data_url: Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes))),
            ..Default::default()
        });
    }

    let text = match e.as_str() {
        "pdf" => extract_pdf(bytes)?,
        "docx" => extract_docx(bytes)?,
        "html" | "htm" | "xhtml" => strip_tags(&decode_text(bytes)),
        _ => {
            if looks_binary(bytes) {
                return Err(format!(
                    "formato não suportado (.{e}). Use texto, Markdown, PDF, DOCX ou imagens"
                ));
            }
            decode_text(bytes)
        }
    };

    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err("nenhum texto encontrado (PDF escaneado? envie como imagem)".into());
    }
    let (text, truncated) = truncate_chars(text, max_chars);
    Ok(Attachment {
        name: name.into(),
        kind: "text",
        mime: if ctype.is_empty() { "text/plain".into() } else { ctype.into() },
        size: bytes.len(),
        chars: text.chars().count(),
        words: text.split_whitespace().count(),
        truncated,
        text: Some(text),
        ..Default::default()
    })
}

fn looks_binary(b: &[u8]) -> bool {
    let head = &b[..b.len().min(8192)];
    head.contains(&0) || (std::str::from_utf8(head).is_err() && !is_cut_utf8(head))
}

/// `head` pode ter cortado um caractere UTF-8 ao meio no final; isso não é binário.
fn is_cut_utf8(head: &[u8]) -> bool {
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

fn decode_text(b: &[u8]) -> String {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        // Fallback latin-1 (comum em .txt antigos em português).
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").replace('\0', "")
}

fn truncate_chars(s: String, max: usize) -> (String, bool) {
    match s.char_indices().nth(max) {
        Some((i, _)) => (s[..i].to_string(), true),
        None => (s, false),
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<String, String> {
    pdf_extract::extract_text_from_mem(bytes).map_err(|e| format!("PDF inválido: {e}"))
}

fn extract_docx(bytes: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| "DOCX inválido".to_string())?;
    let f = zip
        .by_name("word/document.xml")
        .map_err(|_| "DOCX sem word/document.xml".to_string())?;
    let mut xml = String::new();
    f.take(64 * 1024 * 1024)
        .read_to_string(&mut xml)
        .map_err(|_| "DOCX ilegível".to_string())?;
    let xml = xml
        .replace("</w:p>", "\n")
        .replace("<w:tab/>", "\t")
        .replace("<w:br/>", "\n")
        .replace("<w:cr/>", "\n");
    Ok(decode_entities(&strip_tags(&xml)))
}

/// Remove tags HTML/XML (e também o conteúdo de <script>/<style>).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut skip_until: Option<&str> = None;
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if let Some(end) = skip_until {
            match lower[i..].find(end) {
                Some(p) => {
                    i += p + end.len();
                    skip_until = None;
                    in_tag = false;
                    continue;
                }
                None => break,
            }
        }
        let c = bytes[i];
        if in_tag {
            if c == b'>' {
                in_tag = false;
            }
            i += 1;
        } else if c == b'<' {
            if lower[i..].starts_with("<script") {
                skip_until = Some("</script>");
            } else if lower[i..].starts_with("<style") {
                skip_until = Some("</style>");
            } else {
                // Quebras de linha em tags de bloco
                let rest = &lower[i + 1..];
                if ["p", "/p", "br", "div", "/div", "h1", "h2", "h3", "li", "tr"]
                    .iter()
                    .any(|t| rest.starts_with(t))
                {
                    out.push('\n');
                }
                in_tag = true;
            }
            i += 1;
        } else {
            // copia um caractere UTF-8 completo
            let ch_len = utf8_len(c);
            out.push_str(&s[i..(i + ch_len).min(s.len())]);
            i += ch_len;
        }
    }
    decode_entities(&out)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_file() {
        let a = process("nota.md", "", "# Olá\nmundo ação".as_bytes(), 1000).unwrap();
        assert_eq!(a.kind, "text");
        assert_eq!(a.words, 4);
        assert!(!a.truncated);
    }

    #[test]
    fn truncates() {
        let a = process("a.txt", "", "abcdefghij".as_bytes(), 4).unwrap();
        assert_eq!(a.text.unwrap(), "abcd");
        assert!(a.truncated);
    }

    #[test]
    fn rejects_binary() {
        assert!(process("x.bin", "", &[0, 1, 2, 3], 100).is_err());
    }

    #[test]
    fn image_becomes_data_url() {
        let a = process("a.png", "", &[1, 2, 3], 100).unwrap();
        assert!(a.data_url.unwrap().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn html_strip() {
        let t = strip_tags("<p>Olá <b>mundo</b></p><script>x()</script>fim &amp; tudo");
        assert!(t.contains("Olá mundo"));
        assert!(!t.contains("x()"));
        assert!(t.contains("fim & tudo"));
    }

    #[test]
    fn docx_roundtrip() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"<w:document><w:p><w:r><w:t>Primeira</w:t></w:r></w:p><w:p><w:r><w:t>Segunda &amp; fim</w:t></w:r></w:p></w:document>").unwrap();
            z.finish().unwrap();
        }
        let a = process("d.docx", "", &buf, 1000).unwrap();
        let t = a.text.unwrap();
        assert!(t.contains("Primeira\nSegunda & fim"), "{t:?}");
    }
}
