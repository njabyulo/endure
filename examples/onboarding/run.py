"""Exercise actual process interruption against local idempotent fixtures."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


class Receiver(BaseHTTPRequestHandler):
    lock = threading.Lock()
    model_calls = 0
    tool_calls = 0
    effects = {}
    dispatched = threading.Event()
    release = threading.Event()

    def reply(self, status, body=None, session=False):
        raw = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        if session:
            self.send_header("Mcp-Session-Id", "onboarding-fixture")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/v1/chat/completions":
            with self.lock:
                Receiver.model_calls += 1
            self.reply(200, {
                "choices": [{"message": {"content": "Welcome Ada"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6},
            })
            return
        if self.path != "/mcp":
            self.reply(404)
            return
        method = request.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "onboarding-fixture", "version": "1"}}
        elif method == "notifications/initialized":
            self.reply(202)
            return
        elif method == "tools/list":
            result = {"tools": [{
                "name": "register_customer",
                "inputSchema": {
                    "type": "object", "properties": {
                        "operation_id": {"type": "string", "maxLength": 80},
                        "name": {"type": "string", "maxLength": 80},
                    }, "required": ["operation_id", "name"], "additionalProperties": False,
                },
                "outputSchema": {
                    "type": "object", "properties": {"registered": {"type": "string"}},
                    "required": ["registered"], "additionalProperties": False,
                },
            }]}
        elif method == "tools/call" and request["params"]["name"] == "register_customer":
            arguments = request["params"]["arguments"]
            key = arguments["operation_id"]
            with self.lock:
                Receiver.tool_calls += 1
                Receiver.effects.setdefault(key, {"registered": arguments["name"]})
                receipt = Receiver.effects[key]
                first = Receiver.tool_calls == 1
            if first:
                # Commit the mock effect before the runtime can commit its reply.
                Receiver.dispatched.set()
                if not Receiver.release.wait(15):
                    self.reply(500)
                    return
            result = {"content": [{"type": "text", "text": json.dumps(receipt)}],
                      "structuredContent": receipt}
        else:
            self.reply(400)
            return
        self.reply(200, {"jsonrpc": "2.0", "id": request["id"], "result": result}, method == "initialize")

    def do_DELETE(self):
        self.reply(200)

    def log_message(self, *_):
        pass


def command():
    # CI supplies its freshly built executable; local users can run via cargo.
    executable = os.environ.get("ONDURE_BIN")
    return [executable] if executable else ["cargo", "run", "--quiet", "--manifest-path", str(ROOT / "Cargo.toml"), "--"]


def invoke(path, *args):
    options = ["--cpu-ms", "10000"] if args[0] in {"run", "replay", "approve", "wake"} else []
    result = subprocess.run(command() + list(args) + options, cwd=path, capture_output=True,
                            text=True, timeout=30)
    if result.returncode:
        raise AssertionError(f"ondure {args}: {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


def worker_lease_path(journal, run_id):
    # Match RunLease's canonical path and OS lock, without guessing a process PID.
    lease_dir = Path(str(journal.resolve()) + ".leases")
    lease_name = hashlib.sha256(("run:" + run_id).encode()).hexdigest()
    return lease_dir / lease_name


def assert_worker_holds_lease(journal, run_id):
    # This also verifies that Python and Rust are observing the same OS lock.
    with worker_lease_path(journal, run_id).open("r+b") as lease:
        try:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        fcntl.flock(lease, fcntl.LOCK_UN)
        raise AssertionError("dispatching worker does not hold its run lease")


def wait_for_worker_release(journal, run_id):
    deadline = time.monotonic() + 15
    with worker_lease_path(journal, run_id).open("r+b") as lease:
        while True:
            try:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fcntl.flock(lease, fcntl.LOCK_UN)
                return
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise AssertionError("killed CLI worker still holds its run lease")
                time.sleep(0.01)


def main():
    if not os.environ.get("ONDURE_BIN"):
        # Cold compilation is deliberately outside each invocation's timeout.
        subprocess.run(["cargo", "build", "--quiet", "--manifest-path",
                        str(ROOT / "Cargo.toml"), "--bin", "ondure"],
                       cwd=ROOT, check=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    approval = None
    try:
        with tempfile.TemporaryDirectory(prefix="ondure-onboarding-") as folder:
            path = Path(folder)
            origin = f"http://127.0.0.1:{server.server_port}"
            (path / "main.js").write_text((HERE / "main.js").read_text().replace("http://127.0.0.1:8790", origin))
            (path / "reviewer.js").write_text((HERE / "reviewer.js").read_text())
            (path / "ondure.yaml").write_text(json.dumps({
                "grants": {"network_origins": [origin], "tools": ["onboarding.register"],
                           "memory_scopes": ["onboarding"], "child_agents": ["reviewer"]},
                "model": {"endpoint": origin + "/v1/chat/completions", "model": "fixture", "max_output_tokens": 64},
                "resources": {"token_limit": 10000, "dollar_limit_micros": 1000000,
                              "model_input_token_estimate": 16, "elapsed_deadline_ms": 60000},
                "mcp_servers": {"onboarding": {"endpoint": origin + "/mcp", "tools": {
                    "onboarding.register": {"name": "register_customer", "retry_interrupted": True}}}},
                "agents": {"reviewer": {"module": "reviewer.js", "capabilities": {},
                                        "allocation": {"tokens": 128, "dollar_micros": 1000}}},
            }))
            journal = path / ".ondure/journal.sqlite3"
            invoke(path, "run", "main.js", "--request", "onboarding-ada", "--input",
                   json.dumps({"name": "Ada", "operation_id": "onboarding-ada"}))
            with sqlite3.connect(journal) as db:
                run_id, status = db.execute("SELECT id,status FROM runs WHERE request_id='onboarding-ada'").fetchone()
                wait_seq = db.execute("SELECT seq FROM waits WHERE run_id=? AND kind='approval'", (run_id,)).fetchone()[0]
            assert status == "parked"
            assert Receiver.model_calls == 1 and Receiver.tool_calls == 0
            approval = subprocess.Popen(command() + ["approve", run_id, str(wait_seq), "yes", "--note", "Reviewed locally", "--cpu-ms", "10000"],
                                        cwd=path, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            if not Receiver.dispatched.wait(15):
                approval.kill()
                out, err = approval.communicate(timeout=5)
                raise AssertionError(f"mock effect never dispatched: {out}\n{err}")
            assert_worker_holds_lease(journal, run_id)
            approval.kill()  # SIGKILL on Unix; supervisor liveness pipe ends its worker.
            approval.communicate(timeout=5)
            assert approval.returncode == -signal.SIGKILL
            approval = None
            wait_for_worker_release(journal, run_id)
            Receiver.release.set()
            # Recovery must use captured modules/config rather than mutable files.
            for name in ["main.js", "reviewer.js", "ondure.yaml"]:
                (path / name).unlink()
            resumed = invoke(path, "replay", run_id)
            result = json.loads(resumed.stdout.strip())
            assert result["approved"] is True and result["note"] == "Reviewed locally"
            assert result["receipt"] == {"registered": "Ada"}
            assert result["review"] == {"reviewed": True, "text": "Welcome Ada"}
            assert result["saved"]["value"] == "Ada"
            replay = invoke(path, "replay", run_id)
            assert replay.stdout == resumed.stdout
            assert Receiver.model_calls == 1 and Receiver.tool_calls == 2 and len(Receiver.effects) == 1
            log = json.loads(invoke(path, "log", run_id).stdout)
            assert log["status"] == "done"
            tool = next(op for op in log["operations"] if op["name"] == "tool.mcp")
            attempts = [attempt for attempt in log["attempts"] if attempt["seq"] == tool["seq"]]
            assert [attempt["state"] for attempt in attempts] == ["interrupted", "completed"]
            print(resumed.stdout, end="")
            print("Verified: model requests=1, mock tool attempts=2, deduplicated effects=1; replay matched.")
    finally:
        Receiver.release.set()
        if approval is not None:
            approval.kill()
            approval.communicate(timeout=5)
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    main()
