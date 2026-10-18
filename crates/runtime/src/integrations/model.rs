//! Bounded, non-streaming Chat Completions adapter. Secrets resolve in the worker.
use crate::{
    core::{
        embedding::set_property,
        event_loop::JsonJob,
        operations::{
            contains_secret, validate_json, OperationCancellation, RecoveryPolicy,
            MAX_OPERATION_ARGUMENT_BYTES, MAX_OPERATION_RESULT_BYTES,
        },
    },
    host::HostState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, io::Read, time::Duration};

const DEFAULT_MODEL_TIMEOUT_SECONDS: u64 = 30;
fn default_timeout_seconds() -> u64 {
    DEFAULT_MODEL_TIMEOUT_SECONDS
}
fn default_timeout(value: &u64) -> bool {
    *value == DEFAULT_MODEL_TIMEOUT_SECONDS
}

/// Trusted endpoint, credential reference, output ceiling and request timeout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Full HTTP(S) Chat Completions URL. Automatic redirects are disabled.
    pub endpoint: String,
    pub model: String,
    /// Total adapter request timeout; default 30 seconds, configurable up to one hour.
    #[serde(
        default = "default_timeout_seconds",
        skip_serializing_if = "default_timeout"
    )]
    pub timeout_seconds: u64,
    /// Logical key into HostConfig.credentials, whose value names an env variable.
    #[serde(default)]
    pub credential: Option<String>,
    pub max_output_tokens: u64,
    /// An interrupted request may incur additional charges when explicitly retried.
    #[serde(default)]
    pub retry_interrupted: bool,
}
impl ModelConfig {
    /// Enforce endpoint transport, reference syntax and finite request bounds.
    pub fn validate(&self) -> Result<(), String> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| "invalid model endpoint")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "model endpoint must be an HTTP(S) URL without credentials, query or fragment"
                    .into(),
            );
        }
        let loopback = match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if self.credential.is_some() && url.scheme() != "https" && !loopback {
            return Err("credentialed model endpoints must use HTTPS except literal localhost or loopback IPs".into());
        }
        if !(1..=3600).contains(&self.timeout_seconds) {
            return Err("model timeout_seconds must be between 1 and 3600".into());
        }
        if self.model.trim().is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
        {
            return Err("model name must be nonempty and bounded".into());
        }
        if !(1..=1_000_000).contains(&self.max_output_tokens) {
            return Err("model max_output_tokens must be between 1 and 1000000".into());
        }
        if self
            .credential
            .as_ref()
            .is_some_and(|key| key.is_empty() || key.len() > 256)
        {
            return Err("invalid model credential reference".into());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallOptions {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    messages: Option<Vec<Message>>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: String,
    content: String,
}

fn request(config: &ModelConfig, input: Value, extra: Option<Value>) -> Result<Value, String> {
    let mut options: CallOptions = match input {
        Value::String(prompt) => CallOptions {
            prompt: Some(prompt),
            messages: None,
            max_output_tokens: None,
        },
        Value::Array(messages) => CallOptions {
            prompt: None,
            messages: Some(
                serde_json::from_value(Value::Array(messages))
                    .map_err(|_| "invalid model messages")?,
            ),
            max_output_tokens: None,
        },
        Value::Object(_) => {
            serde_json::from_value(input).map_err(|_| "invalid model call options")?
        }
        _ => return Err("model.call expects a prompt, messages, or options object".into()),
    };
    if let Some(extra) = extra {
        let extra: CallOptions =
            serde_json::from_value(extra).map_err(|_| "invalid model output options")?;
        if extra.prompt.is_some() || extra.messages.is_some() {
            return Err("second argument accepts only max_output_tokens".into());
        }
        if extra.max_output_tokens.is_some() {
            options.max_output_tokens = extra.max_output_tokens;
        }
    }
    let messages = match (options.prompt, options.messages) {
        (Some(prompt), None) => vec![Message {
            role: "user".into(),
            content: prompt,
        }],
        (None, Some(messages)) => messages,
        _ => return Err("provide exactly one of prompt or messages".into()),
    };
    if messages.is_empty()
        || messages.len() > 128
        || messages.iter().any(|m| {
            !matches!(m.role.as_str(), "system" | "user" | "assistant") || m.content.is_empty()
        })
    {
        return Err("messages require 1 to 128 system/user/assistant text messages".into());
    }
    let limit = options
        .max_output_tokens
        .unwrap_or(config.max_output_tokens);
    if limit == 0 || limit > config.max_output_tokens {
        return Err("requested output tokens exceed configured model limit".into());
    }
    let value = json!({"model":config.model,"messages":messages,"max_tokens":limit,"stream":false});
    validate_json(&value, MAX_OPERATION_ARGUMENT_BYTES, "arguments")?;
    Ok(value)
}

pub(crate) fn install_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    context: v8::Local<'s, v8::Object>,
) -> Result<(), String> {
    let model = v8::Object::new(scope);
    let call = v8::Function::new(scope, call).ok_or("model callback")?;
    set_property(scope, model, "call", call.into());
    set_property(scope, context, "model", model.into());
    Ok(())
}
fn call(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    let result = (|| -> Result<(), String> {
        let (config, credentials) = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let host = state
                .host_config
                .as_ref()
                .ok_or("model is not configured")?;
            let config = host.model.clone().ok_or("model is not configured")?;
            state.require_network(&config.endpoint)?;
            (config, host.credentials.clone())
        };
        let input = json_argument(scope, args.get(0))?;
        let extra = if args.get(1).is_undefined() {
            None
        } else {
            Some(json_argument(scope, args.get(1))?)
        };
        let request = request(&config, input, extra)?;
        let journal_args = json!({"endpoint":config.endpoint,"request":request});
        let recovery = if config.retry_interrupted {
            RecoveryPolicy::RetrySafe
        } else {
            RecoveryPolicy::Interrupt
        };
        let job = JsonJob {
            name: "model.call".into(),
            args: journal_args,
            network_url: Some(config.endpoint.clone()),
            required_tool: None,
            required_child: None,
            task: Box::new(move |cancellation| {
                execute(&config, &credentials, &request, &cancellation)
            }),
        };
        let retained = v8::Global::new(scope, resolver);
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        if session.is_none() {
            return Err("model calls require a durable run".into());
        }
        let result = state
            .operations
            .borrow_mut()
            .json(job, recovery, retained, session.as_mut());
        result
    })();
    if let Err(message) = result {
        if let Some(message) = v8::String::new(scope, &message) {
            let error = v8::Exception::error(scope, message);
            resolver.reject(scope, error);
        }
    }
    output.set(resolver.get_promise(scope).into());
}
fn json_argument(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Value, String> {
    let text = v8::json::stringify(scope, value).ok_or("model arguments must be JSON")?;
    if text.length() > MAX_OPERATION_ARGUMENT_BYTES {
        return Err("model arguments exceed limit".into());
    }
    serde_json::from_str(&text.to_rust_string_lossy(scope))
        .map_err(|_| "invalid model arguments".into())
}

fn execute(
    config: &ModelConfig,
    credentials: &BTreeMap<String, String>,
    request: &Value,
    cancellation: &OperationCancellation,
) -> Result<Value, String> {
    cancellation.check()?;
    let client = ureq::AgentBuilder::new().redirects(0).build();
    let mut call = client
        .post(&config.endpoint)
        .timeout(Duration::from_secs(config.timeout_seconds))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    let secret = config
        .credential
        .as_ref()
        .map(|reference| {
            let variable = credentials
                .get(reference)
                .ok_or("model credential reference is not configured")?;
            let secret = std::env::var(variable)
                .map_err(|_| "model credential environment variable is unavailable")?;
            if secret.is_empty() || !secret.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                return Err("model credential is invalid");
            }
            Ok(secret)
        })
        .transpose()?;
    if let Some(secret) = &secret {
        call = call.set("Authorization", &format!("Bearer {secret}"));
    }
    cancellation.check()?;
    let response = match call.send_string(&request.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => return Err(format!("model HTTP status {status}")),
        Err(_) => return Err("model transport failed; request outcome may be unknown".into()),
    };
    if !(200..300).contains(&response.status()) {
        return Err(format!("model HTTP status {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_OPERATION_RESULT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "model response read failed; request outcome may be unknown")?;
    if bytes.len() > MAX_OPERATION_RESULT_BYTES {
        return Err("model response exceeds result limit".into());
    }
    cancellation.check()?;
    let response: Value =
        serde_json::from_slice(&bytes).map_err(|_| "model response is not valid JSON")?;
    if secret
        .as_ref()
        .is_some_and(|secret| contains_secret(&response, secret))
    {
        return Err("model response contained a host credential".into());
    }
    normalize(response)
}
fn normalize(response: Value) -> Result<Value, String> {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or("model response requires a choice")?;
    let text = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .ok_or("model response requires text message content")?;
    let usage = match response.get("usage") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Object(values)) => {
            let mut usage = serde_json::Map::new();
            for name in ["prompt_tokens", "completion_tokens", "total_tokens"] {
                if let Some(value) = values.get(name) {
                    let count = value
                        .as_u64()
                        .ok_or("model usage counts must be unsigned integers")?;
                    usage.insert(name.into(), json!(count));
                }
            }
            Value::Object(usage)
        }
        _ => return Err("model usage must be an object".into()),
    };
    let finish = choice.get("finish_reason").cloned().unwrap_or(Value::Null);
    if !finish.is_null() && !finish.is_string() {
        return Err("invalid model finish reason".into());
    }
    Ok(json!({"text":text,"usage":usage,"finish_reason":finish}))
}
