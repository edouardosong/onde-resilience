# ONDE E2E — Midscene.js + Playwright

Tests E2E pilotés par IA (Midscene.js) sur l'UI réelle de l'app (`android/app/src/main/assets/index.html`),
exécutés en headless Chromium avec un LLM **local** via LM Studio (politique 100 % gratuit/local).

## Configuration
- `MIDSCENE_MODEL_NAME=huihui-qwen3.8-27b-abliterated` (vision, via LM Studio)
- `OPENAI_BASE_URL=http://192.168.1.23:1234/v1` (endpoint OpenAI-compatible local)

## Exécution
```bash
cd e2e && npm install   # première fois
npm test                # 9 scénarios, ~1-3 min/scénario selon charge LLM local
```

Chaque `aiTap/aiInput/aiAssert` = capture d'écran → LLM multimodal local → action/assertion.
Les mêmes scénarios sont exécutés sur les appareils physiques via CDP (voir reports/e2e-bugs-t32b.md).
