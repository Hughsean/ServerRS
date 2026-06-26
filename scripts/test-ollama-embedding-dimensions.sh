#!/usr/bin/env bash
set -euo pipefail

# Test whether the current Ollama instance honors the embedding "dimensions"
# request field on both native /api/embed and OpenAI-compatible /v1/embeddings.
#
# Usage:
#   bash scripts/test-ollama-embedding-dimensions.sh
#   OLLAMA_BASE_URL=http://127.0.0.1:11434 OLLAMA_MODEL=qwen3-embedding:4b TEST_DIMENSIONS=128 bash scripts/test-ollama-embedding-dimensions.sh

BASE_URL="${OLLAMA_BASE_URL:-http://127.0.0.1:11434}"
MODEL="${OLLAMA_MODEL:-qwen3-embedding:4b}"
TEST_DIMENSIONS="${TEST_DIMENSIONS:-128}"
TIMEOUT_SECS="${TIMEOUT_SECS:-60}"
INPUT_TEXT="${INPUT_TEXT:-测试 embedding dimensions 参数是否生效}"

BASE_URL="${BASE_URL%/}"
ROOT_URL="$BASE_URL"
if [[ "$ROOT_URL" == */v1 ]]; then
  ROOT_URL="${ROOT_URL%/v1}"
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required for JSON parsing" >&2
  exit 1
fi

json_escape() {
  python3 -c 'import json,sys; print(json.dumps(sys.argv[1], ensure_ascii=False))' "$1"
}

vector_dim() {
  local schema="$1"
  local response_file="$2"
  python3 - "$schema" "$response_file" <<'PY'
import json
import sys

schema = sys.argv[1]
response_file = sys.argv[2]
with open(response_file, "r", encoding="utf-8") as f:
    raw = f.read()
try:
    data = json.loads(raw)
    if schema == "native":
        vec = data["embeddings"][0]
    else:
        vec = data["data"][0]["embedding"]
    print(len(vec))
except Exception as exc:
    print(f"PARSE_ERROR: {exc}")
    print(raw[:1000])
    sys.exit(2)
PY
}

post_json() {
  local url="$1"
  local body="$2"
  local outfile="$3"
  curl -sS \
    --max-time "$TIMEOUT_SECS" \
    -o "$outfile" \
    -w "%{http_code}" \
    -H "Content-Type: application/json" \
    -X POST "$url" \
    -d "$body"
}

test_endpoint() {
  local label="$1"
  local url="$2"
  local schema="$3"
  local escaped_input
  escaped_input="$(json_escape "$INPUT_TEXT")"

  local body_base body_dim tmp_base tmp_dim code_base code_dim dim_base dim_dim
  body_base="{\"model\":\"$MODEL\",\"input\":$escaped_input}"
  body_dim="{\"model\":\"$MODEL\",\"input\":$escaped_input,\"dimensions\":$TEST_DIMENSIONS}"
  tmp_base="$(mktemp)"
  tmp_dim="$(mktemp)"

  echo
  echo "== $label =="
  echo "URL: $url"

  code_base="$(post_json "$url" "$body_base" "$tmp_base" || true)"
  if [[ "$code_base" != 2* ]]; then
    echo "baseline request failed: HTTP $code_base"
    sed -n '1,20p' "$tmp_base"
    rm -f "$tmp_base" "$tmp_dim"
    return 0
  fi

  dim_base="$(vector_dim "$schema" "$tmp_base" || true)"
  echo "baseline dimension: $dim_base"

  code_dim="$(post_json "$url" "$body_dim" "$tmp_dim" || true)"
  if [[ "$code_dim" != 2* ]]; then
    echo "dimensions request failed: HTTP $code_dim"
    sed -n '1,20p' "$tmp_dim"
    rm -f "$tmp_base" "$tmp_dim"
    return 0
  fi

  dim_dim="$(vector_dim "$schema" "$tmp_dim" || true)"
  echo "requested dimensions: $TEST_DIMENSIONS"
  echo "returned dimension:   $dim_dim"

  if [[ "$dim_dim" == "$TEST_DIMENSIONS" ]]; then
    echo "RESULT: dimensions is supported and effective."
  elif [[ "$dim_dim" == "$dim_base" ]]; then
    echo "RESULT: request succeeded, but dimensions appears ignored."
  else
    echo "RESULT: request succeeded, but returned an unexpected dimension."
  fi

  rm -f "$tmp_base" "$tmp_dim"
}

echo "Ollama root: $ROOT_URL"
echo "Model:       $MODEL"
echo "Probe dims:  $TEST_DIMENSIONS"

version_tmp="$(mktemp)"
version_code="$(curl -sS --max-time 10 -o "$version_tmp" -w "%{http_code}" "$ROOT_URL/api/version" || true)"
if [[ "$version_code" == 2* ]]; then
  echo "Version:     $(cat "$version_tmp")"
else
  echo "Version:     unavailable (HTTP $version_code)"
fi
rm -f "$version_tmp"

test_endpoint "Ollama native /api/embed" "$ROOT_URL/api/embed" "native"
test_endpoint "OpenAI-compatible /v1/embeddings" "$ROOT_URL/v1/embeddings" "openai"
