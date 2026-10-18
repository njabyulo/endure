//! Agent-facing MCP entrypoint; adapters and credentials remain in the host.
use super::mcp::McpCall;
use crate::{
    core::{embedding::set_property, event_loop::JsonJob},
    host::HostState,
};
use serde_json::json;

pub(crate) fn install_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    context: v8::Local<'s, v8::Object>,
) -> Result<(), String> {
    let tools = v8::Object::new(scope);
    let call = v8::Function::new(scope, call).ok_or("MCP tool callback")?;
    set_property(scope, tools, "call", call.into());
    set_property(scope, context, "tools", tools.into());
    Ok(())
}
fn call(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = (|| -> Result<v8::Local<'_, v8::Promise>, String> {
        if !args.get(0).is_string() {
            return Err("tools.call requires a configured tool name".into());
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        let arguments =
            v8::json::stringify(scope, args.get(1)).ok_or("MCP tool arguments must be JSON")?;
        let arguments: serde_json::Value =
            serde_json::from_str(&arguments.to_rust_string_lossy(scope))
                .map_err(|_| "invalid MCP arguments")?;
        let call = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let config = state
                .host_config
                .as_ref()
                .ok_or("MCP requires trusted host configuration")?;
            let authority = state.effective_capabilities.borrow();
            let authority = authority.as_ref().ok_or("permission denied: MCP tool")?;
            McpCall::prepare(config, authority, &name, arguments.clone())?
        };
        let recovery = call.recovery();
        let endpoint = call.endpoint().to_string();
        let resolver = v8::PromiseResolver::new(scope).ok_or("MCP promise resolver")?;
        let retained = v8::Global::new(scope, resolver);
        let job = JsonJob {
            name: "tool.mcp".into(),
            args: json!({"name":name,"arguments":arguments}),
            network_url: Some(endpoint),
            required_child: None,
            required_tool: Some(name),
            task: Box::new(move |cancellation| call.run(cancellation)),
        };
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        state
            .operations
            .borrow_mut()
            .json(job, recovery, retained, session.as_mut())?;
        Ok(resolver.get_promise(scope))
    })();
    match result {
        Ok(promise) => output.set(promise.into()),
        Err(error) => {
            if let Some(message) = v8::String::new(scope, &error) {
                let exception = v8::Exception::error(scope, message);
                scope.throw_exception(exception);
            }
        }
    }
}
