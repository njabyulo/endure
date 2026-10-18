"""Run a configured MCP tool followed by an attenuated local child agent."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import threading
from http.server import ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("mcp_fixture", ROOT / "examples/mcp/fixture.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class Handler(fixture.Handler):
    def reply(self, status, body=None, session=False):
        if body and "structuredContent" in body.get("result", {}):
            self.server.tool_calls += 1
        super().reply(status, body, session)

    def log_message(self, *_):
        pass


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.tool_calls = 0
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="ondure-interfaces-") as folder:
            path = Path(folder)
            origin = f"http://127.0.0.1:{server.server_port}"
            (path / "main.js").write_text((HERE / "main.js").read_text().replace("http://127.0.0.1:8789", origin))
            (path / "reviewer.js").write_text((HERE / "reviewer.js").read_text())
            (path / "ondure.yaml").write_text(json.dumps({
                "grants": {"network_origins": [origin], "tools": ["fixture.greet"], "child_agents": ["reviewer"]},
                "mcp_servers": {"fixture": {"endpoint": origin + "/mcp", "tools": {
                    "fixture.greet": {"name": "greet"}}}},
                "agents": {"reviewer": {"module": "reviewer.js", "capabilities": {},
                                        "allocation": {"tokens": 128, "dollar_micros": 1000}}},
            }))
            executable = os.environ.get("ONDURE_BIN")
            command = [executable] if executable else ["cargo", "run", "--quiet", "--manifest-path", str(ROOT / "Cargo.toml"), "--"]

            def invoke(*args):
                output = subprocess.run(command + list(args), cwd=path, text=True, capture_output=True, timeout=30)
                if output.returncode:
                    raise AssertionError(f"ondure {args}: {output.stdout}\n{output.stderr}")
                return output.stdout

            first = invoke("run", "main.js", "--run-id", "interfaces")
            assert json.loads(first) == {"reviewed": True, "text": "Hello, Reader"}
            for name in ["main.js", "reviewer.js", "ondure.yaml"]:
                (path / name).unlink()
            assert invoke("replay", "interfaces") == first
            assert server.tool_calls == 1
            with sqlite3.connect(path / ".ondure/journal.sqlite3") as db:
                assert db.execute("SELECT COUNT(*) FROM runs").fetchone()[0] == 2
            print(first, end="")
            print("Verified: one MCP tool call, one durable child run; replay matched.")
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    main()
