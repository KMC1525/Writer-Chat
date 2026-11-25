# ✒︎ Writer-Chat

WebUI de chat **multi-turn para escrita criativa**: front-end em HTML/CSS/JS puro (sem build, sem CDN) e backend em **Rust** (axum).

- **Modelo padrão:** [`DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic`](https://huggingface.co/DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic) em **Q4** (`Q4_K_M`).
- **Tamanho da resposta em palavras:** presets (Curta 150 · Média 400 · Longa 800 · Muito longa 1500 · Capítulo 3000 · Automático) ou qualquer valor de 20 a 8000. O alvo vai nas instruções do sistema e como lembrete na última mensagem, e o `max_tokens` é derivado dele. Cada resposta mostra `palavras geradas / alvo`.
- **Anexos:** `.txt .md .pdf .docx .html .json .csv .srt .xml .yaml .log` (o texto é extraído no servidor) e imagens `png/jpg/webp/gif` (visão, se o servidor tiver `mmproj`). Botão 📎, arrastar-e-soltar ou colar.
- **Multi-turn:** histórico completo a cada turno, *Continuar*, *Regenerar*, editar mensagem e reenviar, apagar daqui para baixo, exportar como Markdown.
- **Streaming**, botão **Parar**, bloco de raciocínio (`<think>`) recolhível e opção *Pensar antes*.
- Conversas salvas em disco (`data/conversations/*.json`).

## Arquitetura

```
navegador ──► Writer-Chat (Rust/axum) ──► servidor de inferência (API OpenAI-compatível)
 web/*.html      /api/chat (SSE)            llama.cpp `llama-server`, Ollama, LM Studio, vLLM…
                 /api/upload  /api/models
                 /api/conversations
```

O Writer-Chat **não carrega o modelo sozinho**: ele conversa com um servidor compatível com `/v1/chat/completions`. O padrão é `http://127.0.0.1:8080` (porta do `llama-server`).

## Começando

### 1. Tenha o modelo em Q4 (GGUF)

Na data desta versão não encontrei um GGUF publicado com esse nome exato (o repositório do autor tem só safetensors). Se nenhum quant aparecer no Hugging Face, gere o seu com o [llama.cpp](https://github.com/ggml-org/llama.cpp) (use uma versão recente, com suporte à arquitetura `qwen3_5`):

```bash
huggingface-cli download DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic --local-dir deckard
python convert_hf_to_gguf.py deckard --outfile deckard-f16.gguf --outtype f16
llama-quantize deckard-f16.gguf deckard-Q4_K_M.gguf Q4_K_M
# (opcional, para anexar imagens)
python convert_hf_to_gguf.py deckard --mmproj --outfile deckard-mmproj.gguf
```

O autor recomenda no mínimo `Q4_K_S` (sem imatrix) ou `IQ3_S` (com imatrix). Se já tiver um GGUF, pule este passo.

### 2. Suba o servidor do modelo

```bash
MODEL_GGUF=./deckard-Q4_K_M.gguf ./scripts/run-model.sh
# com visão:  MMPROJ=./deckard-mmproj.gguf MODEL_GGUF=… ./scripts/run-model.sh
```

### 3. Rode o Writer-Chat

```bash
cargo run --release
# abra http://127.0.0.1:3000
```

A bolinha no canto inferior esquerdo indica se o servidor do modelo está acessível.

## Configuração (variáveis de ambiente)

| Variável | Padrão | Descrição |
|---|---|---|
| `WRITER_ADDR` | `127.0.0.1:3000` | Onde o Writer-Chat escuta |
| `WRITER_BACKEND_URL` | `http://127.0.0.1:8080` | Servidor de inferência OpenAI-compatível |
| `WRITER_API_KEY` | – | Chave enviada como `Bearer` ao servidor de inferência |
| `WRITER_MODEL` | `DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic` | Id do modelo enviado na requisição |
| `WRITER_QUANT` | `Q4_K_M` | Rótulo da quantização exibido na UI |
| `WRITER_DATA_DIR` | `data` | Onde as conversas são gravadas |
| `WRITER_MAX_ATTACH_CHARS` | `120000` | Limite de texto extraído por anexo |

Modelo, instruções do sistema (estilo do escritor) e amostragem (temperatura, top-p, top-k, min-p, repetição) também podem ser mudados em ⚙ na interface.

> **Segurança:** o servidor não tem autenticação e, por padrão, só escuta em `127.0.0.1`. Não exponha na rede sem um proxy com autenticação.

## Notas

- **Contexto:** anexos grandes consomem contexto. Aumente `CTX` no `run-model.sh` se necessário; o texto de cada anexo é cortado em `WRITER_MAX_ATTACH_CHARS`.
- **Tamanho em palavras é um alvo**, não uma garantia: modelos erram a contagem. A faixa pedida é ±15% e o limite de tokens é uma proteção contra respostas sem fim. Se a resposta for cortada pelo limite, aparece o aviso "cortada pelo limite" e o botão *Continuar* retoma de onde parou.
- **Raciocínio:** *Pensar antes* envia `chat_template_kwargs.enable_thinking` (suportado pelo `llama-server` com `--jinja`). Desligado por padrão, pois consome tokens.
- **PDF escaneado** não tem texto extraível: envie as páginas como imagem.

## Desenvolvimento

```bash
cargo test     # testes de extração de anexos, montagem do prompt e validação de ids
cargo run      # o front-end (web/) é embutido no binário em tempo de compilação
```

### API

| Rota | Descrição |
|---|---|
| `GET /api/config` | Modelo padrão, presets de tamanho, instruções padrão |
| `GET /api/models` | Status/lista de modelos do servidor de inferência |
| `POST /api/chat` | Streaming SSE: `{type: content\|reasoning\|done\|error}` |
| `POST /api/upload` | `multipart/form-data` (campo `file`, múltiplos) → anexos processados |
| `GET/PUT/DELETE /api/conversations[/{id}]` | Persistência das conversas |

## Licença

MIT — veja [LICENSE](LICENSE).
