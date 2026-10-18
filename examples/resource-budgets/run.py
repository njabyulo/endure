"""Run a credential-free local provider fixture, then replay without dispatch."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]


class Provider(BaseHTTPRequestHandler):
    requests = 0

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert self.path == "/v1/chat/completions"
        assert request["stream"] is False
        assert request["max_tokens"] == 10
        Provider.requests += 1
        response = json.dumps({
            "choices": [{"message": {"content": "Local fixture completed"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *_):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    origin = f"http://127.0.0.1:{server.server_port}"
    with tempfile.TemporaryDirectory(prefix="ondure-budget-example-") as folder:
        path = Path(folder)
        source = (Path(__file__).parent / "main.ts").read_text()
        (path / "main.ts").write_text(source.replace("http://127.0.0.1:8787", origin))
        (path / "ondure.yaml").write_text(json.dumps({
            "grants": {"network_origins": [origin]},
            "model": {"endpoint": origin + "/v1/chat/completions", "model": "fixture", "max_output_tokens": 10},
            "resources": {
                "token_limit": 25,
                "dollar_limit_micros": 10000,
                "model_input_token_estimate": 10,
                "input_price_micros_per_million_tokens": 1000000,
                "output_price_micros_per_million_tokens": 2000000,
                "model_concurrency": 1,
                "tool_concurrency": 2,
                "model_rate": {"max_calls": 1, "window_ms": 80},
                "tool_rate": {"max_calls": 2, "window_ms": 100},
                "elapsed_deadline_ms": 5000,
            },
        }))
        command = ["cargo", "run", "--quiet", "--manifest-path", str(ROOT / "Cargo.toml"), "--"]
        first = subprocess.run(command + ["run", "main.ts", "--request", "greeting", "--input", '{"name":"Ada"}'], cwd=path, text=True, capture_output=True, check=True)
        import sqlite3
        with sqlite3.connect(path / ".ondure/journal.sqlite3") as db:
            run_id = db.execute("SELECT id FROM runs").fetchone()[0]
        replay = subprocess.run(command + ["replay", run_id], cwd=path, text=True, capture_output=True, check=True)
        assert first.stdout == replay.stdout
        assert Provider.requests == 2
        assert "BudgetError: token reservation exhausted" in first.stdout
        print(first.stdout, end="")
        print("Replay matched; provider requests: 2")
finally:
    server.shutdown()
    server.server_close()
    thread.join()
