from __future__ import annotations

from typing import Any


def apply_response_options(request_json: dict[str, Any], config: dict[str, Any]) -> dict[str, Any]:
    """Attach xAI Responses API knobs shared by decision, manager, and diary calls."""
    xai_cfg = config.get("xai", {})
    max_output_tokens = int(xai_cfg.get("max_output_tokens") or 0)
    if max_output_tokens > 0:
        request_json["max_output_tokens"] = max_output_tokens

    reasoning_effort = str(xai_cfg.get("reasoning_effort") or "").strip().lower()
    if reasoning_effort and reasoning_effort != "default":
        request_json["reasoning"] = {"effort": reasoning_effort}

    return request_json


def timeout_seconds(config: dict[str, Any], default: int = 600) -> int:
    """Return a bounded timeout so long reasoning calls do not fail at 120 seconds."""
    value = int(config.get("xai", {}).get("timeout_seconds", default) or default)
    return max(30, value)
