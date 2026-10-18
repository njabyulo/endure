"""Local MCP Streamable HTTP fixture. No credentials or external effects."""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body=None, session=False):
        raw = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        if session:
            self.send_header("Mcp-Session-Id", "ondure-local-fixture")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        if self.path != "/mcp":
            self.reply(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        if length > 256 * 1024:
            self.reply(413)
            return
        request = json.loads(self.rfile.read(length))
        method = request.get("method")
        if method != "initialize" and (
            self.headers.get("Mcp-Session-Id") != "ondure-local-fixture"
            or self.headers.get("MCP-Protocol-Version") != "2025-11-25"
        ):
            self.reply(400)
            return
        if method == "notifications/initialized":
            self.reply(202)
            return
        if method == "initialize":
            result = {
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "ondure-local-fixture", "version": "1"},
            }
        elif method == "tools/list":
            result = {
                "tools": [{
                    "name": "greet",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"name": {"type": "string", "maxLength": 80}},
                        "required": ["name"],
                        "additionalProperties": False,
                    },
                    "outputSchema": {
                        "type": "object",
                        "properties": {"greeting": {"type": "string"}},
                        "required": ["greeting"],
                        "additionalProperties": False,
                    },
                }]
            }
        elif method == "tools/call" and request["params"]["name"] == "greet":
            greeting = "Hello, " + request["params"]["arguments"]["name"]
            print("tools/call greet", file=sys.stderr, flush=True)
            result = {
                "content": [{"type": "text", "text": greeting}],
                "structuredContent": {"greeting": greeting},
            }
        else:
            self.reply(400)
            return
        self.reply(200, {"jsonrpc": "2.0", "id": request["id"], "result": result}, method == "initialize")

    def do_DELETE(self):
        self.reply(200)


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", 8789), Handler).serve_forever()
