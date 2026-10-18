//! Bounded MCP Streamable HTTP tool adapter; no subprocess or ambient discovery.
use super::mcp_schema::{validate_instance, validate_schema};
use crate::core::operations::validate_json;
use crate::{
    Capabilities, HostConfig, OperationCancellation, RecoveryPolicy, MAX_OPERATION_ARGUMENT_BYTES,
    MAX_OPERATION_RESULT_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read},
    time::Duration,
};

const PROTOCOL: &str = "2025-11-25";
const VERSIONS: &[&str] = &[PROTOCOL, "2025-06-18", "2025-03-26"];
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerConfig {
    pub endpoint: String,
    /// Logical key in HostConfig.credentials; values are resolved only in host.
    pub credential: Option<String>,
    /// Configured public alias -> remote tool and explicit retry authority.
    pub tools: BTreeMap<String, McpToolConfig>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpToolConfig {
    pub name: String,
    pub retry_interrupted: bool,
}
impl McpServerConfig {
    pub(crate) fn validate(&self, config: &HostConfig) -> Result<(), String> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| "invalid MCP endpoint")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
        {
            return Err(
                "MCP endpoint must be an HTTP(S) URL without credentials, query, or fragment"
                    .into(),
            );
        }
        if self
            .credential
            .as_ref()
            .is_some_and(|key| !config.credentials.contains_key(key))
        {
            return Err("MCP credential must name a configured environment reference".into());
        }
        for (alias, tool) in &self.tools {
            if alias.is_empty()
                || alias.len() > 128
                || !alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                || tool.name.is_empty()
                || tool.name.len() > 128
                || tool.name.chars().any(char::is_control)
            {
                return Err("invalid configured MCP tool name".into());
            }
        }
        Ok(())
    }
}

pub(crate) struct McpCall {
    server: McpServerConfig,
    tool: McpToolConfig,
    arguments: Value,
    authority: Capabilities,
    alias: String,
    credential_env: Option<String>,
}
impl McpCall {
    pub(crate) fn prepare(
        config: &HostConfig,
        authority: &Capabilities,
        alias: &str,
        arguments: Value,
    ) -> Result<Self, String> {
        config.validate()?;
        authority.require_tool(alias)?;
        validate_json(&arguments, MAX_OPERATION_ARGUMENT_BYTES, "arguments")?;
        if !arguments.is_object() {
            return Err("MCP tool arguments must be a JSON object".into());
        }
        let (server, tool) = config
            .mcp_servers
            .values()
            .find_map(|server| server.tools.get(alias).map(|tool| (server, tool)))
            .ok_or("unknown configured MCP tool")?;
        authority.require_network(&server.endpoint)?;
        Ok(Self {
            server: server.clone(),
            tool: tool.clone(),
            arguments,
            authority: authority.clone(),
            alias: alias.into(),
            credential_env: server
                .credential
                .as_ref()
                .map(|key| config.credentials[key].clone()),
        })
    }
    pub(crate) fn recovery(&self) -> RecoveryPolicy {
        if self.tool.retry_interrupted {
            RecoveryPolicy::RetrySafe
        } else {
            RecoveryPolicy::Interrupt
        }
    }
    pub(crate) fn endpoint(&self) -> &str {
        &self.server.endpoint
    }
    pub(crate) fn run(self, cancellation: OperationCancellation) -> Result<Value, String> {
        let secret = self
            .credential_env
            .as_ref()
            .map(|name| {
                std::env::var(name).map_err(|_| {
                    "configured MCP credential environment variable is unavailable".to_string()
                })
            })
            .transpose()?;
        if secret
            .as_ref()
            .is_some_and(|s| s.is_empty() || !s.bytes().all(|b| (0x21..=0x7e).contains(&b)))
        {
            return Err("MCP credential must be a nonempty ASCII bearer token".into());
        }
        let mut client = Client {
            endpoint: self.server.endpoint,
            authority: self.authority,
            alias: self.alias,
            secret,
            session: None,
            protocol: None,
            cancellation,
            client: ureq::AgentBuilder::new().redirects(0).build(),
        };
        let result = client.invoke(&self.tool.name, &self.arguments);
        client.close();
        result
    }
}
struct Client {
    endpoint: String,
    authority: Capabilities,
    alias: String,
    secret: Option<String>,
    session: Option<String>,
    protocol: Option<String>,
    cancellation: OperationCancellation,
    client: ureq::Agent,
}
impl Client {
    fn request(&self, method: &str) -> Result<ureq::Request, String> {
        self.cancellation.check()?;
        self.authority.require_tool(&self.alias)?;
        self.authority.require_network(&self.endpoint)?;
        let mut request = self
            .client
            .request(method, &self.endpoint)
            .timeout(TIMEOUT)
            .set("Accept", "application/json, text/event-stream")
            .set("Content-Type", "application/json");
        if let Some(secret) = &self.secret {
            request = request.set("Authorization", &format!("Bearer {secret}"));
        }
        if let Some(session) = &self.session {
            request = request.set("Mcp-Session-Id", session);
        }
        if let Some(protocol) = &self.protocol {
            request = request.set("MCP-Protocol-Version", protocol);
        }
        Ok(request)
    }
    fn post(&mut self, body: &Value, expected_id: Option<u64>) -> Result<Value, String> {
        let response = self
            .request("POST")?
            .send_string(&body.to_string())
            .map_err(|_| "MCP HTTP request failed; remote outcome may be unknown".to_string())?;
        self.cancellation.check()?;
        if let Some(session) = response.header("Mcp-Session-Id") {
            if session.is_empty()
                || session.len() > 512
                || !session.bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                return Err("invalid MCP session identifier".into());
            }
            if let Some(existing) = &self.session {
                if existing != session {
                    return Err("MCP session identifier changed".into());
                }
            } else if expected_id == Some(1) {
                self.session = Some(session.into());
            } else {
                return Err("MCP session identifier supplied after initialization".into());
            }
        }
        if expected_id.is_none() {
            if response.status() != 202 {
                return Err("MCP notification was not accepted".into());
            }
            let mut byte = [0];
            if response
                .into_reader()
                .read(&mut byte)
                .map_err(|_| "MCP notification response read failed")?
                != 0
            {
                return Err("MCP notification response must have no body".into());
            }
            return Ok(Value::Null);
        }
        let id = expected_id.unwrap();
        if response.status() != 200 {
            return Err("unexpected MCP response status".into());
        }
        let content_type = response
            .header("Content-Type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let message = match content_type.as_str() {
            "application/json" => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_OPERATION_RESULT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "MCP response read failed")?;
                if bytes.len() > MAX_OPERATION_RESULT_BYTES {
                    return Err("MCP response exceeds byte limit".into());
                }
                serde_json::from_slice(&bytes).map_err(|_| "invalid MCP JSON response")?
            }
            "text/event-stream" => read_sse(response.into_reader(), id)?,
            _ => return Err("unsupported MCP response content type".into()),
        };
        let result = rpc_result(message, id)?;
        if self
            .secret
            .as_ref()
            .is_some_and(|secret| contains_secret(&result, secret))
        {
            return Err("MCP response contained a host credential".into());
        }
        self.cancellation.check()?;
        Ok(result)
    }
    fn invoke(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let initialized=self.post(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"ondure","version":env!("CARGO_PKG_VERSION")}}}),Some(1))?;
        let protocol = initialized["protocolVersion"]
            .as_str()
            .filter(|version| VERSIONS.contains(version))
            .ok_or("unsupported MCP protocol version")?;
        if !initialized["capabilities"]["tools"].is_object()
            || !initialized["serverInfo"]["name"].is_string()
            || !initialized["serverInfo"]["version"].is_string()
        {
            return Err("invalid MCP initialization response or missing tools capability".into());
        }
        self.protocol = Some(protocol.into());
        self.post(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            None,
        )?;
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut selected = None;
        let mut next_id = 2;
        for page in 0..8 {
            let params = cursor
                .as_ref()
                .map_or(json!({}), |cursor| json!({"cursor":cursor}));
            let list = self.post(
                &json!({"jsonrpc":"2.0","id":next_id,"method":"tools/list","params":params}),
                Some(next_id),
            )?;
            next_id += 1;
            let tools = list["tools"]
                .as_array()
                .ok_or("invalid MCP tools/list response")?;
            for tool in tools {
                let tool_name = tool["name"].as_str().ok_or("invalid MCP tool name")?;
                if !seen.insert(tool_name.to_string()) || seen.len() > 512 {
                    return Err("duplicate or excessive MCP tools".into());
                }
                if tool_name == name {
                    selected = Some(tool.clone());
                }
            }
            cursor = match list.get("nextCursor") {
                None => None,
                Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
                _ => return Err("invalid MCP pagination cursor".into()),
            };
            if cursor.is_none() {
                break;
            }
            if page == 7 {
                return Err("MCP tool pagination exceeds limit".into());
            }
        }
        let tool = selected.ok_or("configured MCP tool was not advertised by server")?;
        if tool["inputSchema"]["type"] != "object" {
            return Err("MCP input schema must have object type".into());
        }
        validate_instance(&tool["inputSchema"], arguments)?;
        if let Some(output) = tool.get("outputSchema") {
            if output["type"] != "object" {
                return Err("MCP output schema must have object type".into());
            }
            validate_schema(output)?;
        }
        let result=self.post(&json!({"jsonrpc":"2.0","id":next_id,"method":"tools/call","params":{"name":name,"arguments":arguments}}),Some(next_id))?;
        validate_tool_result(&result)?;
        if let Some(output) = tool.get("outputSchema") {
            if result["isError"] != true {
                validate_instance(
                    output,
                    result
                        .get("structuredContent")
                        .ok_or("MCP output schema requires structuredContent")?,
                )?;
            }
        }
        validate_json(&result, MAX_OPERATION_RESULT_BYTES - 128, "result")?;
        Ok(result)
    }
    fn close(&self) {
        if self.session.is_some() {
            if let Ok(request) = self.request("DELETE") {
                let _ = request.call();
            }
        }
    }
}
fn rpc_result(message: Value, id: u64) -> Result<Value, String> {
    if message["jsonrpc"] != "2.0"
        || message["id"].as_u64() != Some(id)
        || message.get("method").is_some()
        || message.get("result").is_some() == message.get("error").is_some()
    {
        return Err("invalid MCP JSON-RPC envelope or response ID".into());
    }
    if message.get("error").is_some() {
        return Err("MCP server returned a JSON-RPC error".into());
    }
    Ok(message["result"].clone())
}
fn read_sse(reader: impl Read, id: u64) -> Result<Value, String> {
    let mut reader = BufReader::new(reader.take(MAX_OPERATION_RESULT_BYTES as u64 + 1));
    let mut bytes = 0;
    let mut data = String::new();
    loop {
        let mut line = String::new();
        let count = reader
            .read_line(&mut line)
            .map_err(|_| "invalid MCP SSE stream")?;
        bytes += count;
        if bytes > MAX_OPERATION_RESULT_BYTES {
            return Err("MCP SSE response exceeds byte limit".into());
        }
        if count == 0 {
            return Err("MCP SSE stream ended before response".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if data.trim_end_matches('\n').is_empty() {
                data.clear();
                continue;
            }
            let message: Value = serde_json::from_str(data.trim_end_matches('\n'))
                .map_err(|_| "invalid MCP SSE JSON")?;
            data.clear();
            if message.get("id").is_some() {
                if message["id"].as_u64() != Some(id) {
                    return Err("MCP SSE response ID mismatch or unsupported server request".into());
                }
                return Ok(message);
            }
            if message["jsonrpc"] != "2.0"
                || !message["method"]
                    .as_str()
                    .is_some_and(|method| method.starts_with("notifications/"))
            {
                return Err("unsupported MCP SSE message".into());
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            data.push('\n');
        }
    }
}
fn contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(text) => text.contains(secret),
        Value::Array(values) => values.iter().any(|v| contains_secret(v, secret)),
        Value::Object(values) => values
            .iter()
            .any(|(key, v)| key.contains(secret) || contains_secret(v, secret)),
        _ => false,
    }
}
fn validate_tool_result(result: &Value) -> Result<(), String> {
    if result.get("isError").is_some_and(|v| !v.is_boolean()) {
        return Err("invalid MCP tool error flag".into());
    }
    let content = result["content"]
        .as_array()
        .ok_or("invalid MCP tool content")?;
    // Initial API accepts text and JSON structured content; binary/resource
    // content is deliberately rejected rather than fetched or interpreted.
    for block in content {
        if block["type"] != "text" || !block["text"].is_string() {
            return Err("unsupported MCP content block (text profile only)".into());
        }
    }
    if result
        .get("structuredContent")
        .is_some_and(|value| !value.is_object())
    {
        return Err("invalid MCP structuredContent".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread::{self, JoinHandle},
    };
    struct Fixture {
        endpoint: String,
        stop: Arc<AtomicBool>,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        worker: Option<JoinHandle<()>>,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let worker_stop = stop.clone();
            let log = requests.clone();
            let mode = mode.to_string();
            let worker = thread::spawn(move || loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let (headers, body) = read_request(&mut stream);
                        let method = body["method"].as_str().unwrap_or("DELETE").to_string();
                        log.lock().unwrap().push((method.clone(), headers));
                        if method == "DELETE" || method == "notifications/initialized" {
                            let _=write!(stream,"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            continue;
                        }
                        let result = match method.as_str() {
                            "initialize" => {
                                json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
                            }
                            "tools/list" => {
                                let schema = if mode == "schema_ref" {
                                    json!({"type":"object","$ref":"https://untrusted.example/schema"})
                                } else {
                                    json!({"type":"object","properties":{"name":{"type":"string","maxLength":20}},"required":["name"],"additionalProperties":false})
                                };
                                json!({"tools":[{"name":"greet","inputSchema":schema,"outputSchema":{"type":"object","properties":{"greeting":{"type":"string"}},"required":["greeting"],"additionalProperties":false}}]})
                            }
                            "tools/call" => {
                                if mode == "oversized" {
                                    json!({"content":[{"type":"text","text":"x".repeat(MAX_OPERATION_RESULT_BYTES+1)}]})
                                } else if mode == "bad_output" {
                                    json!({"content":[],"structuredContent":{"greeting":5}})
                                } else {
                                    json!({"content":[{"type":"text","text":"hello"}],"structuredContent":{"greeting":"hello"}})
                                }
                            }
                            _ => panic!("unexpected method"),
                        };
                        let id = if mode == "wrong_id" {
                            json!(999)
                        } else {
                            body["id"].clone()
                        };
                        let message = json!({"jsonrpc":"2.0","id":id,"result":result});
                        let (mime, text) = if mode == "sse" && method == "tools/call" {
                            ("text/event-stream",format!("id: first\ndata:\n\n: heartbeat\n\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{{}}}}\n\nevent: message\ndata: {message}\n\n"))
                        } else {
                            ("application/json", message.to_string())
                        };
                        let session = if method == "initialize" {
                            "Mcp-Session-Id: fixture-session\r\n"
                        } else {
                            ""
                        };
                        let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("fixture: {e}"),
                }
            });
            Self {
                endpoint,
                stop,
                requests,
                worker: Some(worker),
            }
        }
        fn call(&self, args: Value) -> Result<Value, String> {
            let (config, authority) = self.configuration();
            McpCall::prepare(&config, &authority, "fixture.greet", args)?
                .run(OperationCancellation::default())
        }
        fn configuration(&self) -> (HostConfig, Capabilities) {
            let grants = Capabilities {
                tools: ["fixture.greet".into()].into(),
                network_origins: [url::Url::parse(&self.endpoint)
                    .unwrap()
                    .origin()
                    .ascii_serialization()]
                .into(),
                ..Default::default()
            };
            let config = HostConfig {
                grants: grants.clone(),
                mcp_servers: [(
                    "fixture".into(),
                    McpServerConfig {
                        endpoint: self.endpoint.clone(),
                        tools: [(
                            "fixture.greet".into(),
                            McpToolConfig {
                                name: "greet".into(),
                                retry_interrupted: false,
                            },
                        )]
                        .into(),
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            };
            (config, grants)
        }
        fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(m, _)| m.clone())
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.worker.take().unwrap().join().unwrap();
        }
    }
    fn read_request(stream: &mut TcpStream) -> (String, Value) {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).unwrap() == 0 {
                break;
            }
            bytes.push(byte[0]);
        }
        let headers = String::from_utf8(bytes).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        (
            headers,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }
    #[test]
    fn json_and_sse_tools_negotiate_sessions_and_validate_results() {
        for mode in ["json", "sse"] {
            let fixture = Fixture::new(mode);
            let result = fixture.call(json!({"name":"reader"})).unwrap();
            assert_eq!(result["structuredContent"]["greeting"], "hello");
            assert_eq!(
                fixture.methods(),
                [
                    "initialize",
                    "notifications/initialized",
                    "tools/list",
                    "tools/call",
                    "DELETE"
                ]
            );
            for (_, headers) in fixture.requests.lock().unwrap().iter().skip(1) {
                let headers = headers.to_ascii_lowercase();
                assert!(headers.contains("mcp-session-id: fixture-session"));
                assert!(headers.contains("mcp-protocol-version: 2025-11-25"));
            }
        }
    }
    #[test]
    fn denied_authority_never_contacts_server() {
        let fixture = Fixture::new("json");
        let (config, mut authority) = fixture.configuration();
        authority.tools.clear();
        assert!(McpCall::prepare(&config, &authority, "fixture.greet", json!({})).is_err());
        authority = config.grants.clone();
        authority.network_origins.clear();
        assert!(McpCall::prepare(&config, &authority, "fixture.greet", json!({})).is_err());
        assert!(fixture.methods().is_empty());
    }
    #[test]
    fn malicious_schema_or_invalid_arguments_never_invoke_tool() {
        for (mode, args) in [
            ("schema_ref", json!({"name":"reader"})),
            ("json", json!({"name":123})),
        ] {
            let fixture = Fixture::new(mode);
            assert!(fixture.call(args).is_err());
            assert!(!fixture.methods().contains(&"tools/call".into()));
        }
    }
    #[test]
    fn malformed_envelopes_outputs_and_oversized_responses_fail() {
        for mode in ["wrong_id", "bad_output", "oversized"] {
            let fixture = Fixture::new(mode);
            assert!(fixture.call(json!({"name":"reader"})).is_err(), "{mode}");
        }
    }
    #[test]
    fn recovery_requires_explicit_administrator_choice() {
        let fixture = Fixture::new("json");
        let (config, authority) = fixture.configuration();
        let call = McpCall::prepare(
            &config,
            &authority,
            "fixture.greet",
            json!({"name":"reader"}),
        )
        .unwrap();
        assert_eq!(call.recovery(), RecoveryPolicy::Interrupt);
        let mut config = config;
        config
            .mcp_servers
            .get_mut("fixture")
            .unwrap()
            .tools
            .get_mut("fixture.greet")
            .unwrap()
            .retry_interrupted = true;
        assert_eq!(
            McpCall::prepare(
                &config,
                &authority,
                "fixture.greet",
                json!({"name":"reader"})
            )
            .unwrap()
            .recovery(),
            RecoveryPolicy::RetrySafe
        );
        assert!(fixture.methods().is_empty());
    }
    fn agent_project(
        fixture: &Fixture,
        body: &str,
    ) -> (std::path::PathBuf, crate::ModuleGraph, HostConfig) {
        let (config, capabilities) = fixture.configuration();
        let directory =
            std::env::temp_dir().join(format!("ondure-mcp-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let source=format!("import {{agent}} from 'ondure'; export default agent('mcp', {{capabilities:{}}}, async ctx => {{ {body} }});",serde_json::to_string(&capabilities).unwrap());
        std::fs::write(directory.join("main.js"), source).unwrap();
        let graph = crate::ModuleGraph::load(&directory.join("main.js")).unwrap();
        (directory, graph, config)
    }
    #[test]
    fn agent_tool_result_commits_before_delivery_and_replays_without_server() {
        let fixture = Fixture::new("sse");
        let (directory,graph,config)=agent_project(&fixture,"let count=0; const input={toJSON(){count++;return {name:'reader'};}}; const result=await ctx.tools.call('fixture.greet',input);console.log(result.structuredContent.greeting,count);");
        let journal = directory.join("journal.sqlite3");
        let first = crate::run_durable_with_config(
            &config,
            &graph,
            &journal,
            "run",
            crate::RunLimits::default(),
        )
        .unwrap();
        assert_eq!(first.stdout, "hello 1\n");
        let methods = fixture.methods();
        assert_eq!(crate::replay_run(&journal, "run", None).unwrap(), first);
        assert_eq!(fixture.methods(), methods);
        let journal = crate::Journal::open(&journal).unwrap();
        let operations = journal.operations("run").unwrap();
        assert_eq!(operations[0].name, "tool.mcp");
        assert_eq!(operations[0].state, "completed");
        assert_eq!(operations[0].policy, "never");
        assert_eq!(journal.attempts("run").unwrap().len(), 1);
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn agent_denial_produces_no_mcp_request() {
        let fixture = Fixture::new("json");
        let (directory, graph, mut config) = agent_project(
            &fixture,
            "await ctx.tools.call('fixture.greet',{name:'reader'});",
        );
        config.grants.tools.clear();
        let error = crate::run_durable_with_config(
            &config,
            &graph,
            &directory.join("journal.sqlite3"),
            "run",
            crate::RunLimits::default(),
        )
        .unwrap_err();
        assert!(error.message.contains("permission denied"));
        assert!(fixture.methods().is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn unfinished_unsafe_tools_never_redispatch_but_explicit_safe_tools_can() {
        for safe in [false, true] {
            let fixture = Fixture::new("json");
            let (directory, graph, mut config) = agent_project(
                &fixture,
                "await ctx.tools.call('fixture.greet',{name:'reader'});",
            );
            config
                .mcp_servers
                .get_mut("fixture")
                .unwrap()
                .tools
                .get_mut("fixture.greet")
                .unwrap()
                .retry_interrupted = safe;
            let journal = directory.join("journal.sqlite3");
            crate::run_durable_with_config(
                &config,
                &graph,
                &journal,
                "run",
                crate::RunLimits::default(),
            )
            .unwrap();
            let methods = fixture.methods();
            // Deterministically model the dispatch-before-outcome-commit window.
            let connection = rusqlite::Connection::open(&journal).unwrap();
            connection.execute("UPDATE operations SET state='pending',result=NULL,completion_order=NULL WHERE run_id='run' AND seq=0",[]).unwrap();
            connection.execute("UPDATE operation_attempts SET state='dispatched',outcome=NULL WHERE run_id='run'",[]).unwrap();
            connection
                .execute("UPDATE runs SET status='running' WHERE id='run'", [])
                .unwrap();
            drop(connection);
            let resumed = crate::replay_run(&journal, "run", None);
            if safe {
                assert!(resumed.is_ok(), "{resumed:?}");
                assert_eq!(
                    fixture
                        .methods()
                        .iter()
                        .filter(|m| m.as_str() == "tools/call")
                        .count(),
                    2
                );
                assert_eq!(
                    crate::Journal::open(&journal)
                        .unwrap()
                        .attempts("run")
                        .unwrap()
                        .len(),
                    2
                );
            } else {
                assert!(resumed.unwrap_err().message.contains("InterruptedError"));
                assert_eq!(fixture.methods(), methods);
            }
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
