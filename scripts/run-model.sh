#!/usr/bin/env bash
# Sobe o llama.cpp (llama-server) com o modelo padrão do Writer-Chat em Q4.
#
# Uso:
#   MODEL_GGUF=/caminho/Deckard-Pro-Writer-Q4_K_M.gguf ./scripts/run-model.sh
# Opcional:
#   MMPROJ=/caminho/mmproj.gguf   # habilita anexos de imagem (visão)
#   CTX=16384  NGL=99  PORT=8080
set -euo pipefail

: "${MODEL_GGUF:?defina MODEL_GGUF com o caminho do arquivo .gguf (Q4)}"
CTX="${CTX:-16384}"
NGL="${NGL:-99}"
PORT="${PORT:-8080}"

args=(-m "$MODEL_GGUF" -c "$CTX" -ngl "$NGL" --port "$PORT" --jinja
      --alias "DavidAU/Qwen3.5-9B-The-Deckard-Pro-Writer-Uncensored-Heretic")
[[ -n "${MMPROJ:-}" ]] && args+=(--mmproj "$MMPROJ")

exec llama-server "${args[@]}"
