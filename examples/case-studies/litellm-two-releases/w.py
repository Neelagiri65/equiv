import os, warnings, logging
os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] = "True"
warnings.filterwarnings("ignore"); logging.disable(logging.CRITICAL)
import litellm
litellm.drop_params = False
MODELS = ["claude-sonnet-5-5", "anthropic/claude-opus-4-8", "vertex_ai/claude-sonnet-5-5", "azure_ai/claude-opus-4-8", "anthropic.claude-opus-4-8", "claude-sonnet-4-6"]

def temperature_one(model: str) -> str:
    try:
        litellm.completion(model=model, messages=[{"role": "user", "content": "hi"}], temperature=1, mock_response="ok", api_key="sk-test")
        return "ok"
    except Exception as e:
        return type(e).__name__

def tokens(text: str) -> int:
    return litellm.token_counter(model="gpt-4o", text=text)
