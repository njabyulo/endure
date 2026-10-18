//! Native context methods never expose a dispatch bridge on globalThis.
use crate::{core::{embedding::set_property,memory::ContextLimits},host::HostState};
use serde_json::{Map,Value};

pub(crate) fn install_context<'s>(scope:&mut v8::PinScope<'s,'_>,ctx:v8::Local<'s,v8::Object>)->Result<(),String> {
    for (namespace,methods) in [("context",&["add","messages"][..]),("memory",&["remember","recall"][..]),("state",&["get","set","compareAndSet"][..])] {
        let object=v8::Object::new(scope);
        for method in methods {
            let name=v8::String::new(scope,&format!("{namespace}.{method}")).ok_or("memory method allocation")?;
            let function=v8::Function::builder(call).data(name.into()).build(scope).ok_or("memory callback allocation")?;
            set_property(scope,object,method,function.into());
        }
        set_property(scope,ctx,namespace,object.into());
    }
    Ok(())
}
fn parse(scope:&mut v8::PinScope<'_,'_>,value:v8::Local<'_,v8::Value>)->Result<Value,String> {
    let text=v8::json::stringify(scope,value).ok_or("memory arguments must be JSON serializable")?;
    serde_json::from_str(&text.to_rust_string_lossy(scope)).map_err(|e|e.to_string())
}
fn call(scope:&mut v8::PinScope<'_,'_>,args:v8::FunctionCallbackArguments,mut output:v8::ReturnValue) {
    let name=args.data().to_rust_string_lossy(scope);
    let result=(|| {
        let options_index=match name.as_str(){"context.add"|"memory.remember"|"memory.recall"|"state.get"=>1,"state.set"=>2,"state.compareAndSet"=>3,_=>0};
        let options=args.get(options_index);
        let mut value=if options.is_undefined() || name=="context.messages" { Map::new() } else { parse(scope,options)?.as_object().cloned().ok_or("options must be a JSON object")? };
        let fields:&[(&str,i32)]=match name.as_str(){"context.add"=>&[("message",0)],"memory.remember"=>&[("text",0)],"memory.recall"=>&[("query",0)],"state.get"=>&[("key",0)],"state.set"=>&[("key",0),("value",1)],"state.compareAndSet"=>&[("key",0),("expectedVersion",1),("value",2)],_=>&[]};
        for (key,index) in fields { value.insert((*key).into(),parse(scope,args.get(*index))?); }
        let value=Value::Object(value);
        let state=scope.get_slot::<HostState>().ok_or("host state unavailable")?;
        if !name.starts_with("context.") {
            let scope_name=value.get("scope").map_or(Ok("default"),|v|v.as_str().ok_or("scope must be a string"))?;
            state.require_memory_scope(scope_name)?;
        }
        let mut session=state.session.borrow_mut();
        session.as_mut().ok_or("durable session required")?.memory_op(&name,&value,ContextLimits::default())
    })();
    let Some(resolver)=v8::PromiseResolver::new(scope) else {return};
    match result {
        Ok(value)=>{
            let Some(text)=v8::String::new(scope,&value.to_string()) else {return};
            let Some(value)=v8::json::parse(scope,text) else {return};
            resolver.resolve(scope,value);
        }
        Err(error)=>{if let Some(message)=v8::String::new(scope,&error){let error=v8::Exception::error(scope,message);resolver.reject(scope,error);}}
    }
    output.set(resolver.get_promise(scope).into());
}
