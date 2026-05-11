from __future__ import annotations

import os
import tempfile
from pathlib import Path

import yaml
from fastapi.testclient import TestClient

from saxo_daytrader_xai.api.app import create_app


def main() -> int:
    with Path("config.yaml").open("r", encoding="utf-8") as handle:
        config = yaml.safe_load(handle)

    with tempfile.TemporaryDirectory(prefix="saxo_phase41_") as tmp_dir:
        temp_root = Path(tmp_dir)
        database_path = temp_root / "ledger.db"
        config["portfolio"]["database_path"] = str(database_path)
        config["portfolio"]["source_csv"] = ""
        config["execution"]["mode"] = "simulation"
        config["app"]["dry_run"] = True
        config["notifications"]["slack"]["enabled"] = False
        config["scheduler"]["enabled"] = False
        temp_config_path = temp_root / "config.yaml"
        temp_config_path.write_text(yaml.safe_dump(config, sort_keys=False), encoding="utf-8")

        previous = os.environ.get("DAYTRADER_CONFIG")
        os.environ["DAYTRADER_CONFIG"] = str(temp_config_path)
        try:
            client = TestClient(create_app(str(temp_config_path)))
            health = client.get("/api/health")
            overview = client.get("/api/overview")
            positions = client.get("/api/portfolio/positions?limit=25")
            performance = client.get("/api/performance?range_key=1D")
            market = client.get("/api/market/status")
            execution = client.get("/api/execution?limit=20")

            assert health.status_code == 200, health.text
            assert overview.status_code == 200, overview.text
            assert positions.status_code == 200, positions.text
            assert performance.status_code == 200, performance.text
            assert market.status_code == 200, market.text
            assert execution.status_code == 200, execution.text

            overview_payload = overview.json()
            performance_payload = performance.json()
            execution_payload = execution.json()

            assert overview_payload["portfolio_summary"]["position_count"] == 0, overview_payload
            assert isinstance(performance_payload["history"], list), performance_payload
            assert isinstance(execution_payload["orders"], list), execution_payload

            print("Phase 41 validation passed.")
            print(f"Execution mode: {overview_payload['execution']['mode']}")
            print(f"Portfolio value DKK: {overview_payload['portfolio_summary']['total_market_value_dkk']:.2f}")
            print(f"Market rows: {len(market.json()['items'])}")
            print(f"Execution rows: {len(execution_payload['orders'])}")
        finally:
            if previous is None:
                os.environ.pop("DAYTRADER_CONFIG", None)
            else:
                os.environ["DAYTRADER_CONFIG"] = previous
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
